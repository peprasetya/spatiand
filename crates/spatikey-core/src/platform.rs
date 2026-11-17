//! A holder's own half, asked through its app: the biometric key, the prompt, asking the owner
//! about a device. docs/spatikey.md, "The holder's half".
//!
//! The service is the same on every holder; what only the platform can do is its app's --
//! `SpatiKeyGuard.swift` in Spatiand.app on a Mac (through `sp_key_*`), `KeyGuard.java` in the APK
//! on an Android device (through JNI). Both are asked the same questions in the same JSON, through
//! one callback set with [`set_app`], each question carrying a number its answer comes back with
//! ([`answer`]): the asking thread waits for the answer with its number, the pattern of
//! `SP_DESCRIBE`. [`AppGuard`] is the service's [`Guard`] over it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::service::Guard;
use crate::vault::{TouchId, VaultKey};
use crate::wire::{Code, Refusal};
use crate::{b64, holders, hooks};

/// What the service asks of the app. The numbers are `spatiand_core.h`'s `SP_KEY_*` and
/// `KeyGuard.java`'s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Asked {
    /// `{reason, blob, wrapped, linger}`: a finger, then the vault key: `{"vk": base64}`.
    Unlock = 1,
    /// `{vk}`: a new key, and the vault key wrapped to it: `{public, blob, wrapped}`. The Mac's
    /// only: on Android the vault key is wrapped here, after [`Asked::Make`], so that it never
    /// sits in Java strings, which cannot be wiped.
    Enroll = 2,
    /// `{}`: whether the sensor could be used now, asking nobody: `{"ok": bool}`.
    CanTouch = 3,
    /// `{name, fp, code}`: the owner's yes or no to a device: `{"allow": bool}`.
    Allow = 4,
    /// `{}`: a new key, with nothing wrapped to it yet: `{public, blob}`. For asking to be added
    /// as a holder; the holder that adds it wraps the vault key to the public half.
    Make = 5,
}

/// The app's callback: given the question's number, what is asked, and the question as JSON. It
/// copies the question and returns at once, and answers later with [`answer`].
pub type App = dyn Fn(u64, Asked, &str) + Send + Sync;

/// A person at the device: the prompt may be left up this long. The app takes it down a little
/// before, so that no finger opens the vault for an answer nobody hears.
const TOUCH_WAIT: Duration = Duration::from_secs(120);
/// The owner reads six digits off the device before answering; the app's alert goes a little before.
const ALLOW_WAIT: Duration = Duration::from_secs(180);
/// Whether the sensor is there, or making a key, asks nobody: this long and something is stuck.
const QUICK_WAIT: Duration = Duration::from_secs(10);

/// The app's callback, while it is there.
static APP: Mutex<Option<Arc<App>>> = Mutex::new(None);
/// Questions asked of the app, by number, waiting for [`answer`].
static WAITING: Mutex<Vec<(u64, Sender<Zeroizing<String>>)>> = Mutex::new(Vec::new());

/// A lock that a panic elsewhere does not take away: the service outlives any one request.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The app that answers from now on (None: nobody, and every question is refused at once).
pub fn set_app(app: Option<Arc<App>>) {
    *lock(&APP) = app;
}

/// The app's answer to question `id`. An answer nobody waits for any more is dropped. It may
/// carry the vault key: the one copy is overwritten when the asker is done with it.
pub fn answer(id: u64, json: Zeroizing<String>) {
    let waiting = {
        let mut waiting = lock(&WAITING);
        waiting.iter().position(|(n, _)| *n == id).map(|at| waiting.remove(at).1)
    };
    if let Some(tx) = waiting {
        let _ = tx.send(json);
    }
}

