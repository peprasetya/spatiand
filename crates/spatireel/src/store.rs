//! What SpatiReel keeps between one session and the next: the servers it has been signed in
//! to, and where this machine's own films were left.
//!
//! One small file. A server is kept as a token or a session's cookies, never as a password.
//! Nothing of a server's library is kept at all -- lists and pictures are fetched when they
//! are looked at and forgotten when the application closes.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::source::{jellyfin, vibrefy};

/// Where one of this machine's films was left.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub seconds: f64,
    pub duration: f64,
    /// When, in seconds since 1970: the most lately watched is shown first.
    pub when: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    /// This device's name for itself to a server, made once.
    pub device: String,
    pub jellyfin: Vec<jellyfin::Account>,
    pub vibrefy: Vec<vibrefy::Account>,
    /// By the film's path.
    pub places: BTreeMap<String, Place>,
}

pub fn path() -> PathBuf {
    if let Some(named) = std::env::var_os("SPATIREEL_STORE") {
        return named.into();
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into())).join(".config"));
    base.join("spatiand").join("spatireel.json")
}

impl Store {
    pub fn load() -> Store {
        let mut store: Store = std::fs::read_to_string(path()).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        if store.device.is_empty() {
            // Not a secret, only a name a server can tell this device from another by.
            let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
            store.device = format!("spatireel-{:016x}", (seed as u64) ^ ((std::process::id() as u64) << 32));
            store.save();
        }
        store
    }

    pub fn save(&self) {
        let path = path();
        let written = path.parent().map(std::fs::create_dir_all).transpose().and_then(|_| {
            let text = serde_json::to_string_pretty(self).unwrap_or_default();
            // Whole or not at all: a file half written is a list of servers lost.
            let scratch = path.with_extension("json.new");
            std::fs::write(&scratch, text)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o600));
            }
            std::fs::rename(&scratch, &path)
        });
        if let Err(e) = written {
            log::warn!("could not keep {}: {e}", path.display());
        }
    }

    /// Keep where a film of this machine's was left; or forget it, once it is nearly over or
    /// barely begun.
    pub fn leave(&mut self, film: &str, seconds: f64, duration: f64) {
        let worth_keeping = seconds > 30.0 && (duration <= 0.0 || seconds < duration * 0.95);
        if worth_keeping {
            let when = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            self.places.insert(film.to_string(), Place { seconds, duration, when });
        } else {
            self.places.remove(film);
        }
        // Films that have since been moved or deleted are not kept for ever.
        if self.places.len() > 200 {
            let oldest = self.places.iter().min_by_key(|(_, place)| place.when).map(|(film, _)| film.clone());
            if let Some(oldest) = oldest {
                self.places.remove(&oldest);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_film_barely_begun_or_nearly_over_has_no_place_kept() {
        let mut store = Store::default();
        store.leave("/a.mkv", 12.0, 3600.0);
        assert!(store.places.is_empty());
        store.leave("/a.mkv", 900.0, 3600.0);
        assert_eq!(store.places["/a.mkv"].seconds, 900.0);
        store.leave("/a.mkv", 3500.0, 3600.0);
        assert!(store.places.is_empty());
    }

    #[test]
    fn what_is_kept_comes_back_and_holds_no_password() {
        let mut store = Store { device: "d".into(), ..Store::default() };
        store.jellyfin.push(jellyfin::Account {
            flavour: jellyfin::Flavour::Emby,
            address: "http://h:8096".into(),
            user: "ann".into(),
            user_id: "u1".into(),
            token: "t0k".into(),
            device: "d".into(),
        });
        let text = serde_json::to_string(&store).unwrap();
        assert!(!text.to_lowercase().contains("password") && !text.contains("\"Pw\""));
        assert_eq!(serde_json::from_str::<Store>(&text).unwrap(), store);
    }
}
