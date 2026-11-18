//! The service: the only thing that reads the vault. docs/spatikey.md, "Asking for something".
//!
//! On a device with biometrics -- a Mac with Touch ID, an Android device with a strong sensor --
//! it is an *approver*: it opens the vault with the [`Guard`] (the platform's half) once it holds
//! it (docs/spatikey.md, "Every device with biometrics holds the vault"), and answers. On the Deck
//! and the Beam Pro it is a *forwarder*: it sends each request to a holder on the same network
//! ([`crate::remote::Asker`]) and keeps for itself only what needs this machine -- typing into its
//! windows, the window to type into, finding and pairing holders. A phone with a sensor that does
//! not hold the vault yet is both ([`Mode::Both`]).
//!
//! An open SpatiKey is a *session*: the touch that opened it gave the vault key, which the
//! session keeps, for the one connection that opened it, until that connection closes it or goes
//! away or leaves it unused for the idle time; the key is wiped from memory as it goes.
//! Everything else asks for a touch each time, one prompt at a time, and the prompt says what
//! for and who asks.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use zeroize::{Zeroize, Zeroizing};

use crate::entry::{Entry, Kind, Summary};
use crate::hooks::{self, Room, Step};
use crate::remote::Asker;
use crate::vault::{self, TouchDevice, TouchId, Vault, VaultKey};
use crate::wire::{
    Asking, Choice, ChoicesAnswer, Code, Device, DevicesAnswer, Envelope, Field, FillAnswer, FindAnswer, ForgetAnswer, GetAnswer, Holder,
    HolderAnswer, IdAnswer, ListAnswer, Login, OpenAnswer, PasskeyCreate, PasskeyGet, Refusal, Reply, Request, RevealAnswer,
    Settings, SettingsAnswer, SetupAnswer, StatusAnswer, TargetAnswer, TotpAnswer, Who,
};
use crate::{b64, holders, site, totp, webauthn};

/// How long a forwarder's status waits for its Macs to answer the broadcast.
const NEAR: Duration = Duration::from_secs(1);

/// How long looking for Macs waits for them to answer.
const FIND: Duration = Duration::from_secs(2);

/// How often open sessions are looked over for ones left unused too long, so that a key is not
/// kept in memory long past its time when nobody asks again.
const SWEEP: Duration = Duration::from_secs(5);

/// How long one touch may cover what follows, in seconds.
const TOUCH_SECS: [u64; 3] = [0, 60, 300];

/// How long an open SpatiKey may be left unused, in seconds: ten seconds to a day.
const OPEN_SECS: std::ops::RangeInclusive<u64> = 10..=86_400;

/// The only key SpatiKey makes passkeys with (COSE's number for it).
const ES256: i64 = -7;

/// Names longer than this are cut short in a prompt.
const LONGEST_NAME: usize = 60;

/// What only a holder's own code can do: its biometric key and the prompt for it (the Secure
/// Enclave and Touch ID on a Mac, the Keystore and BiometricPrompt on Android), and asking its
/// owner. [`crate::platform::AppGuard`] is the real one.
pub trait Guard: Send + Sync {
    /// Ask for a finger and hand back the vault key. `reason` finishes macOS's sentence
    /// "Spatiand is trying to ...". `linger_secs` lets one touch cover what follows for that
    /// long (0: this use only).
    fn unlock(&self, reason: &str, touch: &TouchId, linger_secs: u64) -> Result<VaultKey, Refusal>;
    /// Make this device's key and wrap `key` to it. The answer's `vault` is left empty, for the
    /// caller to fill.
    fn enroll(&self, key: &VaultKey) -> Result<TouchId, Refusal>;
    /// Make this device's key with nothing wrapped to it yet, asking nobody: for asking a holder
    /// to add this device ([`Request::HolderRequest`]). The answer's `wrapped` is empty.
    fn make(&self) -> Result<TouchId, Refusal> {
        Err(refuse(Code::NotHere, "this device cannot make a key to open SpatiKey with"))
    }
    /// What this device's biometrics are: [`holders::MAC`] or [`holders::ANDROID`].
    fn kind(&self) -> &'static str {
        holders::MAC
    }
    /// The sensor is there and has a finger enrolled (asks nothing).
    fn can_touch(&self) -> bool;
    /// The owner's yes or no to a device that wants to ask this Mac, with the six digits both show.
    fn allow_device(&self, name: &str, fp: &str, code: &str) -> bool;
    /// A card in the room while a finger is wanted ([`Room::notice`]); an empty text takes it
    /// away.
    fn notice(&self, text: &str);
    /// The card's last word when no finger came ([`Room::notice_briefly`]).
    fn notice_briefly(&self, text: &str) {
        let _ = text;
    }
}

/// This machine.
#[derive(Debug, Clone)]
pub struct Me {
    /// Its link fingerprint, 64 hex.
    pub fp: String,
    pub name: String,
}

/// One caller's connection: a session belongs to the connection that opened it.
pub type ConnId = u64;

