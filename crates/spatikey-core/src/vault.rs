//! The vault's files and their encryption. docs/spatikey.md, "The vault".
//!
//! Everything under `apps/spatikey` is synced to every linked Spatiand, deepmagpie included, so
//! nothing there is readable or alterable without the vault key: each entry is sealed with
//! ChaCha20-Poly1305 under a key derived from it, bound by its associated data to its own name
//! and its own vault. What is this Mac's alone (its Touch ID half) is in `local/spatikey`.

use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305, NONCE_LEN};
use ring::hkdf;
use ring::rand::{SecureRandom, SystemRandom};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::b64;
use crate::entry::{Entry, Summary};

/// An entry is padded to a multiple of this, so that a file's size says little of what is in it.
pub const PAD_TO: usize = 256;

/// The most an entry may be once padded. The synchroniser drops files over 4 MiB and a host
/// closes a session whose message passes 1 MiB; a vault of many small files stays far from both.
pub const LARGEST_ENTRY: usize = 64 * 1024;

/// What the entry key is derived for (HKDF's info).
const ENTRY_INFO: &[u8] = b"spatikey entries v1";

/// What `vault.json`'s check seals, to tell a right key from a wrong one.
const CHECK: &[u8] = b"spatikey";

/// The vault key: 32 random bytes, never written anywhere in the clear.
#[derive(Clone)]
pub struct VaultKey(pub(crate) Zeroizing<[u8; 32]>);

impl VaultKey {
    pub fn random() -> VaultKey {
        let mut key = Zeroizing::new([0u8; 32]);
        fill_random(&mut key[..]);
        VaultKey(key)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<VaultKey, String> {
        if bytes.len() != 32 {
            return Err(format!("a vault key is 32 bytes, not {}", bytes.len()));
        }
        let mut key = Zeroizing::new([0u8; 32]);
        key.copy_from_slice(bytes);
        Ok(VaultKey(key))
    }

    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for VaultKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VaultKey(..)")
    }
}

/// `vault.json`: who the vault is. Written once, at creation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Header {
    pub v: u32,
    /// 16 hex characters, random.
    pub id: String,
    pub created: u64,
    /// The short fingerprint of the Mac that made it.
    pub by: String,
    /// "spatikey" sealed under the entry key (base64 of nonce ‖ ciphertext), to tell a right key
    /// from a wrong one.
    pub check: String,
}

/// `devices/<fp>.json`: a Mac that can open the vault with its Touch ID.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TouchDevice {
    /// The Mac's fingerprint, 64 hex.
    pub device: String,
    pub name: String,
    /// "mac-touch-id".
    pub kind: String,
    /// Its Secure Enclave public key, X9.63, base64.
    pub public: String,
    /// The vault key wrapped to that key (docs/spatikey.md, "Keys"), base64.
    pub wrapped: String,
    pub added: u64,
}

/// `local/spatikey/touch-id.json`: this Mac's own half, never sent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TouchId {
    /// The vault it opens.
    pub vault: String,
    /// The Secure Enclave public key, X9.63, base64.
    pub public: String,
    /// CryptoKit's dataRepresentation of the Secure Enclave key, base64: usable only by this
    /// Mac's Secure Enclave.
    pub blob: String,
    /// The vault key wrapped to it, base64.
    pub wrapped: String,
}

/// An entry's file: `{"v":1,"id":..,"n":<nonce>,"c":<ciphertext and tag>}`, both standard base64.
#[derive(Serialize, Deserialize)]
struct Sealed {
    v: u32,
    id: String,
    n: String,
    c: String,
}

/// What is sealed: the entry, and spaces enough to make it a multiple of [`PAD_TO`].
#[derive(Serialize)]
struct Plain<'a> {
    entry: &'a Entry,
    pad: &'a str,
}

#[derive(Deserialize)]
struct Opened {
    entry: Entry,
}

pub struct Vault {
    shared: PathBuf,
    local: PathBuf,
    header: Header,
}

impl Vault {
    /// Is there a vault in `shared`?
    pub fn exists(shared: &Path) -> bool {
        shared.join("vault.json").is_file()
    }

