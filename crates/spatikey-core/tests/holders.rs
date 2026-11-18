//! Every device with biometrics holds the vault (docs/spatikey.md, "Every device with biometrics
//! holds the vault"): a second holder asked for and added, either one opening the vault, one
//! removed and brought back by the recovery code, a vault set up with one Mac before there were
//! several taken as it is, and a device without biometrics asking whichever holder answers.
//!
//! Each holder's key is a P-256 key in software standing in for the Secure Enclave's or the
//! Keystore's, doing the very ECDH they do: what a holder wraps for another is opened as that
//! other's sensor would open it.

use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};

use p256::elliptic_curve::sec1::ToEncodedPoint;
use serde::de::DeserializeOwned;
use serde_json::Value;
use spatiand_stream::Identity;
use spatikey_core::hooks::{self, Room, Step, Target};
use spatikey_core::lan::{self, Lan};
use spatikey_core::remote::{self, Asker, Network, Ports};
use spatikey_core::service::{next_conn, ConnId, Guard, Me, Mode, Service};
use spatikey_core::vault::{TouchDevice, TouchId, Vault, VaultKey};
use spatikey_core::wire::*;
use spatikey_core::{b64, holders, Entry, Kind};

/// The room is one for the whole process, and a device asking a holder shows its card there: the
/// tests take turns.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    ONE_AT_A_TIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Argon2id at its least cost: the recovery code's derivation, quickly.
const TINY: (u32, u32, u32) = (8, 1, 1);

/// A holder's sensor in software: its key is the blob (as the Secure Enclave's blob or the
/// Keystore alias is), and every unlock does the ECDH the real one does once a finger is given.
struct SoftHolder {
    kind: &'static str,
    unlocks: AtomicUsize,
    reasons: Mutex<Vec<String>>,
    lingers: Mutex<Vec<u64>>,
    notices: Mutex<Vec<String>>,
    /// The sensor cannot be used now (a Mac's lid shut).
    no_touch: AtomicBool,
    /// The person says no.
    refuse: AtomicBool,
}

impl SoftHolder {
    fn new(kind: &'static str) -> Arc<SoftHolder> {
        Arc::new(SoftHolder {
            kind,
            unlocks: AtomicUsize::new(0),
            reasons: Mutex::default(),
            lingers: Mutex::default(),
            notices: Mutex::default(),
            no_touch: AtomicBool::new(false),
            refuse: AtomicBool::new(false),
        })
    }

    fn unlocks(&self) -> usize {
        self.unlocks.load(SeqCst)
    }

    fn last_reason(&self) -> String {
        self.reasons.lock().unwrap().last().cloned().unwrap_or_default()
    }
}

/// What a holder's sensor does with its private key: the ECDH with the wrap's ephemeral key.
fn soft_open(touch: &TouchId) -> Result<VaultKey, String> {
    let secret = p256::SecretKey::from_slice(&b64::decode(&touch.blob)?).map_err(|e| e.to_string())?;
    let wrapped = b64::decode(&touch.wrapped)?;
    let public = b64::decode(&touch.public)?;
    if wrapped.len() != holders::WRAPPED_LEN {
        return Err("not a wrap".into());
    }
    let ephemeral = p256::PublicKey::from_sec1_bytes(&wrapped[..holders::PUBLIC_LEN]).map_err(|e| e.to_string())?;
    let shared = p256::ecdh::diffie_hellman(secret.to_nonzero_scalar(), ephemeral.as_affine());
    holders::open_with_shared(shared.raw_secret_bytes(), &public, &wrapped)
}

impl Guard for SoftHolder {
    fn unlock(&self, reason: &str, touch: &TouchId, linger_secs: u64) -> Result<VaultKey, Refusal> {
        self.unlocks.fetch_add(1, SeqCst);
        self.reasons.lock().unwrap().push(reason.to_string());
        self.lingers.lock().unwrap().push(linger_secs);
        if self.no_touch.load(SeqCst) {
            return Err(Refusal::new(Code::TouchIdUnavailable, "the lid is shut"));
        }
        if self.refuse.load(SeqCst) {
            return Err(Refusal::new(Code::Denied, "the person said no"));
        }
        soft_open(touch).map_err(|e| Refusal::new(Code::FingerprintsChanged, e))
    }