/// A connection id never used before in this process.
pub fn next_conn() -> ConnId {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

pub enum Mode {
    /// A Mac: it holds the vault, or is set up to.
    Approver(Arc<dyn Guard>),
    /// A device with no biometrics (the Deck, the Beam Pro): it asks a holder on its network.
    Forwarder(Arc<crate::remote::Asker>),
    /// A device with biometrics that may hold the vault and may not yet (an Android phone with a
    /// strong sensor): what it holds it answers itself, and until it does, it asks a holder as a
    /// forwarder does. A request from another device is answered here or refused, never passed on.
    Both(Arc<dyn Guard>, Arc<crate::remote::Asker>),
}

/// An open SpatiKey.
struct Session {
    conn: ConnId,
    /// The vault it opened: one made since (on another Mac, synced here) is not this one.
    vault: String,
    key: VaultKey,
    used: Instant,
}

pub struct Service {
    mode: Mode,
    me: Me,
    shared: PathBuf,
    local: PathBuf,
    /// (Approver) open sessions, by their token.
    sessions: Mutex<HashMap<String, Session>>,
    /// (Forwarder) the sessions a Mac gave, and the connection here each was opened on: a
    /// session belongs to the one program that opened it, however the Mac tells this device's
    /// connections apart.
    relayed: Mutex<HashMap<String, ConnId>>,
    /// One prompt at a time: a second request waits for the person's answer to the first.
    prompt: Mutex<()>,
    /// (Approver) The host's paired list, whose devices may ask too. See [`Service::also_allow`].
    host_paired: Mutex<Option<PathBuf>>,
    /// One writer at a time, so that two changes to one entry each count its version up.
    writing: Mutex<()>,
    /// The titles of the entries this service has opened, by id: the prompt for an entry asked
    /// for outside a session names it before the touch that would open it.
    titles: Mutex<HashMap<String, String>>,
    /// The cards up while a finger is wanted, oldest first, by number: the room shows the
    /// newest, and when it goes, the one before it again. See [`Service::with_card`].
    cards: Mutex<Vec<(u64, String)>>,
}

impl Service {
    pub fn new(mode: Mode, me: Me, shared: PathBuf, local: PathBuf) -> Arc<Service> {
        let approver = !matches!(mode, Mode::Forwarder(_));
        let service = Arc::new(Service {
            mode,
            me,
            shared,
            local,
            sessions: Mutex::default(),
            relayed: Mutex::default(),
            prompt: Mutex::default(),
            host_paired: Mutex::default(),
            writing: Mutex::default(),
            titles: Mutex::default(),
            cards: Mutex::default(),
        });
        if approver {
            sweep_in_background(Arc::downgrade(&service));
        }
        service
    }

    pub fn me(&self) -> &Me {
        &self.me
    }

    /// Whether this device has biometrics of its own to open the vault with (whether or not it
    /// holds it yet).
    pub fn is_approver(&self) -> bool {
        !matches!(self.mode, Mode::Forwarder(_))
    }

    /// Answer one request from connection `conn`. Blocks while a person is asked.
    pub fn handle(&self, conn: ConnId, env: Envelope) -> Reply {
        let id = env.id;
        let answered = match &self.mode {
            Mode::Approver(guard) => self.approve(guard.as_ref(), conn, env),
            Mode::Forwarder(asker) => self.forward(asker, conn, env),
            Mode::Both(guard, asker) => {
                if self.answered_here(guard.as_ref(), &env) {
                    self.approve(guard.as_ref(), conn, env)
                } else {
                    self.forward(asker, conn, env)
                }
            }
        };
        match answered {
            Ok(value) => Reply { id, ok: Some(value), err: None },
            Err(refusal) => Reply::err(id, refusal),
        }
    }

    /// (Both) Whether a request is answered by this device rather than passed to a holder.
    fn answered_here(&self, guard: &dyn Guard, env: &Envelope) -> bool {
        // Another device's: answered here or refused, so that it asks another holder.
        if !env.who.device.is_empty() {
            return true;
        }
        match &env.request {
            Request::Status | Request::Setup { .. } | Request::Recover { .. } | Request::HolderRequest { .. } | Request::Target => true,
            Request::Find | Request::PairWith { .. } | Request::PairConfirm { .. } | Request::Pair { .. } => false,
            request => match session_of(request) {
                // A session belongs to whoever opened it: here, or a holder asked.
                Some(token) => !lock(&self.relayed).contains_key(token),
                None => self.holds(guard),
            },
        }
    }

    /// Whether this device opens the vault itself now.
    fn holds(&self, guard: &dyn Guard) -> bool {
        Vault::exists(&self.shared) && Vault::open(&self.shared, &self.local).is_ok_and(|vault| self.half(guard, &vault).is_some())
    }

    /// The connection has gone: its sessions close.
    pub fn closed(&self, conn: ConnId) {
        lock(&self.sessions).retain(|_, session| session.conn != conn);
        if let Mode::Forwarder(asker) | Mode::Both(_, asker) = &self.mode {
            let gone: Vec<String> = {
                let mut relayed = lock(&self.relayed);
                let gone = relayed.iter().filter(|(_, c)| **c == conn).map(|(token, _)| token.clone()).collect();
                relayed.retain(|_, c| *c != conn);
                gone
            };
            if !gone.is_empty() {
                // The Mac closes them too, rather than keeping their keys until they time out.
                // Not waited for: the Mac may be gone as well.
                let asker = asker.clone();
                let _ = std::thread::Builder::new().name("spatikey-close".into()).spawn(move || {
                    for session in gone {
                        let _ = asker.call(Envelope { id: 0, who: Who::default(), request: Request::Close { session } });
                    }
                });
            }
        }
    }

    /// (Approver) the devices allowed to ask, from `local/spatikey/allowed.json`.
    pub fn allowed(&self) -> Vec<Device> {
        vault::read_json(&self.local.join("allowed.json")).unwrap_or_default()
    }

    /// (Approver) a device paired with this Mac as a host needs no second pairing: it is the same
    /// certificate, and every use still wants a finger here and the device on this network.
    pub fn also_allow(&self, host_paired: PathBuf) {
        *lock(&self.host_paired) = Some(host_paired);
    }

    /// (Approver) the devices that may ask: those paired with SpatiKey, and with the host. The
    /// list has only fingerprints, so a prompt cannot name one by anything the owner approved.
    pub fn may_ask(&self) -> Vec<Device> {
        let mut devices = self.allowed();
        let path = lock(&self.host_paired).clone();
        let text = path.and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        for fp in text.lines().filter_map(|l| l.split_whitespace().next()) {
            if !devices.iter().any(|d| d.fp.eq_ignore_ascii_case(fp)) {
                devices.push(Device { fp: fp.to_string(), name: "your paired device".into(), added: 0 });
            }
        }
        devices
    }

    /// (Approver) a device off the owner's list may ask no longer.
    pub fn drop_device(&self, fp: &str) {
        let _writing = lock(&self.writing);
        let mut allowed = self.allowed();
        let before = allowed.len();
        allowed.retain(|d| !d.fp.eq_ignore_ascii_case(fp));
        if allowed.len() != before {
            if let Err(e) = vault::write_json(&self.local.join("allowed.json"), &allowed) {
                log::warn!("could not write the allowed list: {e}");
            }
        }
    }

    /// (Approver) let a device ask from now on.
    pub fn allow(&self, device: Device) -> Result<(), String> {
        if device.fp.trim().is_empty() {
            return Err("a device is let in by its fingerprint".into());
        }
        let mut device = device;
        if device.added == 0 {
            device.added = crate::now_ms();
        }
        let _writing = lock(&self.writing);
        let mut allowed = self.allowed();
        allowed.retain(|d| !d.fp.eq_ignore_ascii_case(&device.fp));
        allowed.push(device);
        vault::write_json(&self.local.join("allowed.json"), &allowed)
    }

    /// (Approver) the Guard, for the remote side's pairing.
    pub fn guard(&self) -> Option<&Arc<dyn Guard>> {
        match &self.mode {
            Mode::Approver(guard) | Mode::Both(guard, _) => Some(guard),
            Mode::Forwarder(_) => None,
        }
    }

    // ---- the approver ----

    fn approve(&self, guard: &dyn Guard, conn: ConnId, env: Envelope) -> Result<Value, Refusal> {
        let Envelope { who, request, .. } = env;
        // What is about this Mac itself, or types into its own windows, is not asked from another
        // device: the device types into its own, and sets nothing up here.
        let elsewhere = !who.device.is_empty();
        match request {
            Request::Status => answer(self.status(guard)),
            Request::Setup { .. }
            | Request::Recover { .. }
            | Request::HolderRequest { .. }
            | Request::Type { .. }
            | Request::Target
                if elsewhere =>
            {
                Err(refuse(Code::NotHere, "that is done on the Mac itself"))
            }
            Request::HolderRequest { name } => self.holder_request(guard, &name),
            Request::HolderApprove { session, fp, code } => self.holder_approve(guard, conn, &who, &session, &fp, &code),
            Request::HolderRefuse { session, fp } => self.holder_refuse(conn, &session, &fp),
            Request::Setup { name } => self.setup(guard, &name),
            Request::Open => self.open(guard, conn, &who),
            Request::Close { session } => self.close(conn, &session),
            Request::List { session } => self.list(conn, &session),
            Request::Get { session, id } => self.get(conn, &session, &id),
            Request::Put { session, entry } => self.put(conn, &session, entry),
            Request::Delete { session, id } => self.delete(conn, &session, &id),
            Request::Totp { session, id } => self.totp(guard, conn, &who, session.as_deref(), &id),
            Request::Reveal { session, id, fields } => self.reveal(guard, conn, &who, session.as_deref(), &id, &fields),
            Request::Fill { origin } => self.fill(guard, &who, &origin),
            Request::Save { origin, title, username, password } => {
                self.save(guard, &who, &origin, &title, &username, Zeroizing::new(password))
            }
            Request::ProfileUnlock { profile } => self.profile_unlock(guard, &who, &profile),
            Request::PasskeyCreate(request) => self.passkey_create(guard, &who, &request),
            Request::PasskeyGet(request) => self.passkey_get(guard, &who, &request),
            Request::Type { session, id, fields, window } => {
                self.type_here(guard, conn, &who, session.as_deref(), &id, &fields, window)
            }
            Request::Target => target(),
            Request::RecoveryNew { session } => self.recovery_new(conn, &session),
            Request::Recover { code } => self.recover(guard, Zeroizing::new(code)),
            Request::Devices { session } => self.devices(conn, &session),
            Request::ForgetDevice { session, fp, recovery_code } => {
                self.forget_device(guard, conn, &who, &session, &fp, recovery_code.map(Zeroizing::new))
            }
            Request::Settings { session, set } => self.settings_asked(conn, &session, set),
            Request::Pair { .. } => Err(refuse(Code::BadRequest, "pairing is asked of a Mac over the network, not here")),
            Request::Find | Request::PairWith { .. } | Request::PairConfirm { .. } => {
                Err(refuse(Code::NotHere, "a Mac does not pair with Macs: a device pairs with it"))
            }
        }
    }

    fn status(&self, guard: &dyn Guard) -> StatusAnswer {
        let vault = Vault::exists(&self.shared);
        let opened = if vault { Vault::open(&self.shared, &self.local).ok() } else { None };
        let opens_here = opened.as_ref().is_some_and(|v| self.half(guard, v).is_some());
        let request_code = match &opened {
            Some(v) if !opens_here => self.waiting_code(v),
            _ => String::new(),
        };
        // A device that may only ask, until it holds the vault: whether its holders answer.
        let approvers = match &self.mode {
            Mode::Both(_, asker) if !opens_here => asker.near(NEAR),
            _ => Vec::new(),
        };
        StatusAnswer {
            vault,
            opens_here,
            can_set_up: !opens_here && guard.can_touch(),
            approvers,
            name: self.me.name.clone(),
            fp: self.me.fp.clone(),
            holders: opened.as_ref().map(holders_of).unwrap_or_default(),
            kind: guard.kind().to_string(),
            request_code,
        }
    }

    /// The digits this device shows while its request to hold `vault` waits; empty if it made none.
    fn waiting_code(&self, vault: &Vault) -> String {
        match (vault.pending(), vault.asking(&self.me.fp)) {
            (Some(pending), Some(asking)) if asking.public == pending.public => {
                holders::code(vault.id(), &self.me.fp, &pending.public)
            }
            _ => String::new(),
        }
    }

    /// This device's half for `vault`, as it stands in the synced folder now:
    ///
    /// * removed as a holder there (its record a tombstone): the half is let go, and None;
    /// * a holder whose record is not there (a vault from before records, or one lost): the
    ///   record is written again from the half -- the one change a vault set up with a single Mac
    ///   needs, made in place;
    /// * no half, but a request this device made that a holder has since added: the half is made
    ///   from the key kept while it waited and the wrap the holder wrote;
    /// * the vault's keys changed without it (it was not among the holders the new keys were
    ///   wrapped to, and entries are sealed under them already): the half is let go, as a removed
    ///   one is, for the device to be added again.
    fn half(&self, guard: &dyn Guard, vault: &Vault) -> Option<TouchId> {
        let record = vault.touch_device(&self.me.fp);
        if let Some(touch) = vault.touch_id() {
            if !record.as_ref().is_some_and(|r| r.removed) {
                let at = vault.half().map(|h| h.key).unwrap_or_default();
                if vault.left_out(&at, &self.me.fp, &touch.public) {
                    log::warn!("spatikey: vault {}'s keys were changed without this device; letting its half go", vault.id());
                    if let Err(e) = vault.forget_touch_id() {
                        log::warn!("spatikey: could not let go of this device's half: {e}");
                    }
                    return None;
                }
            }
            return match record {
                Some(record) if record.removed => {
                    log::warn!("spatikey: this device was removed from those that open vault {}; letting its half go", vault.id());
                    if let Err(e) = vault.forget_touch_id() {
                        log::warn!("spatikey: could not let go of this device's half: {e}");
                    }
                    None
                }
                Some(_) => Some(touch),
                None => {
                    let record = TouchDevice {
                        device: self.me.fp.to_ascii_lowercase(),
                        name: self.me.name.clone(),
                        kind: guard.kind().to_string(),
                        public: touch.public.clone(),
                        wrapped: touch.wrapped.clone(),
                        added: crate::now_ms(),
                        ..TouchDevice::default()
                    };
                    match vault.add_touch_device(&record) {
                        Ok(()) => log::info!("spatikey: wrote this device's record as a holder of vault {}", vault.id()),
                        Err(e) => log::warn!("spatikey: could not write this device's record as a holder: {e}"),
                    }
                    Some(touch)
                }
            };
        }
        let pending = vault.pending()?;
        let record = record.filter(|r| !r.removed && r.public == pending.public && !r.wrapped.is_empty())?;
        let touch = TouchId { vault: vault.id().to_string(), public: pending.public, blob: pending.blob, wrapped: record.wrapped };
        // Which of the vault's keys the holder wrapped: its record says. The proof of it is kept
        // when the key is first had.
        if let Err(e) = vault.set_touch_id(&touch).and_then(|()| vault.set_follow(&touch.public, &record.key, &touch.wrapped, "")) {
            log::warn!("spatikey: could not keep the half a holder made for this device: {e}");
            return None;
        }
        let _ = vault.forget_pending();
        let _ = vault.close_asking(&self.me.fp);
        log::info!("spatikey: added to the devices that open vault {} (by {})", vault.id(), record.by);
        Some(touch)
    }

    /// A device with biometrics asks to hold the vault: its key made, its half kept here, its
    /// request in the synced folder, and the digits to compare.
    fn holder_request(&self, guard: &dyn Guard, name: &str) -> Result<Value, Refusal> {
        let vault = self.vault()?;
        if self.half(guard, &vault).is_some() {
            return Err(refuse(Code::BadRequest, "this device opens SpatiKey already"));
        }
        if !guard.can_touch() {
            return Err(refuse(
                Code::TouchIdUnavailable,
                format!("{} cannot be used on this device now", holders::sensor(guard.kind())),
            ));
        }
        let _prompt = lock(&self.prompt);
        // The same request while it waits: a new key would show other digits than the ones the
        // owner may already be comparing.
        let pending = match vault.pending() {
            Some(pending) => pending,
            None => {
                let made = guard.make()?;
                let pending = holders::Pending { vault: vault.id().to_string(), public: made.public, blob: made.blob, made: crate::now_ms() };
                vault.set_pending(&pending).map_err(failed)?;
                pending
            }
        };
        if vault.asking(&self.me.fp).is_none_or(|asking| asking.public != pending.public) {
            let name = match name.trim() {
                "" => self.me.name.clone(),
                name => name.to_string(),
            };
            let asking = holders::Asking {
                v: 1,
                device: self.me.fp.to_ascii_lowercase(),
                name,
                kind: guard.kind().to_string(),
                public: pending.public.clone(),
                made: crate::now_ms(),
                closed: false,
            };
            vault.ask_to_hold(&asking).map_err(failed)?;
            log::info!("spatikey: asked to be added to the devices that open vault {}", vault.id());
        }
        answer(HolderAnswer { code: holders::code(vault.id(), &self.me.fp, &pending.public) })
    }

    /// A holder adds a device that asked: a new touch here, and the vault key wrapped to the
    /// device's key -- if the request is still the one whose digits the owner compared.
    fn holder_approve(&self, guard: &dyn Guard, conn: ConnId, who: &Who, token: &str, fp: &str, code: &str) -> Result<Value, Refusal> {
        let vault = self.vault()?;
        self.session(conn, token, &vault)?;
        let Some(asking) = vault.asking(fp) else {
            return Err(refuse(Code::NoMatch, "no device is asking to be added with that fingerprint"));
        };
        if !holders::known(&asking.kind) {
            return Err(refuse(Code::BadRequest, format!("SpatiKey does not know how to open the vault with {}", shown(&asking.kind))));
        }
        if !holders::same_code(&holders::code(vault.id(), &asking.device, &asking.public), code) {
            return Err(refuse(Code::Denied, "that request changed since its digits were shown: look at it again"));
        }
        let public = b64::decode(&asking.public).map_err(|_| refuse(Code::BadRequest, "the request's key is not base64"))?;
        // Before the touch: a key nothing can be wrapped to is refused without asking anybody.
        holders::wrap(&VaultKey::random(), &public).map_err(|e| refuse(Code::BadRequest, e))?;
        let name = match shown(&asking.name) {
            name if name.is_empty() => "a device".to_string(),
            name => name,
        };
        let key = self.unlock_lingering(guard, &vault, who, &format!("add {name} to the devices that open SpatiKey"), 0)?;
        let wrapped = holders::wrap(&key, &public).map_err(failed)?;
        let _writing = lock(&self.writing);
        let mut record = TouchDevice {
            device: asking.device.to_ascii_lowercase(),
            name: asking.name.clone(),
            kind: asking.kind.clone(),
            public: asking.public.clone(),
            wrapped: b64::encode(&wrapped),
            added: crate::now_ms(),
            by: self.short(),
            ..TouchDevice::default()
        };
        // Sealed under the key this holder has just had: a later change of keys wraps the new
        // key to this device because a holder said it is one.
        vault.seal_holder(&key, &mut record).map_err(failed)?;
        vault.add_touch_device(&record).map_err(failed)?;
        vault.close_asking(&asking.device).map_err(failed)?;
        log::info!("spatikey: added {name} ({}) to the devices that open vault {}", &record.device[..8], vault.id());
        nothing()
    }

    fn holder_refuse(&self, conn: ConnId, token: &str, fp: &str) -> Result<Value, Refusal> {
        let vault = self.vault()?;
        self.session(conn, token, &vault)?;
        if vault.asking(fp).is_none() {
            return Err(refuse(Code::NoMatch, "no device is asking to be added with that fingerprint"));
        }
        vault.close_asking(fp).map_err(failed)?;
        nothing()
    }

    fn setup(&self, guard: &dyn Guard, name: &str) -> Result<Value, Refusal> {
        let exists = || refuse(Code::VaultExists, "there already is a vault: open it, or set this Mac up from the recovery code");
        if Vault::exists(&self.shared) {
            return Err(exists());
        }
        if !guard.can_touch() {
            return Err(refuse(Code::TouchIdUnavailable, "Touch ID cannot be used on this Mac now"));
        }
        let _prompt = lock(&self.prompt);
        if Vault::exists(&self.shared) {
            return Err(exists());
        }
        // Touch ID takes the key before anything is written: a cancelled panel leaves no vault.
        let key = VaultKey::random();
        let touch = self.with_card(&ByGuard(guard), "set SpatiKey up", || guard.enroll(&key), |touch| touch.as_ref().err())?;
        let vault = Vault::create_with(&self.shared, &self.local, &self.short(), &key).map_err(failed)?;
        self.enrolled(&vault, touch, name, guard.kind(), &key)?;
        let recovery_code = vault.set_recovery(&key).map_err(failed)?;
        answer(SetupAnswer { recovery_code })
    }

    /// Keep what the sensor made for `vault`: this device's half in `local` (with which of the
    /// vault's keys `key` is, and the proof of it), and its record in `devices` sealed under that
    /// key, each replacing any before.
    fn enrolled(&self, vault: &Vault, touch: TouchId, name: &str, kind: &str, key: &VaultKey) -> Result<(), Refusal> {
        vault.set_touch_id(&touch).map_err(failed)?;
        let generation = vault.generation_of(key).ok_or_else(|| failed("the key does not open this vault".into()))?;
        vault.set_follow(&touch.public, &generation, &touch.wrapped, &vault.proof(key).unwrap_or_default()).map_err(failed)?;
        let name = match name.trim() {
            "" => self.me.name.clone(),
            name => name.to_string(),
        };
        let mut device = TouchDevice {
            device: self.me.fp.clone(),
            name,
            kind: kind.to_string(),
            public: touch.public,
            wrapped: touch.wrapped,
            added: crate::now_ms(),
            ..TouchDevice::default()
        };
        vault.seal_holder(key, &mut device).map_err(failed)?;
        vault.add_touch_device(&device).map_err(failed)?;
        // A request this device made while it could not open the vault is answered now.
        if vault.pending().is_some() {
            let _ = vault.forget_pending();
            let _ = vault.close_asking(&self.me.fp);
        }
        Ok(())
    }

    fn open(&self, guard: &dyn Guard, conn: ConnId, who: &Who) -> Result<Value, Refusal> {
        let vault = self.vault()?;
        let key = self.unlock(guard, &vault, who, "open SpatiKey")?;
        let entries = self.summaries(&vault, &key)?;
        let session = vault::random_hex(16);
        let open = Session { conn, vault: vault.id().to_string(), key, used: Instant::now() };
        lock(&self.sessions).insert(session.clone(), open);
        answer(OpenAnswer { session, entries })
    }

    fn close(&self, conn: ConnId, token: &str) -> Result<Value, Refusal> {
        let mut sessions = lock(&self.sessions);
        if sessions.get(token).is_some_and(|session| session.conn != conn) {
            return Err(refuse(Code::BadSession, "that session is another program's"));
        }
        // Closing one already closed is no error.
        sessions.remove(token);
        nothing()
    }

    fn list(&self, conn: ConnId, token: &str) -> Result<Value, Refusal> {
        let vault = self.vault()?;
        let key = self.session(conn, token, &vault)?;
        answer(ListAnswer { entries: self.summaries(&vault, &key)? })
    }

    fn get(&self, conn: ConnId, token: &str, id: &str) -> Result<Value, Refusal> {
        let vault = self.vault()?;
        let key = self.session(conn, token, &vault)?;
        let mut entry = Wiping(self.read_live(&vault, &key, id)?);
        // A passkey's private key is never given out: signing is done here. A put that leaves
        // it out keeps the one in the vault.
        if let Some(passkey) = entry.passkey.as_mut() {
            passkey.key.zeroize();
        }
        let got = GetAnswer { entry: std::mem::take(&mut *entry) };
        let value = answer(&got);
        drop(Wiping(got.entry));
        value
    }

    fn put(&self, conn: ConnId, token: &str, entry: Entry) -> Result<Value, Refusal> {
        let mut entry = Wiping(entry);
        if !entry.id.is_empty() && !vault::is_entry_id(&entry.id) {
            return Err(refuse(Code::BadRequest, "an entry id is 32 hex digits"));
        }
        let vault = self.vault()?;
        let key = self.session(conn, token, &vault)?;
        let _writing = lock(&self.writing);
        let id = entry.id.clone();
        if let Some(passkey) = entry.passkey.as_mut().filter(|p| p.key.is_empty()) {
            // Given out without its private key: the key is the one in the vault, if it is the
            // same passkey.
            if vault.contains(&id) {
                let old = Wiping(vault.read(&key, &id).map_err(failed)?);
                if let Some(kept) = old.passkey.as_ref().filter(|old| old.credential_id == passkey.credential_id) {
                    passkey.key = kept.key.clone();
                }
            }
            if passkey.key.is_empty() {
                return Err(refuse(Code::BadRequest, "a passkey is not kept without its key"));
            }
        }
        vault.write(&key, &mut entry, &self.short()).map_err(failed)?;
        self.remember(&entry);
        answer(IdAnswer { id: entry.id.clone() })
    }

    fn delete(&self, conn: ConnId, token: &str, id: &str) -> Result<Value, Refusal> {
        let vault = self.vault()?;
        let key = self.session(conn, token, &vault)?;
        if !vault::is_entry_id(id) {
            return Err(refuse(Code::BadRequest, "an entry id is 32 hex digits"));
        }
        if !vault.contains(id) {
            return Err(no_entry());
        }
        let _writing = lock(&self.writing);
        vault.delete(&key, id, &self.short()).map_err(failed)?;
        lock(&self.titles).remove(id);
        nothing()
    }

    fn profile_unlock(&self, guard: &dyn Guard, who: &Who, profile: &str) -> Result<Value, Refusal> {
        let vault = self.vault()?;
        self.unlock_lingering(guard, &vault, who, &format!("open the profile {}", shown(profile)), 0)?;
        Ok(json!({}))
    }

    fn totp(&self, guard: &dyn Guard, conn: ConnId, who: &Who, session: Option<&str>, id: &str) -> Result<Value, Refusal> {
        let vault = self.vault()?;
        entry_id(id)?;
        let key = self.key_for(guard, conn, who, session, &vault, || match self.title_of(id) {
            Some(title) => format!("show the code for {title}"),
            None => "show a 2FA code".into(),
        })?;
        let entry = Wiping(self.read_live(&vault, &key, id)?);
        let Some(secret) = entry.totp.as_ref() else {
            return Err(refuse(Code::NoMatch, format!("{} has no 2FA code", shown(&entry.title))));
        };
        let (code, remaining) = totp::code(secret, now_secs()).map_err(failed)?;
        answer(TotpAnswer { code, remaining, period: secret.period })
    }

    fn reveal(
        &self,
        guard: &dyn Guard,
        conn: ConnId,
        who: &Who,
        session: Option<&str>,
        id: &str,
        fields: &[Field],
    ) -> Result<Value, Refusal> {
        let vault = self.vault()?;
        entry_id(id)?;
        if !fields.iter().any(|&field| needs_secret(field)) {
            // Only keys: there is nothing to open the vault for.
            if !vault.contains(id) {
                return Err(no_entry());
            }
            return answer(RevealAnswer { values: vec![String::new(); fields.len()] });
        }
        // A field is revealed for typing it, on the device that asked.
        let key = self.key_for(guard, conn, who, session, &vault, || format!("type {}", self.what_of(id, fields)))?;
        let entry = Wiping(self.read_live(&vault, &key, id)?);
        let values = fields.iter().map(|&field| value_of(&entry, field)).collect::<Result<Vec<Zeroizing<String>>, Refusal>>()?;
        let mut got = RevealAnswer { values: values.iter().map(|value| value.to_string()).collect() };
        let value = answer(&got);
        got.values.iter_mut().for_each(Zeroize::zeroize);
        value
    }

    fn fill(&self, guard: &dyn Guard, who: &Who, origin: &str) -> Result<Value, Refusal> {
        let host = page_host(origin)?;
        let vault = self.vault()?;
        let key = self.unlock(guard, &vault, who, &format!("fill in your password for {host}"))?;
        let entries = Wiping(vault.entries(&key).map_err(failed)?);
        self.remember_all(&entries);
        let mut logins: Vec<&Entry> = entries
            .iter()
            .filter(|e| e.kind == Kind::Login && e.sites.iter().any(|s| site::site_matches(s, origin)))
            .collect();
        if logins.is_empty() {
            return Err(refuse(Code::NoMatch, format!("SpatiKey has no password for {host}")));
        }
        logins.sort_by(|a, b| b.changed.cmp(&a.changed));
        let now = now_secs();
        let got = FillAnswer {
            logins: logins
                .iter()
                .map(|e| Login {
                    id: e.id.clone(),
                    title: e.title.clone(),
                    username: e.username.clone(),
                    password: e.password.clone(),
                    totp: e.totp.as_ref().and_then(|t| totp::code(t, now).ok()).map(|(code, _)| code),
                })
                .collect(),
        };
        let value = answer(&got);
        for mut login in got.logins {
            login.password.zeroize();
            if let Some(code) = login.totp.as_mut() {
                code.zeroize();
            }
        }
        value
    }

    fn save(
        &self,
        guard: &dyn Guard,
        who: &Who,
        origin: &str,
        title: &str,
        username: &str,
        password: Zeroizing<String>,
    ) -> Result<Value, Refusal> {
        let host = page_host(origin)?;
        if password.is_empty() {
            return Err(refuse(Code::BadRequest, "there is no password to save"));
        }
        let vault = self.vault()?;
        let key = self.unlock(guard, &vault, who, &format!("save a password for {host}"))?;
        let _writing = lock(&self.writing);
        let mut entries = Wiping(vault.entries(&key).map_err(failed)?);
        self.remember_all(&entries);
        let by = self.short();
        // The same account on the same site is changed rather than kept twice.
        let same = entries
            .iter_mut()
            .filter(|e| e.kind == Kind::Login && e.username == username && e.sites.iter().any(|s| site::site_matches(s, origin)))
            .max_by_key(|e| e.changed);
        let id = match same {
            Some(entry) => {
                if entry.password != *password {
                    entry.password.zeroize();
                    entry.password.push_str(&password);
                    vault.write(&key, entry, &by).map_err(failed)?;
                }
                entry.id.clone()
            }
            None => {
                let title = match title.trim() {
                    "" => site::registrable_domain(&host).unwrap_or_else(|| host.clone()),
                    given => given.to_string(),
                };
                let mut entry = Wiping(Entry {
                    kind: Kind::Login,
                    title,
                    sites: vec![site::site_for(origin)],
                    username: username.to_string(),
                    password: password.to_string(),
                    ..Entry::default()
                });
                vault.write(&key, &mut entry, &by).map_err(failed)?;
                self.remember(&entry);
                entry.id.clone()
            }
        };
        answer(IdAnswer { id })
    }

    fn passkey_create(&self, guard: &dyn Guard, who: &Who, request: &PasskeyCreate) -> Result<Value, Refusal> {
        if !site::rp_id_ok(&request.rp_id, &request.origin) {
            return Err(refuse(
                Code::BadRequest,
                format!("a page at {} cannot make passkeys for {}", request.origin, request.rp_id),
            ));
        }
        let user = b64::decode_url(&request.user_id).map_err(|e| refuse(Code::BadRequest, format!("the account's id is {e}")))?;
        if user.is_empty() || user.len() > 64 {
            return Err(refuse(Code::BadRequest, "an account's id is 1 to 64 bytes"));
        }
        client_data_hash(&request.client_data_hash)?;
        // Before the touch, which would be wasted on a site that takes none of SpatiKey's keys.
        if !request.algs.is_empty() && !request.algs.contains(&ES256) {
            return Err(refuse(Code::BadRequest, "the site takes none of the keys SpatiKey makes (ES256)"));
        }
        let vault = self.vault()?;
        let key = self.unlock(guard, &vault, who, &format!("make a passkey for {}", request.rp_id))?;
        let _writing = lock(&self.writing);
        let mut entries = Wiping(vault.entries(&key).map_err(failed)?);
        self.remember_all(&entries);
        let excluded = decoded(&request.exclude);
        let already = entries
            .iter()
            .filter_map(|e| e.passkey.as_ref())
            .any(|p| p.rp_id == request.rp_id && listed(&p.credential_id, &excluded));
        if already {
            return Err(refuse(
                Code::Excluded,
                format!("SpatiKey already has a passkey for this account on {}", request.rp_id),
            ));
        }
        let mut created = webauthn::create(request)?;
        // One passkey per site and account: a new one takes the old one's place, and any other
        // copy (two Macs making one each while apart) is buried.
        let mut same = entries.iter_mut().filter(|e| {
            e.passkey.as_ref().is_some_and(|p| p.rp_id == request.rp_id && b64::decode_url(&p.user_handle).is_ok_and(|h| h == user))
        });
        let mut entry = Wiping(same.next().map(std::mem::take).unwrap_or_default());
        let others: Vec<String> = same.map(|e| e.id.clone()).collect();
        entry.kind = Kind::Passkey;
        if entry.title.trim().is_empty() {
            entry.title = match request.rp_name.trim() {
                "" => request.rp_id.clone(),
                name => name.to_string(),
            };
        }
        entry.sites = vec![request.rp_id.clone()];
        entry.username = request.user_name.clone();
        if let Some(old) = entry.passkey.as_mut() {
            old.key.zeroize();
        }
        entry.passkey = Some(std::mem::take(&mut created.passkey));
        entry.deleted = false;
        let by = self.short();
        vault.write(&key, &mut entry, &by).map_err(failed)?;
        for id in others {
            if let Err(e) = vault.delete(&key, &id, &by) {
                log::warn!("spatikey: could not bury the older passkey {id}: {e}");
            }
        }
        self.remember(&entry);
        answer(&created.answer)
    }

    fn passkey_get(&self, guard: &dyn Guard, who: &Who, request: &PasskeyGet) -> Result<Value, Refusal> {
        if !site::rp_id_ok(&request.rp_id, &request.origin) {
            return Err(refuse(
                Code::BadRequest,
                format!("a page at {} cannot sign in with passkeys for {}", request.origin, request.rp_id),
            ));
        }
        let hash = client_data_hash(&request.client_data_hash)?;
        let allowed = decoded(&request.allow);
        let vault = self.vault()?;
        let key = self.unlock(guard, &vault, who, &format!("sign in to {} with a passkey", request.rp_id))?;
        let entries = Wiping(vault.entries(&key).map_err(failed)?);
        self.remember_all(&entries);
        let mut found: Vec<&Entry> = entries
            .iter()
            .filter(|e| {
                e.passkey.as_ref().is_some_and(|p| {
                    p.rp_id == request.rp_id && (request.allow.is_empty() || listed(&p.credential_id, &allowed))
                })
            })
            .collect();
        if found.is_empty() {
            return Err(refuse(Code::NoMatch, format!("SpatiKey has no passkey for {}", request.rp_id)));
        }
        // Newest first: one touch covers choosing between them.
        found.sort_by(|a, b| b.changed.cmp(&a.changed));
        let mut choices = Vec::with_capacity(found.len());
        for passkey in found.iter().filter_map(|e| e.passkey.as_ref()) {
            let assertion = webauthn::assert(passkey, &hash)?;
            choices.push(Choice { user_name: passkey.user_name.clone(), user_display: passkey.user_display.clone(), assertion });
        }
        answer(ChoicesAnswer { choices })
    }

    #[allow(clippy::too_many_arguments)]
    fn type_here(
        &self,
        guard: &dyn Guard,
        conn: ConnId,
        who: &Who,
        session: Option<&str>,
        id: &str,
        fields: &[Field],
        window: Option<u64>,
    ) -> Result<Value, Refusal> {
        // Whether there is anything to type into, before anyone is asked for a finger.
        let (room, window, into) = typing_target(window)?;
        let steps = if fields.iter().any(|&field| needs_secret(field)) {
            let vault = self.vault()?;
            entry_id(id)?;
            let key =
                self.key_for(guard, conn, who, session, &vault, || format!("type {} into {into}", self.what_of(id, fields)))?;
            let entry = Wiping(self.read_live(&vault, &key, id)?);
            fields
                .iter()
                .map(|&field| match field {
                    Field::Tab => Ok(Step::Tab),
                    Field::Enter => Ok(Step::Enter),
                    field => value_of(&entry, field).map(Step::Text),
                })
                .collect::<Result<Vec<Step>, Refusal>>()?
        } else {
            keys_only(fields)
        };
        room.type_into(window, &steps).map_err(failed)?;
        nothing()
    }

    fn recovery_new(&self, conn: ConnId, token: &str) -> Result<Value, Refusal> {
        let vault = self.vault()?;
        let key = self.session(conn, token, &vault)?;
        let recovery_code = vault.set_recovery(&key).map_err(failed)?;
        answer(SetupAnswer { recovery_code })
    }

    fn recover(&self, guard: &dyn Guard, code: Zeroizing<String>) -> Result<Value, Refusal> {
        let vault = self.vault()?;
        crate::recovery::canonical(&code).map(Zeroizing::new).map_err(|e| refuse(Code::BadRequest, e))?;
        if !vault.has_recovery() {
            return Err(refuse(Code::Failed, "this vault has no recovery code"));
        }
        if !guard.can_touch() {
            return Err(refuse(
                Code::TouchIdUnavailable,
                format!("{} cannot be used on this device now", holders::sensor(guard.kind())),
            ));
        }
        let key = vault.recover(&code).map_err(|e| refuse(Code::Denied, e))?;
        let _prompt = lock(&self.prompt);
        let what = match guard.kind() {
            holders::MAC => "set Touch ID up again",
            _ => "open SpatiKey with your fingerprint",
        };
        let touch = self.with_card(&ByGuard(guard), what, || guard.enroll(&key), |touch| touch.as_ref().err())?;
        self.enrolled(&vault, touch, "", guard.kind(), &key)?;
        // With the newest key in hand: whatever a change of keys left to do.
        let _writing = lock(&self.writing);
        if let Err(e) = vault.settle(&key, &self.me.fp) {
            log::warn!("spatikey: could not finish what the vault's change of keys left to do: {e}");
        }
        log::info!("spatikey: this device opens vault {} again, set up from the recovery code", vault.id());
        nothing()
    }

    fn devices(&self, conn: ConnId, token: &str) -> Result<Value, Refusal> {
        let vault = self.vault()?;
        self.session(conn, token, &vault)?;
        let touch_ids = vault
            .touch_devices()
            .into_iter()
            .filter(|d| d.kind == holders::MAC)
            .map(|d| Device { fp: d.device, name: d.name, added: d.added })
            .collect();
        let requests = vault
            .askings()
            .into_iter()
            .map(|a| Asking {
                code: holders::code(vault.id(), &a.device, &a.public),
                fp: a.device,
                name: a.name,
                kind: a.kind,
                made: a.made,
            })
            .collect();
        answer(DevicesAnswer { allowed: self.allowed(), touch_ids, holders: holders_of(&vault), requests })
    }

    /// Remove a device: from those that may ask, and from the holders. **A holder removed is new
    /// keys for the vault** (`Vault::rekey`): it once had the old one.
    fn forget_device(
        &self,
        guard: &dyn Guard,
        conn: ConnId,
        who: &Who,
        token: &str,
        fp: &str,
        recovery_code: Option<Zeroizing<String>>,
    ) -> Result<Value, Refusal> {
        let vault = self.vault()?;
        self.session(conn, token, &vault)?;
        if fp.eq_ignore_ascii_case(&self.me.fp) {
            return Err(refuse(
                Code::BadRequest,
                "this device cannot remove itself: remove it from another that opens SpatiKey",
            ));
        }
        let mut done = ForgetAnswer::default();
        // A holder goes only with a new touch here, whatever the session covers: it is a device
        // that opens every password.
        if let Some(holder) = vault.touch_devices().into_iter().find(|d| d.device.eq_ignore_ascii_case(fp)) {
            // Before the touch: a code that is not one is refused without asking anybody.
            if let Some(code) = &recovery_code {
                crate::recovery::canonical(code).map(Zeroizing::new).map_err(|e| refuse(Code::BadRequest, e))?;
            }
            let name = match shown(&holder.name) {
                name if name.is_empty() => "a device".to_string(),
                name => name,
            };
            let key = self.unlock_lingering(guard, &vault, who, &format!("remove {name} from the devices that open SpatiKey"), 0)?;
            let _writing = lock(&self.writing);
            // The touch gave the vault key; a new one is made, the removed device left out of it.
            // One file is the change: before it nothing has happened, after it the vault is the
            // new one, and the rest is finished here or by whoever opens it next.
            let plan = match &recovery_code {
                Some(code) => vault::RecoveryPlan::Keep(code),
                None => vault::RecoveryPlan::New,
            };
            let made = vault
                .rekey(&key, std::slice::from_ref(&holder.device), &self.me.fp, &self.short(), plan, crate::recovery::COST)
                .map_err(|e| if recovery_code.is_some() && e.contains("recovery code") { refuse(Code::Denied, e) } else { failed(e) })?;
            if let Err(e) = vault.settle(&made.key, &self.me.fp) {
                log::warn!("spatikey: the vault has new keys, and not everything after is done yet: {e}");
            }
            // What is open on this device goes on, with the new key.
            for session in lock(&self.sessions).values_mut().filter(|s| s.vault == vault.id()) {
                session.key = made.key.clone();
            }
            log::info!("spatikey: removed {name} ({}) from the devices that open vault {}", &holder.device[..8.min(holder.device.len())], vault.id());
            done.rekeyed = true;
            done.recovery_code = made.recovery_code.map(|code| code.to_string()).unwrap_or_default();
            done.left_out = made.left_out;
        }
        let _writing = lock(&self.writing);
        let mut forgot = done.rekeyed;
        let mut allowed = self.allowed();
        let before = allowed.len();
        allowed.retain(|d| !d.fp.eq_ignore_ascii_case(fp));
        if allowed.len() != before {
            vault::write_json(&self.local.join("allowed.json"), &allowed).map_err(failed)?;
            forgot = true;
        }
        if vault.touch_devices().iter().any(|d| d.device.eq_ignore_ascii_case(fp)) {
            vault.remove_touch_device(fp).map_err(failed)?;
            forgot = true;
        }
        if !forgot {
            return Err(refuse(Code::NoMatch, "no device has that fingerprint"));
        }
        answer(done)
    }

    fn settings_asked(&self, conn: ConnId, token: &str, set: Option<Settings>) -> Result<Value, Refusal> {
        let vault = self.vault()?;
        self.session(conn, token, &vault)?;
        if let Some(set) = set {
            if !TOUCH_SECS.contains(&set.touch_secs) {
                return Err(refuse(Code::BadRequest, "a touch lasts 0, 60 or 300 seconds"));
            }
            if !OPEN_SECS.contains(&set.open_secs) {
                return Err(refuse(Code::BadRequest, "SpatiKey stays open from ten seconds to a day"));
            }
            vault::write_json(&self.local.join("settings.json"), &set).map_err(failed)?;
        }
        answer(SettingsAnswer { settings: self.settings() })
    }

    // ---- keys, sessions and prompts ----

    /// The vault, when there is one.
    fn vault(&self) -> Result<Vault, Refusal> {
        if !Vault::exists(&self.shared) {
            return Err(refuse(Code::NoVault, "there is no vault yet: set SpatiKey up on a Mac"));
        }
        Vault::open(&self.shared, &self.local).map_err(failed)
    }

    /// Ask for a finger, for `what` ("open SpatiKey"), and take the vault key.
    fn unlock(&self, guard: &dyn Guard, vault: &Vault, who: &Who, what: &str) -> Result<VaultKey, Refusal> {
        self.unlock_lingering(guard, vault, who, what, self.settings().touch_secs)
    }

    /// The same, with how long this touch may go on covering others, and be covered by them.
    ///
    /// **The key is the vault's newest.** Where its keys were changed since this device last
    /// had one (a holder removed elsewhere), the wrap the change made for this device is the one
    /// taken to the sensor -- still one touch -- and the key that comes out is believed only if
    /// it leads back to the key this device had (`Vault::follows`). What a change of keys left
    /// to do is then finished (`Vault::settle`).
    fn unlock_lingering(&self, guard: &dyn Guard, vault: &Vault, who: &Who, what: &str, linger: u64) -> Result<VaultKey, Refusal> {
        let not_set_up = || {
            refuse(
                Code::NotHere,
                format!(
                    "{} is not set up for this vault on this device: add it from a device that opens SpatiKey, or set it up from the recovery code",
                    holders::sensor(guard.kind())
                ),
            )
        };
        if self.half(guard, vault).is_none() {
            return Err(not_set_up());
        }
        let Some(half) = vault.half() else { return Err(not_set_up()) };
        let reason = format!("{what}{}", asking(who));
        let _prompt = lock(&self.prompt);
        // A card in the room, whoever asks: macOS's panel is on the Mac's own screen, which a
        // wearer of the glasses does not see, whether the asking is this Mac's or another device's.
        let ask = |touch: &TouchId| self.with_card(&ByGuard(guard), &reason, || guard.unlock(&reason, touch, linger), |key| key.as_ref().err());
        // The key this device has itself, checked against what it kept of it.
        let own = || -> Result<VaultKey, Refusal> {
            let key = ask(&half.touch)?;
            if !vault.follows(&key, &half.key, &half.key, &half.proof) {
                return Err(refuse(Code::Failed, format!("{} gave a key that does not open this vault", holders::sensor(guard.kind()))));
            }
            Ok(key)
        };
        let after = vault.successors(&half.key, &self.me.fp, &half.touch.public);
        let key = if after.is_empty() {
            own()?
        } else {
            let mut real: Vec<VaultKey> = Vec::new();
            for change in &after {
                match ask(&TouchId { wrapped: change.wrapped.clone(), ..half.touch.clone() }) {
                    Ok(key) if vault.follows(&key, &change.id, &half.key, &half.proof) => real.push(key),
                    Ok(_) => log::warn!("spatikey: the change of keys {} does not follow from this device's key; not followed", change.id),
                    // The finger did not come: nothing is known of the change, and nothing is done.
                    Err(refusal) if matches!(refusal.code, Code::Denied | Code::Timeout | Code::TouchIdUnavailable) => return Err(refusal),
                    Err(refusal) => {
                        log::warn!("spatikey: the change of keys {} has no wrap this device opens ({}); not followed", change.id, refusal.message)
                    }
                }
            }
            match real.len() {
                1 => {
                    log::info!("spatikey: vault {} has new keys; this device follows", vault.id());
                    real.remove(0)
                }
                // Nothing that claims to follow is real: the key this device has is the newest.
                0 => own()?,
                _ => {
                    // Two that are real: this device cannot tell whose to take, and takes neither.
                    if let Err(e) = vault.forget_touch_id() {
                        log::warn!("spatikey: could not let go of this device's half: {e}");
                    }
                    return Err(refuse(Code::Denied, vault::RIVALS));
                }
            }
        };
        drop(_prompt);
        let _writing = lock(&self.writing);
        if let Err(e) = vault.settle(&key, &self.me.fp) {
            log::warn!("spatikey: could not finish what the vault's change of keys left to do: {e}");
        }
        Ok(key)
    }

    /// The key an open session holds, if `token` is one that this connection opened and has used
    /// within the idle time.
    fn session(&self, conn: ConnId, token: &str, vault: &Vault) -> Result<VaultKey, Refusal> {
        let idle = Duration::from_secs(self.settings().open_secs);
        let mut sessions = lock(&self.sessions);
        let Some(session) = sessions.get_mut(token) else {
            return Err(refuse(Code::BadSession, "SpatiKey is not open: open it again"));
        };
        if session.conn != conn {
            return Err(refuse(Code::BadSession, "that session is another program's"));
        }
        if session.used.elapsed() > idle {
            sessions.remove(token);
            return Err(refuse(Code::BadSession, "SpatiKey closed after sitting unused"));
        }
        if session.vault != vault.id() {
            sessions.remove(token);
            return Err(refuse(Code::BadSession, "the vault changed since SpatiKey was opened"));
        }
        session.used = Instant::now();
        Ok(session.key.clone())
    }

    /// The key for what may be asked in a session or alone: alone, it asks for a finger, for
    /// what `what` says.
    fn key_for(
        &self,
        guard: &dyn Guard,
        conn: ConnId,
        who: &Who,
        session: Option<&str>,
        vault: &Vault,
        what: impl FnOnce() -> String,
    ) -> Result<VaultKey, Refusal> {
        match session.filter(|token| !token.is_empty()) {
            Some(token) => self.session(conn, token, vault),
            None => self.unlock(guard, vault, who, &what()),
        }
    }

    /// Close the sessions left unused past the idle time.
    fn sweep(&self) {
        let idle = Duration::from_secs(self.settings().open_secs);
        lock(&self.sessions).retain(|_, session| session.used.elapsed() <= idle);
    }

    /// `local/spatikey/settings.json`, or the defaults.
    fn settings(&self) -> Settings {
        let mut settings: Settings = vault::read_json(&self.local.join("settings.json")).unwrap_or_default();
        if settings.open_secs == 0 {
            settings.open_secs = Settings::default().open_secs;
        }
        settings
    }

    /// This Mac's short fingerprint, which every entry it changes is stamped with.
    fn short(&self) -> String {
        self.me.fp.chars().take(8).collect::<String>().to_ascii_lowercase()
    }

    /// Every live entry's summary.
    fn summaries(&self, vault: &Vault, key: &VaultKey) -> Result<Vec<Summary>, Refusal> {
        let entries = Wiping(vault.entries(key).map_err(failed)?);
        self.remember_all(&entries);
        Ok(entries.iter().map(Entry::summary).collect())
    }

    /// A live entry, or why there is none.
    fn read_live(&self, vault: &Vault, key: &VaultKey, id: &str) -> Result<Entry, Refusal> {
        entry_id(id)?;
        if !vault.contains(id) {
            return Err(no_entry());
        }
        let mut entry = vault.read(key, id).map_err(failed)?;
        if entry.deleted {
            entry.wipe();
            return Err(no_entry());
        }
        self.remember(&entry);
        Ok(entry)
    }

    fn remember_all(&self, entries: &[Entry]) {
        let mut titles = lock(&self.titles);
        titles.clear();
        titles.extend(entries.iter().map(|e| (e.id.clone(), e.title.clone())));
    }

    fn remember(&self, entry: &Entry) {
        lock(&self.titles).insert(entry.id.clone(), entry.title.clone());
    }

    fn title_of(&self, id: &str) -> Option<String> {
        lock(&self.titles).get(id).map(|title| shown(title)).filter(|title| !title.is_empty())
    }

    /// "your GitHub username and password": what typing `fields` of the entry `id` gives away,
    /// for a prompt. The title only when this service has opened the entry before.
    fn what_of(&self, id: &str, fields: &[Field]) -> String {
        let mut names: Vec<&str> = Vec::new();
        for field in fields {
            let name = match field {
                Field::Username => "username",
                Field::Password => "password",
                Field::Totp => "2FA code",
                Field::Tab | Field::Enter => continue,
            };
            if !names.contains(&name) {
                names.push(name);
            }
        }
        let list = match names.as_slice() {
            [] => "keys".to_string(),
            [one] => one.to_string(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        };
        match self.title_of(id) {
            Some(title) => format!("your {title} {list}"),
            None => format!("your {list}"),
        }
    }

    // ---- the forwarder ----

    fn forward(&self, asker: &Arc<Asker>, conn: ConnId, env: Envelope) -> Result<Value, Refusal> {
        match &env.request {
            Request::Status => {
                let opened = Vault::open(&self.shared, &self.local).ok();
                return answer(StatusAnswer {
                    vault: Vault::exists(&self.shared),
                    opens_here: false,
                    can_set_up: false,
                    approvers: asker.near(NEAR),
                    name: self.me.name.clone(),
                    fp: self.me.fp.clone(),
                    holders: opened.as_ref().map(holders_of).unwrap_or_default(),
                    kind: String::new(),
                    request_code: String::new(),
                });
            }
            Request::Setup { .. } | Request::Recover { .. } | Request::HolderRequest { .. } => {
                return Err(refuse(
                    Code::NotHere,
                    "this device has no fingerprint reader of its own to open SpatiKey with: it asks a device that has",
                ))
            }
            Request::Find => return answer(FindAnswer { macs: asker.find(FIND) }),
            Request::PairWith { fp } => return answer(asker.pair(fp)?),
            Request::PairConfirm { fp, ok } => {
                asker.confirm(fp, *ok)?;
                return nothing();
            }
            Request::Target => return target(),
            Request::Pair { .. } => return Err(refuse(Code::BadRequest, "pairing is asked of a Mac, not of this device")),
            Request::Close { session } => match lock(&self.relayed).get(session) {
                None => return nothing(),
                Some(&opener) if opener != conn => {
                    return Err(refuse(Code::BadSession, "that session is another program's"))
                }
                Some(_) => {}
            },
            _ => {}
        }
        let token = session_of(&env.request).map(str::to_string);
        if let Some(token) = &token {
            if lock(&self.relayed).get(token) != Some(&conn) {
                return Err(refuse(Code::BadSession, "SpatiKey is not open for this program: open it again"));
            }
        }
        if matches!(env.request, Request::Type { .. }) {
            return self.type_forwarded(asker, env);
        }
        let opening = matches!(env.request, Request::Open);
        let closing = matches!(env.request, Request::Close { .. });
        let id = env.id;
        let mut reply = self.call_showing_wait(asker, env);
        reply.id = id;
        let answered = reply.into_result();
        match &answered {
            Ok(value) if opening => {
                if let Some(session) = value.get("session").and_then(Value::as_str) {
                    lock(&self.relayed).insert(session.to_string(), conn);
                }
            }
            Err(refusal) if refusal.code == Code::BadSession => {
                if let Some(token) = &token {
                    lock(&self.relayed).remove(token);
                }
            }
            _ => {}
        }
        if closing {
            if let Some(token) = &token {
                lock(&self.relayed).remove(token);
            }
        }
        answered
    }

    /// Typing on this device: the Mac reveals the fields (asking for a finger when there is no
    /// session) and this device's room types them.
    fn type_forwarded(&self, asker: &Asker, env: Envelope) -> Result<Value, Refusal> {
        let Envelope { id, who, request } = env;
        let Request::Type { session, id: entry, fields, window } = request else {
            return Err(refuse(Code::BadRequest, "not a request to type"));
        };
        let (room, window, _) = typing_target(window)?;
        let steps = if fields.iter().any(|&field| needs_secret(field)) {
            let reveal = Envelope { id, who, request: Request::Reveal { session, id: entry, fields: fields.clone() } };
            let value = self.call_showing_wait(asker, reveal).into_result()?;
            // Never the parser's message: it can quote what it could not read.
            let mut got: RevealAnswer =
                serde_json::from_value(value).map_err(|_| refuse(Code::Failed, "the Mac's answer could not be read"))?;
            let steps = steps_from(&fields, &got.values);
            got.values.iter_mut().for_each(Zeroize::zeroize);
            steps?
        } else {
            keys_only(&fields)
        };
        room.type_into(window, &steps).map_err(failed)?;
        nothing()
    }
}

/// An entry, or entries, whose secrets are wiped when they go, however the request ends.
struct Wiping<T: Wipe>(T);

trait Wipe {
    fn wipe_all(&mut self);
}

impl Wipe for Entry {
    fn wipe_all(&mut self) {
        self.wipe();
    }
}

impl Wipe for Vec<Entry> {
    fn wipe_all(&mut self) {
        self.iter_mut().for_each(Entry::wipe);
    }
}

impl<T: Wipe> Drop for Wiping<T> {
    fn drop(&mut self) {
        self.0.wipe_all();
    }
}

impl<T: Wipe> std::ops::Deref for Wiping<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Wipe> std::ops::DerefMut for Wiping<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

/// Look over `service`'s sessions now and then, for as long as it is there.
fn sweep_in_background(service: Weak<Service>) {
    let _ = std::thread::Builder::new().name("spatikey-sweep".into()).spawn(move || loop {
        std::thread::sleep(SWEEP);
        let Some(alive) = service.upgrade() else { break };
        alive.sweep();
    });
}

/// The window the wearer was in before SpatiKey's own (the room says which), or the one asked for.
fn target() -> Result<Value, Refusal> {
    let Some(room) = hooks::room() else {
        return Err(refuse(Code::NotHere, "there is no room here"));
    };
    let Some(target) = room.target() else {
        return Err(refuse(Code::NoMatch, "there is no window to type into"));
    };
    answer(TargetAnswer { window: target.window, app: target.app, title: target.title })
}

/// The room, the window to type into (the one asked for, else the one the wearer was in), and
/// what to call that window in a prompt.
fn typing_target(window: Option<u64>) -> Result<(Arc<dyn Room>, u64, String), Refusal> {
    let Some(room) = hooks::room() else {
        return Err(refuse(Code::NotHere, "there is no room here to type with"));
    };
    let target = room.target();
    let Some(window) = window.or(target.as_ref().map(|t| t.window)) else {
        return Err(refuse(Code::NoMatch, "there is no window to type into"));
    };
    let into = match target.filter(|t| t.window == window) {
        Some(t) if !shown(&t.title).is_empty() => shown(&t.title),
        Some(t) if !shown(&t.app).is_empty() => shown(&t.app),
        _ => "a window".to_string(),
    };
    Ok((room, window, into))
}

impl Service {
    /// A request to the Mac, with a card in this room for as long as a finger may be wanted there:
    /// macOS asks on the Mac's own screen, which the wearer may not be looking at.
    fn call_showing_wait(&self, asker: &Asker, env: Envelope) -> Reply {
        match finger_wanted(&env.request) {
            // Which holder is asked is known once one answers on the network: the card names it
            // then ("Touch ID on darkPhoenix to ..."), and again if another is asked instead.
            Some(what) => self.with_card_as(
                &ByRoom,
                touch_card(&what),
                &what,
                |rename| asker.call_with(env, &|fp: &str, name: &str| rename(self.holder_card(fp, name, &what))),
                |reply| reply.err.as_ref(),
            ),
            None => asker.call(env),
        }
    }

    /// The card while the holder `fp`, which answered as `name`, is asked for a finger.
    fn holder_card(&self, fp: &str, name: &str, what: &str) -> String {
        let record = Vault::open(&self.shared, &self.local).ok().and_then(|vault| vault.touch_device(fp)).filter(|r| !r.removed);
        let name = match (shown(name), record.as_ref().map(|r| shown(&r.name))) {
            (name, _) if !name.is_empty() => name,
            (_, Some(name)) if !name.is_empty() => name,
            _ => return touch_card(what),
        };
        holder_card(record.as_ref().map_or(holders::MAC, |r| r.kind.as_str()), &name, what)
    }

    /// `ask`, which may want a finger on the Mac for `what`, with a card saying so up while it
    /// runs; then the card of another still waiting, if there is one, or the card taken away --
    /// with a few seconds' word on why, when the finger did not come.
    ///
    /// The one way a card is put up, on every platform: on a holder by its guard, into its own
    /// room, for its own requests and other devices' alike; on the Deck and the Beam Pro into
    /// theirs, for as long as a holder is asked.
    fn with_card<T>(&self, shows: &dyn Shows, what: &str, ask: impl FnOnce() -> T, refusal: impl FnOnce(&T) -> Option<&Refusal>) -> T {
        let card = match self.guard().map(|guard| guard.kind()) {
            Some(holders::ANDROID) => format!("Fingerprint on this phone to {what}"),
            _ => touch_card(what),
        };
        self.with_card_as(shows, card, what, |_| ask(), refusal)
    }

    /// [`Service::with_card`] with the card's words given, and a way for `ask` to change them while
    /// it waits.
    fn with_card_as<T>(
        &self,
        shows: &dyn Shows,
        card: String,
        what: &str,
        ask: impl FnOnce(&(dyn Fn(String) + Sync)) -> T,
        refusal: impl FnOnce(&T) -> Option<&Refusal>,
    ) -> T {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        {
            // Said under the lock, so that what the room shows is always the newest still waiting.
            let mut cards = lock(&self.cards);
            cards.push((n, card.clone()));
            shows.show(&card);
        }
        let rename = |text: String| {
            let mut cards = lock(&self.cards);
            let newest = cards.last().is_some_and(|(m, _)| *m == n);
            if let Some((_, said)) = cards.iter_mut().find(|(m, _)| *m == n) {
                if *said != text {
                    *said = text.clone();
                    if newest {
                        shows.show(&text);
                    }
                }
            }
        };
        let result = ask(&rename);
        let mut cards = lock(&self.cards);
        cards.retain(|(m, _)| *m != n);
        match (cards.last(), refusal(&result)) {
            (Some((_, waiting)), _) => shows.show(waiting),
            (None, Some(refused)) if touch_missed(refused.code) => {
                shows.show("");
                shows.brief(&missed_card(what, &refused.message));
            }
            (None, _) => shows.show(""),
        }
        result
    }
}

/// Who puts the card up: the Mac's guard, or this device's room.
trait Shows: Sync {
    fn show(&self, text: &str);
    fn brief(&self, text: &str);
}

struct ByGuard<'a>(&'a dyn Guard);

impl Shows for ByGuard<'_> {
    fn show(&self, text: &str) {
        self.0.notice(text);
    }

    fn brief(&self, text: &str) {
        self.0.notice_briefly(text);
    }
}

