//! Known answers: a vault written by SpatiKey before the RustCrypto upgrade (argon2 0.5,
//! base64 0.22), kept here byte for byte, must still open, and what is written must still be
//! written the same way. Every Mac, Deck and phone shares one vault through the synced folder,
//! and they are not updated at the same moment: a machine on a new build must read what one on
//! an old build wrote, and the other way round.
//!
//! The fixture's vault key is 32 bytes of 7 and its recovery code `0123456789ABCDEFGHJKMNPQR`;
//! it holds nothing of anybody's.

use std::path::{Path, PathBuf};

use spatikey_core::b64;
use spatikey_core::recovery::{self, RecoveryRecord};
use spatikey_core::vault::{Vault, VaultKey};

const VAULT_JSON: &str = r#"{"v":1,"id":"5207d5b8721436c6","created":1794623429677,"by":"0badf00d","check":"ObodTi8yREWwc9NtdaKzMcxjErE47uqCXIoVe5fhAKXI8SCG"}"#;
const ENTRY_ID: &str = "00112233445566778899aabbccddeeff";
const ENTRY_JSON: &str = r#"{"v":1,"id":"00112233445566778899aabbccddeeff","n":"CKKHhZEqK7d9b/Aq","c":"D9J1tQZGB+DXyp2hE1GCdgoFfm8A9tJFeN6EwJM8ralq/0IHZH5HOugGQgNRrAEnHAdWgWzEm+FcGvad/hkmHaKxm+MyOAzoBcsocUz++hPibJBSwZVSAj89FXWy4lH58Sx0sBU7yB4xRHci4Q5tkbI4hIRM9C0TeZNUnflVYDvVnfCnu4eBdzoYKJmKe+cejMtGG0ZM5+8ssb9yVyXgm9huQzyw/K7lPdBDDjOVi59oITOpQx2K91DbWXsXbrUESjIydcm0rNpEMh1nIuX9Z6mBahR49Td5xi8gxstpv941D21Nvidv/P4Y83a3sUKgJKnRs6d9zVIaCeDNeY9WkFJwTIAAMSLXJnHaS1OLy5M7XAhRMCcGc9zacmGvCt+b/BcfCgTq4McPZU9oURUYWM8Q81+UfDUFaEKUxv/tUQTEFKbeB/jqUhTqedAXjuXSeoGyv9j0zn3RZjdtOfMp8Q7l0XvgGeKpP0l+a6AbJevAoS+0tcuQHBuJgdU44EAGqdFE3BJGZCYm943lDYkCa3sapiyLJaNix052hkKSAjzsdsJa4M/kr3InA1VOMpQqZPVTvFhw2SZFsrEO7goQPwcewUyoRIoQaLyWc1u5bl0YEFb0sTuJhIFY2HVdC6RG/8/4/CiMTnpi18BsF6e4lM3ZtXkrPM0k1NFKG12QDgLUuZK6tjLZlNpyR/s4p5X0"}"#;
/// `recovery.json` at Argon2id's least cost (8 KiB, 1 pass, 1 lane): the KDF, quickly.
const RECOVERY_TINY: &str = r#"{"v":1,"kdf":"argon2id","m":8,"t":1,"p":1,"salt":"7mvmAsc24hqI5Ef0sYEBAw==","n":"/nNMdTRmaZd3Qrgf","c":"/A7Xdp718ElCf2W/+0LlXLSa2xCfmWKcGqYpDupddLAagM8uRdQaonl7JV6eZ0aO","made":1794623429678}"#;
/// And at the cost SpatiKey writes (64 MiB, 3 passes, 1 lane): the KDF as the owner's vault has it.
const RECOVERY_FULL: &str = r#"{"v":1,"kdf":"argon2id","m":65536,"t":3,"p":1,"salt":"o1dXCUadETLMzyOJQv+Hng==","n":"AipB0w+pT+dI/lO2","c":"jGaxGgALE2iurBOFLWu1iqSDIbgZqWmV2GYnwEBxRdwRWYjiHqrz2z5+cFnSG43c","made":1794623429760}"#;
const CODE: &str = "0123456789ABCDEFGHJKMNPQR";

fn key() -> VaultKey {
    VaultKey::from_bytes(&[7u8; 32]).unwrap()
}

fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("spatikey-known-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("shared/entries")).unwrap();
    std::fs::create_dir_all(dir.join("local")).unwrap();
    std::fs::write(dir.join("shared/vault.json"), VAULT_JSON).unwrap();
    std::fs::write(dir.join(format!("shared/entries/{ENTRY_ID}.json")), ENTRY_JSON).unwrap();
    dir
}

fn open(dir: &Path) -> Vault {
    Vault::open(&dir.join("shared"), &dir.join("local")).unwrap()
}