    fn enroll(&self, key: &VaultKey) -> Result<TouchId, Refusal> {
        let mut made = self.make()?;
        made.wrapped = b64::encode(&holders::wrap(key, &b64::decode(&made.public).unwrap()).unwrap());
        Ok(made)
    }

    fn make(&self) -> Result<TouchId, Refusal> {
        let secret = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let public = secret.public_key().to_encoded_point(false).as_bytes().to_vec();
        Ok(TouchId { vault: String::new(), public: b64::encode(&public), blob: b64::encode(&secret.to_bytes()), wrapped: String::new() })
    }

    fn kind(&self) -> &'static str {
        self.kind
    }

    fn can_touch(&self) -> bool {
        !self.no_touch.load(SeqCst)
    }

    fn allow_device(&self, _name: &str, _fp: &str, _code: &str) -> bool {
        true
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
        let root = std::env::temp_dir().join(format!("skh-{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, SeqCst)));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Place { root }
    }

    /// The synced folder, the same for every device.
    fn shared(&self) -> PathBuf {
        self.root.join("apps").join("spatikey")
    }

    /// One device's own folder.
    fn local(&self, device: &str) -> PathBuf {
        self.root.join(device).join("local").join("spatikey")
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn ask_from(service: &Service, conn: ConnId, who: Who, request: Request) -> Result<Value, Refusal> {
    let reply = service.handle(conn, Envelope { id: 5, who, request });
    assert_eq!(reply.id, 5);
    reply.into_result()
}

fn ask(service: &Service, conn: ConnId, request: Request) -> Result<Value, Refusal> {
    ask_from(service, conn, Who::default(), request)
}

fn answer<T: DeserializeOwned>(answered: Result<Value, Refusal>) -> T {
    match answered {
        Ok(value) => serde_json::from_value(value).expect("an answer of the documented shape"),
        Err(refusal) => panic!("refused: {refusal}"),
    }
}

fn code(answered: Result<Value, Refusal>) -> Code {
    match answered {
        Ok(value) => panic!("answered {value} where a refusal was expected"),
        Err(refusal) => refusal.code,
    }
}

fn status(service: &Service) -> StatusAnswer {
    answer(ask(service, next_conn(), Request::Status))
}

fn open(service: &Service, conn: ConnId) -> OpenAnswer {
    answer(ask(service, conn, Request::Open))
}

fn login(title: &str, password: &str) -> Entry {
    Entry {
        kind: Kind::Login,
        title: title.into(),
        sites: vec!["example.com".into()],
        username: "paulus".into(),
        password: password.into(),
        ..Entry::default()
    }
}

fn loopback() -> Network {
    let name = if cfg!(any(target_os = "macos", target_os = "ios")) { "lo0" } else { "lo" };
    Network::fixed(vec![Lan {
        name: name.into(),
        index: lan::index_of(name).expect("a loopback interface"),
        addr: IpAddr::V4(Ipv4Addr::LOCALHOST),
        prefix: 8,
        broadcast: Some(Ipv4Addr::LOCALHOST),
    }])
}

/// A device's asking half that is never asked to reach anybody here: no holder is paired with
/// it, and its broadcasts would go to loopback's discard port.
fn idle_asker(identity: &Identity, local: &Path, name: &str) -> Arc<Asker> {
    Arc::new(Asker::with_network(identity.clone(), local.to_path_buf(), name.into(), loopback(), 9))
}

/// One holder and the vault, as `setup` leaves them (the recovery code at the least cost), with
/// its record written as SpatiKey wrote it before there were several holders.
struct First {
    service: Arc<Service>,
    guard: Arc<SoftHolder>,
    identity: Identity,
    key: VaultKey,
    recovery: String,
}

fn first(place: &Place, name: &str) -> First {
    let identity = Identity::new().unwrap();
    let fp = identity.fingerprint().to_string();
    let guard = SoftHolder::new(holders::MAC);
    let (shared, local) = (place.shared(), place.local(name));
    let (vault, key) = Vault::create(&shared, &local, &fp[..8]).unwrap();
    let touch = guard.enroll(&key).unwrap();
    vault.set_touch_id(&touch).unwrap();
    let old_record = serde_json::json!({
        "device": fp, "name": name, "kind": "mac-touch-id", "public": touch.public, "wrapped": touch.wrapped, "added": 1_794_623_429_677u64,
    });
    std::fs::write(shared.join("devices").join(format!("{fp}.json")), serde_json::to_vec_pretty(&old_record).unwrap()).unwrap();
    let recovery = vault.set_recovery_with(&key, TINY).unwrap();
    let service = Service::new(Mode::Approver(guard.clone()), Me { fp, name: name.into() }, shared, local);
    First { service, guard, identity, key, recovery }
}

/// A device with biometrics of its own, which may hold the vault and asks a holder until it does.
struct Next {
    service: Arc<Service>,
    guard: Arc<SoftHolder>,
    identity: Identity,
}

fn next(place: &Place, name: &str, kind: &'static str) -> Next {
    let identity = Identity::new().unwrap();
    let guard = SoftHolder::new(kind);
    let local = place.local(name);
    let me = Me { fp: identity.fingerprint().to_string(), name: name.into() };
    let mode = Mode::Both(guard.clone(), idle_asker(&identity, &local, name));
    let service = Service::new(mode, me, place.shared(), local);
    Next { service, guard, identity }
}

/// `next` asks to be added, and `first` adds it, as their owner would from SpatiKey on each.
fn add(first: &First, next: &Next) {
    let code = answer::<HolderAnswer>(ask(&next.service, next_conn(), Request::HolderRequest { name: String::new() })).code;
    let conn = next_conn();
    let session = open(&first.service, conn).session;
    let request = Request::HolderApprove { session: session.clone(), fp: next.identity.fingerprint().to_string(), code };
    answer::<Value>(ask(&first.service, conn, request));
    answer::<Value>(ask(&first.service, conn, Request::Close { session }));
}

#[test]
fn a_device_with_biometrics_is_added_by_a_holder_and_opens_the_same_vault() {
    let _turn = one_at_a_time();
    let place = Place::new("added");
    let mac = first(&place, "darkPhoenix");
    let phone = next(&place, "Pixel", holders::ANDROID);
    let phone_fp = phone.identity.fingerprint().to_string();

    // The vault has reached the phone, which cannot open it yet.
    let before = status(&phone.service);
    assert!(before.vault && !before.opens_here && before.can_set_up);
    assert_eq!((before.kind.as_str(), before.request_code.as_str()), (holders::ANDROID, ""));
    assert_eq!(before.holders.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), ["darkPhoenix"]);
    // Until it holds the vault, the phone asks a holder, as the Beam Pro does: none is paired here.
    assert_eq!(code(ask(&phone.service, next_conn(), Request::Open)), Code::NoApprover);

    // It asks to be added: its key made, nothing asked of anybody.
    let asked = answer::<HolderAnswer>(ask(&phone.service, next_conn(), Request::HolderRequest { name: "Pixel 9".into() }));
    assert_eq!(asked.code.len(), 7);
    assert_eq!(phone.guard.unlocks(), 0);
    let again = answer::<HolderAnswer>(ask(&phone.service, next_conn(), Request::HolderRequest { name: String::new() }));
    assert_eq!(again.code, asked.code, "the same request while it waits, with the same digits");
    assert_eq!(status(&phone.service).request_code, asked.code);
    // A device that only asks has no key to make.
    let deck = Service::new(
        Mode::Forwarder(idle_asker(&Identity::new().unwrap(), &place.local("deck"), "Steam Deck")),
        Me { fp: "d0".repeat(32), name: "Steam Deck".into() },
        place.shared(),
        place.local("deck"),
    );
    assert_eq!(code(ask(&deck, next_conn(), Request::HolderRequest { name: String::new() })), Code::NotHere);

    // On the Mac, in SpatiKey: the request, with the digits the phone shows.
    let conn = next_conn();
    let session = open(&mac.service, conn).session;
    let unlocks = mac.guard.unlocks();
    let devices: DevicesAnswer = answer(ask(&mac.service, conn, Request::Devices { session: session.clone() }));
    assert_eq!(devices.requests.len(), 1);
    let request = &devices.requests[0];
    assert_eq!((request.fp.as_str(), request.name.as_str(), request.kind.as_str()), (phone_fp.as_str(), "Pixel 9", holders::ANDROID));
    assert_eq!(request.code, asked.code);
    // Other digits than the phone's: refused before any touch.
    let approve = |code: &str| Request::HolderApprove { session: session.clone(), fp: phone_fp.clone(), code: code.into() };
    let wrong = if asked.code == "000 000" { "000 001" } else { "000 000" };
    assert_eq!(code(ask(&mac.service, conn, approve(wrong))), Code::Denied);
    assert_eq!(mac.guard.unlocks(), unlocks, "no finger is asked for a request that is not the one compared");
    // The person says no on the Mac: nothing is added.
    mac.guard.refuse.store(true, SeqCst);
    assert_eq!(code(ask(&mac.service, conn, approve(&asked.code))), Code::Denied);
    mac.guard.refuse.store(false, SeqCst);
    assert!(!status(&phone.service).opens_here);
    // Yes: a new touch, whatever the session covers, and the vault key wrapped to the phone's key.
    answer::<Value>(ask(&mac.service, conn, approve(&asked.code)));
    assert_eq!(mac.guard.unlocks(), unlocks + 2);
    assert_eq!(mac.guard.last_reason(), "add Pixel 9 to the devices that open SpatiKey");
    assert_eq!(*mac.guard.lingers.lock().unwrap().last().unwrap(), 0, "a touch that covers nothing else");
    let devices: DevicesAnswer = answer(ask(&mac.service, conn, Request::Devices { session: session.clone() }));
    assert!(devices.requests.is_empty());
    assert_eq!(
        devices.holders.iter().map(|h| (h.name.as_str(), h.kind.as_str())).collect::<Vec<_>>(),
        [("darkPhoenix", holders::MAC), ("Pixel 9", holders::ANDROID)]
    );
    assert_eq!(devices.touch_ids.len(), 1, "the old answer's list has the Macs only");
    assert_eq!(code(ask(&mac.service, conn, approve(&asked.code))), Code::NoMatch, "added once");

    // The phone takes its half up from the synced folder, and opens the vault with its own finger.
    let mut entry = login("Example", "hunter2");
    let id = answer::<IdAnswer>(ask(&mac.service, conn, Request::Put { session: session.clone(), entry: entry.clone() })).id;
    let after = status(&phone.service);
    assert!(after.opens_here && after.request_code.is_empty(), "{after:?}");
    let vault = Vault::open(&place.shared(), &place.local("Pixel")).unwrap();
    assert!(vault.pending().is_none() && vault.touch_id().is_some() && vault.askings().is_empty());
    let phone_conn = next_conn();
    let opened = open(&phone.service, phone_conn);
    assert_eq!(phone.guard.unlocks(), 1);
    assert_eq!(opened.entries.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["Example"]);
    assert_eq!(*phone.guard.notices.lock().unwrap(), ["Fingerprint on this phone to open SpatiKey", ""]);
    let got: GetAnswer = answer(ask(&phone.service, phone_conn, Request::Get { session: opened.session.clone(), id: id.clone() }));
    assert_eq!(got.entry.password, "hunter2");
    // What the phone saves the Mac reads.
    entry.id = id.clone();
    entry.password = "correct horse".into();
    answer::<IdAnswer>(ask(&phone.service, phone_conn, Request::Put { session: opened.session, entry }));
    let got: GetAnswer = answer(ask(&mac.service, conn, Request::Get { session, id }));
    assert_eq!(got.entry.password, "correct horse");
    // Nothing of the vault key in anything synced.
    for file in walk(&place.shared()) {
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains(&b64::encode(mac.key.bytes())), "{} holds no key in the clear", file.display());
    }
}

#[test]
fn a_holder_removed_lets_go_and_the_recovery_code_brings_it_back() {
    let _turn = one_at_a_time();
    let place = Place::new("removed");
    let mac = first(&place, "darkPhoenix");
    let phone = next(&place, "Pixel", holders::ANDROID);
    let phone_fp = phone.identity.fingerprint().to_string();
    add(&mac, &phone);
    assert!(status(&phone.service).opens_here);

    // Removed from the Mac, with a touch there.
    let conn = next_conn();
    let session = open(&mac.service, conn).session;
    let unlocks = mac.guard.unlocks();
    let forget = |fp: &str| Request::ForgetDevice { session: session.clone(), fp: fp.into(), recovery_code: None };
    assert_eq!(code(ask(&mac.service, conn, forget(&mac.identity.fingerprint().to_string()))), Code::BadRequest);
    mac.guard.refuse.store(true, SeqCst);
    assert_eq!(code(ask(&mac.service, conn, forget(&phone_fp))), Code::Denied);
    mac.guard.refuse.store(false, SeqCst);
    assert!(status(&phone.service).opens_here, "a removal the Mac's owner said no to removes nothing");
    answer::<Value>(ask(&mac.service, conn, forget(&phone_fp)));
    assert_eq!(mac.guard.unlocks(), unlocks + 2);
    assert_eq!(mac.guard.last_reason(), "remove Pixel from the devices that open SpatiKey");

    // The phone sees it is removed and lets its half go: it opens nothing any more.
    let after = status(&phone.service);
    assert!(!after.opens_here && after.can_set_up);
    assert_eq!(after.holders.len(), 1);
    assert!(!place.local("Pixel").join("touch-id.json").exists());
    let unlocks = phone.guard.unlocks();
    assert_eq!(code(ask(&phone.service, next_conn(), Request::Open)), Code::NoApprover, "it asks a holder again, as any device without one");
    assert_eq!(phone.guard.unlocks(), unlocks);

    // The recovery code, typed on the phone, makes it a holder again.
    answer::<Value>(ask(&phone.service, next_conn(), Request::Recover { code: mac.recovery.clone() }));
    assert!(status(&phone.service).opens_here);
    let vault = Vault::open(&place.shared(), &place.local("Pixel")).unwrap();
    let record = vault.touch_device(&phone_fp).unwrap();
    assert!(!record.removed && record.kind == holders::ANDROID && record.by.is_empty());
    open(&phone.service, next_conn());
}

#[test]
fn a_vault_set_up_with_one_mac_is_taken_as_it_is_and_takes_a_second_holder() {
    let _turn = one_at_a_time();
    let place = Place::new("migrated");
    let mac = first(&place, "darkPhoenix");
    let fp = mac.identity.fingerprint().to_string();
    let record_file = place.shared().join("devices").join(format!("{fp}.json"));
    let files_before: Vec<(PathBuf, Vec<u8>)> = walk(&place.root).into_iter().map(|f| (f.clone(), std::fs::read(&f).unwrap())).collect();

    // The Mac as it was: holder #1, opening the vault with its finger, its record left alone.
    let mine = status(&mac.service);
    assert!(mine.vault && mine.opens_here && !mine.can_set_up);
    assert_eq!(
        mine.holders,
        vec![Holder { fp: fp.clone(), name: "darkPhoenix".into(), kind: holders::MAC.into(), added: 1_794_623_429_677 }]
    );
    let conn = next_conn();
    let session = open(&mac.service, conn).session;
    answer::<IdAnswer>(ask(&mac.service, conn, Request::Put { session, entry: login("Example", "hunter2") }));
    for (file, bytes) in &files_before {
        assert_eq!(&std::fs::read(file).unwrap(), bytes, "{} is as it was", file.display());
    }
    // The recovery code is the same code, for the same key.
    let vault = Vault::open(&place.shared(), &place.local("darkPhoenix")).unwrap();
    assert_eq!(vault.recover(&mac.recovery).unwrap().bytes(), mac.key.bytes());

    // A second holder is added to it as to any other.
    let phone = next(&place, "Pixel", holders::ANDROID);
    add(&mac, &phone);
    assert!(status(&phone.service).opens_here);
    assert_eq!(vault.recover(&mac.recovery).unwrap().bytes(), mac.key.bytes(), "the recovery code is untouched");

    // A holder whose record is not in the synced folder (lost, or from before there were
    // records) writes it again from its own half the first time it looks.
    std::fs::remove_file(&record_file).unwrap();
    assert_eq!(status(&mac.service).holders.len(), 2);
    let record: TouchDevice = serde_json::from_slice(&std::fs::read(&record_file).unwrap()).unwrap();
    assert_eq!((record.device.as_str(), record.kind.as_str(), record.removed), (fp.as_str(), holders::MAC, false));
    assert_eq!(record.public, vault.touch_id().unwrap().public);
}

/// The room on the device without biometrics: what its card says.
#[derive(Default)]
struct Glasses {
    said: Mutex<Vec<String>>,
}

impl Room for Glasses {
    fn target(&self) -> Option<Target> {
        None
    }

    fn type_into(&self, _window: u64, _steps: &[Step]) -> Result<(), String> {
        Err("no typing here".into())
    }

    fn notice(&self, text: &str) {
        self.said.lock().unwrap().push(text.to_string());
    }
}

#[test]
fn a_device_without_biometrics_asks_whichever_holder_answers() {
    let _turn = one_at_a_time();
    let place = Place::new("forwarded");
    let mac = first(&place, "darkPhoenix");
    let phone = next(&place, "Pixel", holders::ANDROID);
    add(&mac, &phone);

    // Both holders answer devices on the network (loopback standing in for it, a port each).
    let deck_identity = Identity::new().unwrap();
    let deck_fp = deck_identity.fingerprint().to_string();
    for service in [&mac.service, &phone.service] {
        service.allow(Device { fp: deck_fp.clone(), name: "Steam Deck".into(), added: 0 }).unwrap();
    }
    let any = Ports { discovery: 0, ask: 0 };
    let mac_listening = remote::serve_with(mac.service.clone(), mac.identity.clone(), loopback(), any).unwrap();
    let phone_listening = remote::serve_with(phone.service.clone(), phone.identity.clone(), loopback(), any).unwrap();
    let ports = vec![mac_listening.ports().discovery, phone_listening.ports().discovery];

    // The Deck, paired with both.
    let deck_local = place.local("deck");
    std::fs::create_dir_all(&deck_local).unwrap();
    let approvers = [
        Device { fp: mac.identity.fingerprint().to_string(), name: "darkPhoenix".into(), added: 1 },
        Device { fp: phone.identity.fingerprint().to_string(), name: "Pixel".into(), added: 2 },
    ];
    std::fs::write(deck_local.join("approvers.json"), serde_json::to_vec(&approvers).unwrap()).unwrap();
    let asker = Arc::new(Asker::with_ports(deck_identity, deck_local.clone(), "Steam Deck".into(), loopback(), ports));
    let deck = Service::new(Mode::Forwarder(asker.clone()), Me { fp: deck_fp, name: "Steam Deck".into() }, place.shared(), deck_local);
    let glasses = Arc::new(Glasses::default());
    hooks::set_room(Some(glasses.clone()));
    let said = || std::mem::take(&mut *glasses.said.lock().unwrap());
    let name_of = |fp: &str| if *fp == mac.identity.fingerprint().to_string() { "darkPhoenix" } else { "Pixel" };
    let guard_of = |fp: &str| if *fp == mac.identity.fingerprint().to_string() { mac.guard.clone() } else { phone.guard.clone() };

    // Whichever answers first is asked, and the card in the glasses names it.
    let status: StatusAnswer = answer(ask(&deck, next_conn(), Request::Status));
    assert!(status.approvers.iter().all(|a| a.near) && status.approvers.len() == 2, "{status:?}");
    assert_eq!(status.holders.len(), 2);
    let opened = open(&deck, next_conn());
    assert!(opened.entries.is_empty());
    let asked = asker.last().expect("a holder answered");
    let first_card = said();
    let named = |what: &str, fp: &str| {
        let kind = if name_of(fp) == "Pixel" { holders::ANDROID } else { holders::MAC };
        spatikey_core::service::holder_card(kind, name_of(fp), what)
    };
    assert_eq!(first_card.first().map(String::as_str), Some("Touch ID on your Mac to open SpatiKey"));
    assert!(first_card.contains(&named("open SpatiKey", &asked)), "{first_card:?}");
    assert_eq!(first_card.last().map(String::as_str), Some(""));
    assert_eq!(guard_of(&asked).last_reason(), "open SpatiKey on Steam Deck");

    // The one asked last is asked again.
    let fill = || Request::Fill { origin: "https://example.com".into() };
    assert_eq!(code(ask(&deck, next_conn(), fill())), Code::NoMatch);
    assert_eq!(asker.last().as_deref(), Some(asked.as_str()));

    // Its sensor cannot be used now (a lid shut): the other is asked in its place, and named.
    let other = [mac.identity.fingerprint().to_string(), phone.identity.fingerprint().to_string()]
        .into_iter()
        .find(|fp| *fp != asked)
        .unwrap();
    guard_of(&asked).no_touch.store(true, SeqCst);
    said();
    let before = guard_of(&other).unlocks();
    open(&deck, next_conn());
    assert_eq!(guard_of(&other).unlocks(), before + 1);
    assert_eq!(asker.last().as_deref(), Some(other.as_str()));
    let cards = said();
    assert!(cards.contains(&named("open SpatiKey", &asked)) && cards.contains(&named("open SpatiKey", &other)), "{cards:?}");
    assert_eq!(cards.last().map(String::as_str), Some(""));

    // Both cannot: the last one's reason is the answer.
    guard_of(&other).no_touch.store(true, SeqCst);
    assert_eq!(code(ask(&deck, next_conn(), Request::Open)), Code::TouchIdUnavailable);
    guard_of(&asked).no_touch.store(false, SeqCst);
    guard_of(&other).no_touch.store(false, SeqCst);

    // One gone from the network altogether: the one still there answers.
    let gone_mac = asker.last().as_deref() == Some(mac.identity.fingerprint().to_string().as_str());
    if gone_mac {
        drop(mac_listening);
        open(&deck, next_conn());
        assert_eq!(asker.last(), Some(phone.identity.fingerprint().to_string()));
        drop(phone_listening);
    } else {
        drop(phone_listening);
        open(&deck, next_conn());
        assert_eq!(asker.last(), Some(mac.identity.fingerprint().to_string()));
        drop(mac_listening);
    }
    hooks::set_room(None);
    drop(deck);
}

/// Every file under `dir`.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(listing) = std::fs::read_dir(dir) else { return out };
    for entry in listing.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out.sort();
    out
}