/// The room as it is at each word: one may come or go while the Mac is asked.
struct ByRoom;

impl Shows for ByRoom {
    fn show(&self, text: &str) {
        if let Some(room) = hooks::room() {
            room.notice(text);
        }
    }

    fn brief(&self, text: &str) {
        if let Some(room) = hooks::room() {
            room.notice_briefly(text);
        }
    }
}

/// The card while a finger is wanted for `what` ("save a password for github.com").
pub fn touch_card(what: &str) -> String {
    format!("Touch ID on your Mac to {what}")
}

/// The card while the holder `name`, of `kind`, is asked for a finger for `what`: "Touch ID on
/// darkPhoenix to save a password for github.com".
pub fn holder_card(kind: &str, name: &str, what: &str) -> String {
    match kind {
        holders::ANDROID => format!("Fingerprint on {name} to {what}"),
        _ => format!("Touch ID on {name} to {what}"),
    }
}

/// The holders of `vault`, as the protocol has them.
fn holders_of(vault: &Vault) -> Vec<Holder> {
    vault.touch_devices().into_iter().map(|d| Holder { fp: d.device, name: d.name, kind: d.kind, added: d.added }).collect()
}

/// Whether a refusal is the finger not coming -- cancelled, not in time, Touch ID not there --
/// which the card's last word says; anything else is the asking program's to say.
fn touch_missed(code: Code) -> bool {
    matches!(code, Code::Denied | Code::Timeout | Code::TouchIdUnavailable | Code::FingerprintsChanged)
}

