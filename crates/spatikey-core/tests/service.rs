//! The service as its callers see it, with a soft guard standing in for Touch ID: setting up and
//! recovering, the files the vault is kept in, sessions, prompts, typing, a forwarder's own
//! answers, and the socket.
//!
//! What reaches `site`, `totp` or `webauthn` is ignored until they are written ("needs
//! site/totp/webauthn"), and a forwarder's asking a Mac until `remote` is ("needs remote").

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::de::DeserializeOwned;
use serde_json::Value;
use spatiand_stream::Identity;
use spatikey_core::hooks::{self, Room, Step, Target};
use spatikey_core::remote::Asker;
use spatikey_core::service::{next_conn, ConnId, Guard, Me, Mode, Service};
use spatikey_core::socket::{self, Client};
use spatikey_core::vault::{TouchDevice, TouchId, Vault, VaultKey};
use spatikey_core::wire::*;
use spatikey_core::{b64, Entry, Kind, Passkey, Totp};
use zeroize::Zeroizing;

/// What the soft guard's Secure Enclave key "is": checked to be what the service hands back.
const BLOB: &str = "c29mdC1zZWN1cmUtZW5jbGF2ZQ==";

/// Touch ID as these tests have it: the key it was enrolled with, kept in memory, and a record
/// of what it was asked.
#[derive(Default)]
struct SoftGuard {
    key: Mutex<Option<VaultKey>>,
    unlocks: AtomicUsize,
    enrolls: AtomicUsize,
    reasons: Mutex<Vec<String>>,
    notices: Mutex<Vec<String>>,
    lingers: Mutex<Vec<u64>>,
    /// No Touch ID now (the lid shut).
    no_touch: AtomicBool,
    /// The person says no.
    refuse: AtomicBool,
    allow: AtomicBool,
    /// Take a while to answer, and notice a second prompt arriving meanwhile.
    slow: AtomicBool,
    busy: AtomicBool,
    overlapped: AtomicBool,
}

impl SoftGuard {
    fn unlocks(&self) -> usize {
        self.unlocks.load(SeqCst)
    }

    fn last_reason(&self) -> String {
        self.reasons.lock().unwrap().last().cloned().unwrap_or_default()
    }

    fn key_bytes(&self) -> [u8; 32] {
        *self.key.lock().unwrap().as_ref().expect("a key enrolled").bytes()
    }
}

impl Guard for SoftGuard {
    fn unlock(&self, reason: &str, touch: &TouchId, linger_secs: u64) -> Result<VaultKey, Refusal> {
        self.unlocks.fetch_add(1, SeqCst);
        self.reasons.lock().unwrap().push(reason.to_string());
        self.lingers.lock().unwrap().push(linger_secs);
        assert_eq!(touch.blob, BLOB, "the guard is handed this Mac's own half");
        if self.busy.swap(true, SeqCst) {
            self.overlapped.store(true, SeqCst);
        }
        if self.slow.load(SeqCst) {
            std::thread::sleep(Duration::from_millis(150));
        }
        self.busy.store(false, SeqCst);
        if self.no_touch.load(SeqCst) {
            return Err(Refusal::new(Code::TouchIdUnavailable, "the lid is shut"));
        }
        if self.refuse.load(SeqCst) {
            return Err(Refusal::new(Code::Denied, "the person said no"));
        }
        self.key.lock().unwrap().clone().ok_or_else(|| Refusal::new(Code::FingerprintsChanged, "no key enrolled"))
    }

    fn enroll(&self, key: &VaultKey) -> Result<TouchId, Refusal> {
        self.enrolls.fetch_add(1, SeqCst);
        if self.refuse.load(SeqCst) {
            return Err(Refusal::new(Code::Denied, "the person said no"));
        }
        *self.key.lock().unwrap() = Some(key.clone());
        Ok(TouchId {
            vault: String::new(),
            public: "c29mdC1wdWJsaWM=".into(),
            blob: BLOB.into(),
            wrapped: "c29mdC13cmFwcGVk".into(),
        })
    }

    fn can_touch(&self) -> bool {
        !self.no_touch.load(SeqCst)
    }

    fn allow_device(&self, _name: &str, _fp: &str, _code: &str) -> bool {
        self.allow.load(SeqCst)
    }

    fn notice(&self, text: &str) {
        self.notices.lock().unwrap().push(text.to_string());
    }
}

/// A folder of this test's own, gone when the test is.
struct Place {
    root: PathBuf,
}