#[test]
fn removing_a_holder_gives_the_vault_new_keys_that_the_others_follow() {
    let _turn = one_at_a_time();
    let place = Place::new("rekeyed");
    let mac = first(&place, "darkPhoenix");
    let phone = next(&place, "Pixel", holders::ANDROID);
    let tablet = next(&place, "Tablet", holders::ANDROID);
    let (phone_fp, tablet_fp) = (phone.identity.fingerprint().to_string(), tablet.identity.fingerprint().to_string());
    add(&mac, &phone);
    add(&mac, &tablet);
    let vault = Vault::open(&place.shared(), &place.local("darkPhoenix")).unwrap();

    // SpatiKey open on all three, with something in it.
    let (mac_conn, phone_conn, tablet_conn) = (next_conn(), next_conn(), next_conn());
    let session = open(&mac.service, mac_conn).session;
    let put = |service: &Service, conn, session: &str, title: &str| {
        answer::<IdAnswer>(ask(service, conn, Request::Put { session: session.into(), entry: login(title, &format!("{title}-secret")) })).id
    };
    put(&mac.service, mac_conn, &session, "GitHub");
    let phone_session = open(&phone.service, phone_conn).session;
    put(&phone.service, phone_conn, &phone_session, "Bank");
    let tablet_session = open(&tablet.service, tablet_conn).session;
    // What the tablet could keep for later: its half, and through it the vault key.
    let kept: TouchId = serde_json::from_slice(&std::fs::read(place.local("Tablet").join("touch-id.json")).unwrap()).unwrap();
    let old = soft_open(&kept).unwrap();
    assert_eq!(old.bytes(), mac.key.bytes());

    // The Mac removes the tablet, with a touch.
    let forget = |fp: &str, recovery_code: Option<String>| Request::ForgetDevice { session: session.clone(), fp: fp.into(), recovery_code };
    let done: ForgetAnswer = answer(ask(&mac.service, mac_conn, forget(&tablet_fp, None)));
    assert!(done.rekeyed && done.recovery_code.is_empty() && done.left_out.is_empty(), "{done:?}");
    // The Mac's open SpatiKey goes on, with the new key; and writes with it.
    assert_eq!(answer::<ListAnswer>(ask(&mac.service, mac_conn, Request::List { session: session.clone() })).entries.len(), 2);
    put(&mac.service, mac_conn, &session, "Steam");
    // The key the tablet had opens nothing of the vault as it now is.
    assert_eq!(vault.entries(&old).unwrap_err(), spatikey_core::vault::REKEYED);
    let new = vault.recover(&mac.recovery).expect("the same recovery code");
    assert_ne!(new.bytes(), old.bytes());
    assert_eq!(vault.entries(&new).unwrap().len(), 3);
    // The tablet: its open SpatiKey is shut, it lets its half go, and what it kept gives the
    // old key and no more.
    assert_eq!(code(ask(&tablet.service, tablet_conn, Request::List { session: tablet_session })), Code::BadSession);
    assert!(!status(&tablet.service).opens_here);
    assert!(!place.local("Tablet").join("touch-id.json").exists() && !place.local("Tablet").join("follow.json").exists());
    assert_eq!(soft_open(&kept).unwrap().bytes(), old.bytes());
    for file in walk(&place.shared()) {
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains(&kept.public) && !text.contains(&kept.wrapped), "{} still has something of the tablet's", file.display());
    }

    // The phone's open SpatiKey is told to open again; opened again, with one touch, it has
    // everything, and what it writes the Mac reads.
    assert_eq!(code(ask(&phone.service, phone_conn, Request::List { session: phone_session })), Code::BadSession);
    let unlocks = phone.guard.unlocks();
    let opened = open(&phone.service, phone_conn);
    assert_eq!(phone.guard.unlocks(), unlocks + 1, "following the new keys is the one touch it always was");
    assert_eq!(opened.entries.len(), 3);
    put(&phone.service, phone_conn, &opened.session, "Forum");
    assert_eq!(answer::<ListAnswer>(ask(&mac.service, mac_conn, Request::List { session: session.clone() })).entries.len(), 4);
    let unlocks = phone.guard.unlocks();
    assert_eq!(open(&phone.service, next_conn()).entries.len(), 4);
    assert_eq!(phone.guard.unlocks(), unlocks + 1);

    // The recovery code, typed on the tablet, makes it a holder again, and it stays one when
    // the others next open the vault.
    answer::<Value>(ask(&tablet.service, next_conn(), Request::Recover { code: mac.recovery.clone() }));
    assert_eq!(open(&tablet.service, next_conn()).entries.len(), 4);
    open(&mac.service, next_conn());
    open(&phone.service, next_conn());
    assert!(status(&tablet.service).opens_here);
    assert_eq!(vault.touch_devices().len(), 3);

    // A recovery code typed with a removal is to be the vault's: a wrong one removes nobody.
    let now = vault.recover(&mac.recovery).unwrap();
    assert_eq!(code(ask(&mac.service, mac_conn, forget(&tablet_fp, Some("not a code".into())))), Code::BadRequest);
    assert_eq!(code(ask(&mac.service, mac_conn, forget(&tablet_fp, Some(spatikey_core::recovery::new_code())))), Code::Denied);
    assert_eq!(vault.touch_devices().len(), 3, "a wrong code removes nobody");
    let done: ForgetAnswer = answer(ask(&mac.service, mac_conn, forget(&tablet_fp, Some(mac.recovery.to_lowercase()))));
    assert!(done.rekeyed && done.recovery_code.is_empty());
    let third = vault.recover(&mac.recovery).unwrap();
    assert_ne!(third.bytes(), now.bytes());
    assert_eq!(vault.entries(&third).unwrap().len(), 4);
    let _ = phone_fp;
}