/// The card's last word when the finger did not come, in the refusal's own words for why.
pub fn missed_card(what: &str, why: &str) -> String {
    let why = shown(why);
    if why.is_empty() {
        format!("Did not {what}")
    } else {
        format!("Did not {what}: {why}")
    }
}

/// What a request asks a finger for, in the Mac's words, if it does.
fn finger_wanted(request: &Request) -> Option<String> {
    match request {
        Request::Open => Some("open SpatiKey".to_string()),
        Request::Fill { origin } => Some(format!("fill in your password for {}", page_host(origin).ok()?)),
        Request::Save { origin, .. } => Some(format!("save your password for {}", page_host(origin).ok()?)),
        Request::Totp { session: None, .. } => Some("get a code".to_string()),
        Request::Reveal { session: None, .. } => Some("type a password".to_string()),
        Request::ProfileUnlock { profile } => Some(format!("open the profile {}", shown(profile))),
        Request::PasskeyCreate(request) => Some(format!("add a passkey for {}", request.rp_id)),
        Request::PasskeyGet(request) => Some(format!("sign in to {} with a passkey", request.rp_id)),
        Request::HolderApprove { .. } => Some("add a device to those that open SpatiKey".to_string()),
        _ => None,
    }
}

/// Does typing `field` need the vault opened? Tab and Enter do not.
fn needs_secret(field: Field) -> bool {
    matches!(field, Field::Username | Field::Password | Field::Totp)
}

