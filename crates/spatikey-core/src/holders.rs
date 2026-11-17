//! The devices that open the vault: its *holders*. docs/spatikey.md, "Every device with
//! biometrics holds the vault".
//!
//! The vault key is wrapped once per holder, each time to a key of that device's own that only
//! its biometric sensor lets be used: a Mac's Secure Enclave key behind Touch ID, an Android
//! phone's Keystore key behind a strong (class 3) fingerprint or face. Every wrap is the same
//! ECIES, docs/spatikey.md "Keys": an ephemeral P-256 key, ECDH with the holder's public key,
//! HKDF-SHA256 (salt `spatikey`, info = ephemeral public ‖ holder public, both X9.63), AES-256-GCM
//! with the associated data `spatikey touch-id v1`, kept as ephemeral public (65) ‖ nonce (12) ‖
//! ciphertext (32) ‖ tag (16).
//!
//! Wrapping needs only the holder's public key, so it is done here, in Rust, by whichever holder
//! adds another: the vault key it has just had from its own finger is wrapped to the new
//! device's public key, and nothing else leaves it. Unwrapping needs the holder's private key,
//! which never leaves its Secure Enclave or Keystore: that is the platform's half
//! (`SpatiKeyGuard.swift`, `KeyGuard.java`), and it is what asks for the finger.

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::agreement::{self, EphemeralPrivateKey, UnparsedPublicKey, ECDH_P256};
use ring::hkdf;
use ring::rand::SystemRandom;
use serde::{Deserialize, Serialize};

use crate::vault::{fill_random, VaultKey};

/// A Mac, with Touch ID in front of a Secure Enclave key.
pub const MAC: &str = "mac-touch-id";
/// An Android device with a strong (class 3) biometric in front of a Keystore key.
pub const ANDROID: &str = "android-biometric";

/// HKDF's salt for every holder's wrap.
const SALT: &[u8] = b"spatikey";
/// The wrap's associated data: the name it was first given on the Mac, the same for every kind.
const AAD: &[u8] = b"spatikey touch-id v1";
/// An uncompressed P-256 point, X9.63.
pub const PUBLIC_LEN: usize = 65;
/// Ephemeral public (65) ‖ nonce (12) ‖ vault key (32) ‖ tag (16).
pub const WRAPPED_LEN: usize = PUBLIC_LEN + NONCE_LEN + 32 + 16;

/// `requests/<fp>.json` in the synced folder: a device with biometrics asking to hold the vault.
/// Nothing in it is secret; what keeps it honest is the six digits ([`code`]) the owner compares
/// on both devices before a holder adds it -- anyone who can write the synced folder (a host
/// among them) could leave a request there.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Asking {
    pub v: u32,
    /// The device's link fingerprint, 64 hex: what its holder record will be named by.
    pub device: String,
    pub name: String,
    /// [`MAC`] or [`ANDROID`].
    pub kind: String,
    /// Its new key's public half, X9.63, base64.
    pub public: String,
    pub made: u64,
    /// Turned down, or added: a request is buried rather than removed, so that the burial travels
    /// as the newest file does (the synchroniser may not carry a removal).
    #[serde(default)]
    pub closed: bool,
}

/// `local/spatikey/asking.json`: this device's half of a request, until a holder adds it -- the
/// key's blob, which only its own Secure Enclave or Keystore can use.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Pending {
    pub vault: String,
    pub public: String,
    pub blob: String,
    pub made: u64,
}

/// What a holder's sensor is called in a sentence: "Touch ID", "your fingerprint".
pub fn sensor(kind: &str) -> &'static str {
    match kind {
        ANDROID => "your fingerprint",
        _ => "Touch ID",
    }
}

/// Is `kind` one this SpatiKey knows how to hold the vault with?
pub fn known(kind: &str) -> bool {
    matches!(kind, MAC | ANDROID)
}

/// The vault key wrapped to a holder's public key (X9.63, 65 bytes).
pub fn wrap(key: &VaultKey, public: &[u8]) -> Result<Vec<u8>, String> {
    if public.len() != PUBLIC_LEN || public[0] != 4 {
        return Err("a holder's key is an uncompressed P-256 point".into());
    }
    let rng = SystemRandom::new();
    let ephemeral = EphemeralPrivateKey::generate(&ECDH_P256, &rng).map_err(|_| "could not make an ephemeral key")?;
    let ephemeral_public = ephemeral.compute_public_key().map_err(|_| "could not make an ephemeral key")?;
    let ephemeral_public = ephemeral_public.as_ref().to_vec();
    let sealing = agreement::agree_ephemeral(ephemeral, &UnparsedPublicKey::new(&ECDH_P256, public), |shared| {
        sealing_key(shared, &ephemeral_public, public)
    })
    .map_err(|_| "that is not a point on P-256".to_string())??;
    let mut nonce = [0u8; NONCE_LEN];
    fill_random(&mut nonce);
    let mut sealed = zeroize::Zeroizing::new(Vec::with_capacity(32 + 16));
    sealed.extend_from_slice(key.bytes());
    sealing
        .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(AAD), &mut *sealed)
        .map_err(|_| "could not seal the vault key".to_string())?;
    let mut out = ephemeral_public;
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    debug_assert_eq!(out.len(), WRAPPED_LEN);
    Ok(out)
}

