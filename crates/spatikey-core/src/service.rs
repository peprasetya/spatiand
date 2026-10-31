//! The service: the only thing that reads the vault. docs/spatikey.md, "Asking for something".
//!
//! On a Mac with Touch ID it is an *approver*: it opens the vault with the [`Guard`] (the Swift
//! half) and answers. On the Deck and the Beam Pro it is a *forwarder*: it sends each request to
//! a Mac on the same network ([`crate::remote::Asker`]) and keeps for itself only what needs
//! this machine -- typing into its windows, the window to type into, finding and pairing Macs.
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
    Choice, ChoicesAnswer, Code, Device, DevicesAnswer, Envelope, Field, FillAnswer, FindAnswer, GetAnswer, IdAnswer,
    ListAnswer, Login, OpenAnswer, PasskeyCreate, PasskeyGet, Refusal, Reply, Request, RevealAnswer, Settings,
    SettingsAnswer, SetupAnswer, StatusAnswer, TargetAnswer, TotpAnswer, Who,
};
use crate::{b64, site, totp, webauthn};

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

/// What only the Mac's own code can do: the Secure Enclave and Touch ID, and asking its owner.
pub trait Guard: Send + Sync {
    /// Ask for a finger and hand back the vault key. `reason` finishes macOS's sentence
    /// "Spatiand is trying to ...". `linger_secs` lets one touch cover what follows for that
    /// long (0: this use only).
    fn unlock(&self, reason: &str, touch: &TouchId, linger_secs: u64) -> Result<VaultKey, Refusal>;
    /// Make this Mac's Secure Enclave key and wrap `key` to it. The answer's `vault` is left
    /// empty, for the caller to fill.
    fn enroll(&self, key: &VaultKey) -> Result<TouchId, Refusal>;
    /// Touch ID is there and has a finger enrolled (asks nothing).
    fn can_touch(&self) -> bool;
    /// The owner's yes or no to a device that wants to ask this Mac, with the six digits both show.
    fn allow_device(&self, name: &str, fp: &str, code: &str) -> bool;
    /// Tell the owner something (a card in the room, on the Mac's screen).
    fn notice(&self, text: &str);
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
    Approver(Arc<dyn Guard>),
    Forwarder(Arc<crate::remote::Asker>),
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
}