/// The keys for fields that are only keys.
fn keys_only(fields: &[Field]) -> Vec<Step> {
    fields
        .iter()
        .filter_map(|field| match field {
            Field::Tab => Some(Step::Tab),
            Field::Enter => Some(Step::Enter),
            _ => None,
        })
        .collect()
}

/// The steps for `fields`, given the Mac's values for them, in the same order.
fn steps_from(fields: &[Field], values: &[String]) -> Result<Vec<Step>, Refusal> {
    if fields.len() != values.len() {
        return Err(refuse(Code::Failed, "the Mac answered for other fields than were asked"));
    }
    Ok(fields
        .iter()
        .zip(values)
        .map(|(field, value)| match field {
            Field::Tab => Step::Tab,
            Field::Enter => Step::Enter,
            _ => Step::Text(Zeroizing::new(value.clone())),
        })
        .collect())
}

/// One field's value: tab and enter are keys, not text, and give empty strings.
fn value_of(entry: &Entry, field: Field) -> Result<Zeroizing<String>, Refusal> {
    Ok(Zeroizing::new(match field {
        Field::Username => entry.username.clone(),
        Field::Password => entry.password.clone(),
        Field::Totp => {
            let Some(secret) = entry.totp.as_ref() else {
                return Err(refuse(Code::NoMatch, format!("{} has no 2FA code", shown(&entry.title))));
            };
            totp::code(secret, now_secs()).map_err(failed)?.0
        }
        Field::Tab | Field::Enter => String::new(),
    }))
}

