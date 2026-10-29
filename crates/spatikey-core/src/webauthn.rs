//! The authenticator: making passkeys and signing with them (WebAuthn Level 3, ES256, "none"
//! attestation, synced-passkey flags).
//!
//! STUB: the signatures are the contract; the bodies are to be written.

use crate::entry::Passkey;
use crate::wire::{AssertedAnswer, CreatedAnswer, PasskeyCreate, Refusal};

/// SpatiKey's AAGUID: its own, the same on every machine (a site may show it as the
/// authenticator's name). 5b9e3f0a-c1d2-4e5f-8a6b-7c8d9e0f1a2b.
pub const AAGUID: [u8; 16] = [
    0x5b, 0x9e, 0x3f, 0x0a, 0xc1, 0xd2, 0x4e, 0x5f, 0x8a, 0x6b, 0x7c, 0x8d, 0x9e, 0x0f, 0x1a, 0x2b,
];

/// A new passkey and what the page is given.
pub struct Created {
    pub passkey: Passkey,
    pub answer: CreatedAnswer,
}

/// Make a passkey for `request`: an ES256 key (refused with `bad_request` if the site's `algs`
/// are given and do not include -7), a random 16-byte credential id, authenticator data with
/// UP, UV, BE, BS and AT set and a counter of 0, and a "none" attestation object.
pub fn create(request: &PasskeyCreate) -> Result<Created, Refusal> {
    let _ = request;
    todo!()
}

/// Sign in with `passkey`: authenticator data (UP, UV, BE, BS, counter 0) and an ES256
/// signature (ASN.1 DER) over it and the client data hash.
pub fn assert(passkey: &Passkey, client_data_hash: &[u8]) -> Result<AssertedAnswer, Refusal> {
    let _ = (passkey, client_data_hash);
    todo!()
}

/// clientDataJSON exactly as browsers write it: `type`, `challenge`, `origin`, `crossOrigin`, and
/// `topOrigin` when the frame is not the top one, in that order, with JSON string escaping.
pub fn client_data_json(kind: &str, challenge: &str, origin: &str, cross_origin: bool, top_origin: Option<&str>) -> String {
    let _ = (kind, challenge, origin, cross_origin, top_origin);
    todo!()
}

/// SHA-256 of an rp id.
pub fn rp_id_hash(rp_id: &str) -> [u8; 32] {
    let digest = ring::digest::digest(&ring::digest::SHA256, rp_id.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(digest.as_ref());
    out
}
