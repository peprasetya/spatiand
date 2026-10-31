//! SpatiKey's service on the Mac, behind `sp_key_*` (docs/spatikey.md, "The Mac's half").
//!
//! The service is spatikey-core's, started with the app and running whether or not the room is
//! up: it answers SpatiKey, SpatiMesh, the browsers and the room on the local socket, and paired
//! devices on the same network. What only the app can do -- the Secure Enclave key and Touch ID,
//! and asking the owner about a device -- is `SpatiKeyGuard.swift`'s, asked through one callback
//! and answered with [`sp_key_answer`]: the pattern of `SP_DESCRIBE`, where the asking thread
//! waits for the answer that carries its number. The Touch ID panel names the program that asks,
//! and it must say Spatiand, which is why this half lives in the app and not in a helper.

use std::ffi::{c_char, c_void, CStr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use spatiand_stream::Identity;
use spatikey_core::service::{Guard, Me, Mode, Service};
use spatikey_core::vault::{TouchId, VaultKey};
use spatikey_core::wire::{Code, Refusal};
use spatikey_core::{b64, hooks, paths, remote, socket};
use zeroize::Zeroizing;

/// What the service asks of the app. The numbers are the header's `SP_KEY_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
enum Asked {
    /// `{reason, blob, wrapped, linger}`: a finger, then the vault key: `{"vk": base64}`.
    Unlock = 1,
    /// `{vk}`: a new Secure Enclave key, and the vault key wrapped to it: `{public, blob, wrapped}`.
    Enroll = 2,
    /// `{}`: whether Touch ID could be used now, asking nobody: `{"ok": bool}`.
    CanTouch = 3,
    /// `{name, fp, code}`: the owner's yes or no to a device: `{"allow": bool}`.
    Allow = 4,
}

/// The app's callback: called from the service's threads, it copies the question and returns at
/// once, and answers later with [`sp_key_answer`].
pub type AskFn = extern "C" fn(user: *mut c_void, id: u64, what: i32, json: *const c_char);

/// A person at the Mac: the Touch ID panel may be left up this long. The app takes it down a
/// little before, so that no finger opens the vault for an answer nobody hears.
const TOUCH_WAIT: Duration = Duration::from_secs(120);
/// The owner reads six digits off the device before answering; the app's alert goes a little before.
const ALLOW_WAIT: Duration = Duration::from_secs(180);
/// Whether Touch ID is there asks nobody: this long and something is stuck.
const QUICK_WAIT: Duration = Duration::from_secs(10);

/// The app's callback and its pointer, while the service runs.
static ASK: Mutex<Option<(AskFn, usize)>> = Mutex::new(None);
/// Questions asked of the app, by number, waiting for [`sp_key_answer`].
static WAITING: Mutex<Vec<(u64, Sender<Zeroizing<String>>)>> = Mutex::new(Vec::new());
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

/// Ask the app, and wait for its answer: at most `wait`.
fn ask(what: Asked, json: &str, wait: Duration) -> Result<Zeroizing<String>, Refusal> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let Some((callback, user)) = *lock(&ASK) else {
        return Err(Refusal::new(Code::TouchIdUnavailable, "SpatiKey's half in the app is not running"));
    };
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = channel();
    lock(&WAITING).push((id, tx));
    // The question may carry the vault key: this copy is overwritten as soon as the app has its own.
    let mut text = Zeroizing::new(Vec::with_capacity(json.len() + 1));
    text.extend(json.bytes().filter(|&b| b != 0));
    text.push(0);
    callback(user as *mut c_void, id, what as i32, text.as_ptr().cast());
    drop(text);
    let answer = rx.recv_timeout(wait);
    // Answered or given up on, nobody waits for this number any more.
    lock(&WAITING).retain(|(waiting, _)| *waiting != id);
    match answer {
        Ok(answer) => Ok(answer),
        Err(RecvTimeoutError::Timeout) => Err(Refusal::new(
            Code::Timeout,
            match what {
                Asked::Allow => "the owner of this Mac did not answer in time",
                Asked::Unlock | Asked::Enroll => "no finger was put on Touch ID in time",
                Asked::CanTouch => "Touch ID did not say whether it is there",
            },
        )),
        Err(RecvTimeoutError::Disconnected) => Err(Refusal::new(Code::Failed, "SpatiKey stopped while it was asking")),
    }
}