/// The session a request is made in, if any.
fn session_of(request: &Request) -> Option<&str> {
    match request {
        Request::Close { session }
        | Request::List { session }
        | Request::Get { session, .. }
        | Request::Put { session, .. }
        | Request::Delete { session, .. }
        | Request::RecoveryNew { session }
        | Request::Devices { session }
        | Request::ForgetDevice { session, .. }
        | Request::HolderApprove { session, .. }
        | Request::HolderRefuse { session, .. }
        | Request::Settings { session, .. } => Some(session.as_str()),
        Request::Totp { session, .. } | Request::Reveal { session, .. } | Request::Type { session, .. } => {
            session.as_deref().filter(|token| !token.is_empty())
        }
        _ => None,
    }
}

/// The host of a page that may be filled or saved: https, or http on this machine.
fn page_host(origin: &str) -> Result<String, Refusal> {
    if !site::is_secure_origin(origin) {
        return Err(refuse(Code::BadRequest, "SpatiKey fills and saves only on https pages (or http on this machine)"));
    }
    site::host_of(origin).ok_or_else(|| refuse(Code::BadRequest, "that is not a page's origin"))
}

/// The page's SHA-256 of clientDataJSON, which is what a passkey signs.
fn client_data_hash(text: &str) -> Result<Vec<u8>, Refusal> {
    match b64::decode_url(text) {
        Ok(hash) if hash.len() == 32 => Ok(hash),
        _ => Err(refuse(Code::BadRequest, "the client data hash is not 32 bytes of base64url")),
    }
}