/// The app's answer to an unlock, as the vault key's bytes rather than JSON: Android hands them
/// over as a byte array it then overwrites, where a Java string could not be.
pub fn answer_key(id: u64, vk: &[u8]) {
    let encoded = Zeroizing::new(b64::encode(vk));
    let mut json = Zeroizing::new(String::with_capacity(encoded.len() + 10));
    json.push_str(r#"{"vk":""#);
    json.push_str(&encoded);
    json.push_str(r#""}"#);
    answer(id, json);
}

/// Nothing more is asked of the app, and every question waiting is refused.
pub fn stop() {
    set_app(None);
    lock(&WAITING).clear();
}

/// How many questions wait for the app.
pub fn waiting() -> usize {
    lock(&WAITING).len()
}

/// Ask the app, and wait for its answer: at most `wait`. `sensor` names it in a refusal.
pub fn ask(what: Asked, json: &str, wait: Duration, sensor: &str) -> Result<Zeroizing<String>, Refusal> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let Some(app) = lock(&APP).clone() else {
        return Err(Refusal::new(Code::TouchIdUnavailable, "SpatiKey's half in the app is not running"));
    };
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = channel();
    lock(&WAITING).push((id, tx));
    // The question may carry the vault key: this copy is overwritten as soon as the app has its own.
    let text = Zeroizing::new(json.chars().filter(|&c| c != '\0').collect::<String>());
    app(id, what, &text);
    drop(text);
    let answer = rx.recv_timeout(wait);
    // Answered or given up on, nobody waits for this number any more.
    lock(&WAITING).retain(|(waiting, _)| *waiting != id);
    match answer {
        Ok(answer) => Ok(answer),
        Err(RecvTimeoutError::Timeout) => Err(Refusal::new(
            Code::Timeout,
            match what {
                Asked::Allow => "the owner of this device did not answer in time".to_string(),
                Asked::Unlock | Asked::Enroll => format!("no finger was put on {sensor} in time"),
                Asked::CanTouch => format!("{sensor} did not say whether it is there"),
                Asked::Make => "the new key was not made in time".to_string(),
            },
        )),
        Err(RecvTimeoutError::Disconnected) => Err(Refusal::new(Code::Failed, "SpatiKey stopped while it was asking")),
    }
}

/// [`Guard`] for the service on a holder: everything is asked of its app.
pub struct AppGuard {
    kind: &'static str,
}

impl AppGuard {
    /// A Mac's: Touch ID, through `SpatiKeyGuard.swift`, which wraps the vault key itself on
    /// enrolling.
    pub fn mac() -> AppGuard {
        AppGuard { kind: holders::MAC }
    }

    /// An Android device's: a strong biometric, through `KeyGuard.java`.
    pub fn android() -> AppGuard {
        AppGuard { kind: holders::ANDROID }
    }

    fn sensor(&self) -> &'static str {
        match self.kind {
            holders::ANDROID => "the fingerprint sensor",
            _ => "Touch ID",
        }
    }
}

impl Guard for AppGuard {
    fn unlock(&self, reason: &str, touch: &TouchId, linger_secs: u64) -> Result<VaultKey, Refusal> {
        let asked = json!({ "reason": reason, "blob": touch.blob, "wrapped": touch.wrapped, "linger": linger_secs });
        let answer = ask(Asked::Unlock, &asked.to_string(), TOUCH_WAIT, self.sensor())?;
        unlocked(&answer, self.sensor())
    }

    fn enroll(&self, key: &VaultKey) -> Result<TouchId, Refusal> {
        if self.kind != holders::MAC {
            // The key made, and the vault key wrapped to it here.
            let mut made = self.make()?;
            let public = b64::decode(&made.public).map_err(|_| Refusal::new(Code::Failed, "the new key's public half is not base64"))?;
            let wrapped = holders::wrap(key, &public).map_err(|e| Refusal::new(Code::Failed, e))?;
            made.wrapped = b64::encode(&wrapped);
            return Ok(made);
        }
        let vk = Zeroizing::new(b64::encode(key.bytes()));
        let answer = ask(Asked::Enroll, &enroll_question(&vk), TOUCH_WAIT, self.sensor())?;
        enrolled(&answer, true, self.sensor())
    }

    fn make(&self) -> Result<TouchId, Refusal> {
        let answer = ask(Asked::Make, "{}", QUICK_WAIT, self.sensor())?;
        enrolled(&answer, false, self.sensor())
    }

    fn kind(&self) -> &'static str {
        self.kind
    }

    fn can_touch(&self) -> bool {
        match ask(Asked::CanTouch, "{}", QUICK_WAIT, self.sensor()) {
            Ok(answer) => flag(&answer, "ok").unwrap_or(false),
            Err(refusal) => {
                log::warn!("spatikey: whether {} is there is not known: {refusal}", self.sensor());
                false
            }
        }
    }

    fn allow_device(&self, name: &str, fp: &str, code: &str) -> bool {
        let asked = json!({ "name": name, "fp": fp, "code": code });
        match ask(Asked::Allow, &asked.to_string(), ALLOW_WAIT, self.sensor()) {
            Ok(answer) => flag(&answer, "allow").unwrap_or(false),
            Err(refusal) => {
                log::warn!("spatikey: {name} was not let in: {refusal}");
                false
            }
        }
    }

    /// A card in the room, for this device's own asking and other devices' alike: in the glasses
    /// the device's own screen is not seen, and its prompt is on it. Not the app's banner: a notice
    /// asks nothing, and on a Mac's own desktop (2D) the panel already says what is asked -- the
    /// room draws its card only in the glasses. The room is there once the glasses have been on
    /// in this run of the app.
    fn notice(&self, text: &str) {
        match hooks::room() {
            Some(room) => room.notice(text),
            None if !text.is_empty() => log::info!("spatikey: {text}"),
            None => {}
        }
    }

    fn notice_briefly(&self, text: &str) {
        match hooks::room() {
            Some(room) => room.notice_briefly(text),
            None => log::info!("spatikey: {text}"),
        }
    }
}