/// [`Guard`] for the service on this Mac: everything is asked of `SpatiKeyGuard.swift`.
struct MacGuard;

impl Guard for MacGuard {
    fn unlock(&self, reason: &str, touch: &TouchId, linger_secs: u64) -> Result<VaultKey, Refusal> {
        let asked = json!({ "reason": reason, "blob": touch.blob, "wrapped": touch.wrapped, "linger": linger_secs });
        let answer = ask(Asked::Unlock, &asked.to_string(), TOUCH_WAIT)?;
        unlocked(&answer)
    }

    fn enroll(&self, key: &VaultKey) -> Result<TouchId, Refusal> {
        let vk = Zeroizing::new(b64::encode(key.bytes()));
        let answer = ask(Asked::Enroll, &enroll_question(&vk), TOUCH_WAIT)?;
        enrolled(&answer)
    }

    fn can_touch(&self) -> bool {
        match ask(Asked::CanTouch, "{}", QUICK_WAIT) {
            Ok(answer) => flag(&answer, "ok").unwrap_or(false),
            Err(refusal) => {
                log::warn!("spatikey: whether Touch ID is there is not known: {refusal}");
                false
            }
        }
    }

    fn allow_device(&self, name: &str, fp: &str, code: &str) -> bool {
        let asked = json!({ "name": name, "fp": fp, "code": code });
        match ask(Asked::Allow, &asked.to_string(), ALLOW_WAIT) {
            Ok(answer) => flag(&answer, "allow").unwrap_or(false),
            Err(refusal) => {
                log::warn!("spatikey: {name} was not let in: {refusal}");
                false
            }
        }
    }

    /// A card in the room when it is up. Not the app's: a notice asks nothing, and on the Mac's
    /// own screen the Touch ID panel already says what is asked.
    fn notice(&self, text: &str) {
        match hooks::room() {
            Some(room) => room.notice(text),
            None if !text.is_empty() => log::info!("spatikey: {text}"),
            None => {}
        }
    }
}