/// base64url credential ids as bytes, leaving out what is not base64url.
fn decoded(ids: &[String]) -> Vec<Vec<u8>> {
    ids.iter().filter_map(|id| b64::decode_url(id).ok()).collect()
}

/// Is base64url `id` one of `ids`? Compared as bytes, as pages write padding or not.
fn listed(id: &str, ids: &[Vec<u8>]) -> bool {
    b64::decode_url(id).is_ok_and(|id| ids.contains(&id))
}

/// What the prompt adds about who asks: " in Chrome", " on Steam Deck". SpatiKey and SpatiMesh
/// are not named: they are Spatiand's own, and the sentence already names Spatiand.
fn asking(who: &Who) -> String {
    let mut out = String::new();
    let app = shown(&who.app);
    if !app.is_empty() && !app.eq_ignore_ascii_case("SpatiKey") && !app.eq_ignore_ascii_case("SpatiMesh") {
        out.push_str(" in ");
        out.push_str(&app);
    }
    let device = shown(&who.device);
    if !device.is_empty() {
        out.push_str(" on ");
        out.push_str(&device);
    }
    out
}

/// A name as a prompt shows it: one line, and not so long that the sentence is lost.
fn shown(name: &str) -> String {
    let line: String = name.chars().filter(|c| !c.is_control()).collect();
    let line = line.trim();
    if line.chars().count() > LONGEST_NAME {
        format!("{}...", line.chars().take(LONGEST_NAME - 3).collect::<String>().trim_end())
    } else {
        line.to_string()
    }
}