#[test]
fn a_vault_from_before_keeps_its_recovery_code_if_typed_and_is_given_a_new_one_if_not() {
    let _turn = one_at_a_time();
    for typed in [true, false] {
        let place = Place::new(if typed { "code-typed" } else { "code-new" });
        let mac = first(&place, "darkPhoenix");
        let phone = next(&place, "Pixel", holders::ANDROID);
        add(&mac, &phone);
        let vault = Vault::open(&place.shared(), &place.local("darkPhoenix")).unwrap();
        // The record of the code as SpatiKey wrote it before 2026-11-18: only the code itself
        // can wrap another vault key to it.
        let before = spatikey_core::recovery::wrap_with(&mac.key, &mac.recovery, vault.id(), TINY).unwrap();
        std::fs::write(place.shared().join("recovery.json"), serde_json::to_vec(&before).unwrap()).unwrap();
        let conn = next_conn();
        let session = open(&mac.service, conn).session;
        answer::<IdAnswer>(ask(&mac.service, conn, Request::Put { session: session.clone(), entry: login("GitHub", "hunter2") }));
        let recovery_code = typed.then(|| mac.recovery.clone());
        let forget = Request::ForgetDevice { session: session.clone(), fp: phone.identity.fingerprint().to_string(), recovery_code };
        let done: ForgetAnswer = answer(ask(&mac.service, conn, forget));
        assert!(done.rekeyed);
        let code_now = if typed {
            assert!(done.recovery_code.is_empty(), "the code typed is the code kept");
            mac.recovery.clone()
        } else {
            assert!(!done.recovery_code.is_empty(), "a new code, to be shown once");
            assert!(vault.recover(&mac.recovery).is_err(), "the code before opens the vault no more");
            done.recovery_code.clone()
        };
        let key = vault.recover(&code_now).unwrap();
        assert_ne!(key.bytes(), mac.key.bytes());
        assert_eq!(vault.entries(&key).unwrap().len(), 1);
        assert_eq!(vault.touch_devices().len(), 1);
        assert!(!status(&phone.service).opens_here);
        assert_eq!(open(&mac.service, next_conn()).entries.len(), 1);
    }
}
