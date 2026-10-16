//! Extensions: SpatiMesh's own WebExtensions runtime, for Chrome's and Firefox's extensions on
//! every engine (docs/spatimesh.md, "Extensions").
//!
//! What an extension is ([`manifest`], [`locale`]), how it is installed ([`install`]), where its
//! scripts go ([`pattern`]) and what it keeps ([`storage`]) need no engine; the APIs are
//! answered in the window process, and each engine only carries scripts and messages.

pub mod install;
pub mod locale;
pub mod manifest;
pub mod pattern;
pub mod runtime;
pub mod set;
pub mod storage;
pub mod zip;

/// Sixteen bytes as an extension's id: hex digits written with the letters a to p, as Chrome
/// writes them.
pub fn id_of_bytes(bytes: &[u8]) -> String {
    bytes.iter().flat_map(|b| [b >> 4, b & 15]).map(|n| (b'a' + n) as char).collect()
}

/// Chrome's id for a public key (its DER bytes): the first half of its SHA-256.
pub fn id_of_key(der: &[u8]) -> String {
    use sha2::Digest as _;
    id_of_bytes(&sha2::Sha256::digest(der)[..16])
}

/// Chrome's id for an extension loaded from a folder with no key: from where it is.
pub fn id_of_path(path: &std::path::Path) -> String {
    use sha2::Digest as _;
    id_of_bytes(&sha2::Sha256::digest(path.to_string_lossy().as_bytes())[..16])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_as_chrome_makes_them() {
        assert_eq!(id_of_bytes(&[0x01, 0xff]), "abpp");
        // From a key: 32 letters a to p, the same each time, another for another key.
        let id = id_of_key(b"a public key's DER bytes");
        assert_eq!(id.len(), 32);
        assert!(id.chars().all(|c| ('a'..='p').contains(&c)));
        assert_eq!(id, id_of_key(b"a public key's DER bytes"));
        assert_ne!(id, id_of_key(b"another key"));
        // The first byte of SHA-256("abc") is 0xba: "lk".
        assert!(id_of_key(b"abc").starts_with("lk"));
    }
}
