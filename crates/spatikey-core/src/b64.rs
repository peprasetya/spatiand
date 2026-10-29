//! The two base64s: standard (with padding) for what is kept in files, URL-safe without padding
//! for what WebAuthn and the pages speak.

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;

pub fn encode(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

pub fn decode(text: &str) -> Result<Vec<u8>, String> {
    STANDARD.decode(text.trim()).map_err(|e| format!("not base64: {e}"))
}

pub fn encode_url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// URL-safe base64, with or without padding (pages send both).
pub fn decode_url(text: &str) -> Result<Vec<u8>, String> {
    URL_SAFE_NO_PAD
        .decode(text.trim().trim_end_matches('='))
        .map_err(|e| format!("not base64url: {e}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn both_round_trip() {
        let bytes = [0u8, 255, 62, 63, 1, 2, 3];
        assert_eq!(super::decode(&super::encode(&bytes)).unwrap(), bytes);
        assert_eq!(super::decode_url(&super::encode_url(&bytes)).unwrap(), bytes);
        assert_eq!(super::decode_url("AP8-PwECAw==").unwrap(), bytes);
    }
}
