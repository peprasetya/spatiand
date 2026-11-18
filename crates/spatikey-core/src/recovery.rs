//! The recovery code. docs/spatikey.md, "Keys".
//!
//! 25 characters of Crockford's base32 (125 random bits), shown once at setup for the owner to
//! keep in Apple Passwords. The wrapping is `recovery.json`, synced with the vault, so any device
//! with biometrics can be set up from the code alone.
//!
//! **Version 2** (2026-11-18): Argon2id of the code gives a P-256 key of the code's own, and the
//! vault key is wrapped *to its public half*, as it is to a holder's (`crate::holders`). So a new
//! vault key -- made when a holder is removed -- is wrapped to the same code by a holder that does
//! not know the code, and what it leaves in the synced folder (a salt, a public key) is of no use
//! to a device that once held the old vault key.
//!
//! **Version 1** had Argon2id's key wrap the vault key itself: nothing but the code can make such
//! a wrap for another vault key. Still read; never written any more.

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305, NONCE_LEN};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::b64;
use crate::vault::{fill_random, VaultKey};

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
    /// (Version 1) base64: the nonce and the wrapped vault key.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub n: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub c: String,
    pub made: u64,
    /// (Version 2) The code's own public key, X9.63, base64 ...
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub public: String,
    /// ... and the vault key wrapped to it, as to a holder's, base64.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub wrapped: String,
    /// (Version 2) Which of the vault's keys that is: empty for its first, else the name of the
    /// change of keys that made it (`crate::vault::Rekey`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
}

impl RecoveryRecord {
    /// Whether a new vault key can be wrapped to this code without the code ([`rewrap`]).
    pub fn rewraps(&self) -> bool {
        self.v == 2 && !self.public.is_empty()
    }
}

/// Argon2id's cost: 64 MiB, 3 passes, 1 lane.
pub const COST: (u32, u32, u32) = (65536, 3, 1);

/// Crockford's base32: no I, L, O or U, so that nothing is read as something else.
pub const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The most a `recovery.json` may ask of Argon2id. The file is synced, so a host could write one
/// that asks for all the memory there is; this is far above [`COST`] and far below that.
const MOST: (u32, u32, u32) = (1 << 20, 64, 16);

/// A new code: 25 Crockford base32 characters in five groups, "XXXXX-XXXXX-XXXXX-XXXXX-XXXXX".
pub fn new_code() -> String {
    let mut bytes = Zeroizing::new([0u8; 16]);
    fill_random(&mut bytes[..]);
    // The first 125 of the 128 bits, five at a time.
    let mut bits = u128::from_be_bytes(*bytes) >> 3;
    let mut code = String::with_capacity(29);
    for i in 0..25 {
        if i > 0 && i % 5 == 0 {
            code.push('-');
        }
        let digit = (bits >> (5 * (24 - i))) & 31;
        code.push(ALPHABET[digit as usize] as char);
    }
    bits.zeroize();
    code
}

/// The code as the key is made from it: 25 capitals, no dashes or spaces; O read as 0, I and L
/// as 1; U refused (not in Crockford's alphabet).
pub fn canonical(code: &str) -> Result<String, String> {
    let mut out = String::with_capacity(25);
    for c in code.chars() {
        if c == '-' || c.is_whitespace() {
            continue;
        }
        let c = match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        // Never the character itself in the message: it is part of the code.
        if !c.is_ascii() || !ALPHABET.contains(&(c as u8)) {
            out.zeroize();
            return Err("a recovery code has only digits and the letters A to Z without U".into());
        }
        out.push(c);
    }
    if out.len() != 25 {
        let found = out.len();
        out.zeroize();
        return Err(format!("a recovery code has 25 characters, not {found}"));
    }
    Ok(out)
}

pub fn wrap(key: &VaultKey, code: &str, vault_id: &str) -> Result<RecoveryRecord, String> {
    wrap_to_code(key, code, vault_id, "", COST)
}