/// The AES-256-GCM key of a wrap, from the ECDH's shared secret (the x coordinate).
fn sealing_key(shared: &[u8], ephemeral: &[u8], recipient: &[u8]) -> Result<LessSafeKey, String> {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, SALT).extract(shared);
    let info = [ephemeral, recipient];
    let okm = prk.expand(&info, &AES_256_GCM).map_err(|_| "could not derive the wrap's key".to_string())?;
    Ok(LessSafeKey::new(UnboundKey::from(okm)))
}

/// The vault key from a wrap, given the ECDH's shared secret with the wrap's ephemeral key: what
/// a platform's half does after its sensor lets the private key be used. Here for the tests'
/// software holders and for checking the format against the Mac's and Android's.
pub fn open_with_shared(shared: &[u8], recipient: &[u8], wrapped: &[u8]) -> Result<VaultKey, String> {
    if wrapped.len() != WRAPPED_LEN {
        return Err(format!("a wrap is {WRAPPED_LEN} bytes, not {}", wrapped.len()));
    }
    let (ephemeral, rest) = wrapped.split_at(PUBLIC_LEN);
    let (nonce, sealed) = rest.split_at(NONCE_LEN);
    let opening = sealing_key(shared, ephemeral, recipient)?;
    let mut sealed = zeroize::Zeroizing::new(sealed.to_vec());
    let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| "a nonce of the wrong length".to_string())?;
    let plain = opening
        .open_in_place(nonce, Aad::from(AAD), &mut sealed[..])
        .map_err(|_| "the wrap does not open with that key".to_string())?;
    VaultKey::from_bytes(plain)
}

/// The six digits both devices show while one is added as a holder ("123 456"): from the vault,
/// the device and its new public key, so that a request swapped in the synced folder -- another
/// key under the same name -- shows other digits.
pub fn code(vault: &str, device: &str, public: &str) -> String {
    let mut text = Vec::new();
    for part in ["spatikey holder v1", vault, &device.to_ascii_lowercase(), public] {
        text.extend_from_slice(&(part.len() as u32).to_be_bytes());
        text.extend_from_slice(part.as_bytes());
    }
    let digest = ring::digest::digest(&ring::digest::SHA256, &text);
    let n = u32::from_be_bytes([digest.as_ref()[0], digest.as_ref()[1], digest.as_ref()[2], digest.as_ref()[3]]) % 1_000_000;
    format!("{:03} {:03}", n / 1000, n % 1000)
}

/// Do two codes say the same six digits, however they are spaced?
pub fn same_code(a: &str, b: &str) -> bool {
    let digits = |s: &str| s.chars().filter(char::is_ascii_digit).collect::<String>();
    let (a, b) = (digits(a), digits(b));
    a.len() == 6 && a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A holder in software: a P-256 key, and the ECDH a Secure Enclave or Keystore would do.
    fn holder() -> (p256::SecretKey, Vec<u8>) {
        let secret = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let public = p256::elliptic_curve::sec1::ToEncodedPoint::to_encoded_point(&secret.public_key(), false).as_bytes().to_vec();
        (secret, public)
    }

    fn open(secret: &p256::SecretKey, public: &[u8], wrapped: &[u8]) -> Result<VaultKey, String> {
        let ephemeral = p256::PublicKey::from_sec1_bytes(&wrapped[..PUBLIC_LEN]).map_err(|e| e.to_string())?;
        let shared = p256::ecdh::diffie_hellman(secret.to_nonzero_scalar(), ephemeral.as_affine());
        open_with_shared(shared.raw_secret_bytes(), public, wrapped)
    }

    #[test]
    fn a_wrap_opens_with_the_holders_key_and_no_other() {
        let key = VaultKey::random();
        let (secret, public) = holder();
        let wrapped = wrap(&key, &public).unwrap();
        assert_eq!(wrapped.len(), 125, "the length the Mac's files have");
        assert_eq!(open(&secret, &public, &wrapped).unwrap().bytes(), key.bytes());
        assert_ne!(wrapped, wrap(&key, &public).unwrap(), "a new ephemeral key each time");
        let (other, other_public) = holder();
        assert!(open(&other, &other_public, &wrapped).is_err());
        assert!(open(&other, &public, &wrapped).is_err());
        let mut altered = wrapped.clone();
        altered[100] ^= 1;
        assert!(open(&secret, &public, &altered).is_err());
    }

    #[test]
    fn only_a_point_on_the_curve_is_wrapped_to() {
        let key = VaultKey::random();
        assert!(wrap(&key, &[4u8; 65]).is_err());
        assert!(wrap(&key, &[4u8; 33]).is_err());
        let (_, public) = holder();
        let mut compressed = public.clone();
        compressed[0] = 2;
        assert!(wrap(&key, &compressed).is_err());
    }

    #[test]
    fn the_code_changes_with_the_key_and_reads_however_spaced() {
        let one = code("0123456789abcdef", &"ab".repeat(32), "BAAA");
        assert_eq!(one.len(), 7);
        assert_eq!(one, code("0123456789abcdef", &"AB".repeat(32), "BAAA"), "the fingerprint in any case");
        assert_ne!(one, code("0123456789abcdef", &"ab".repeat(32), "BAAB"));
        assert_ne!(one, code("0123456789abcdee", &"ab".repeat(32), "BAAA"));
        assert!(same_code(&one, &one.replace(' ', "")));
        assert!(!same_code("123 456", "123 457"));
        assert!(!same_code("", ""));
    }

    #[test]
    fn sensors_are_named_by_kind() {
        assert_eq!(sensor(MAC), "Touch ID");
        assert_eq!(sensor(ANDROID), "your fingerprint");
        assert!(known(MAC) && known(ANDROID) && !known("windows-hello"));
    }
}
