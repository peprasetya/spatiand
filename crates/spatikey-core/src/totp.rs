//! One-time codes (RFC 6238 over RFC 4226), and `otpauth://` links (Google Authenticator's
//! key-uri format).
//!
//! STUB: the signatures are the contract; the bodies are to be written.

use crate::entry::Totp;

/// Read an `otpauth://totp/Issuer:label?secret=...&issuer=...&digits=...&period=...&algorithm=...`
/// link. `hotp` is refused.
pub fn parse_uri(uri: &str) -> Result<Totp, String> {
    let _ = uri;
    todo!()
}

/// A bare secret as people paste it: base32, any case, spaces and dashes ignored, padding
/// optional. Six digits, thirty seconds, SHA1.
pub fn from_secret(secret: &str) -> Result<Totp, String> {
    let _ = secret;
    todo!()
}

/// The code at `unix_secs`, and the seconds it has left.
pub fn code(totp: &Totp, unix_secs: u64) -> Result<(String, u64), String> {
    let _ = (totp, unix_secs);
    todo!()
}