    /// Make a new vault (refused if one is there). Returns it and its key.
    pub fn create(shared: &Path, local: &Path, by: &str) -> Result<(Vault, VaultKey), String> {
        let key = VaultKey::random();
        let vault = Vault::create_with(shared, local, by, &key)?;
        Ok((vault, key))
    }

    /// [`Vault::create`] with a key made beforehand: the service has Touch ID take the key
    /// before anything is written, so that a cancelled panel leaves no vault behind.
    pub fn create_with(shared: &Path, local: &Path, by: &str, key: &VaultKey) -> Result<Vault, String> {
        if Vault::exists(shared) {
            return Err("there already is a vault".into());
        }
        make_dir(&shared.join("entries"))?;
        make_dir(&shared.join("devices"))?;
        let id = random_hex(8);
        let keys = Keys::new(&id, key)?;
        let check = keys.seal_check()?;
        let header = Header { v: 1, id, created: crate::now_ms(), by: by.to_string(), check };
        let bytes = serde_json::to_vec(&header).map_err(|e| e.to_string())?;
        write_synced(&shared.join("vault.json"), &bytes).map_err(|e| format!("could not write vault.json: {e}"))?;
        log::info!("spatikey: made vault {}", header.id);
        Ok(Vault { shared: shared.to_path_buf(), local: local.to_path_buf(), header })
    }

    pub fn open(shared: &Path, local: &Path) -> Result<Vault, String> {
        let path = shared.join("vault.json");
        let bytes = std::fs::read(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "there is no vault".to_string(),
            _ => format!("{}: {e}", path.display()),
        })?;
        let header: Header =
            serde_json::from_slice(&bytes).map_err(|e| format!("{} is not a vault's: {e}", path.display()))?;
        if header.v != 1 {
            return Err(format!("a vault of version {}; this SpatiKey reads version 1", header.v));
        }
        if !is_hex(&header.id, 16) {
            return Err(format!("{} names its vault with something other than 16 hex digits", path.display()));
        }
        Ok(Vault { shared: shared.to_path_buf(), local: local.to_path_buf(), header })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn id(&self) -> &str {
        &self.header.id
    }

    /// Does `key` open this vault?
    pub fn check(&self, key: &VaultKey) -> bool {
        let Ok(keys) = Keys::new(self.id(), key) else { return false };
        let Ok(mut sealed) = b64::decode(&self.header.check) else { return false };
        if sealed.len() < NONCE_LEN {
            return false;
        }
        let nonce: Vec<u8> = sealed.drain(..NONCE_LEN).collect();
        keys.open(&check_aad(self.id()), &nonce, &mut sealed).is_ok_and(|plain| plain == CHECK)
    }

    /// Every live entry, decrypted. One that does not open (damaged, altered, another vault's)
    /// is skipped and logged by its id only.
    pub fn entries(&self, key: &VaultKey) -> Result<Vec<Entry>, String> {
        let keys = Keys::new(self.id(), key)?;
        let mut found = Vec::new();
        for id in self.entry_ids()? {
            match self.open_file(&keys, &id) {
                Ok(entry) if entry.deleted => {}
                Ok(entry) => found.push(entry),
                Err(e) => log::warn!("spatikey: entry {id} does not open ({e}); left out"),
            }
        }
        found.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
        Ok(found)
    }

    pub fn summaries(&self, key: &VaultKey) -> Result<Vec<Summary>, String> {
        let mut entries = self.entries(key)?;
        let summaries = entries.iter().map(Entry::summary).collect();
        entries.iter_mut().for_each(Entry::wipe);
        Ok(summaries)
    }

    /// Is there a file for the entry `id`, opening or not?
    pub fn contains(&self, id: &str) -> bool {
        is_entry_id(id) && self.entry_path(id).is_file()
    }

    /// One entry, tombstone or not.
    pub fn read(&self, key: &VaultKey, id: &str) -> Result<Entry, String> {
        if !is_entry_id(id) {
            return Err("an entry id is 32 hex digits".into());
        }
        let keys = Keys::new(self.id(), key)?;
        self.open_file(&keys, id)
    }