/// `{"vk": base64}`, written by hand so that its only copies are this one and the caller's, both
/// overwritten when let go.
fn enroll_question(vk: &str) -> Zeroizing<String> {
    let mut question = Zeroizing::new(String::with_capacity(vk.len() + 16));
    question.push_str(r#"{"vk":""#);
    question.push_str(vk);
    question.push_str(r#""}"#);
    question
}

/// The vault key from the app's answer to [`Asked::Unlock`], or why there is none.
fn unlocked(answer: &str) -> Result<VaultKey, Refusal> {
    #[derive(Deserialize)]
    struct Answer {
        #[serde(default)]
        vk: Option<String>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        message: String,
    }
    let answer: Answer = serde_json::from_str(answer)
        .map_err(|e| Refusal::new(Code::Failed, format!("Touch ID's answer could not be read: {e}")))?;
    let vk = answer.vk.map(Zeroizing::new);
    if let Some(code) = answer.error {
        return Err(refused(&code, answer.message));
    }
    let vk = vk.ok_or_else(|| Refusal::new(Code::Failed, "Touch ID answered without the key"))?;
    // Not the decoder's own words: they quote the character it stopped at.
    let bytes = Zeroizing::new(b64::decode(&vk).map_err(|_| Refusal::new(Code::Failed, "Touch ID's key is not base64"))?);
    VaultKey::from_bytes(&bytes).map_err(|e| Refusal::new(Code::Failed, e))
}

/// This Mac's half of Touch ID from the app's answer to [`Asked::Enroll`], checked for the shape
/// docs/spatikey.md gives it before the service writes it down: an uncompressed P-256 point, and
/// a wrap of 125 bytes.
fn enrolled(answer: &str) -> Result<TouchId, Refusal> {
    #[derive(Deserialize)]
    struct Answer {
        #[serde(default)]
        public: String,
        #[serde(default)]
        blob: String,
        #[serde(default)]
        wrapped: String,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        message: String,
    }
    let answer: Answer = serde_json::from_str(answer)
        .map_err(|e| Refusal::new(Code::Failed, format!("Touch ID's answer could not be read: {e}")))?;
    if let Some(code) = answer.error {
        return Err(refused(&code, answer.message));
    }
    let public = b64::decode(&answer.public).unwrap_or_default();
    let blob = b64::decode(&answer.blob).unwrap_or_default();
    let wrapped = b64::decode(&answer.wrapped).unwrap_or_default();
    if public.len() != 65 || public[0] != 4 || blob.is_empty() || wrapped.len() != 125 {
        return Err(Refusal::new(Code::Failed, "the Secure Enclave key came back in a shape SpatiKey does not keep"));
    }
    Ok(TouchId { vault: String::new(), public: answer.public, blob: answer.blob, wrapped: answer.wrapped })
}

/// A yes or no from the app's answer.
fn flag(answer: &str, name: &str) -> Option<bool> {
    serde_json::from_str::<Value>(answer).ok()?.get(name)?.as_bool()
}

/// The app's word for why not, as the protocol's.
fn refused(code: &str, message: String) -> Refusal {
    let code = match code {
        "denied" => Code::Denied,
        "touch_id_unavailable" => Code::TouchIdUnavailable,
        "fingerprints_changed" => Code::FingerprintsChanged,
        "timeout" => Code::Timeout,
        _ => Code::Failed,
    };
    Refusal::new(code, message)
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
    *lock(&ASK) = Some((ask, user as usize));
    let me = Me { fp: identity.fingerprint().to_string(), name };
    let service = Service::new(Mode::Approver(Arc::new(MacGuard)), me, paths::shared(), paths::local());
    service.also_allow(host_paired);
    let path = paths::socket();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = socket::serve(&path, service.clone()) {
        log::error!("spatikey: could not serve {}: {e}", path.display());
        *lock(&ASK) = None;
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
    let waiting = {
        let mut waiting = lock(&WAITING);
        waiting.iter().position(|(n, _)| *n == id).map(|at| waiting.remove(at).1)
    };
    if let Some(tx) = waiting {
        let _ = tx.send(answer);
    }
}

/// Stop: nothing more is asked of the app, every question waiting is refused, the network side
/// stops and the socket goes. The threads already serving finish what they are doing; the
/// socket's own thread stays parked on a socket nobody can reach (spatikey-core gives it no stop).
#[no_mangle]
pub extern "C" fn sp_key_stop() {
    *lock(&ASK) = None;
    lock(&WAITING).clear();
    let running = lock(&RUNNING).take();
    if let Some(running) = running {
        let _ = std::fs::remove_file(&running.socket);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    /// The statics are the process's: one test at a time.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn with_app<T>(callback: Option<AskFn>, test: impl FnOnce() -> T) -> T {
        let _one = lock(&ONE_AT_A_TIME);
        *lock(&ASK) = callback.map(|c| (c, 0));
        let result = test();
        *lock(&ASK) = None;
        result
    }

    /// Stands in for `SpatiKeyGuard.swift`: copies the question and answers it from another
    /// thread, a moment later.
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
    fn each_question_waits_for_the_answer_with_its_number() {
        with_app(Some(app), || {
            let askers: Vec<_> = ["123 456", "654 321", "123 456", "000 000"]
                .into_iter()
                .map(|code| std::thread::spawn(move || MacGuard.allow_device("Steam Deck", "ab", code)))
                .collect();
            let answers: Vec<bool> = askers.into_iter().map(|t| t.join().unwrap()).collect();
            assert_eq!(answers, [true, false, true, false]);
            assert!(MacGuard.can_touch());
            assert!(!MacGuard.allow_device("Beam Pro", "ab", "123 456"));
            assert!(lock(&WAITING).is_empty());
        });
    }

    #[test]
    fn an_unlock_carries_the_reason_and_the_files_and_keeps_the_apps_refusal() {
        with_app(Some(app), || {
            let refused = MacGuard.unlock("sign in to github.com", &touch(), 60).unwrap_err();
            assert_eq!(refused.code, Code::FingerprintsChanged);
            assert_eq!(refused.message, "the fingers changed");
        });
    }

    #[test]
    fn with_no_app_nothing_is_let_in_and_no_finger_is_asked_for() {
        with_app(None, || {
            assert!(!MacGuard.can_touch());
            assert!(!MacGuard.allow_device("Steam Deck", "ab", "123 456"));
            assert_eq!(MacGuard.unlock("open SpatiKey", &touch(), 0).unwrap_err().code, Code::TouchIdUnavailable);
        });
    }

    #[test]
    fn a_question_nobody_answers_is_given_up_and_forgotten() {
        with_app(Some(silent), || {
            let refused = ask(Asked::Unlock, "{}", Duration::from_millis(50)).unwrap_err();
            assert_eq!(refused.code, Code::Timeout);
            assert!(lock(&WAITING).is_empty());
        });
    }

    #[test]
    fn stopping_refuses_what_is_waiting() {
        with_app(Some(silent), || {
            let asker = std::thread::spawn(|| ask(Asked::Allow, "{}", Duration::from_secs(30)));
            while lock(&WAITING).is_empty() {
                std::thread::sleep(Duration::from_millis(5));
            }
            sp_key_stop();
            assert_eq!(asker.join().unwrap().unwrap_err().code, Code::Failed);
            assert!(lock(&ASK).is_none());
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
    fn the_apps_words_keep_their_meaning() {
        assert_eq!(refused("denied", String::new()).code, Code::Denied);
        assert_eq!(refused("touch_id_unavailable", String::new()).code, Code::TouchIdUnavailable);
        assert_eq!(refused("fingerprints_changed", String::new()).code, Code::FingerprintsChanged);
        assert_eq!(refused("timeout", String::new()).code, Code::Timeout);
        assert_eq!(refused("something new", String::new()).code, Code::Failed);
        let denied = unlocked(r#"{"error":"denied","message":"cancelled"}"#).unwrap_err();
        assert_eq!((denied.code, denied.message.as_str()), (Code::Denied, "cancelled"));
        assert_eq!(unlocked("{}").unwrap_err().code, Code::Failed);
        assert_eq!(unlocked(r#"{"vk":"not base64!"}"#).unwrap_err().code, Code::Failed);
        assert_eq!(unlocked("not json").unwrap_err().code, Code::Failed);
    }

    #[test]
    fn a_new_key_is_kept_only_in_the_shape_the_design_gives_it() {
        let made = |public: &[u8], wrapped: &[u8]| {
            json!({ "public": b64::encode(public), "blob": b64::encode(&[7u8; 569]), "wrapped": b64::encode(wrapped) }).to_string()
        };
        let mut point = [9u8; 65];
        point[0] = 4;
        let touch = enrolled(&made(&point, &[1u8; 125])).unwrap();
        assert!(touch.vault.is_empty(), "the service fills in which vault");
        assert_eq!(b64::decode(&touch.wrapped).unwrap().len(), 125);
        assert_eq!(enrolled(&made(&point, &[1u8; 124])).unwrap_err().code, Code::Failed);
        assert_eq!(enrolled(&made(&[9u8; 65], &[1u8; 125])).unwrap_err().code, Code::Failed);
        let unavailable = enrolled(r#"{"error":"touch_id_unavailable","message":"lid shut"}"#).unwrap_err();
        assert_eq!(unavailable.code, Code::TouchIdUnavailable);
    }

    #[test]
    fn the_enrol_question_is_json_with_the_key_and_nothing_else() {
        let question = enroll_question("q83vEjRWeJA=");
        let value: Value = serde_json::from_str(&question).unwrap();
        assert_eq!(value, json!({ "vk": "q83vEjRWeJA=" }));
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
            let answer = ask(Asked::CanTouch, "{\"a\":\"b\0c\"}", Duration::from_secs(5)).unwrap();
            assert_eq!(flag(&answer, "ok"), Some(true));
        });
    }

    #[test]
    fn a_notice_is_a_card_in_the_room_when_there_is_one() {
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
        }
        let _one = lock(&ONE_AT_A_TIME);
        let room = Arc::new(Room(Mutex::new(Vec::new())));
        hooks::set_room(Some(room.clone()));
        MacGuard.notice("Touch ID on darkPhoenix to sign in to github.com");
        hooks::set_room(None);
        MacGuard.notice("only logged");
        assert_eq!(*room.0.lock().unwrap(), ["Touch ID on darkPhoenix to sign in to github.com"]);
    }
}

/// A device removed from the owner's list may no longer ask SpatiKey.
pub fn forget_device(fp: &str) {
    if let Some(running) = lock(&RUNNING).as_ref() {
        running.service.drop_device(fp);
    }
}