/// A version 2 record: a new salt, the code's key made from the code and that salt, and `key`
/// -- the vault key named `generation` -- wrapped to its public half.
pub fn wrap_to_code(key: &VaultKey, code: &str, vault_id: &str, generation: &str, cost: (u32, u32, u32)) -> Result<RecoveryRecord, String> {
    let code = Zeroizing::new(canonical(code)?);
    let mut salt = [0u8; 16];
    fill_random(&mut salt);
    let secret = code_key(&code, &salt, cost)?;
    let public = p256::elliptic_curve::sec1::ToEncodedPoint::to_encoded_point(&secret.public_key(), false).as_bytes().to_vec();
    let wrapped = crate::holders::wrap_as(key, &public, aad2(vault_id).as_bytes())?;
    let (m, t, p) = cost;
    Ok(RecoveryRecord {
        v: 2,
        kdf: "argon2id".into(),
        m,
        t,
        p,
        salt: b64::encode(&salt),
        n: String::new(),
        c: String::new(),
        made: crate::now_ms(),
        public: b64::encode(&public),
        wrapped: b64::encode(&wrapped),
        key: generation.to_string(),
    })
}

/// The same code for another vault key, by whoever has that key and not the code: the record's
/// salt and public key kept, the new key wrapped to that public key. Version 2 only.
pub fn rewrap(record: &RecoveryRecord, key: &VaultKey, vault_id: &str, generation: &str) -> Result<RecoveryRecord, String> {
    if !record.rewraps() {
        return Err("only the recovery code itself can wrap another key to a recovery record of version 1".into());
    }
    let public = b64::decode(&record.public)?;
    let wrapped = crate::holders::wrap_as(key, &public, aad2(vault_id).as_bytes())?;
    Ok(RecoveryRecord { wrapped: b64::encode(&wrapped), key: generation.to_string(), made: crate::now_ms(), ..record.clone() })
}

/// A version 1 record, as vaults made before 2026-11-18 have: for the tests that read one.
pub fn wrap_with(key: &VaultKey, code: &str, vault_id: &str, cost: (u32, u32, u32)) -> Result<RecoveryRecord, String> {
    let code = Zeroizing::new(canonical(code)?);
    let mut salt = [0u8; 16];
    fill_random(&mut salt);
    let wrapping = sealer(&code, &salt, cost)?;
    let mut nonce = [0u8; NONCE_LEN];
    fill_random(&mut nonce);
    let mut sealed = Zeroizing::new(Vec::with_capacity(32 + CHACHA20_POLY1305.tag_len()));
    sealed.extend_from_slice(key.bytes());
    wrapping
        .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(aad(vault_id).as_bytes()), &mut *sealed)
        .map_err(|_| "could not wrap the vault key".to_string())?;
    let (m, t, p) = cost;
    Ok(RecoveryRecord {
        v: 1,
        kdf: "argon2id".into(),
        m,
        t,
        p,
        salt: b64::encode(&salt),
        n: b64::encode(&nonce),
        c: b64::encode(&sealed),
        made: crate::now_ms(),
        public: String::new(),
        wrapped: String::new(),
        key: String::new(),
    })
}

pub fn unwrap(record: &RecoveryRecord, code: &str, vault_id: &str) -> Result<VaultKey, String> {
    if !(record.v == 1 || record.v == 2) || record.kdf != "argon2id" {
        return Err(format!("a recovery record of version {} with {}, which this SpatiKey cannot read", record.v, record.kdf));
    }
    if record.m > MOST.0 || record.t > MOST.1 || record.p > MOST.2 {
        return Err("the recovery record asks for more work than a recovery code is given".into());
    }
    let code = Zeroizing::new(canonical(code)?);
    let salt = b64::decode(&record.salt)?;
    if record.v == 2 {
        let secret = code_key(&code, &salt, (record.m, record.t, record.p))?;
        let public = p256::elliptic_curve::sec1::ToEncodedPoint::to_encoded_point(&secret.public_key(), false).as_bytes().to_vec();
        // The code makes the key the record names, or it is another code (or another record's salt).
        if b64::decode(&record.public)? != public {
            return Err("that recovery code does not open this vault".into());
        }
        let wrapped = b64::decode(&record.wrapped)?;
        if wrapped.len() != crate::holders::WRAPPED_LEN {
            return Err("the recovery record's wrap is not a wrap".into());
        }
        let ephemeral = p256::PublicKey::from_sec1_bytes(&wrapped[..crate::holders::PUBLIC_LEN])
            .map_err(|_| "the recovery record's wrap is not a wrap".to_string())?;
        let shared = p256::ecdh::diffie_hellman(secret.to_nonzero_scalar(), ephemeral.as_affine());
        return crate::holders::open_with_shared_as(shared.raw_secret_bytes(), &public, &wrapped, aad2(vault_id).as_bytes())
            .map_err(|_| "that recovery code does not open this vault".to_string());
    }
    let nonce = b64::decode(&record.n)?;
    let mut sealed = Zeroizing::new(b64::decode(&record.c)?);
    let wrapping = sealer(&code, &salt, (record.m, record.t, record.p))?;
    let nonce = Nonce::try_assume_unique_for_key(&nonce).map_err(|_| "a nonce of the wrong length".to_string())?;
    let key = wrapping
        .open_in_place(nonce, Aad::from(aad(vault_id).as_bytes()), &mut sealed[..])
        .map_err(|_| "that recovery code does not open this vault".to_string())?;
    VaultKey::from_bytes(key)
}