    /// Write `entry`: a new random id when it has none, `version` one more than what is on disk,
    /// `changed` now, `changed_by` = `by`, `created` kept (or now). Padded, atomic, with a file
    /// time that only moves forward.
    pub fn write(&self, key: &VaultKey, entry: &mut Entry, by: &str) -> Result<(), String> {
        if entry.id.is_empty() {
            entry.id = random_hex(16);
        } else if !is_entry_id(&entry.id) {
            return Err("an entry id is 32 hex digits".into());
        }
        let keys = Keys::new(self.id(), key)?;
        let path = self.entry_path(&entry.id);
        // What is there now, for its version and when it was made. One that does not open is
        // counted as not there: it cannot be told from a forgery.
        let (on_disk, created) = match std::fs::read(&path).ok().and_then(|bytes| open_entry(&keys, &entry.id, &bytes).ok())
        {
            Some(mut old) => {
                old.wipe();
                (old.version, old.created)
            }
            None => (0, 0),
        };
        let now = crate::now_ms();
        entry.version = on_disk.max(entry.version) + 1;
        entry.created = [created, entry.created, now].into_iter().find(|&t| t > 0).unwrap_or(now);
        entry.changed = now;
        entry.changed_by = by.to_string();
        if entry.deleted {
            // A tombstone carries nothing, whoever asked for it.
            entry.bury();
        }
        let bytes = seal_entry(&keys, entry)?;
        write_synced(&path, &bytes).map_err(|e| format!("could not write entry {}: {e}", entry.id))
    }

    /// Rewrite an entry as a tombstone.
    pub fn delete(&self, key: &VaultKey, id: &str, by: &str) -> Result<(), String> {
        let mut entry = self.read(key, id)?;
        if entry.deleted {
            return Ok(());
        }
        entry.bury();
        self.write(key, &mut entry, by)
    }

    pub fn touch_devices(&self) -> Vec<TouchDevice> {
        let dir = self.shared.join("devices");
        let mut devices: Vec<TouchDevice> =
            json_names(&dir).into_iter().filter_map(|name| read_json(&dir.join(format!("{name}.json")))).collect();
        devices.sort_by(|a, b| a.added.cmp(&b.added).then_with(|| a.device.cmp(&b.device)));
        devices
    }

    pub fn add_touch_device(&self, device: &TouchDevice) -> Result<(), String> {
        if !is_hex(&device.device, 64) {
            return Err("a Mac's fingerprint is 64 hex digits".into());
        }
        write_json(&self.device_path(&device.device), device)
    }

    pub fn remove_touch_device(&self, fp: &str) -> Result<(), String> {
        if !is_hex(fp, 64) {
            return Err("a Mac's fingerprint is 64 hex digits".into());
        }
        match std::fs::remove_file(self.device_path(fp)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("could not forget {fp}: {e}")),
        }
    }

    /// This Mac's own half, if it has one for this vault.
    pub fn touch_id(&self) -> Option<TouchId> {
        read_json::<TouchId>(&self.local.join("touch-id.json")).filter(|touch| touch.vault == self.header.id)
    }

    /// Keep this Mac's half, as this vault's whatever its `vault` said.
    pub fn set_touch_id(&self, touch: &TouchId) -> Result<(), String> {
        let touch = TouchId { vault: self.header.id.clone(), ..touch.clone() };
        write_json(&self.local.join("touch-id.json"), &touch)
    }

    pub fn has_recovery(&self) -> bool {
        self.shared.join("recovery.json").is_file()
    }

    /// A new recovery code, replacing the old: returns the code, to show once.
    pub fn set_recovery(&self, key: &VaultKey) -> Result<String, String> {
        self.set_recovery_with(key, crate::recovery::COST)
    }

    /// [`Vault::set_recovery`] at another cost (tests: Argon2id at its full cost takes seconds in a
    /// debug build).
    pub fn set_recovery_with(&self, key: &VaultKey, cost: (u32, u32, u32)) -> Result<String, String> {
        if !self.check(key) {
            return Err("that key does not open this vault".into());
        }
        let code = crate::recovery::new_code();
        let record = crate::recovery::wrap_with(key, &code, self.id(), cost)?;
        write_json(&self.shared.join("recovery.json"), &record)?;
        Ok(code)
    }

    /// The vault key, from the recovery code.
    pub fn recover(&self, code: &str) -> Result<VaultKey, String> {
        let path = self.shared.join("recovery.json");
        let record: crate::recovery::RecoveryRecord = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("recovery.json cannot be read: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err("this vault has no recovery code".into()),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let key = crate::recovery::unwrap(&record, code, self.id())?;
        if !self.check(&key) {
            return Err("the recovery code gave a key that does not open this vault".into());
        }
        Ok(key)
    }

    fn entry_path(&self, id: &str) -> PathBuf {
        self.shared.join("entries").join(format!("{id}.json"))
    }

    fn device_path(&self, fp: &str) -> PathBuf {
        self.shared.join("devices").join(format!("{}.json", fp.to_ascii_lowercase()))
    }

    /// The ids of the entries on disk: files named by 32 hex digits, nothing hidden or half
    /// written.
    fn entry_ids(&self) -> Result<Vec<String>, String> {
        let dir = self.shared.join("entries");
        match std::fs::read_dir(&dir) {
            Ok(_) => Ok(json_names(&dir).into_iter().filter(|name| is_entry_id(name)).collect()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(format!("{}: {e}", dir.display())),
        }
    }

    fn open_file(&self, keys: &Keys, id: &str) -> Result<Entry, String> {
        let bytes = std::fs::read(self.entry_path(id)).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => format!("no entry {id}"),
            _ => e.to_string(),
        })?;
        open_entry(keys, id, &bytes)
    }
}