/// `{"vk": base64}`, written by hand so that its only copies are this one and the caller's, both
/// overwritten when let go.
pub fn enroll_question(vk: &str) -> Zeroizing<String> {
    let mut question = Zeroizing::new(String::with_capacity(vk.len() + 16));
    question.push_str(r#"{"vk":""#);
    question.push_str(vk);
    question.push_str(r#""}"#);
    question
}

/// The vault key from the app's answer to [`Asked::Unlock`], or why there is none.
pub fn unlocked(answer: &str, sensor: &str) -> Result<VaultKey, Refusal> {
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
        .map_err(|e| Refusal::new(Code::Failed, format!("{sensor}'s answer could not be read: {e}")))?;
    let vk = answer.vk.map(Zeroizing::new);
    if let Some(code) = answer.error {
        return Err(refused(&code, answer.message));
    }
    let vk = vk.ok_or_else(|| Refusal::new(Code::Failed, format!("{sensor} answered without the key")))?;
    // Not the decoder's own words: they quote the character it stopped at.
    let bytes = Zeroizing::new(b64::decode(&vk).map_err(|_| Refusal::new(Code::Failed, format!("{sensor}'s key is not base64")))?);
    VaultKey::from_bytes(&bytes).map_err(|e| Refusal::new(Code::Failed, e))
}

/// The device's half from the app's answer to [`Asked::Enroll`] (`wrapped`) or [`Asked::Make`],
/// checked for the shape docs/spatikey.md gives it before the service writes it down: an
/// uncompressed P-256 point, and a wrap of 125 bytes.
pub fn enrolled(answer: &str, wrapped: bool, sensor: &str) -> Result<TouchId, Refusal> {
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
        .map_err(|e| Refusal::new(Code::Failed, format!("{sensor}'s answer could not be read: {e}")))?;
    if let Some(code) = answer.error {
        return Err(refused(&code, answer.message));
    }
    let public = b64::decode(&answer.public).unwrap_or_default();
    let blob = b64::decode(&answer.blob).unwrap_or_default();
    let wrap = b64::decode(&answer.wrapped).unwrap_or_default();
    let wrap_ok = if wrapped { wrap.len() == holders::WRAPPED_LEN } else { answer.wrapped.is_empty() };
    if public.len() != holders::PUBLIC_LEN || public[0] != 4 || blob.is_empty() || !wrap_ok {
        return Err(Refusal::new(Code::Failed, "the new key came back in a shape SpatiKey does not keep"));
    }
    Ok(TouchId { vault: String::new(), public: answer.public, blob: answer.blob, wrapped: answer.wrapped })
}

/// A yes or no from the app's answer.
pub fn flag(answer: &str, name: &str) -> Option<bool> {
    serde_json::from_str::<Value>(answer).ok()?.get(name)?.as_bool()
}

/// The app's word for why not, as the protocol's.
pub fn refused(code: &str, message: String) -> Refusal {
    let code = match code {
        "denied" => Code::Denied,
        "touch_id_unavailable" => Code::TouchIdUnavailable,
        "fingerprints_changed" => Code::FingerprintsChanged,
        "timeout" => Code::Timeout,
        _ => Code::Failed,
    };
    Refusal::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The statics are the process's: one test at a time.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn with_app<T>(app: Option<Arc<App>>, test: impl FnOnce() -> T) -> T {
        let _one = lock(&ONE_AT_A_TIME);
        set_app(app);
        let result = test();
        set_app(None);
        result
    }

    /// Answers from another thread, a moment later, as both apps do.
    fn later(id: u64, answer: Value) {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            super::answer(id, Zeroizing::new(answer.to_string()));
        });
    }

    fn app(id: u64, what: Asked, json: &str) {
        let question: Value = serde_json::from_str(json).expect("every question is JSON");
        let answer = match what {
            Asked::Unlock if question["reason"] == "sign in to github.com" && question["blob"] == "YmxvYg==" && question["linger"] == 60 => {
                json!({ "error": "fingerprints_changed", "message": "the fingers changed" })
            }
            Asked::CanTouch => json!({ "ok": true }),
            Asked::Allow => json!({ "allow": question["name"] == "Steam Deck" && question["code"] == "123 456" }),
            _ => json!({ "error": "failed", "message": format!("asked {what:?} with {question}") }),
        };
        later(id, answer);
    }

    fn touch() -> TouchId {
        TouchId { vault: "0123456789abcdef".into(), public: String::new(), blob: "YmxvYg==".into(), wrapped: "d3JhcA==".into() }
    }

    fn point() -> Vec<u8> {
        let secret = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        p256::elliptic_curve::sec1::ToEncodedPoint::to_encoded_point(&secret.public_key(), false).as_bytes().to_vec()
    }

    #[test]
    fn each_question_waits_for_the_answer_with_its_number() {
        with_app(Some(Arc::new(app)), || {
            let askers: Vec<_> = ["123 456", "654 321", "123 456", "000 000"]
                .into_iter()
                .map(|code| std::thread::spawn(move || AppGuard::mac().allow_device("Steam Deck", "ab", code)))
                .collect();
            let answers: Vec<bool> = askers.into_iter().map(|t| t.join().unwrap()).collect();
            assert_eq!(answers, [true, false, true, false]);
            assert!(AppGuard::mac().can_touch());
            assert!(!AppGuard::mac().allow_device("Beam Pro", "ab", "123 456"));
            assert_eq!(waiting(), 0);
        });
    }

    #[test]
    fn an_unlock_carries_the_reason_and_the_files_and_keeps_the_apps_refusal() {
        with_app(Some(Arc::new(app)), || {
            let refused = AppGuard::mac().unlock("sign in to github.com", &touch(), 60).unwrap_err();
            assert_eq!(refused.code, Code::FingerprintsChanged);
            assert_eq!(refused.message, "the fingers changed");
        });
    }

    #[test]
    fn with_no_app_nothing_is_let_in_and_no_finger_is_asked_for() {
        with_app(None, || {
            assert!(!AppGuard::mac().can_touch());
            assert!(!AppGuard::android().allow_device("Steam Deck", "ab", "123 456"));
            assert_eq!(AppGuard::mac().unlock("open SpatiKey", &touch(), 0).unwrap_err().code, Code::TouchIdUnavailable);
            assert_eq!(AppGuard::android().make().unwrap_err().code, Code::TouchIdUnavailable);
        });
    }

    #[test]
    fn a_question_nobody_answers_is_given_up_and_forgotten() {
        with_app(Some(Arc::new(|_: u64, _: Asked, _: &str| {})), || {
            let refused = ask(Asked::Unlock, "{}", Duration::from_millis(50), "Touch ID").unwrap_err();
            assert_eq!((refused.code, refused.message.as_str()), (Code::Timeout, "no finger was put on Touch ID in time"));
            assert_eq!(waiting(), 0);
        });
    }

    #[test]
    fn stopping_refuses_what_is_waiting() {
        with_app(Some(Arc::new(|_: u64, _: Asked, _: &str| {})), || {
            let asker = std::thread::spawn(|| ask(Asked::Allow, "{}", Duration::from_secs(30), "Touch ID"));
            while waiting() == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
            stop();
            assert_eq!(asker.join().unwrap().unwrap_err().code, Code::Failed);
            assert!(lock(&APP).is_none());
        });
    }

    #[test]
    fn an_answer_nobody_waits_for_is_dropped() {
        answer(u64::MAX, Zeroizing::new(r#"{"vk":"AAAA"}"#.into()));
        answer_key(u64::MAX, &[7u8; 32]);
    }

    #[test]
    fn the_apps_words_keep_their_meaning() {
        assert_eq!(refused("denied", String::new()).code, Code::Denied);
        assert_eq!(refused("touch_id_unavailable", String::new()).code, Code::TouchIdUnavailable);
        assert_eq!(refused("fingerprints_changed", String::new()).code, Code::FingerprintsChanged);
        assert_eq!(refused("timeout", String::new()).code, Code::Timeout);
        assert_eq!(refused("something new", String::new()).code, Code::Failed);
        let denied = unlocked(r#"{"error":"denied","message":"cancelled"}"#, "Touch ID").unwrap_err();
        assert_eq!((denied.code, denied.message.as_str()), (Code::Denied, "cancelled"));
        assert_eq!(unlocked("{}", "Touch ID").unwrap_err().code, Code::Failed);
        assert_eq!(unlocked(r#"{"vk":"not base64!"}"#, "Touch ID").unwrap_err().code, Code::Failed);
        assert_eq!(unlocked("not json", "Touch ID").unwrap_err().code, Code::Failed);
    }

    #[test]
    fn a_new_key_is_kept_only_in_the_shape_the_design_gives_it() {
        let made = |public: &[u8], wrapped: &[u8]| {
            json!({ "public": b64::encode(public), "blob": b64::encode(&[7u8; 569]), "wrapped": b64::encode(wrapped) }).to_string()
        };
        let mut point = [9u8; 65];
        point[0] = 4;
        let touch = enrolled(&made(&point, &[1u8; 125]), true, "Touch ID").unwrap();
        assert!(touch.vault.is_empty(), "the service fills in which vault");
        assert_eq!(b64::decode(&touch.wrapped).unwrap().len(), 125);
        assert_eq!(enrolled(&made(&point, &[1u8; 124]), true, "Touch ID").unwrap_err().code, Code::Failed);
        assert_eq!(enrolled(&made(&[9u8; 65], &[1u8; 125]), true, "Touch ID").unwrap_err().code, Code::Failed);
        let unavailable = enrolled(r#"{"error":"touch_id_unavailable","message":"lid shut"}"#, true, "Touch ID").unwrap_err();
        assert_eq!(unavailable.code, Code::TouchIdUnavailable);
        // A key made to be asked for: nothing wrapped to it yet.
        let only = json!({ "public": b64::encode(&point), "blob": "YWxpYXM=" }).to_string();
        assert!(enrolled(&only, false, "x").unwrap().wrapped.is_empty());
        assert!(enrolled(&made(&point, &[1u8; 125]), false, "x").is_err(), "a key asked to be made carries no wrap");
    }

    #[test]
    fn the_enrol_question_is_json_with_the_key_and_nothing_else() {
        let question = enroll_question("q83vEjRWeJA=");
        let value: Value = serde_json::from_str(&question).unwrap();
        assert_eq!(value, json!({ "vk": "q83vEjRWeJA=" }));
    }

    #[test]
    fn a_question_never_carries_a_nul_into_the_app() {
        with_app(
            Some(Arc::new(|id: u64, _: Asked, json: &str| {
                let ok = json == r#"{"a":"bc"}"#;
                super::answer(id, Zeroizing::new(json!({ "ok": ok }).to_string()));
            })),
            || {
                let answer = ask(Asked::CanTouch, "{\"a\":\"b\0c\"}", Duration::from_secs(5), "Touch ID").unwrap();
                assert_eq!(flag(&answer, "ok"), Some(true));
            },
        );
    }

    /// Android's enrolling: the app makes the key and gives its public half, and the vault key is
    /// wrapped to it here -- it never goes to Java.
    #[test]
    fn androids_enrolling_makes_the_key_there_and_wraps_here() {
        let public = point();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let seen = asked.clone();
        let public_b64 = b64::encode(&public);
        with_app(
            Some(Arc::new(move |id: u64, what: Asked, json: &str| {
                lock(&seen).push((what, json.to_string()));
                later(id, json!({ "public": public_b64, "blob": b64::encode(b"spatikey-alias") }));
            })),
            || {
                let key = VaultKey::random();
                let touch = AppGuard::android().enroll(&key).unwrap();
                assert_eq!(touch.public, b64::encode(&public));
                assert_eq!(b64::decode(&touch.wrapped).unwrap().len(), holders::WRAPPED_LEN);
                assert_eq!(*lock(&asked), vec![(Asked::Make, "{}".to_string())], "the vault key was not asked of the app");
                assert_eq!(AppGuard::android().kind(), holders::ANDROID);
            },
        );
    }

    #[test]
    fn a_key_is_answered_as_bytes_too() {
        with_app(
            Some(Arc::new(|id: u64, _: Asked, _: &str| {
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(10));
                    answer_key(id, &[5u8; 32]);
                });
            })),
            || {
                let key = AppGuard::android().unlock("open SpatiKey", &touch(), 0).unwrap();
                assert_eq!(key.bytes(), &[5u8; 32]);
            },
        );
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
            fn notice_briefly(&self, text: &str) {
                self.0.lock().unwrap().push(format!("briefly: {text}"));
            }
        }
        let _one = lock(&ONE_AT_A_TIME);
        let room = Arc::new(Room(Mutex::new(Vec::new())));
        hooks::set_room(Some(room.clone()));
        AppGuard::mac().notice("Touch ID on your Mac to sign in to github.com");
        AppGuard::mac().notice_briefly("Did not sign in to github.com: Touch ID was cancelled");
        hooks::set_room(None);
        AppGuard::mac().notice("only logged");
        assert_eq!(
            *room.0.lock().unwrap(),
            ["Touch ID on your Mac to sign in to github.com", "briefly: Did not sign in to github.com: Touch ID was cancelled"]
        );
    }
}