/// The associated data: the code's key unwraps this vault's key and no other's.
fn aad(vault_id: &str) -> String {
    format!("spatikey recovery v1/{vault_id}")
}

/// Version 2's associated data: a wrap to the code's key is this vault's, and not a holder's.
fn aad2(vault_id: &str) -> String {
    format!("spatikey recovery v2/{vault_id}")
}

/// Argon2id of the canonical code: 32 bytes that only the code and the salt give.
fn stretched(code: &str, salt: &[u8], (m, t, p): (u32, u32, u32)) -> Result<Zeroizing<[u8; 32]>, String> {
    let params = argon2::Params::new(m, t, p, Some(32)).map_err(|e| format!("Argon2id cannot take that cost: {e}"))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon.hash_password_into(code.as_bytes(), salt, &mut key[..]).map_err(|e| format!("Argon2id failed: {e}"))?;
    Ok(key)
}

/// (Version 2) The code's own P-256 key: SHA-256 of `spatikey recovery key v2`, a count and
/// Argon2id's output, the first count (from 0) that gives a number a P-256 key can be -- all but
/// one in four thousand million do at once.
fn code_key(code: &str, salt: &[u8], cost: (u32, u32, u32)) -> Result<p256::SecretKey, String> {
    let seed = stretched(code, salt, cost)?;
    for count in 0u8..=255 {
        let mut text = Zeroizing::new(Vec::with_capacity(64));
        text.extend_from_slice(b"spatikey recovery key v2");
        text.push(count);
        text.extend_from_slice(&seed[..]);
        let digest = ring::digest::digest(&ring::digest::SHA256, &text);
        if let Ok(secret) = p256::SecretKey::from_slice(digest.as_ref()) {
            return Ok(secret);
        }
    }
    Err("the recovery code gave no key".into())
}