/// The entry key, ready to seal and open with, and the vault it is for.
struct Keys {
    aead: LessSafeKey,
    vault: String,
}

impl Keys {
    fn new(vault: &str, key: &VaultKey) -> Result<Keys, String> {
        let bytes = entry_key(vault, key)?;
        let unbound = UnboundKey::new(&CHACHA20_POLY1305, &bytes[..]).map_err(|_| "the entry key is not a key")?;
        Ok(Keys { aead: LessSafeKey::new(unbound), vault: vault.to_string() })
    }

    /// Seal `in_out` in place (the tag is appended) under a fresh random nonce, returned.
    fn seal(&self, aad: &str, in_out: &mut Vec<u8>) -> Result<[u8; NONCE_LEN], String> {
        let mut nonce = [0u8; NONCE_LEN];
        fill_random(&mut nonce);
        self.aead
            .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(aad.as_bytes()), in_out)
            .map_err(|_| "could not seal".to_string())?;
        Ok(nonce)
    }

    fn open<'a>(&self, aad: &str, nonce: &[u8], sealed: &'a mut [u8]) -> Result<&'a mut [u8], String> {
        let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| "a nonce of the wrong length".to_string())?;
        self.aead
            .open_in_place(nonce, Aad::from(aad.as_bytes()), sealed)
            .map_err(|_| "it does not open with this key (altered, moved, or another vault's)".to_string())
    }

    /// The header's check: base64 of nonce ‖ ciphertext ‖ tag.
    fn seal_check(&self) -> Result<String, String> {
        let mut sealed = CHECK.to_vec();
        let nonce = self.seal(&check_aad(&self.vault), &mut sealed)?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&sealed);
        Ok(b64::encode(&out))
    }
}

/// HKDF-SHA256 with the vault id as salt, the vault key as input and `spatikey entries v1` as
/// info: the key every entry of this vault is sealed under.
fn entry_key(vault: &str, key: &VaultKey) -> Result<Zeroizing<[u8; 32]>, String> {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, vault.as_bytes()).extract(key.bytes());
    let mut out = Zeroizing::new([0u8; 32]);
    prk.expand(&[ENTRY_INFO], &CHACHA20_POLY1305)
        .and_then(|okm| okm.fill(&mut out[..]))
        .map_err(|_| "could not derive the entry key".to_string())?;
    Ok(out)
}

/// An entry's associated data: its vault and its own id, so that a file moved to another name
/// or another vault does not open.
fn entry_aad(vault: &str, id: &str) -> String {
    format!("spatikey/1/{vault}/{id}")
}

fn check_aad(vault: &str) -> String {
    format!("spatikey/1/{vault}/check")
}

/// Counts what is written, to learn a serialisation's length without keeping a copy of it.
struct Count(usize);

