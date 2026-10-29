//! What the vault holds: logins, passkeys and notes, one [`Entry`] each.

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    Login,
    Passkey,
    Note,
}

/// A site's one-time-code secret (RFC 6238).
#[derive(Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Totp {
    /// Base32, capitals, no padding.
    pub secret: String,
    #[serde(default = "six")]
    pub digits: u32,
    #[serde(default = "thirty")]
    pub period: u64,
    /// `SHA1`, `SHA256` or `SHA512`.
    #[serde(default = "sha1")]
    pub algorithm: String,
    #[serde(default)]
    pub issuer: String,
    #[serde(default)]
    pub label: String,
}

/// A WebAuthn credential SpatiKey made and keeps (a "synced" passkey: it is in the vault, not in
/// one machine's hardware).
#[derive(Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Passkey {
    pub rp_id: String,
    #[serde(default)]
    pub rp_name: String,
    /// base64url, no padding.
    pub credential_id: String,
    /// base64url, no padding: the site's own id for the account.
    pub user_handle: String,
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub user_display: String,
    /// The private key, PKCS#8 DER, standard base64.
    pub key: String,
    /// COSE algorithm; -7 is ES256, the only one made.
    #[serde(default = "es256")]
    pub alg: i64,
    /// Always 0: a passkey kept on several machines cannot count uses (WebAuthn allows 0).
    #[serde(default)]
    pub sign_count: u32,
}

#[derive(Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Entry {
    /// 32 hex characters, random; also the file's name.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub kind: Kind,
    #[serde(default)]
    pub title: String,
    /// What a page must belong to: registrable domains ("github.com"), or whole origins
    /// ("https://192.168.1.2:8443") for a site that must match exactly.
    #[serde(default)]
    pub sites: Vec<String>,
    /// Window application ids this login is for (Steam's is "steam"), for typing into windows.
    #[serde(default)]
    pub apps: Vec<String>,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub totp: Option<Totp>,
    #[serde(default)]
    pub passkey: Option<Passkey>,
    /// Milliseconds since 1970.
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub changed: u64,
    /// The short fingerprint (8 hex) of the machine that last changed it.
    #[serde(default)]
    pub changed_by: String,
    /// Counts up with every change, so an older copy coming back is known for one.
    #[serde(default)]
    pub version: u64,
    /// A tombstone: kept, emptied, so that the deletion travels as the newest version.
    #[serde(default)]
    pub deleted: bool,
}

/// An entry without its secrets, for lists.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Summary {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub sites: Vec<String>,
    pub apps: Vec<String>,
    pub username: String,
    pub has_password: bool,
    pub has_totp: bool,
    /// For a passkey: its site and account.
    pub passkey_rp: Option<String>,
    pub passkey_user: Option<String>,
    pub changed: u64,
}

impl Entry {
    pub fn summary(&self) -> Summary {
        Summary {
            id: self.id.clone(),
            kind: self.kind,
            title: self.title.clone(),
            sites: self.sites.clone(),
            apps: self.apps.clone(),
            username: self.username.clone(),
            has_password: !self.password.is_empty(),
            has_totp: self.totp.is_some(),
            passkey_rp: self.passkey.as_ref().map(|p| p.rp_id.clone()),
            passkey_user: self.passkey.as_ref().map(|p| p.user_name.clone()),
            changed: self.changed,
        }
    }

    /// Empty it into a tombstone, keeping only what says which entry it was.
    pub fn bury(&mut self) {
        self.wipe();
        self.title.clear();
        self.sites.clear();
        self.apps.clear();
        self.username.clear();
        self.password.clear();
        self.notes.clear();
        self.totp = None;
        self.passkey = None;
        self.deleted = true;
    }

    /// Overwrite every secret in memory. Not done on drop -- that would forbid `..Entry::default()`
    /// everywhere, and the JSON an entry passes through has copies anyway -- but where an entry's
    /// secrets have been used and the entry is about to go (the service, after answering).
    pub fn wipe(&mut self) {
        self.password.zeroize();
        self.notes.zeroize();
        if let Some(t) = self.totp.as_mut() {
            t.secret.zeroize();
        }
        if let Some(p) = self.passkey.as_mut() {
            p.key.zeroize();
        }
    }
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the secrets, whoever prints it.
        f.debug_struct("Entry")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("title", &self.title)
            .field("sites", &self.sites)
            .field("username", &self.username)
            .field("version", &self.version)
            .field("deleted", &self.deleted)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Totp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Totp").field("issuer", &self.issuer).field("label", &self.label).finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Passkey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Passkey").field("rp_id", &self.rp_id).field("user_name", &self.user_name).finish_non_exhaustive()
    }
}

fn six() -> u32 {
    6
}
fn thirty() -> u64 {
    30
}
fn sha1() -> String {
    "SHA1".into()
}
fn es256() -> i64 {
    -7
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_buried_entry_keeps_only_who_it_was() {
        let mut e = Entry {
            id: "ab".into(),
            title: "GitHub".into(),
            username: "paulus".into(),
            password: "hunter2".into(),
            version: 3,
            ..Entry::default()
        };
        e.bury();
        assert!(e.deleted);
        assert_eq!(e.id, "ab");
        assert_eq!(e.version, 3);
        assert!(e.password.is_empty() && e.title.is_empty() && e.username.is_empty());
    }

    #[test]
    fn debug_never_shows_a_password() {
        let e = Entry { password: "hunter2".into(), ..Entry::default() };
        assert!(!format!("{e:?}").contains("hunter2"));
    }
}
