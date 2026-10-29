//! The recovery code. docs/spatikey.md, "Keys".
//!
//! STUB: the signatures are the contract; the bodies are to be written.

use serde::{Deserialize, Serialize};

use crate::vault::VaultKey;

/// `recovery.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryRecord {
    pub v: u32,
    /// "argon2id".
    pub kdf: String,
    /// Memory in KiB, passes, lanes.
    pub m: u32,
    pub t: u32,
    pub p: u32,
    /// base64.
    pub salt: String,
    /// base64: the nonce and the wrapped vault key.
    pub n: String,
    pub c: String,
    pub made: u64,
}

/// Argon2id's cost: 64 MiB, 3 passes, 1 lane.
pub const COST: (u32, u32, u32) = (65536, 3, 1);

/// A new code: 25 Crockford base32 characters in five groups, "XXXXX-XXXXX-XXXXX-XXXXX-XXXXX".
pub fn new_code() -> String {
    todo!()
}

/// The code as the key is made from it: 25 capitals, no dashes or spaces; O read as 0, I and L
/// as 1; U refused (not in Crockford's alphabet).
pub fn canonical(code: &str) -> Result<String, String> {
    let _ = code;
    todo!()
}

pub fn wrap(key: &VaultKey, code: &str, vault_id: &str) -> Result<RecoveryRecord, String> {
    wrap_with(key, code, vault_id, COST)
}

/// [`wrap`] at another cost (tests).
pub fn wrap_with(key: &VaultKey, code: &str, vault_id: &str, cost: (u32, u32, u32)) -> Result<RecoveryRecord, String> {
    let _ = (key, code, vault_id, cost);
    todo!()
}

pub fn unwrap(record: &RecoveryRecord, code: &str, vault_id: &str) -> Result<VaultKey, String> {
    let _ = (record, code, vault_id);
    todo!()
}