impl std::io::Write for Count {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn seal_entry(keys: &Keys, entry: &Entry) -> Result<Vec<u8>, String> {
    let mut count = Count(0);
    serde_json::to_writer(&mut count, &Plain { entry, pad: "" }).map_err(|_| "could not write the entry".to_string())?;
    let padded = count.0.div_ceil(PAD_TO) * PAD_TO;
    if padded > LARGEST_ENTRY {
        return Err(format!(
            "an entry is kept under {} KiB, and this one would be {} KiB",
            LARGEST_ENTRY / 1024,
            padded.div_ceil(1024)
        ));
    }
    let pad = " ".repeat(padded - count.0);
    // Room for the tag from the start, so that sealing in place never leaves a copy of the
    // plaintext behind in a buffer given back to the allocator.
    let mut plain = Zeroizing::new(Vec::with_capacity(padded + CHACHA20_POLY1305.tag_len()));
    serde_json::to_writer(&mut *plain, &Plain { entry, pad: &pad }).map_err(|_| "could not write the entry".to_string())?;
    if plain.len() != padded {
        return Err("the entry did not come out at the length it was padded to".into());
    }
    let nonce = keys.seal(&entry_aad(&keys.vault, &entry.id), &mut plain)?;
    let file = Sealed { v: 1, id: entry.id.clone(), n: b64::encode(&nonce), c: b64::encode(&plain) };
    serde_json::to_vec(&file).map_err(|e| e.to_string())
}

/// Open an entry file read from `entries/<id>.json`. The name is what the associated data is
/// made from: the id the file claims for itself must agree, and so must the entry inside.
fn open_entry(keys: &Keys, id: &str, bytes: &[u8]) -> Result<Entry, String> {
    let file: Sealed = serde_json::from_slice(bytes).map_err(|e| format!("not an entry file: {e}"))?;
    if file.v != 1 {
        return Err(format!("version {} of the entry format", file.v));
    }
    if file.id != id {
        return Err("its name and the id inside differ".into());
    }
    let nonce = b64::decode(&file.n)?;
    let mut sealed = Zeroizing::new(b64::decode(&file.c)?);
    let plain = keys.open(&entry_aad(&keys.vault, id), &nonce, &mut sealed[..])?;
    // Never the parser's message: it can quote what it could not read, which is a secret here.
    let opened: Opened = serde_json::from_slice(plain).map_err(|_| "what it holds is not an entry".to_string())?;
    let mut entry = opened.entry;
    if entry.id.is_empty() {
        entry.id = id.to_string();
    } else if entry.id != id {
        entry.wipe();
        return Err("the entry inside is another one".into());
    }
    Ok(entry)
}

/// Is `id` an entry's id as this vault makes them: 32 lowercase hex digits?
pub fn is_entry_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_hex(text: &str, length: usize) -> bool {
    text.len() == length && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Fill `out` from the system's randomness. There is no carrying on without it.
pub(crate) fn fill_random(out: &mut [u8]) {
    SystemRandom::new().fill(out).expect("the system's random numbers");
}

/// `bytes` random bytes, as lowercase hex.
pub(crate) fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    fill_random(&mut buffer);
    data_encoding::HEXLOWER.encode(&buffer)
}

/// Make a folder (and those above it) only this user can enter.
fn make_dir(dir: &Path) -> Result<(), String> {
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| format!("{}: {e}", dir.display()))
}

/// The names of the `.json` files in `dir`, without it, leaving out hidden ones.
fn json_names(dir: &Path) -> Vec<String> {
    let Ok(listing) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = listing
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .filter_map(|n| n.strip_suffix(".json").map(str::to_string))
        .collect();
    names.sort();
    names
}

/// A small JSON file, read; None if it is not there or is not what it should be.
pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(value) => Some(value),
        Err(e) => {
            log::warn!("spatikey: {} could not be read: {e}", path.display());
            None
        }
    }
}