fn entry_id(id: &str) -> Result<(), Refusal> {
    if vault::is_entry_id(id) {
        Ok(())
    } else {
        Err(refuse(Code::BadRequest, "an entry id is 32 hex digits"))
    }
}

fn now_secs() -> u64 {
    crate::now_ms() / 1000
}

/// A lock, whatever a thread that panicked while holding it left: what each guards is whole at
/// every step.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn answer(value: impl Serialize) -> Result<Value, Refusal> {
    serde_json::to_value(value).map_err(|e| refuse(Code::Failed, format!("could not write the answer: {e}")))
}

fn nothing() -> Result<Value, Refusal> {
    Ok(json!({}))
}

fn refuse(code: Code, message: impl Into<String>) -> Refusal {
    Refusal::new(code, message)
}

fn failed(message: String) -> Refusal {
    // The vault's keys changed under an open SpatiKey: it is to be opened again.
    let code = if message == vault::REKEYED { Code::BadSession } else { Code::Failed };
    Refusal::new(code, message)
}

fn no_entry() -> Refusal {
    refuse(Code::NoMatch, "there is no such entry")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A room that writes down each word: `show:` and `brief:`.
    #[derive(Default)]
    struct Said(Mutex<Vec<String>>);

    impl Shows for Said {
        fn show(&self, text: &str) {
            lock(&self.0).push(format!("show:{text}"));
        }

        fn brief(&self, text: &str) {
            lock(&self.0).push(format!("brief:{text}"));
        }
    }

    impl Said {
        fn take(&self) -> Vec<String> {
            std::mem::take(&mut *lock(&self.0))
        }
    }

    fn service() -> Arc<Service> {
        let place = std::env::temp_dir().join(format!("sk-cards-{}", std::process::id()));
        let me = Me { fp: "ab".repeat(32), name: "Mac".into() };
        let asker = Asker::new(spatiand_stream::Identity::new().unwrap(), place.join("local"), "Mac".into());
        Service::new(Mode::Forwarder(Arc::new(asker)), me, place.join("shared"), place.join("local"))
    }

    fn refused(code: Code, why: &str) -> Result<(), Refusal> {
        Err(Refusal::new(code, why))
    }

    #[test]
    fn a_card_is_up_while_the_finger_is_wanted_and_then_gone() {
        let service = service();
        let room = Said::default();
        let during = service.with_card(&room, "save a password for github.com", || room.take(), |_| None);
        assert_eq!(during, ["show:Touch ID on your Mac to save a password for github.com"]);
        assert_eq!(room.take(), ["show:"]);
        // Answered: gone, with nothing more said.
        let _ = service.with_card(&room, "open SpatiKey", || Ok::<_, Refusal>(()), |r| r.as_ref().err());
        assert_eq!(room.take(), ["show:Touch ID on your Mac to open SpatiKey", "show:"]);
    }

    #[test]
    fn a_finger_that_did_not_come_is_said_briefly_and_other_refusals_are_not() {
        let service = service();
        let room = Said::default();
        let ask = |code, why: &str| {
            let _ = service.with_card(&room, "save a password for github.com", || refused(code, why), |r| r.as_ref().err());
            room.take()
        };
        assert_eq!(
            ask(Code::Denied, "Touch ID was cancelled"),
            [
                "show:Touch ID on your Mac to save a password for github.com",
                "show:",
                "brief:Did not save a password for github.com: Touch ID was cancelled"
            ]
        );
        assert_eq!(ask(Code::Timeout, "no finger was put on Touch ID in time")[2], "brief:Did not save a password for github.com: no finger was put on Touch ID in time");
        assert_eq!(ask(Code::TouchIdUnavailable, "")[2], "brief:Did not save a password for github.com");
        assert_eq!(ask(Code::FingerprintsChanged, "the fingers changed").len(), 3);
        // Not the finger: the asking program says it, and the card just goes.
        for code in [Code::NoMatch, Code::NoApprover, Code::NotReachable, Code::Failed, Code::BadRequest] {
            assert_eq!(ask(code, "something")[1..], ["show:".to_string()], "{code:?}");
        }
    }

    #[test]
    fn a_card_still_waiting_comes_back_when_a_newer_one_goes() {
        let service = service();
        let room = Said::default();
        // One inside the other, as two programs asking at once on a device would be.
        service.with_card(&room, "open SpatiKey", || service.with_card(&room, "get a code", || (), |_| None), |_| None);
        assert_eq!(
            room.take(),
            [
                "show:Touch ID on your Mac to open SpatiKey",
                "show:Touch ID on your Mac to get a code",
                "show:Touch ID on your Mac to open SpatiKey",
                "show:"
            ]
        );
        // The older answered first: the newer's card stays up, and when it goes unanswered, that
        // is said.
        let (finish, finished) = std::sync::mpsc::channel::<()>();
        std::thread::scope(|scope| {
            let mut newer = None;
            fn wait(finished: std::sync::mpsc::Receiver<()>) -> Result<(), Refusal> {
                let _ = finished.recv();
                refused(Code::Timeout, "no finger was put on Touch ID in time")
            }
            service.with_card(
                &room,
                "open SpatiKey",
                || {
                    newer = Some(scope.spawn(|| service.with_card(&room, "get a code", || wait(finished), |r| r.as_ref().err())));
                    while lock(&service.cards).len() < 2 {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                },
                |_| None,
            );
            assert_eq!(
                room.take(),
                [
                    "show:Touch ID on your Mac to open SpatiKey",
                    "show:Touch ID on your Mac to get a code",
                    "show:Touch ID on your Mac to get a code"
                ]
            );
            finish.send(()).unwrap();
            let _ = newer.unwrap().join();
        });
        assert_eq!(room.take(), ["show:", "brief:Did not get a code: no finger was put on Touch ID in time"]);
        assert!(lock(&service.cards).is_empty());
    }

    #[test]
    fn a_prompt_names_who_asks_but_not_spatiands_own() {
        let who = |app: &str, device: &str| Who { app: app.into(), device: device.into() };
        assert_eq!(asking(&who("", "")), "");
        assert_eq!(asking(&who("SpatiMesh", "")), "");
        assert_eq!(asking(&who("SpatiKey", "Steam Deck")), " on Steam Deck");
        assert_eq!(asking(&who("Chrome", "Steam Deck")), " in Chrome on Steam Deck");
        assert_eq!(asking(&who("Fire\nfox", "")), " in Firefox");
        let long = shown(&"x".repeat(200));
        assert!(long.chars().count() <= LONGEST_NAME && long.ends_with("..."));
    }

    #[test]
    fn sessions_are_named_in_every_request_that_has_one() {
        let session = || "s".to_string();
        assert_eq!(session_of(&Request::List { session: session() }), Some("s"));
        assert_eq!(session_of(&Request::Settings { session: session(), set: None }), Some("s"));
        assert_eq!(session_of(&Request::Totp { session: Some(session()), id: String::new() }), Some("s"));
        assert_eq!(session_of(&Request::Totp { session: Some(String::new()), id: String::new() }), None);
        assert_eq!(session_of(&Request::Reveal { session: None, id: String::new(), fields: vec![] }), None);
        assert_eq!(session_of(&Request::Open), None);
    }

    #[test]
    fn typed_fields_follow_the_order_asked() {
        let fields = [Field::Username, Field::Tab, Field::Password, Field::Enter];
        let values = ["paulus", "", "hunter2", ""].map(String::from);
        let steps = steps_from(&fields, &values).unwrap();
        assert_eq!(
            steps,
            vec![
                Step::Text(Zeroizing::new("paulus".into())),
                Step::Tab,
                Step::Text(Zeroizing::new("hunter2".into())),
                Step::Enter
            ]
        );
        assert!(steps_from(&fields, &values[..3]).is_err());
        assert_eq!(keys_only(&[Field::Tab, Field::Enter]), vec![Step::Tab, Step::Enter]);
    }
}