impl Service {
    pub fn new(mode: Mode, me: Me, shared: PathBuf, local: PathBuf) -> Arc<Service> {
        let approver = matches!(mode, Mode::Approver(_));
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
        });
        if approver {
            sweep_in_background(Arc::downgrade(&service));
        }
        service
    }

    pub fn me(&self) -> &Me {
        &self.me
    }

    pub fn is_approver(&self) -> bool {
        matches!(self.mode, Mode::Approver(_))
    }

    /// Answer one request from connection `conn`. Blocks while a person is asked.
    pub fn handle(&self, conn: ConnId, env: Envelope) -> Reply {
        let id = env.id;
        let answered = match &self.mode {
            Mode::Approver(guard) => self.approve(guard.as_ref(), conn, env),
            Mode::Forwarder(asker) => self.forward(asker, conn, env),
        };
        match answered {
            Ok(value) => Reply { id, ok: Some(value), err: None },
            Err(refusal) => Reply::err(id, refusal),
        }
    }

    /// The connection has gone: its sessions close.
    pub fn closed(&self, conn: ConnId) {
        lock(&self.sessions).retain(|_, session| session.conn != conn);
        if let Mode::Forwarder(asker) = &self.mode {
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
            Mode::Approver(guard) => Some(guard),
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
            Request::Setup { .. } | Request::Recover { .. } | Request::Type { .. } | Request::Target if elsewhere => {
                Err(refuse(Code::NotHere, "that is done on the Mac itself"))
            }
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
            Request::ForgetDevice { session, fp } => self.forget_device(conn, &session, &fp),
            Request::Settings { session, set } => self.settings_asked(conn, &session, set),
            Request::Pair { .. } => Err(refuse(Code::BadRequest, "pairing is asked of a Mac over the network, not here")),
            Request::Find | Request::PairWith { .. } | Request::PairConfirm { .. } => {
                Err(refuse(Code::NotHere, "a Mac does not pair with Macs: a device pairs with it"))
            }
        }
    }

    fn status(&self, guard: &dyn Guard) -> StatusAnswer {
        let vault = Vault::exists(&self.shared);
        let opens_here = vault && Vault::open(&self.shared, &self.local).is_ok_and(|v| v.touch_id().is_some());
        StatusAnswer {
            vault,
            opens_here,
            can_set_up: !opens_here && guard.can_touch(),
            approvers: Vec::new(),
            name: self.me.name.clone(),
            fp: self.me.fp.clone(),
        }
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
        let touch = guard.enroll(&key)?;
        let vault = Vault::create_with(&self.shared, &self.local, &self.short(), &key).map_err(failed)?;
        self.enrolled(&vault, touch, name)?;
        let recovery_code = vault.set_recovery(&key).map_err(failed)?;
        answer(SetupAnswer { recovery_code })
    }

    /// Keep what Touch ID made for `vault`: this Mac's half in `local`, and its record in
    /// `devices`, each replacing any before.
    fn enrolled(&self, vault: &Vault, touch: TouchId, name: &str) -> Result<(), Refusal> {
        vault.set_touch_id(&touch).map_err(failed)?;
        let name = match name.trim() {
            "" => self.me.name.clone(),
            name => name.to_string(),
        };
        let device = TouchDevice {
            device: self.me.fp.clone(),
            name,
            kind: "mac-touch-id".into(),
            public: touch.public,
            wrapped: touch.wrapped,
            added: crate::now_ms(),
        };
        vault.add_touch_device(&device).map_err(failed)
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
            return Err(refuse(Code::TouchIdUnavailable, "Touch ID cannot be used on this Mac now"));
        }
        let key = vault.recover(&code).map_err(|e| refuse(Code::Denied, e))?;
        let _prompt = lock(&self.prompt);
        let touch = guard.enroll(&key)?;
        self.enrolled(&vault, touch, "")?;
        log::info!("spatikey: Touch ID set up again for vault {} from the recovery code", vault.id());
        nothing()
    }

    fn devices(&self, conn: ConnId, token: &str) -> Result<Value, Refusal> {
        let vault = self.vault()?;
        self.session(conn, token, &vault)?;
        let touch_ids =
            vault.touch_devices().into_iter().map(|d| Device { fp: d.device, name: d.name, added: d.added }).collect();
        answer(DevicesAnswer { allowed: self.allowed(), touch_ids })
    }

    fn forget_device(&self, conn: ConnId, token: &str, fp: &str) -> Result<Value, Refusal> {
        let vault = self.vault()?;
        self.session(conn, token, &vault)?;
        if fp.eq_ignore_ascii_case(&self.me.fp) {
            return Err(refuse(Code::BadRequest, "this Mac cannot forget itself; the recovery code sets Touch ID up again"));
        }
        let _writing = lock(&self.writing);
        let mut forgot = false;
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
        nothing()
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
    fn unlock_lingering(&self, guard: &dyn Guard, vault: &Vault, who: &Who, what: &str, linger: u64) -> Result<VaultKey, Refusal> {
        let Some(touch) = vault.touch_id() else {
            return Err(refuse(
                Code::NotHere,
                "Touch ID is not set up for this vault on this Mac: set it up from the recovery code",
            ));
        };
        let reason = format!("{what}{}", asking(who));
        let _prompt = lock(&self.prompt);
        // Someone asking from another device sees a card here too: the finger is wanted on this Mac.
        let elsewhere = !who.device.is_empty();
        if elsewhere {
            guard.notice(&reason);
        }
        let key = guard.unlock(&reason, &touch, linger);
        if elsewhere {
            guard.notice("");
        }
        let key = key?;
        if !vault.check(&key) {
            return Err(refuse(Code::Failed, "Touch ID gave a key that does not open this vault"));
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
                return answer(StatusAnswer {
                    vault: Vault::exists(&self.shared),
                    opens_here: false,
                    can_set_up: false,
                    approvers: asker.near(NEAR),
                    name: self.me.name.clone(),
                    fp: self.me.fp.clone(),
                })
            }
            Request::Setup { .. } | Request::Recover { .. } => {
                return Err(refuse(Code::NotHere, "SpatiKey is set up on a Mac, with its Touch ID"))
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
        let mut reply = asker.call(env);
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
            let value = asker.call(reveal).into_result()?;
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
    Refusal::new(Code::Failed, message)
}

fn no_entry() -> Refusal {
    refuse(Code::NoMatch, "there is no such entry")
}

#[cfg(test)]
mod tests {
    use super::*;

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
