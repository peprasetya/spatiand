//! What survives between runs: the servers, who is signed in to each, and the lock.
//!
//! The file holds live session cookies, so it is written with owner-only permissions and
//! replaced atomically. A `JSESSIONID` is a bearer credential for four hours; leaving it
//! world-readable in a home directory would undo the whole point of pairing.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One deployment, and the session belonging to it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    /// Normalised, always ending in '/'.
    pub base_url: String,
    /// Whoever the server said we became when the pairing was approved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Usually a single `JSESSIONID`.
    #[serde(default)]
    pub cookies: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    /// Bitstream Dolby and DTS to a receiver rather than decoding here.
    ///
    /// Off by default and desktop-only: it is the only way to hear real Atmos or DTS:X
    /// objects on Linux, and it is meaningless in a room, whose spatialiser needs PCM
    /// channels to place. Someone with the Deck in a dock turns it on; nobody else should
    /// have to think about it.
    #[serde(default)]
    pub audio_passthrough: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            audio_passthrough: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default)]
    pub accounts: Vec<Account>,
    /// Index into [`Self::accounts`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<usize>,
    /// The Argon2 hash from [`crate::lock`]. Outlives sign-outs on purpose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
    /// Whether the application was locked when it last stopped. A lock that a kill and
    /// relaunch defeats is a curtain.
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub settings: Settings,
}

impl Config {
    pub fn active_account(&self) -> Option<&Account> {
        self.accounts.get(self.active?)
    }

    pub fn active_account_mut(&mut self) -> Option<&mut Account> {
        let index = self.active?;
        self.accounts.get_mut(index)
    }

    /// Adds an account, or updates the one already pointing at that server, and makes it
    /// the active one. Returns its index.
    ///
    /// Matching on the URL rather than appending is what stops re-pairing with the same
    /// deployment leaving a trail of dead accounts behind it.
    pub fn remember(&mut self, account: Account) -> usize {
        let index = match self
            .accounts
            .iter()
            .position(|existing| existing.base_url == account.base_url)
        {
            Some(index) => {
                self.accounts[index] = account;
                index
            }
            None => {
                self.accounts.push(account);
                self.accounts.len() - 1
            }
        };
        self.active = Some(index);
        index
    }

    /// Drops the session but keeps the server, so signing in again does not mean typing
    /// the address again.
    pub fn sign_out(&mut self) {
        if let Some(account) = self.active_account_mut() {
            account.cookies.clear();
            account.name = None;
        }
    }

    pub fn load() -> Self {
        Self::load_from(&config_path())
    }

    /// A missing or unreadable file is an empty configuration, not an error. The first run
    /// has no file, and a corrupted one should send someone to the pairing screen rather
    /// than refuse to start.
    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&config_path())
    }

    /// Written to a neighbouring temporary file and renamed over the target, so a crash
    /// half way through leaves the previous configuration rather than half of this one.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        let temporary = path.with_extension("json.new");

        let mut file = std::fs::File::create(&temporary)?;
        restrict(&file)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);

        std::fs::rename(&temporary, path)
    }
}

/// Owner-only. The file holds session cookies, and a `JSESSIONID` is worth as much as a
/// password for as long as the server keeps it.
#[cfg(unix)]
fn restrict(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict(_: &std::fs::File) -> std::io::Result<()> {
    Ok(())
}

/// `$XDG_CONFIG_HOME/vibrefy/config.json`, falling back to `~/.config`.
pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn config_dir() -> PathBuf {
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(base).join("vibrefy");
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config").join("vibrefy")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vibrefy-store-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir.join("config.json")
    }

    fn account(url: &str) -> Account {
        Account {
            base_url: url.into(),
            name: Some("someone@example.com".into()),
            cookies: BTreeMap::from([("JSESSIONID".into(), "abc123".into())]),
        }
    }

    #[test]
    fn a_first_run_has_no_file_and_that_is_not_an_error() {
        let config = Config::load_from(Path::new("/nonexistent/vibrefy/config.json"));
        assert_eq!(Config::default(), config);
        assert!(config.active_account().is_none());
    }

    /// A configuration written by a newer version, or half-written by a crash, should send
    /// someone to the pairing screen rather than refuse to start.
    #[test]
    fn an_unreadable_file_reads_as_an_empty_one() {
        let path = scratch("corrupt");
        std::fs::write(&path, "{ this is not json").unwrap();
        assert_eq!(Config::default(), Config::load_from(&path));
    }

    #[test]
    fn what_is_written_is_what_comes_back() {
        let path = scratch("roundtrip");
        let mut config = Config::default();
        config.remember(account("https://media.example.com/"));
        config.pin = Some("$argon2id$v=19$m=19456,t=2,p=1$abc$def".into());
        config.locked = true;
        config.settings.audio_passthrough = true;
        config.save_to(&path).expect("saved");

        assert_eq!(config, Config::load_from(&path));
    }

    /// The file holds a live session cookie for as long as the server honours it.
    #[cfg(unix)]
    #[test]
    fn the_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let path = scratch("permissions");
        Config::default().save_to(&path).expect("saved");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(0o600, mode & 0o777, "got {:o}", mode & 0o777);
    }

    /// Re-pairing with the same deployment must not leave a trail of dead accounts.
    #[test]
    fn signing_in_again_replaces_the_account_rather_than_adding_one() {
        let mut config = Config::default();
        config.remember(account("https://media.example.com/"));
        config.remember(account("https://other.example.com/"));
        assert_eq!(2, config.accounts.len());

        let mut renewed = account("https://media.example.com/");
        renewed.cookies.insert("JSESSIONID".into(), "rotated".into());
        let index = config.remember(renewed);

        assert_eq!(2, config.accounts.len());
        assert_eq!(Some(index), config.active);
        assert_eq!(
            Some(&"rotated".to_string()),
            config.active_account().unwrap().cookies.get("JSESSIONID")
        );
    }

    /// Signing out drops the session and keeps the address, so getting back in does not
    /// mean typing a URL on a games console.
    #[test]
    fn signing_out_forgets_the_session_and_keeps_the_server() {
        let mut config = Config::default();
        config.remember(account("https://media.example.com/"));
        config.sign_out();

        let account = config.active_account().expect("still there");
        assert_eq!("https://media.example.com/", account.base_url);
        assert!(account.cookies.is_empty());
        assert_eq!(None, account.name);
    }

    /// The PIN is not part of any account: it outlives signing out, which is what makes it
    /// something set once rather than per session.
    #[test]
    fn the_pin_and_the_lock_survive_signing_out() {
        let mut config = Config::default();
        config.remember(account("https://media.example.com/"));
        config.pin = Some("hash".into());
        config.locked = true;
        config.sign_out();
        assert_eq!(Some("hash".to_string()), config.pin);
        assert!(config.locked);
    }

    #[test]
    fn the_config_lives_where_xdg_says() {
        let dir = std::env::temp_dir().join("vibrefy-xdg-test");
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        assert_eq!(dir.join("vibrefy").join("config.json"), config_path());
        std::env::remove_var("XDG_CONFIG_HOME");
    }
}
