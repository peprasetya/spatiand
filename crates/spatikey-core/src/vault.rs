//! The vault's files and their encryption. docs/spatikey.md, "The vault".
//!
//! STUB: the signatures are the contract; the bodies are to be written.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::entry::{Entry, Summary};

/// The vault key: 32 random bytes, never written anywhere in the clear.
#[derive(Clone)]
pub struct VaultKey(pub(crate) Zeroizing<[u8; 32]>);

impl VaultKey {
    pub fn random() -> VaultKey {
        todo!()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<VaultKey, String> {
        let _ = bytes;
        todo!()
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
        let _ = (shared, local, by);
        todo!()
    }

    pub fn open(shared: &Path, local: &Path) -> Result<Vault, String> {
        let _ = (shared, local);
        todo!()
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn id(&self) -> &str {
        &self.header.id
    }

    /// Does `key` open this vault?
    pub fn check(&self, key: &VaultKey) -> bool {
        let _ = key;
        todo!()
    }

    /// Every live entry, decrypted. One that does not open (damaged, altered, another vault's)
    /// is skipped and logged by its id only.
    pub fn entries(&self, key: &VaultKey) -> Result<Vec<Entry>, String> {
        let _ = key;
        todo!()
    }

    pub fn summaries(&self, key: &VaultKey) -> Result<Vec<Summary>, String> {
        let _ = key;
        todo!()
    }

    /// One entry, tombstone or not.
    pub fn read(&self, key: &VaultKey, id: &str) -> Result<Entry, String> {
        let _ = (key, id);
        todo!()
    }

    /// Write `entry`: a new random id when it has none, `version` one more than what is on disk,
    /// `changed` now, `changed_by` = `by`, `created` kept (or now). Padded, atomic, with a file
    /// time that only moves forward.
    pub fn write(&self, key: &VaultKey, entry: &mut Entry, by: &str) -> Result<(), String> {
        let _ = (key, entry, by);
        todo!()
    }

    /// Rewrite an entry as a tombstone.
    pub fn delete(&self, key: &VaultKey, id: &str, by: &str) -> Result<(), String> {
        let _ = (key, id, by);
        todo!()
    }

    pub fn touch_devices(&self) -> Vec<TouchDevice> {
        todo!()
    }

    pub fn add_touch_device(&self, device: &TouchDevice) -> Result<(), String> {
        let _ = device;
        todo!()
    }

    pub fn remove_touch_device(&self, fp: &str) -> Result<(), String> {
        let _ = fp;
        todo!()
    }

    /// This Mac's own half, if it has one for this vault.
    pub fn touch_id(&self) -> Option<TouchId> {
        todo!()
    }

    pub fn set_touch_id(&self, touch: &TouchId) -> Result<(), String> {
        let _ = touch;
        todo!()
    }

    pub fn has_recovery(&self) -> bool {
        self.shared.join("recovery.json").is_file()
    }

    /// A new recovery code, replacing the old: returns the code, to show once.
    pub fn set_recovery(&self, key: &VaultKey) -> Result<String, String> {
        let _ = key;
        todo!()
    }

    /// The vault key, from the recovery code.
    pub fn recover(&self, code: &str) -> Result<VaultKey, String> {
        let _ = code;
        todo!()
    }

    #[allow(dead_code)]
    fn local(&self) -> &Path {
        &self.local
    }
}

/// Write `bytes` to `path` as synced files must be written: a hidden `.<name>.<pid>.new` beside
/// it, fsync, rename; mode 0600; the file's time only moving forward (the old time plus a
/// millisecond if the clock is behind it).
pub fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let _ = (path, bytes);
    todo!()
}
