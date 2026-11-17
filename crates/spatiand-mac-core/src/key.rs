//! SpatiKey's service on the Mac, behind `sp_key_*` (docs/spatikey.md, "The holder's half").
//!
//! The service is spatikey-core's, started with the app and running whether or not the room is
//! up: it answers SpatiKey, SpatiMesh, the browsers and the room on the local socket, and paired
//! devices on the same network. What only the app can do -- the Secure Enclave key and Touch ID,
//! and asking the owner about a device -- is `SpatiKeyGuard.swift`'s, asked through one callback
//! and answered with [`sp_key_answer`]: spatikey-core's `platform` (the same for Android's
//! `KeyGuard.java`), where the asking thread waits for the answer that carries its number. The
//! Touch ID panel names the program that asks, and it must say Spatiand, which is why this half
//! lives in the app and not in a helper.

use std::ffi::{c_char, c_void, CStr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use spatiand_stream::Identity;
use spatikey_core::platform::{self, AppGuard, Asked};
use spatikey_core::service::{Me, Mode, Service};
use spatikey_core::{paths, remote, socket};
use zeroize::Zeroizing;

/// The app's callback: called from the service's threads, it copies the question and returns at
/// once, and answers later with [`sp_key_answer`].
pub type AskFn = extern "C" fn(user: *mut c_void, id: u64, what: i32, json: *const c_char);

/// The running service's handles.
static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

struct Running {
    socket: PathBuf,
    service: Arc<Service>,
    /// Answering paired devices on the network; dropped, it stops. None when it could not start:
    /// the Mac still answers its own programs.
    _remote: Option<remote::Listening>,
}

/// A lock that a panic elsewhere does not take away: the service outlives any one request.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The app's callback as spatikey-core's `platform` calls it: the question as a C string, the
/// one copy overwritten once the app has its own (it may carry the vault key).
fn app_of(ask: AskFn, user: usize) -> Arc<platform::App> {
    Arc::new(move |id: u64, what: Asked, json: &str| {
        let mut text = Zeroizing::new(Vec::with_capacity(json.len() + 1));
        text.extend(json.bytes().filter(|&b| b != 0));
        text.push(0);
        ask(user as *mut c_void, id, what as i32, text.as_ptr().cast());
    })
}

/// Start the service, once: this Mac's link identity from `identity_dir` (made if absent) for its
/// fingerprint, `device_name` for what devices and cards call it. 1 when it is serving its socket
/// (the network side may still have failed, which is logged), 0 when it could not start.
///
/// # Safety
/// `identity_dir` and `device_name` C strings or null. `ask` is called from the service's threads
/// until [`sp_key_stop`], with `user` handed back untouched.
#[no_mangle]
pub unsafe extern "C" fn sp_key_start(
    identity_dir: *const c_char,
    device_name: *const c_char,
    user: *mut c_void,
    ask: Option<AskFn>,
) -> i32 {
    let (Some(dir), Some(ask)) = (super::string(identity_dir), ask) else { return 0 };
    let name = super::string(device_name).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Mac".into());
    let mut running = lock(&RUNNING);
    if running.is_some() {
        return 1;
    }
    let host_paired = PathBuf::from(&dir).with_file_name("host-paired.txt");
    let identity = match Identity::load_or_create(&PathBuf::from(dir)) {
        Ok(identity) => identity,
        Err(e) => {
            log::error!("spatikey: no identity to serve as: {e}");
            return 0;
        }
    };
    platform::set_app(Some(app_of(ask, user as usize)));
    let me = Me { fp: identity.fingerprint().to_string(), name };
    let service = Service::new(Mode::Approver(Arc::new(AppGuard::mac())), me, paths::shared(), paths::local());
    service.also_allow(host_paired);
    let path = paths::socket();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = socket::serve(&path, service.clone()) {
        log::error!("spatikey: could not serve {}: {e}", path.display());
        platform::set_app(None);
        return 0;
    }
    let remote = match remote::serve(service.clone(), identity) {
        Ok(listening) => Some(listening),
        Err(e) => {
            log::warn!("spatikey: devices on the network cannot ask this Mac: {e}");
            None
        }
    };
    log::info!("spatikey: serving {}", path.display());
    *running = Some(Running { socket: path, service: service.clone(), _remote: remote });
    1
}

/// The app's answer to question `id`. An answer nobody waits for any more is dropped.
///
/// # Safety
/// `json` a C string, or null.
#[no_mangle]
pub unsafe extern "C" fn sp_key_answer(id: u64, json: *const c_char) {
    // It may carry the vault key: one copy, overwritten when the asker is done with it.
    let answer = Zeroizing::new(if json.is_null() {
        String::new()
    } else {
        CStr::from_ptr(json).to_string_lossy().into_owned()
    });
    platform::answer(id, answer);
}

/// Stop: nothing more is asked of the app, every question waiting is refused, the network side
/// stops and the socket goes. The threads already serving finish what they are doing; the
/// socket's own thread stays parked on a socket nobody can reach (spatikey-core gives it no stop).
#[no_mangle]
pub extern "C" fn sp_key_stop() {
    platform::stop();
    let running = lock(&RUNNING).take();
    if let Some(running) = running {
        let _ = std::fs::remove_file(&running.socket);
    }
}

/// A device removed from the owner's list may no longer ask SpatiKey.
pub fn forget_device(fp: &str) {
    if let Some(running) = lock(&RUNNING).as_ref() {
        running.service.drop_device(fp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use spatikey_core::hooks;
    use spatikey_core::service::Guard;
    use spatikey_core::vault::TouchId;
    use spatikey_core::wire::Code;
    use std::ffi::CString;
    use std::time::Duration;

    /// The app is the process's: one test at a time.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn with_app<T>(callback: Option<AskFn>, test: impl FnOnce() -> T) -> T {
        let _one = lock(&ONE_AT_A_TIME);
        platform::set_app(callback.map(|c| app_of(c, 0)));
        let result = test();
        platform::set_app(None);
        result
    }

    /// Stands in for `SpatiKeyGuard.swift`: copies the question and answers it through
    /// `sp_key_answer` from another thread, a moment later.
    extern "C" fn app(_user: *mut c_void, id: u64, what: i32, json: *const c_char) {
        let question = unsafe { CStr::from_ptr(json) }.to_string_lossy().into_owned();
        let question: Value = serde_json::from_str(&question).expect("every question is JSON");
        let answer = match what {
            1 if question["reason"] == "sign in to github.com" && question["blob"] == "YmxvYg==" && question["linger"] == 60 => {
                json!({ "error": "fingerprints_changed", "message": "the fingers changed" })
            }
            3 => json!({ "ok": true }),
            4 => json!({ "allow": question["name"] == "Steam Deck" && question["code"] == "123 456" }),
            _ => json!({ "error": "failed", "message": format!("asked {what} with {question}") }),
        };
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            let text = CString::new(answer.to_string()).unwrap();
            unsafe { sp_key_answer(id, text.as_ptr()) };
        });
    }

    /// An app that never answers.
    extern "C" fn silent(_user: *mut c_void, _id: u64, _what: i32, _json: *const c_char) {}

    fn touch() -> TouchId {
        TouchId { vault: "0123456789abcdef".into(), public: String::new(), blob: "YmxvYg==".into(), wrapped: "d3JhcA==".into() }
    }

    #[test]
    fn the_app_is_asked_through_the_c_callback_and_answers_through_sp_key_answer() {
        with_app(Some(app), || {
            let guard = AppGuard::mac();
            assert!(guard.allow_device("Steam Deck", "ab", "123 456"));
            assert!(!guard.allow_device("Steam Deck", "ab", "654 321"));
            assert!(guard.can_touch());
            let refused = guard.unlock("sign in to github.com", &touch(), 60).unwrap_err();
            assert_eq!((refused.code, refused.message.as_str()), (Code::FingerprintsChanged, "the fingers changed"));
            assert_eq!(platform::waiting(), 0);
        });
    }

    #[test]
    fn stopping_refuses_what_is_waiting() {
        with_app(Some(silent), || {
            let asker = std::thread::spawn(|| platform::ask(Asked::Allow, "{}", Duration::from_secs(30), "Touch ID"));
            while platform::waiting() == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
            sp_key_stop();
            assert_eq!(asker.join().unwrap().unwrap_err().code, Code::Failed);
        });
    }

    #[test]
    fn an_answer_nobody_waits_for_is_dropped() {
        let text = CString::new(r#"{"vk":"AAAA"}"#).unwrap();
        unsafe {
            sp_key_answer(u64::MAX, text.as_ptr());
            sp_key_answer(u64::MAX, std::ptr::null());
        }
    }

    #[test]
    fn a_question_never_carries_a_nul_into_the_app() {
        extern "C" fn whole(_user: *mut c_void, id: u64, _what: i32, json: *const c_char) {
            let seen = unsafe { CStr::from_ptr(json) }.to_bytes().to_vec();
            let ok = seen == br#"{"a":"bc"}"#;
            let text = CString::new(json!({ "ok": ok }).to_string()).unwrap();
            unsafe { sp_key_answer(id, text.as_ptr()) };
        }
        with_app(Some(whole), || {
            let answer = platform::ask(Asked::CanTouch, "{\"a\":\"b\0c\"}", Duration::from_secs(5), "Touch ID").unwrap();
            assert_eq!(platform::flag(&answer, "ok"), Some(true));
        });
    }

    /// The whole way on this Mac: its own program asks, the service asks the app for a finger,
    /// and the room's card is up for as long as the app takes to answer, then taken away -- or,
    /// with no finger, the last word said.
    #[test]
    fn this_macs_own_asking_shows_a_card_in_the_room() {
        struct Room(Mutex<Vec<String>>);
        impl hooks::Room for Room {
            fn target(&self) -> Option<hooks::Target> {
                None
            }
            fn type_into(&self, _window: u64, _steps: &[hooks::Step]) -> Result<(), String> {
                Err("no".into())
            }
            fn notice(&self, text: &str) {
                self.0.lock().unwrap().push(text.into());
            }
            fn notice_briefly(&self, text: &str) {
                self.0.lock().unwrap().push(format!("briefly: {text}"));
            }
        }
        /// Touch ID that says no, after the room has been looked at: the card is up meanwhile.
        extern "C" fn refusing(_user: *mut c_void, id: u64, what: i32, _json: *const c_char) {
            let answer = match what {
                3 => json!({ "ok": true }),
                _ => json!({ "error": "denied", "message": "Touch ID was cancelled" }),
            };
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(20));
                let text = CString::new(answer.to_string()).unwrap();
                unsafe { sp_key_answer(id, text.as_ptr()) };
            });
        }
        let place = std::env::temp_dir().join(format!("sk-mac-card-{}", std::process::id()));
        let (shared, local) = (place.join("apps/spatikey"), place.join("local/spatikey"));
        let (vault, _key) = spatikey_core::vault::Vault::create(&shared, &local, "0b8dfc23").unwrap();
        vault.set_touch_id(&touch()).unwrap();
        let me = Me { fp: "ab".repeat(32), name: "darkPhoenix".into() };
        let service = Service::new(Mode::Approver(Arc::new(AppGuard::mac())), me, shared, local);
        let room = Arc::new(Room(Mutex::new(Vec::new())));
        let asked = with_app(Some(refusing), || {
            hooks::set_room(Some(room.clone()));
            let who = spatikey_core::wire::Who { app: "SpatiMesh".into(), device: String::new() };
            let request = spatikey_core::wire::Request::Fill { origin: "https://github.com".into() };
            let reply = service.handle(1, spatikey_core::wire::Envelope { id: 1, who, request });
            hooks::set_room(None);
            reply
        });
        let _ = std::fs::remove_dir_all(&place);
        assert_eq!(asked.err.map(|e| e.code), Some(Code::Denied));
        assert_eq!(
            *room.0.lock().unwrap(),
            [
                "Touch ID on your Mac to fill in your password for github.com",
                "",
                "briefly: Did not fill in your password for github.com: Touch ID was cancelled"
            ]
        );
    }
}