/// A small JSON file, written as [`write_synced`] writes.
pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    write_synced(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Write `bytes` to `path` as synced files must be written: a hidden `.<name>.<pid>.new` beside
/// it, fsync, rename; mode 0600; the file's time only moving forward (the old time plus a
/// millisecond if the clock is behind it). The folder is made (0700) if it is not there.
pub fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "a file with no name"))?
        .to_string_lossy();
    if !dir.is_dir() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    // One write at a time in this process: two writers of one file would share its scratch name.
    static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = WRITING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let scratch = dir.join(format!(".{name}.{}.new", std::process::id()));
    let written = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&scratch)?;
        // A scratch file left by an earlier process of the same id keeps its old mode otherwise.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(bytes)?;
        // In whole milliseconds, as the synchroniser compares them: the same millisecond as the
        // old file would not be newer to it.
        let now = millis(SystemTime::now());
        let at = match std::fs::metadata(path).and_then(|m| m.modified()) {
            Ok(old) => now.max(millis(old) + 1),
            Err(_) => now,
        };
        file.set_modified(UNIX_EPOCH + Duration::from_millis(at))?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&scratch, path)?;
        // The rename itself, made lasting. A file system that cannot sync a folder says so with
        // EINVAL or ENOTSUP; the file is in place all the same.
        match std::fs::File::open(dir)?.sync_all() {
            Err(e) if e.raw_os_error().is_some_and(|n| [libc::EINVAL, libc::ENOTSUP, libc::EOPNOTSUPP].contains(&n)) => Ok(()),
            synced => synced,
        }
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&scratch);
    }
    written
}

fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir()
            .join(format!("spatikey-vault-{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_entry_key_is_hkdf_sha256_as_documented() {
        // HKDF by hand (RFC 5869) with HMAC-SHA256: PRK = HMAC(salt, IKM), T(1) = HMAC(PRK, info ‖ 1).
        use ring::hmac;
        let key = VaultKey::from_bytes(&[7u8; 32]).unwrap();
        let prk = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, b"0123456789abcdef"), key.bytes());
        let mut block = ENTRY_INFO.to_vec();
        block.push(1);
        let okm = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, prk.as_ref()), &block);
        assert_eq!(&entry_key("0123456789abcdef", &key).unwrap()[..], okm.as_ref());
        assert_ne!(entry_key("0123456789abcdee", &key).unwrap(), entry_key("0123456789abcdef", &key).unwrap());
    }

    #[test]
    fn keys_are_32_bytes() {
        assert!(VaultKey::from_bytes(&[0u8; 31]).is_err());
        assert_eq!(VaultKey::from_bytes(&[9u8; 32]).unwrap().bytes(), &[9u8; 32]);
        assert_ne!(VaultKey::random().bytes(), VaultKey::random().bytes());
        assert_eq!(format!("{:?}", VaultKey::random()), "VaultKey(..)");
    }

    #[test]
    fn ids_are_hex_of_their_length() {
        let id = random_hex(16);
        assert!(is_entry_id(&id));
        assert!(is_entry_id("0123456789abcdef0123456789abcdef"));
        assert!(!is_entry_id("0123456789ABCDEF0123456789ABCDEF"));
        assert!(!is_entry_id("../../../../etc/passwd/xxxxxxxxxx"));
        assert!(!is_entry_id(&id[..31]));
        assert_eq!(random_hex(8).len(), 16);
    }

    #[test]
    fn padding_stops_at_the_largest_entry() {
        let keys = Keys::new("0123456789abcdef", &VaultKey::random()).unwrap();
        let mut entry = Entry { id: random_hex(16), notes: "x".repeat(LARGEST_ENTRY), ..Entry::default() };
        assert!(seal_entry(&keys, &entry).unwrap_err().contains("64 KiB"));
        entry.notes = "x".repeat(LARGEST_ENTRY - 1024);
        let sealed: Sealed = serde_json::from_slice(&seal_entry(&keys, &entry).unwrap()).unwrap();
        let padded = b64::decode(&sealed.c).unwrap().len() - 16;
        assert_eq!(padded % PAD_TO, 0);
        assert!(padded <= LARGEST_ENTRY && padded > LARGEST_ENTRY - 1024, "{padded}");
    }

    #[test]
    fn a_synced_write_is_private_whole_and_newer() {
        let dir = folder("write");
        let path = dir.join("deeper").join("file.json");
        write_synced(&path, b"one").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"one");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o077, 0);
        // A file from a machine whose clock is ahead: the next write is still newer.
        let ahead = SystemTime::now() + Duration::from_secs(3600);
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(ahead).unwrap();
        write_synced(&path, b"two").unwrap();
        let after = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(millis(after), millis(ahead) + 1);
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        let left: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left, vec![std::ffi::OsString::from("file.json")], "no scratch file is left");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