#[test]
fn an_old_vault_opens_and_its_entry_reads() {
    let dir = fixture("entry");
    let vault = open(&dir);
    assert!(vault.check(&key()), "the header's check opens with the key");
    assert!(!vault.check(&VaultKey::from_bytes(&[8u8; 32]).unwrap()));
    let entry = vault.read(&key(), ENTRY_ID).unwrap();
    assert_eq!(entry.title, "Example");
    assert_eq!(entry.sites, vec!["example.com".to_string()]);
    assert_eq!(entry.username, "someone@example.com");
    assert_eq!(entry.password, "correct horse battery staple é");
    assert_eq!(entry.notes, "made before the RustCrypto upgrade");
    assert_eq!(entry.changed_by, "0badf00d");
    assert_eq!(entry.version, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_old_recovery_record_gives_the_vault_key_back() {
    let dir = fixture("recovery");
    let vault = open(&dir);
    std::fs::write(dir.join("shared/recovery.json"), RECOVERY_TINY).unwrap();
    assert_eq!(vault.recover(CODE).unwrap().bytes(), key().bytes());
    assert_eq!(vault.recover("0123-45678 9abcd efghj kmnpq r").unwrap().bytes(), key().bytes());
    assert!(vault.recover("1123456789ABCDEFGHJKMNPQR").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_old_recovery_record_at_full_cost_gives_the_vault_key_back() {
    let record: RecoveryRecord = serde_json::from_str(RECOVERY_FULL).unwrap();
    assert_eq!((record.m, record.t, record.p), recovery::COST);
    assert_eq!(recovery::unwrap(&record, CODE, "5207d5b8721436c6").unwrap().bytes(), key().bytes());
}

#[test]
fn what_is_written_now_the_old_code_reads_as_it_wrote() {
    // A record and an entry made now have the old shape: the same fields, standard base64 with
    // padding, the same lengths (salt 16, nonce 12, key + tag 48).
    let record = recovery::wrap_with(&key(), CODE, "5207d5b8721436c6", (8, 1, 1)).unwrap();
    let json: serde_json::Value = serde_json::to_value(&record).unwrap();
    let old: serde_json::Value = serde_json::from_str(RECOVERY_TINY).unwrap();
    let fields = |v: &serde_json::Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(fields(&json), fields(&old));
    assert_eq!(b64::decode(&record.salt).unwrap().len(), 16);
    assert_eq!(b64::decode(&record.n).unwrap().len(), 12);
    assert_eq!(b64::decode(&record.c).unwrap().len(), 48);
    assert!(record.salt.ends_with("=="));
    assert_eq!(recovery::unwrap(&record, CODE, "5207d5b8721436c6").unwrap().bytes(), key().bytes());

    let dir = fixture("write");
    let vault = open(&dir);
    let mut entry = vault.read(&key(), ENTRY_ID).unwrap();
    entry.notes = "written again".into();
    vault.write(&key(), &mut entry, "0badf00d").unwrap();
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join(format!("shared/entries/{ENTRY_ID}.json"))).unwrap()).unwrap();
    let old: serde_json::Value = serde_json::from_str(ENTRY_JSON).unwrap();
    assert_eq!(fields(&written), fields(&old));
    assert_eq!(b64::decode(written["n"].as_str().unwrap()).unwrap().len(), 12);
    assert_eq!(vault.read(&key(), ENTRY_ID).unwrap().notes, "written again");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The two base64s, exactly: standard with padding for files, URL-safe without for WebAuthn
/// and the pages; what each accepts and refuses on the way in.
#[test]
fn both_base64s_are_what_they_were() {
    let bytes: Vec<u8> = (0u8..=255).collect();
    let standard = b64::encode(&bytes);
    assert_eq!(
        standard,
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0+P0BBQkNERUZHSElKS0xNTk9QUVJTVFVWV1hZWltcXV5fYGFiY2RlZmdoaWprbG1ub3BxcnN0dXZ3eHl6e3x9fn+AgYKDhIWGh4iJiouMjY6PkJGSk5SVlpeYmZqbnJ2en6ChoqOkpaanqKmqq6ytrq+wsbKztLW2t7i5uru8vb6/wMHCw8TFxsfIycrLzM3Oz9DR0tPU1dbX2Nna29zd3t/g4eLj5OXm5+jp6uvs7e7v8PHy8/T19vf4+fr7/P3+/w=="
    );
    let url = b64::encode_url(&bytes);
    assert_eq!(url, standard.replace('+', "-").replace('/', "_").trim_end_matches('=').to_string());
    assert_eq!(b64::encode(b"a"), "YQ==");
    assert_eq!(b64::encode_url(b"a"), "YQ");
    assert_eq!(b64::decode(" YQ==\n").unwrap(), b"a");
    assert!(b64::decode("YQ").is_err(), "standard insists on its padding");
    assert!(b64::decode("YR==").is_err(), "and refuses bits left over");
    assert!(b64::decode("Y-8=").is_err(), "and the URL-safe alphabet");
    assert_eq!(b64::decode_url("YQ").unwrap(), b"a");
    assert_eq!(b64::decode_url("YQ==").unwrap(), b"a");
    assert_eq!(b64::decode_url("_-8").unwrap(), [0xff, 0xef]);
    assert!(b64::decode_url("/+8").is_err(), "and URL-safe the standard one");
    assert!(b64::decode_url("YR").is_err());
}