/// (Version 1) Argon2id of the canonical code, as a ChaCha20-Poly1305 key.
fn sealer(code: &str, salt: &[u8], cost: (u32, u32, u32)) -> Result<LessSafeKey, String> {
    let key = stretched(code, salt, cost)?;
    let unbound = UnboundKey::new(&CHACHA20_POLY1305, &key[..]).map_err(|_| "the code's key is not a key".to_string())?;
    Ok(LessSafeKey::new(unbound))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Argon2id's least: quick enough for a debug build.
    const TINY: (u32, u32, u32) = (8, 1, 1);

    #[test]
    fn a_code_is_five_groups_of_five_from_the_alphabet() {
        let code = new_code();
        assert_eq!(code.len(), 29);
        let groups: Vec<&str> = code.split('-').collect();
        assert_eq!(groups.len(), 5);
        assert!(groups.iter().all(|g| g.len() == 5 && g.bytes().all(|b| ALPHABET.contains(&b))));
        assert_ne!(new_code(), code);
        assert_eq!(canonical(&code).unwrap(), code.replace('-', ""));
    }

    #[test]
    fn reading_a_code_is_forgiving_as_crockford_intends() {
        let code = "0123456789ABCDEFGHJKMNPQR";
        assert_eq!(canonical("o12345-6789a bcdef\tghjkm npqr").unwrap(), code);
        assert_eq!(canonical("0I2345678 9ABCDEFGHJKMNPQR").unwrap().as_bytes()[1], b'1');
        assert_eq!(canonical("0L23456789ABCDEFGHJKMNPQR").unwrap(), code);
        assert!(canonical("U123456789ABCDEFGHJKMNPQR").is_err(), "U is not in the alphabet");
        assert!(canonical("0123456789ABCDEFGHJKMNPQ").is_err(), "24 characters");
        assert!(canonical("0123456789ABCDEFGHJKMNPQRS").is_err(), "26 characters");
        assert!(canonical("0123456789ABCDEFGHJKMNPQ\u{e9}").is_err());
        // The refusal says the same whatever was typed: no part of a code is ever repeated back.
        assert_eq!(canonical("#123456789ABCDEFGHJKMNPQR"), canonical("U123456789ABCDEFGHJKMNPQR"));
        assert_eq!(canonical("7XKQ2"), canonical("ABCDE"));
    }

    #[test]
    fn the_code_unwraps_its_own_vaults_key_only() {
        let key = VaultKey::random();
        let code = new_code();
        let record = wrap_with(&key, &code, "0123456789abcdef", TINY).unwrap();
        assert_eq!((record.m, record.t, record.p), TINY);
        assert_eq!(record.kdf, "argon2id");
        assert_eq!(b64::decode(&record.salt).unwrap().len(), 16);
        let back = unwrap(&record, &code.to_lowercase().replace('-', " "), "0123456789abcdef").unwrap();
        assert_eq!(back.bytes(), key.bytes());
        assert!(unwrap(&record, &new_code(), "0123456789abcdef").is_err(), "another code");
        assert!(unwrap(&record, &code, "fedcba9876543210").is_err(), "another vault");
        let mut greedy = record.clone();
        greedy.m = u32::MAX;
        assert!(unwrap(&greedy, &code, "0123456789abcdef").unwrap_err().contains("more work"));
        let mut altered = record;
        altered.salt = b64::encode(&[0u8; 16]);
        assert!(unwrap(&altered, &code, "0123456789abcdef").is_err(), "another salt");
    }

    #[test]
    fn a_new_vault_key_is_wrapped_to_the_same_code_without_the_code() {
        let (first, second) = (VaultKey::random(), VaultKey::random());
        let code = new_code();
        let record = wrap_to_code(&first, &code, "0123456789abcdef", "", TINY).unwrap();
        assert_eq!(record.v, 2);
        assert!(record.n.is_empty() && record.c.is_empty(), "nothing the code's own key seals");
        assert_eq!(unwrap(&record, &code, "0123456789abcdef").unwrap().bytes(), first.bytes());
        assert!(unwrap(&record, &new_code(), "0123456789abcdef").is_err(), "another code");
        assert!(unwrap(&record, &code, "fedcba9876543210").is_err(), "another vault");
        // Whoever has the new key and the record, and not the code.
        let again = rewrap(&record, &second, "0123456789abcdef", "00112233aabbccdd").unwrap();
        assert_eq!((again.salt.as_str(), again.public.as_str()), (record.salt.as_str(), record.public.as_str()));
        assert_eq!(again.key, "00112233aabbccdd");
        assert_ne!(again.wrapped, record.wrapped);
        assert_eq!(unwrap(&again, &code, "0123456789abcdef").unwrap().bytes(), second.bytes());
        // The old record still gives the old key, and only that.
        assert_eq!(unwrap(&record, &code, "0123456789abcdef").unwrap().bytes(), first.bytes());
        // A record whose public key is somebody else's is not opened by trying.
        let mut swapped = again.clone();
        swapped.public = wrap_to_code(&second, &new_code(), "0123456789abcdef", "", TINY).unwrap().public;
        assert!(unwrap(&swapped, &code, "0123456789abcdef").is_err());
        // A wrap to the code's key is not a holder's wrap: the associated data differ.
        let public = b64::decode(&record.public).unwrap();
        let as_holder = crate::holders::wrap(&second, &public).unwrap();
        let mut passed_off = again;
        passed_off.wrapped = b64::encode(&as_holder);
        assert!(unwrap(&passed_off, &code, "0123456789abcdef").is_err());
    }

    #[test]
    fn a_version_1_record_cannot_be_wrapped_again_without_its_code() {
        let key = VaultKey::random();
        let record = wrap_with(&key, &new_code(), "0123456789abcdef", TINY).unwrap();
        assert!(!record.rewraps());
        assert!(rewrap(&record, &VaultKey::random(), "0123456789abcdef", "").is_err());
        let json = serde_json::to_value(&record).unwrap();
        assert!(json.get("public").is_none() && json.get("key").is_none(), "written as it always was");
    }

    #[test]
    fn the_full_cost_is_what_the_design_says() {
        assert_eq!(COST, (64 * 1024, 3, 1));
        assert!(argon2::Params::new(COST.0, COST.1, COST.2, Some(32)).is_ok());
    }
}