impl Place {
    fn new(name: &str) -> Place {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        // Short: a socket's path must fit in 104 bytes on the Mac.
        let root = std::env::temp_dir().join(format!("sk-{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, SeqCst)));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Place { root }
    }

    fn shared(&self) -> PathBuf {
        self.root.join("apps").join("spatikey")
    }

    fn local(&self) -> PathBuf {
        self.root.join("local").join("spatikey")
    }

    fn entry_file(&self, id: &str) -> PathBuf {
        self.shared().join("entries").join(format!("{id}.json"))
    }

    /// The length of an entry's ciphertext and tag, as its file has it.
    fn sealed_length(&self, id: &str) -> usize {
        let file: Value = serde_json::from_slice(&std::fs::read(self.entry_file(id)).unwrap()).unwrap();
        b64::decode(file["c"].as_str().unwrap()).unwrap().len()
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The room is one for the whole process: the tests that set it, or need it unset, take turns.
static ROOM: Mutex<()> = Mutex::new(());

/// A room that records what it typed.
#[derive(Default)]
struct FakeRoom {
    target: Option<Target>,
    typed: Mutex<Vec<(u64, Vec<Step>)>>,
    refuse: bool,
}

impl Room for FakeRoom {
    fn target(&self) -> Option<Target> {
        self.target.clone()
    }

    fn type_into(&self, window: u64, steps: &[Step]) -> Result<(), String> {
        if self.refuse {
            return Err("the focus is not on that window".into());
        }
        self.typed.lock().unwrap().push((window, steps.to_vec()));
        Ok(())
    }

    fn notice(&self, _text: &str) {}
}

const SHORT: &str = "0b8dfc23";

fn me() -> Me {
    Me { fp: format!("{SHORT}{}", "5e".repeat(28)), name: "darkPhoenix".into() }
}

/// An approver over `place`, with no vault yet.
fn approver(place: &Place) -> (Arc<Service>, Arc<SoftGuard>) {
    let guard = Arc::new(SoftGuard::default());
    let service = Service::new(Mode::Approver(guard.clone()), me(), place.shared(), place.local());
    (service, guard)
}

/// An approver with a vault and this Mac's Touch ID set up for it, as setup leaves it but for the
/// recovery code: Argon2id at its full cost takes seconds in a debug build.
fn ready(place: &Place) -> (Arc<Service>, Arc<SoftGuard>, VaultKey) {
    let (service, guard) = approver(place);
    let (vault, key) = Vault::create(&place.shared(), &place.local(), SHORT).unwrap();
    let touch = guard.enroll(&key).unwrap();
    vault.set_touch_id(&touch).unwrap();
    guard.enrolls.store(0, SeqCst);
    (service, guard, key)
}

fn forwarder(place: &Place) -> Arc<Service> {
    let asker = Asker::new(Identity::new().unwrap(), place.local(), "Steam Deck".into());
    let deck = Me { fp: "d0".repeat(32), name: "Steam Deck".into() };
    Service::new(Mode::Forwarder(Arc::new(asker)), deck, place.shared(), place.local())
}

fn ask(service: &Service, conn: ConnId, request: Request) -> Result<Value, Refusal> {
    ask_from(service, conn, Who::default(), request)
}

fn ask_from(service: &Service, conn: ConnId, who: Who, request: Request) -> Result<Value, Refusal> {
    let reply = service.handle(conn, Envelope { id: 77, who, request });
    assert_eq!(reply.id, 77, "an answer carries its question's id");
    reply.into_result()
}

fn answer<T: DeserializeOwned>(answered: Result<Value, Refusal>) -> T {
    match answered {
        Ok(value) => serde_json::from_value(value).expect("an answer of the documented shape"),
        Err(refusal) => panic!("refused: {refusal}"),
    }
}

fn code(answered: Result<Value, Refusal>) -> Code {
    match answered {
        Ok(_) => panic!("answered where a refusal was expected"),
        Err(refusal) => refusal.code,
    }
}

fn open(service: &Service, conn: ConnId) -> String {
    answer::<OpenAnswer>(ask(service, conn, Request::Open)).session
}

fn put(service: &Service, conn: ConnId, session: &str, entry: Entry) -> String {
    answer::<IdAnswer>(ask(service, conn, Request::Put { session: session.into(), entry })).id
}

fn get(service: &Service, conn: ConnId, session: &str, id: &str) -> Entry {
    answer::<GetAnswer>(ask(service, conn, Request::Get { session: session.into(), id: id.into() })).entry
}

fn login(title: &str, site: &str, username: &str, password: &str) -> Entry {
    Entry {
        kind: Kind::Login,
        title: title.into(),
        sites: vec![site.into()],
        username: username.into(),
        password: password.into(),
        ..Entry::default()
    }
}

fn mode(path: &std::path::Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn setup_gives_a_code_that_recovers_the_key() {
    let place = Place::new("setup");
    let (service, guard) = approver(&place);
    let conn = next_conn();
    let before: StatusAnswer = answer(ask(&service, conn, Request::Status));
    assert!(!before.vault && !before.opens_here && before.can_set_up);
    assert_eq!((before.name.as_str(), before.fp.as_str()), ("darkPhoenix", me().fp.as_str()));

    let setup: SetupAnswer = answer(ask(&service, conn, Request::Setup { name: "darkPhoenix".into() }));
    let groups: Vec<&str> = setup.recovery_code.split('-').collect();
    assert!(groups.len() == 5 && groups.iter().all(|g| g.len() == 5), "five groups of five");
    assert_eq!(guard.enrolls.load(SeqCst), 1);
    let after: StatusAnswer = answer(ask(&service, conn, Request::Status));
    assert!(after.vault && after.opens_here && !after.can_set_up);
    assert_eq!(code(ask(&service, conn, Request::Setup { name: String::new() })), Code::VaultExists);

    // The code gives back the very key Touch ID was given.
    let vault = Vault::open(&place.shared(), &place.local()).unwrap();
    let recovered = vault.recover(&setup.recovery_code).unwrap();
    assert_eq!(recovered.bytes(), &guard.key_bytes());
    assert!(vault.check(&recovered));
    assert_eq!(vault.header().by, SHORT);

    // What setup left: this Mac's own half, private; its record; the code's wrapping. Never the key.
    let touch_file = place.local().join("touch-id.json");
    assert_eq!(mode(&touch_file), 0o600);
    assert_eq!(vault.touch_id().unwrap().vault, vault.id());
    let devices = vault.touch_devices();
    assert_eq!(devices.len(), 1);
    assert_eq!(
        (devices[0].device.as_str(), devices[0].name.as_str(), devices[0].kind.as_str()),
        (me().fp.as_str(), "darkPhoenix", "mac-touch-id")
    );
    assert!(vault.has_recovery());
    for file in ["vault.json", "recovery.json"] {
        let text = std::fs::read_to_string(place.shared().join(file)).unwrap();
        assert!(!text.contains(&b64::encode(recovered.bytes())), "{file} holds no key in the clear");
    }

    // A Mac without its half (a new one, or new fingers): the code sets Touch ID up again.
    std::fs::remove_file(&touch_file).unwrap();
    let lost: StatusAnswer = answer(ask(&service, conn, Request::Status));
    assert!(lost.vault && !lost.opens_here && lost.can_set_up);
    assert_eq!(code(ask(&service, conn, Request::Open)), Code::NotHere);
    *guard.key.lock().unwrap() = None;
    assert_eq!(code(ask(&service, conn, Request::Recover { code: "not-a-code".into() })), Code::BadRequest);
    let wrong = spatikey_core::recovery::new_code();
    assert_eq!(code(ask(&service, conn, Request::Recover { code: wrong })), Code::Denied);
    assert_eq!(guard.enrolls.load(SeqCst), 1, "a wrong code enrols nothing");
    let typed = setup.recovery_code.to_lowercase().replace('-', " ");
    answer::<Value>(ask(&service, conn, Request::Recover { code: typed }));
    assert_eq!(guard.key_bytes(), *recovered.bytes());
    assert!(answer::<StatusAnswer>(ask(&service, conn, Request::Status)).opens_here);
    assert_eq!(vault.touch_devices().len(), 1, "this Mac's record is replaced, not added to");
    open(&service, conn);
}

#[test]
fn a_wrong_key_does_not_open_the_vault() {
    let place = Place::new("wrongkey");
    let (vault, key) = Vault::create(&place.shared(), &place.local(), SHORT).unwrap();
    let wrong = VaultKey::random();
    assert!(vault.check(&key));
    assert!(!vault.check(&wrong));
    let mut entry = login("GitHub", "github.com", "paulus", "hunter2");
    vault.write(&key, &mut entry, SHORT).unwrap();
    assert!(vault.read(&wrong, &entry.id).is_err());
    assert!(vault.entries(&wrong).unwrap().is_empty(), "what does not open is left out");
    assert_eq!(vault.entries(&key).unwrap().len(), 1);
    let again = Vault::open(&place.shared(), &place.local()).unwrap();
    assert_eq!(again.header(), vault.header());
    assert!(again.check(&key) && !again.check(&wrong));
    assert!(Vault::create(&place.shared(), &place.local(), SHORT).is_err(), "one vault to a folder");

    // A Touch ID that gives the wrong key opens nothing.
    let (service, guard) = approver(&place);
    let touch = guard.enroll(&wrong).unwrap();
    vault.set_touch_id(&touch).unwrap();
    assert_eq!(code(ask(&service, next_conn(), Request::Open)), Code::Failed);
}

#[test]
fn an_altered_or_renamed_entry_file_does_not_open() {
    let place = Place::new("altered");
    let (vault, key) = Vault::create(&place.shared(), &place.local(), SHORT).unwrap();
    let mut entry = login("GitHub", "github.com", "paulus", "hunter2");
    vault.write(&key, &mut entry, SHORT).unwrap();
    let original = std::fs::read(place.entry_file(&entry.id)).unwrap();
    assert!(!String::from_utf8_lossy(&original).contains("hunter2"));
    let other = "0123456789abcdef0123456789abcdef";

    // Renamed: the same file under another entry's name.
    std::fs::write(place.entry_file(other), &original).unwrap();
    assert!(vault.read(&key, other).is_err());
    // Renamed, and its id changed to match the name: what is sealed still names the first.
    let mut file: Value = serde_json::from_slice(&original).unwrap();
    file["id"] = other.into();
    std::fs::write(place.entry_file(other), serde_json::to_vec(&file).unwrap()).unwrap();
    assert!(vault.read(&key, other).is_err());
    assert_eq!(vault.entries(&key).unwrap().len(), 1, "only the real one opens");

    // Altered: one bit of the ciphertext.
    let mut file: Value = serde_json::from_slice(&original).unwrap();
    let mut sealed = b64::decode(file["c"].as_str().unwrap()).unwrap();
    sealed[10] ^= 1;
    file["c"] = b64::encode(&sealed).into();
    std::fs::write(place.entry_file(&entry.id), serde_json::to_vec(&file).unwrap()).unwrap();
    assert!(vault.read(&key, &entry.id).is_err());
    assert!(vault.entries(&key).unwrap().is_empty());

    // Moved to another vault, even one with the same key: its id is not this one's.
    let elsewhere = Place::new("altered2");
    let second = Vault::create_with(&elsewhere.shared(), &elsewhere.local(), SHORT, &key).unwrap();
    assert_ne!(second.id(), vault.id());
    std::fs::write(elsewhere.entry_file(&entry.id), &original).unwrap();
    assert!(second.read(&key, &entry.id).is_err());

    // Put back as it was, it opens again.
    std::fs::write(place.entry_file(&entry.id), &original).unwrap();
    assert_eq!(vault.read(&key, &entry.id).unwrap().password, "hunter2");
}

#[test]
fn entries_are_padded_to_multiples_of_256() {
    let place = Place::new("padding");
    let (vault, key) = Vault::create(&place.shared(), &place.local(), SHORT).unwrap();
    for length in [0, 1, 100, 255, 256, 1000, 5000, 60_000] {
        let mut entry = Entry { kind: Kind::Note, title: "Note".into(), notes: "n".repeat(length), ..Entry::default() };
        vault.write(&key, &mut entry, SHORT).unwrap();
        let padded = place.sealed_length(&entry.id) - 16;
        assert_eq!(padded % 256, 0, "{length} characters of notes came to {padded} bytes");
        assert_eq!(vault.read(&key, &entry.id).unwrap().notes.len(), length, "and read back whole");
    }
    let mut huge = Entry { notes: "n".repeat(70 * 1024), ..Entry::default() };
    assert!(vault.write(&key, &mut huge, SHORT).is_err(), "over 64 KiB is refused");
    assert!(!place.entry_file(&huge.id).exists());
}

#[test]
fn versions_and_file_times_only_move_forward() {
    let place = Place::new("forward");
    let (vault, key) = Vault::create(&place.shared(), &place.local(), SHORT).unwrap();
    let mut entry = login("GitHub", "github.com", "paulus", "hunter2");
    vault.write(&key, &mut entry, SHORT).unwrap();
    assert_eq!(entry.version, 1);
    assert_eq!(entry.id.len(), 32);
    let created = entry.created;
    assert!(created > 0 && entry.changed >= created);
    assert_eq!(entry.changed_by, SHORT);

    vault.write(&key, &mut entry, "1a2b3c4d").unwrap();
    assert_eq!((entry.version, entry.changed_by.as_str()), (2, "1a2b3c4d"));
    // An older copy written again (a window left open) still counts up from what is on disk,
    // and keeps when the entry was made.
    let mut stale = vault.read(&key, &entry.id).unwrap();
    stale.version = 1;
    stale.created = 5;
    vault.write(&key, &mut stale, SHORT).unwrap();
    assert_eq!((stale.version, stale.created), (3, created));
    // A copy from further ahead: one more than it.
    stale.version = 10;
    vault.write(&key, &mut stale, SHORT).unwrap();
    assert_eq!(vault.read(&key, &entry.id).unwrap().version, 11);

    // The file's time, after another machine's clock ran ahead: still forward, by a millisecond.
    let path = place.entry_file(&entry.id);
    let ahead = SystemTime::now() + Duration::from_secs(600);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(ahead).unwrap();
    vault.write(&key, &mut stale, SHORT).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    assert!(modified > ahead && modified <= ahead + Duration::from_millis(1));
    assert_eq!(mode(&path), 0o600);
    let hidden = std::fs::read_dir(place.shared().join("entries"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
        .count();
    assert_eq!(hidden, 0, "no half-written file is left");
}

#[test]
fn a_deleted_entry_stays_as_a_tombstone() {
    let place = Place::new("tombstone");
    let (service, _guard, key) = ready(&place);
    let conn = next_conn();
    let session = open(&service, conn);
    let id = put(&service, conn, &session, login("GitHub", "github.com", "paulus", "hunter2"));
    let keep = put(&service, conn, &session, login("Steam", "steampowered.com", "paulus", "gaben"));
    answer::<Value>(ask(&service, conn, Request::Delete { session: session.clone(), id: id.clone() }));

    let listed: ListAnswer = answer(ask(&service, conn, Request::List { session: session.clone() }));
    assert_eq!(listed.entries.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec![keep.as_str()]);
    assert_eq!(code(ask(&service, conn, Request::Get { session: session.clone(), id: id.clone() })), Code::NoMatch);
    let vault = Vault::open(&place.shared(), &place.local()).unwrap();
    let tomb = vault.read(&key, &id).unwrap();
    assert!(tomb.deleted);
    assert!(tomb.title.is_empty() && tomb.username.is_empty() && tomb.password.is_empty() && tomb.sites.is_empty());
    assert_eq!((tomb.id.as_str(), tomb.version), (id.as_str(), 2));
    assert!(place.entry_file(&id).exists(), "kept, so that the deletion travels as the newest version");

    // Deleting it again changes nothing; an id that was never there is no match.
    answer::<Value>(ask(&service, conn, Request::Delete { session: session.clone(), id: id.clone() }));
    assert_eq!(vault.read(&key, &id).unwrap().version, 2);
    let never = "0123456789abcdef0123456789abcdef".to_string();
    assert_eq!(code(ask(&service, conn, Request::Delete { session: session.clone(), id: never })), Code::NoMatch);
    assert_eq!(code(ask(&service, conn, Request::Delete { session: session.clone(), id: "../vault".into() })), Code::BadRequest);

    // A put that says deleted leaves a tombstone too, with nothing in it.
    let mut gone = get(&service, conn, &session, &keep);
    gone.deleted = true;
    put(&service, conn, &session, gone);
    let tomb = vault.read(&key, &keep).unwrap();
    assert!(tomb.deleted && tomb.password.is_empty() && tomb.title.is_empty());
    assert!(answer::<ListAnswer>(ask(&service, conn, Request::List { session })).entries.is_empty());
}

#[test]
fn sessions_expire_and_belong_to_their_connection() {
    let place = Place::new("sessions");
    let (service, guard, _key) = ready(&place);
    let (mine, theirs) = (next_conn(), next_conn());
    let session = open(&service, mine);
    assert_eq!(session.len(), 32);
    assert_eq!(guard.unlocks(), 1);
    let list = |session: &str| Request::List { session: session.into() };

    // Another connection can neither use it nor close it.
    assert_eq!(code(ask(&service, theirs, list(&session))), Code::BadSession);
    assert_eq!(code(ask(&service, theirs, Request::Close { session: session.clone() })), Code::BadSession);
    // In it, nothing asks for a finger.
    answer::<ListAnswer>(ask(&service, mine, list(&session)));
    assert_eq!(guard.unlocks(), 1);

    // Closed when its connection goes.
    service.closed(mine);
    assert_eq!(code(ask(&service, mine, list(&session))), Code::BadSession);
    // Closed when asked; closing it again is no error.
    let session = open(&service, mine);
    answer::<Value>(ask(&service, mine, Request::Close { session: session.clone() }));
    assert_eq!(code(ask(&service, mine, list(&session))), Code::BadSession);
    answer::<Value>(ask(&service, mine, Request::Close { session: session.clone() }));
    assert_eq!(code(ask(&service, mine, list("0123456789abcdef0123456789abcdef"))), Code::BadSession);

    // Closed after sitting unused for the idle time the settings give.
    std::fs::write(place.local().join("settings.json"), r#"{"touch_secs":0,"open_secs":1}"#).unwrap();
    let session = open(&service, mine);
    std::thread::sleep(Duration::from_millis(600));
    answer::<ListAnswer>(ask(&service, mine, list(&session)));
    std::thread::sleep(Duration::from_millis(600));
    answer::<ListAnswer>(ask(&service, mine, list(&session)));
    std::thread::sleep(Duration::from_millis(1300));
    assert_eq!(code(ask(&service, mine, list(&session))), Code::BadSession);
}

#[test]
fn a_session_lists_gets_and_changes_entries() {
    let place = Place::new("crud");
    let (service, guard, key) = ready(&place);
    let conn = next_conn();
    let opened: OpenAnswer = answer(ask(&service, conn, Request::Open));
    assert!(opened.entries.is_empty());
    let session = opened.session;

    let id = put(&service, conn, &session, login("GitHub", "github.com", "paulus", "hunter2"));
    let entry = get(&service, conn, &session, &id);
    assert_eq!((entry.password.as_str(), entry.version, entry.changed_by.as_str()), ("hunter2", 1, SHORT));
    let mut changed = entry;
    changed.password = "correct horse".into();
    assert_eq!(put(&service, conn, &session, changed), id);
    let entry = get(&service, conn, &session, &id);
    assert_eq!((entry.password.as_str(), entry.version), ("correct horse", 2));
    let listed: ListAnswer = answer(ask(&service, conn, Request::List { session: session.clone() }));
    assert_eq!(listed.entries.len(), 1);
    assert!(listed.entries[0].has_password && listed.entries[0].title == "GitHub");
    assert_eq!(code(ask(&service, conn, Request::Get { session: session.clone(), id: "nope".into() })), Code::BadRequest);
    let never = "0123456789abcdef0123456789abcdef".to_string();
    assert_eq!(code(ask(&service, conn, Request::Get { session: session.clone(), id: never })), Code::NoMatch);

    // A passkey's private key never leaves the service, and a put without it keeps it.
    let passkey = Entry {
        kind: Kind::Passkey,
        title: "Example".into(),
        passkey: Some(Passkey {
            rp_id: "example.com".into(),
            credential_id: "AAECAw".into(),
            user_handle: "dXNlcg".into(),
            key: "c2VjcmV0LWtleQ==".into(),
            ..Passkey::default()
        }),
        ..Entry::default()
    };
    let pid = put(&service, conn, &session, passkey.clone());
    let given = get(&service, conn, &session, &pid);
    assert!(given.passkey.as_ref().unwrap().key.is_empty());
    let mut renamed = given;
    renamed.title = "Example (work)".into();
    put(&service, conn, &session, renamed);
    let kept = Vault::open(&place.shared(), &place.local()).unwrap().read(&key, &pid).unwrap();
    assert_eq!((kept.title.as_str(), kept.passkey.unwrap().key.as_str()), ("Example (work)", "c2VjcmV0LWtleQ=="));
    let mut keyless = passkey;
    keyless.passkey.as_mut().unwrap().key.clear();
    assert_eq!(code(ask(&service, conn, Request::Put { session: session.clone(), entry: keyless })), Code::BadRequest);
    assert_eq!(guard.unlocks(), 1, "all of it in the one session");
}

#[test]
fn prompts_say_what_for_and_who_asks() {
    let place = Place::new("prompts");
    let (service, guard, _key) = ready(&place);
    let conn = next_conn();
    let session = open(&service, conn);
    assert_eq!(guard.last_reason(), "open SpatiKey");

    // From a browser on another device: both named, and a card on the Mac while the finger is wanted.
    let deck = Who { app: "Chrome".into(), device: "Steam Deck".into() };
    ask_from(&service, next_conn(), deck.clone(), Request::Open).unwrap();
    assert_eq!(guard.last_reason(), "open SpatiKey in Chrome on Steam Deck");
    assert_eq!(*guard.notices.lock().unwrap(), vec!["open SpatiKey in Chrome on Steam Deck".to_string(), String::new()]);
    // SpatiMesh is Spatiand's own: not named.
    let mesh = Who { app: "SpatiMesh".into(), device: String::new() };
    ask_from(&service, next_conn(), mesh, Request::Open).unwrap();
    assert_eq!(guard.last_reason(), "open SpatiKey");
    // What is this Mac's own business is not asked of it from elsewhere.
    for request in [Request::Target, Request::Setup { name: String::new() }, Request::Recover { code: String::new() }] {
        assert_eq!(code(ask_from(&service, next_conn(), deck.clone(), request)), Code::NotHere);
    }

    // An entry asked for outside a session is named, once this service has opened it.
    let id = put(&service, conn, &session, login("GitHub", "github.com", "paulus", "hunter2"));
    let unlocks = guard.unlocks();
    let reveal = |session: Option<String>| Request::Reveal {
        session,
        id: id.clone(),
        fields: vec![Field::Username, Field::Tab, Field::Password],
    };
    let shown: RevealAnswer = answer(ask(&service, conn, reveal(Some(session.clone()))));
    assert_eq!(shown.values, vec!["paulus".to_string(), String::new(), "hunter2".to_string()]);
    assert_eq!(guard.unlocks(), unlocks, "in a session: no finger");
    let shown: RevealAnswer = answer(ask_from(&service, next_conn(), deck.clone(), reveal(None)));
    assert_eq!(shown.values[2], "hunter2");
    assert_eq!(guard.last_reason(), "type your GitHub username and password in Chrome on Steam Deck");
    assert_eq!(guard.unlocks(), unlocks + 1);
    // Keys alone give nothing away: no finger for them.
    let keys = Request::Reveal { session: None, id: id.clone(), fields: vec![Field::Tab, Field::Enter] };
    assert_eq!(answer::<RevealAnswer>(ask(&service, next_conn(), keys)).values, vec![String::new(), String::new()]);
    assert_eq!(guard.unlocks(), unlocks + 1);

    // One touch may cover what follows: the setting is handed to Touch ID.
    let set = |touch_secs, open_secs| Request::Settings { session: session.clone(), set: Some(Settings { touch_secs, open_secs }) };
    assert_eq!(code(ask(&service, conn, set(30, 300))), Code::BadRequest);
    answer::<SettingsAnswer>(ask(&service, conn, set(60, 300)));
    open(&service, next_conn());
    assert_eq!(guard.lingers.lock().unwrap().last(), Some(&60));

    // The person saying no, or no Touch ID now, opens nothing.
    guard.refuse.store(true, SeqCst);
    assert_eq!(code(ask(&service, next_conn(), Request::Open)), Code::Denied);
    guard.refuse.store(false, SeqCst);
    guard.no_touch.store(true, SeqCst);
    assert_eq!(code(ask(&service, next_conn(), Request::Open)), Code::TouchIdUnavailable);
}

#[test]
fn one_prompt_at_a_time() {
    let place = Place::new("oneprompt");
    let (service, guard, _key) = ready(&place);
    guard.slow.store(true, SeqCst);
    let askers: Vec<_> = (0..3)
        .map(|_| {
            let service = service.clone();
            std::thread::spawn(move || ask(&service, next_conn(), Request::Open).is_ok())
        })
        .collect();
    assert!(askers.into_iter().all(|asker| asker.join().unwrap()));
    assert_eq!(guard.unlocks(), 3);
    assert!(!guard.overlapped.load(SeqCst), "a second prompt waited for the first");
}

#[test]
fn typing_needs_a_room_and_types_what_was_asked() {
    let _room = ROOM.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let place = Place::new("typing");
    let (service, guard, _key) = ready(&place);
    let conn = next_conn();
    let session = open(&service, conn);
    let id = put(&service, conn, &session, login("GitHub", "github.com", "paulus", "hunter2"));
    let fields = vec![Field::Username, Field::Tab, Field::Password, Field::Enter];
    let typing = |session: Option<&str>, fields: &[Field], window: Option<u64>| Request::Type {
        session: session.map(str::to_string),
        id: id.clone(),
        fields: fields.to_vec(),
        window,
    };

    hooks::set_room(None);
    let unlocks = guard.unlocks();
    assert_eq!(code(ask(&service, conn, typing(None, &fields, None))), Code::NotHere);
    assert_eq!(code(ask(&service, conn, Request::Target)), Code::NotHere);
    assert_eq!(guard.unlocks(), unlocks, "nobody is asked for a finger with nothing to type into");

    let terminal = Target { window: 42, app: "org.gnome.Terminal".into(), title: "Terminal".into() };
    let room = Arc::new(FakeRoom { target: Some(terminal), ..FakeRoom::default() });
    hooks::set_room(Some(room.clone()));
    let target: TargetAnswer = answer(ask(&service, conn, Request::Target));
    assert_eq!((target.window, target.title.as_str()), (42, "Terminal"));
    answer::<Value>(ask(&service, conn, typing(Some(&session), &fields, None)));
    assert_eq!(guard.unlocks(), unlocks, "in a session: no finger");
    answer::<Value>(ask(&service, conn, typing(None, &[Field::Password], None)));
    assert_eq!(guard.last_reason(), "type your GitHub password into Terminal");
    answer::<Value>(ask(&service, conn, typing(None, &[Field::Password], Some(9))));
    assert_eq!(guard.last_reason(), "type your GitHub password into a window");
    answer::<Value>(ask(&service, conn, typing(None, &[Field::Enter], Some(7))));
    assert_eq!(guard.unlocks(), unlocks + 2, "keys alone open nothing");

    let text = |s: &str| Step::Text(Zeroizing::new(s.to_string()));
    let typed = room.typed.lock().unwrap().clone();
    assert_eq!(
        typed,
        vec![
            (42, vec![text("paulus"), Step::Tab, text("hunter2"), Step::Enter]),
            (42, vec![text("hunter2")]),
            (9, vec![text("hunter2")]),
            (7, vec![Step::Enter]),
        ]
    );
    // A room that cannot type it all says so.
    hooks::set_room(Some(Arc::new(FakeRoom { refuse: true, ..FakeRoom::default() })));
    assert_eq!(code(ask(&service, conn, typing(Some(&session), &fields, Some(42)))), Code::Failed);
    hooks::set_room(None);
}

#[test]
fn devices_settings_and_a_new_recovery_code_in_a_session() {
    let place = Place::new("devices");
    let (service, _guard, key) = ready(&place);
    let conn = next_conn();
    let session = open(&service, conn);
    let deck = "aa".repeat(32);
    service.allow(Device { fp: deck.clone(), name: "Steam Deck".into(), added: 0 }).unwrap();
    service.allow(Device { fp: deck.clone(), name: "Steam Deck OLED".into(), added: 5 }).unwrap();
    let allowed = service.allowed();
    assert_eq!(allowed.len(), 1, "letting a device in again replaces it");
    assert_eq!((allowed[0].name.as_str(), allowed[0].added), ("Steam Deck OLED", 5));
    assert!(service.allow(Device::default()).is_err());

    let vault = Vault::open(&place.shared(), &place.local()).unwrap();
    let other_mac = "bb".repeat(32);
    let record = TouchDevice {
        device: other_mac.clone(),
        name: "workMac".into(),
        kind: "mac-touch-id".into(),
        public: String::new(),
        wrapped: String::new(),
        added: 9,
    };
    vault.add_touch_device(&record).unwrap();
    let devices: DevicesAnswer = answer(ask(&service, conn, Request::Devices { session: session.clone() }));
    assert_eq!(devices.allowed, allowed);
    assert_eq!(devices.touch_ids, vec![Device { fp: other_mac.clone(), name: "workMac".into(), added: 9 }]);

    let forget = |fp: &str| Request::ForgetDevice { session: session.clone(), fp: fp.into() };
    answer::<Value>(ask(&service, conn, forget(&deck)));
    assert!(service.allowed().is_empty());
    answer::<Value>(ask(&service, conn, forget(&other_mac)));
    assert!(vault.touch_devices().is_empty());
    assert_eq!(code(ask(&service, conn, forget(&other_mac))), Code::NoMatch);
    assert_eq!(code(ask(&service, conn, forget(&me().fp))), Code::BadRequest);

    let settings: SettingsAnswer = answer(ask(&service, conn, Request::Settings { session: session.clone(), set: None }));
    assert_eq!(settings.settings, Settings { touch_secs: 0, open_secs: 300 });
    let set = |touch_secs, open_secs| Request::Settings { session: session.clone(), set: Some(Settings { touch_secs, open_secs }) };
    let changed: SettingsAnswer = answer(ask(&service, conn, set(300, 600)));
    assert_eq!(changed.settings, Settings { touch_secs: 300, open_secs: 600 });
    assert_eq!(code(ask(&service, conn, set(0, 5))), Code::BadRequest);
    let kept: Settings = serde_json::from_slice(&std::fs::read(place.local().join("settings.json")).unwrap()).unwrap();
    assert_eq!(kept, Settings { touch_secs: 300, open_secs: 600 });

    let renewed: SetupAnswer = answer(ask(&service, conn, Request::RecoveryNew { session: session.clone() }));
    assert_eq!(vault.recover(&renewed.recovery_code).unwrap().bytes(), key.bytes());
    assert_eq!(code(ask(&service, next_conn(), Request::Devices { session })), Code::BadSession);
}

#[test]
fn status_opens_here_only_with_this_vaults_touch_id() {
    let place = Place::new("status");
    let (service, guard) = approver(&place);
    let status = || answer::<StatusAnswer>(ask(&service, next_conn(), Request::Status));
    let none = status();
    assert!(!none.vault && !none.opens_here && none.can_set_up && none.approvers.is_empty());
    assert_eq!(code(ask(&service, next_conn(), Request::Open)), Code::NoVault);

    // A vault synced from another Mac: there, but not opened here.
    let (vault, key) = Vault::create(&place.shared(), &place.local(), "1a2b3c4d").unwrap();
    let synced = status();
    assert!(synced.vault && !synced.opens_here && synced.can_set_up);
    // This Mac's half for another vault does not count.
    let touch = guard.enroll(&key).unwrap();
    std::fs::create_dir_all(place.local()).unwrap();
    let theirs = TouchId { vault: "0123456789abcdef".into(), ..touch.clone() };
    std::fs::write(place.local().join("touch-id.json"), serde_json::to_vec(&theirs).unwrap()).unwrap();
    assert!(!status().opens_here);
    assert_eq!(code(ask(&service, next_conn(), Request::Open)), Code::NotHere);
    vault.set_touch_id(&touch).unwrap();
    let here = status();
    assert!(here.opens_here && !here.can_set_up);
    // No Touch ID now: nothing to set up with either.
    std::fs::remove_file(place.local().join("touch-id.json")).unwrap();
    guard.no_touch.store(true, SeqCst);
    let shut = status();
    assert!(!shut.opens_here && !shut.can_set_up);
}

#[test]
fn a_forwarder_keeps_to_itself_what_needs_this_machine() {
    let _room = ROOM.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let place = Place::new("forwarder");
    let service = forwarder(&place);
    assert!(!service.is_approver() && service.guard().is_none());
    let conn = next_conn();
    assert_eq!(code(ask(&service, conn, Request::Setup { name: String::new() })), Code::NotHere);
    assert_eq!(code(ask(&service, conn, Request::Recover { code: String::new() })), Code::NotHere);
    assert_eq!(code(ask(&service, conn, Request::Pair { name: "x".into() })), Code::BadRequest);
    hooks::set_room(None);
    assert_eq!(code(ask(&service, conn, Request::Target)), Code::NotHere);
    let typing = Request::Type { session: None, id: "0".repeat(32), fields: vec![Field::Password], window: None };
    assert_eq!(code(ask(&service, conn, typing)), Code::NotHere);
    // A session this device did not see opened for this connection is refused here, unasked.
    let theirs = "0123456789abcdef0123456789abcdef".to_string();
    assert_eq!(code(ask(&service, conn, Request::List { session: theirs.clone() })), Code::BadSession);
    let totp = Request::Totp { session: Some(theirs.clone()), id: "0".repeat(32) };
    assert_eq!(code(ask(&service, conn, totp)), Code::BadSession);
    answer::<Value>(ask(&service, conn, Request::Close { session: theirs }));
    service.closed(conn);
}

#[test]
#[ignore = "needs remote"]
fn a_forwarder_with_no_approver_answers_no_approver() {
    let place = Place::new("noapprover");
    let service = forwarder(&place);
    let conn = next_conn();
    assert_eq!(code(ask(&service, conn, Request::Open)), Code::NoApprover);
    assert_eq!(code(ask(&service, conn, Request::Fill { origin: "https://github.com".into() })), Code::NoApprover);
    let status: StatusAnswer = answer(ask(&service, conn, Request::Status));
    assert!(!status.opens_here && !status.can_set_up && status.approvers.is_empty());
    assert_eq!(status.name, "Steam Deck");
}

#[test]
fn the_socket_answers_this_user_and_names_the_program() {
    let place = Place::new("socket");
    let (service, guard, _key) = ready(&place);
    let path = place.root.join("run").join("spatikey.sock");
    let _serving = socket::serve(&path, service.clone()).unwrap();
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);

    let mut client = Client::connect_to(&path, "SpatiKey").unwrap();
    let status: StatusAnswer = client.ask_as(Request::Status).unwrap();
    assert!(status.vault && status.opens_here);
    // The prompt names the program from its process, not from what it said of itself.
    let opened: OpenAnswer = client.ask_as(Request::Open).unwrap();
    let program = std::env::current_exe().unwrap().file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(guard.last_reason(), format!("open SpatiKey in {program}"));
    // The session is this connection's.
    let mut other = Client::connect_to(&path, "SpatiKey").unwrap();
    assert_eq!(other.ask(Request::List { session: opened.session.clone() }).unwrap_err().code, Code::BadSession);
    let listed: ListAnswer = client.ask_as(Request::List { session: opened.session.clone() }).unwrap();
    assert!(listed.entries.is_empty());

    // Raw, as the browsers' host relays: what is not JSON is refused with id 0, and the
    // connection carries on.
    let reply: Reply = serde_json::from_slice(&client.relay(b"{not json").unwrap()).unwrap();
    assert_eq!((reply.id, reply.err.map(|e| e.code)), (0, Some(Code::BadRequest)));
    let reply: Reply = serde_json::from_slice(&client.relay(br#"{"id":41,"op":"status"}"#).unwrap()).unwrap();
    assert!(reply.id == 41 && reply.ok.is_some());
    let reply: Reply = serde_json::from_slice(&client.relay(br#"{"id":42,"op":"no_such_thing"}"#).unwrap()).unwrap();
    assert_eq!((reply.id, reply.err.map(|e| e.code)), (42, Some(Code::BadRequest)));

    // A second service is not let take a live socket over; a dead one's file is replaced.
    assert_eq!(socket::serve(&path, service.clone()).unwrap_err().kind(), std::io::ErrorKind::AddrInUse);
    let stale = place.root.join("run").join("old.sock");
    drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
    assert!(stale.exists());
    let _again = socket::serve(&stale, service.clone()).unwrap();
    let mut client = Client::connect_to(&stale, "SpatiKey").unwrap();
    assert!(client.ask_as::<StatusAnswer>(Request::Status).unwrap().vault);
    let not_a_socket = place.root.join("run").join("file.sock");
    std::fs::write(&not_a_socket, b"").unwrap();
    assert!(socket::serve(&not_a_socket, service).is_err(), "a file that is not a socket is left alone");
}

// ---- what needs site, totp and webauthn ----

/// The passkey signature checks out with the public key the page was given: ES256 over
/// authenticator data and the client data hash.
fn verify_es256(public_key: &str, authenticator_data: &str, client_data_hash: &[u8], signature: &str) -> bool {
    let spki = b64::decode_url(public_key).unwrap();
    let point = &spki[spki.len() - 65..];
    let mut signed = b64::decode_url(authenticator_data).unwrap();
    signed.extend_from_slice(client_data_hash);
    ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_ASN1, point)
        .verify(&signed, &b64::decode_url(signature).unwrap())
        .is_ok()
}

#[test]
#[ignore = "needs site/totp/webauthn"]
fn fill_gives_the_logins_for_a_page() {
    let place = Place::new("fill");
    let (service, guard, _key) = ready(&place);
    let conn = next_conn();
    let session = open(&service, conn);
    put(&service, conn, &session, login("GitHub", "github.com", "paulus", "hunter2"));
    put(&service, conn, &session, login("GitLab", "gitlab.com", "paulus", "tanuki"));
    put(&service, conn, &session, login("Router", "https://192.168.1.2:8443", "admin", "admin"));
    std::thread::sleep(Duration::from_millis(5));
    let mut work = login("GitHub (work)", "github.com", "paulus-work", "octocat");
    work.totp = Some(Totp { secret: "JBSWY3DPEHPK3PXP".into(), digits: 6, period: 30, algorithm: "SHA1".into(), ..Totp::default() });
    let work = put(&service, conn, &session, work);
    let fill = |origin: &str| ask(&service, next_conn(), Request::Fill { origin: origin.into() });

    let filled: FillAnswer = answer(fill("https://github.com"));
    assert_eq!(guard.last_reason(), "fill in your password for github.com");
    let names: Vec<&str> = filled.logins.iter().map(|l| l.username.as_str()).collect();
    assert_eq!(names, vec!["paulus-work", "paulus"], "newest first");
    assert_eq!(filled.logins[0].id, work);
    let totp = filled.logins[0].totp.clone().unwrap();
    assert!(totp.len() == 6 && totp.bytes().all(|b| b.is_ascii_digit()));
    assert_eq!((filled.logins[1].password.as_str(), filled.logins[1].totp.is_none()), ("hunter2", true));
    assert_eq!(answer::<FillAnswer>(fill("https://gist.github.com")).logins.len(), 2, "a domain covers what is under it");
    assert_eq!(guard.last_reason(), "fill in your password for gist.github.com");
    assert_eq!(answer::<FillAnswer>(fill("https://192.168.1.2:8443")).logins[0].username, "admin");
    assert_eq!(code(fill("https://192.168.1.2")), Code::NoMatch, "a whole origin covers only itself");
    assert_eq!(code(fill("https://example.org")), Code::NoMatch);
    let unlocks = guard.unlocks();
    assert_eq!(code(fill("http://github.com")), Code::BadRequest);
    assert_eq!(code(fill("github.com")), Code::BadRequest);
    assert_eq!(guard.unlocks(), unlocks, "refused before anyone is asked");
}

#[test]
#[ignore = "needs site/totp/webauthn"]
fn save_changes_the_same_login_or_keeps_a_new_one() {
    let place = Place::new("save");
    let (service, guard, _key) = ready(&place);
    let conn = next_conn();
    let session = open(&service, conn);
    let save = |origin: &str, title: &str, username: &str, password: &str| {
        ask(
            &service,
            next_conn(),
            Request::Save { origin: origin.into(), title: title.into(), username: username.into(), password: password.into() },
        )
    };
    let first = answer::<IdAnswer>(save("https://login.github.com", "", "paulus", "one")).id;
    assert_eq!(guard.last_reason(), "save a password for login.github.com");
    let entry = get(&service, conn, &session, &first);
    assert_eq!((entry.title.as_str(), entry.password.as_str()), ("github.com", "one"));
    assert_eq!(entry.sites, vec!["github.com".to_string()]);
    // The same account on the same site: changed, not kept twice; unchanged, not written.
    assert_eq!(answer::<IdAnswer>(save("https://github.com", "", "paulus", "two")).id, first);
    assert_eq!(answer::<IdAnswer>(save("https://github.com", "", "paulus", "two")).id, first);
    let entry = get(&service, conn, &session, &first);
    assert_eq!((entry.password.as_str(), entry.version), ("two", 2));
    // Another account, or a site that must match exactly: new entries.
    let second = answer::<IdAnswer>(save("https://github.com", "Work GitHub", "paulus-work", "three")).id;
    assert_ne!(second, first);
    assert_eq!(get(&service, conn, &session, &second).title, "Work GitHub");
    let router = answer::<IdAnswer>(save("https://192.168.1.2:8443", "", "admin", "admin")).id;
    let entry = get(&service, conn, &session, &router);
    assert_eq!((entry.title.as_str(), entry.sites.clone()), ("192.168.1.2", vec!["https://192.168.1.2:8443".to_string()]));
    let unlocks = guard.unlocks();
    assert_eq!(code(save("http://github.com", "", "paulus", "x")), Code::BadRequest);
    assert_eq!(code(save("https://github.com", "", "paulus", "")), Code::BadRequest);
    assert_eq!(guard.unlocks(), unlocks);
}

#[test]
#[ignore = "needs site/totp/webauthn"]
fn a_passkey_is_made_once_per_account_and_signs_in() {
    let place = Place::new("passkey");
    let (service, guard, _key) = ready(&place);
    let hash = [7u8; 32];
    let create = |user: &[u8], name: &str, exclude: Vec<String>| {
        Request::PasskeyCreate(PasskeyCreate {
            origin: "https://github.com".into(),
            rp_id: "github.com".into(),
            rp_name: "GitHub".into(),
            user_id: b64::encode_url(user),
            user_name: name.into(),
            user_display: name.to_uppercase(),
            client_data_hash: b64::encode_url(&hash),
            exclude,
            algs: vec![-8, -7, -257],
        })
    };
    let sign_in = |origin: &str, rp_id: &str, allow: Vec<String>| {
        Request::PasskeyGet(PasskeyGet {
            origin: origin.into(),
            rp_id: rp_id.into(),
            client_data_hash: b64::encode_url(&hash),
            allow,
            conditional: false,
        })
    };
    let made: CreatedAnswer = answer(ask(&service, next_conn(), create(b"user-1", "paulus", vec![])));
    assert_eq!(guard.last_reason(), "make a passkey for github.com");
    assert_eq!(made.alg, -7);

    let choices: ChoicesAnswer = answer(ask(&service, next_conn(), sign_in("https://github.com", "github.com", vec![])));
    assert_eq!(guard.last_reason(), "sign in to github.com with a passkey");
    assert_eq!(choices.choices.len(), 1);
    let choice = &choices.choices[0];
    assert_eq!((choice.user_name.as_str(), choice.user_display.as_str()), ("paulus", "PAULUS"));
    assert_eq!(choice.assertion.credential_id, made.credential_id);
    assert_eq!(b64::decode_url(&choice.assertion.user_handle).unwrap(), b"user-1");
    assert!(verify_es256(&made.public_key, &choice.assertion.authenticator_data, &hash, &choice.assertion.signature));

    // Made again for the same account: it takes the old one's place.
    let again: CreatedAnswer = answer(ask(&service, next_conn(), create(b"user-1", "paulus", vec![])));
    assert_ne!(again.credential_id, made.credential_id);
    let choices: ChoicesAnswer = answer(ask(&service, next_conn(), sign_in("https://github.com", "github.com", vec![])));
    assert_eq!(choices.choices.len(), 1);
    assert_eq!(choices.choices[0].assertion.credential_id, again.credential_id);
    // A second account: two answers for one touch, the newest first.
    std::thread::sleep(Duration::from_millis(5));
    let second: CreatedAnswer = answer(ask(&service, next_conn(), create(b"user-2", "paulus-work", vec![])));
    let unlocks = guard.unlocks();
    let choices: ChoicesAnswer = answer(ask(&service, next_conn(), sign_in("https://github.com", "github.com", vec![])));
    assert_eq!(guard.unlocks(), unlocks + 1);
    let names: Vec<&str> = choices.choices.iter().map(|c| c.user_name.as_str()).collect();
    assert_eq!(names, vec!["paulus-work", "paulus"]);
    // The site's allow list narrows it, padded or not.
    let allow = vec![format!("{}==", second.credential_id)];
    let choices: ChoicesAnswer = answer(ask(&service, next_conn(), sign_in("https://github.com", "github.com", allow)));
    assert_eq!(choices.choices.len(), 1);
    assert_eq!(choices.choices[0].assertion.credential_id, second.credential_id);
    // A subdomain's page may use the domain's passkeys.
    let choices: ChoicesAnswer = answer(ask(&service, next_conn(), sign_in("https://gist.github.com", "github.com", vec![])));
    assert_eq!(choices.choices.len(), 2);

    // The site already has SpatiKey's for this account: refused, as WebAuthn's InvalidStateError.
    assert_eq!(code(ask(&service, next_conn(), create(b"user-1", "paulus", vec![again.credential_id.clone()]))), Code::Excluded);
    // Another site's page, an http page, keys the site does not take: refused before any touch.
    let unlocks = guard.unlocks();
    assert_eq!(code(ask(&service, next_conn(), sign_in("https://evil.example", "github.com", vec![]))), Code::BadRequest);
    assert_eq!(code(ask(&service, next_conn(), sign_in("http://github.com", "github.com", vec![]))), Code::BadRequest);
    let mut rsa_only = create(b"user-3", "x", vec![]);
    if let Request::PasskeyCreate(request) = &mut rsa_only {
        request.algs = vec![-257];
    }
    assert_eq!(code(ask(&service, next_conn(), rsa_only)), Code::BadRequest);
    assert_eq!(guard.unlocks(), unlocks);
    // A site with none: no match, after the touch that looked.
    assert_eq!(code(ask(&service, next_conn(), sign_in("https://gitlab.com", "gitlab.com", vec![]))), Code::NoMatch);
}

#[test]
#[ignore = "needs site/totp/webauthn"]
fn two_factor_codes_in_a_session_and_alone() {
    let place = Place::new("totp");
    let (service, guard, _key) = ready(&place);
    let conn = next_conn();
    let session = open(&service, conn);
    let secret = Totp { secret: "JBSWY3DPEHPK3PXP".into(), digits: 6, period: 30, algorithm: "SHA1".into(), ..Totp::default() };
    let mut entry = login("GitHub", "github.com", "paulus", "hunter2");
    entry.totp = Some(secret.clone());
    let id = put(&service, conn, &session, entry);
    let bare = put(&service, conn, &session, login("Steam", "steampowered.com", "paulus", "gaben"));
    let expected = |now: u64| spatikey_core::totp::code(&secret, now).unwrap().0;

    let before = spatikey_core::now_ms() / 1000;
    let unlocks = guard.unlocks();
    let got: TotpAnswer = answer(ask(&service, conn, Request::Totp { session: Some(session.clone()), id: id.clone() }));
    let after = spatikey_core::now_ms() / 1000;
    assert!(got.code == expected(before) || got.code == expected(after));
    assert!(got.period == 30 && (1..=30).contains(&got.remaining));
    assert_eq!(guard.unlocks(), unlocks, "in a session: no finger");
    let alone: TotpAnswer = answer(ask(&service, next_conn(), Request::Totp { session: None, id: id.clone() }));
    assert_eq!(alone.code.len(), 6);
    assert_eq!(guard.last_reason(), "show the code for GitHub");
    assert_eq!(code(ask(&service, conn, Request::Totp { session: Some(session.clone()), id: bare })), Code::NoMatch);
    let revealed: RevealAnswer =
        answer(ask(&service, conn, Request::Reveal { session: Some(session), id, fields: vec![Field::Totp] }));
    assert!(revealed.values[0].len() == 6 && revealed.values[0].bytes().all(|b| b.is_ascii_digit()));
}
