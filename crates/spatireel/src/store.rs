//! What SpatiReel keeps between one session and the next: the servers it has been signed in
//! to, and nothing else. Where a server's films were left is the server's to keep; where this
//! machine's were left is not kept at all (the owner's word: continue watching is a server's).
//!
//! One small file. A server is kept as a token or a session's cookies, never as a password.
//! Nothing of a server's library is kept at all -- lists and pictures are fetched when they
//! are looked at and forgotten when the application closes.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::source::{jellyfin, vibrefy};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    /// This device's name for itself to a server, made once. Kept in this machine's own folder,
    /// not with the rest, which every paired machine shares.
    #[serde(skip)]
    pub device: String,
    pub jellyfin: Vec<jellyfin::Account>,
    pub vibrefy: Vec<vibrefy::Account>,
}

pub fn path() -> PathBuf {
    if let Some(named) = std::env::var_os("SPATIREEL_STORE") {
        return named.into();
    }
    // With every Spatiand application's, and shared with the machines this one is paired
    // with; see `spatiand_app::config`.
    spatiand_app::config::shared_file("spatireel.json", Some("spatireel.json"))
}

/// The name this device goes by to a server: this machine's own, not shared, or every
/// machine would be one device to Jellyfin.
fn device_path() -> PathBuf {
    match std::env::var_os("SPATIREEL_STORE") {
        Some(named) => PathBuf::from(format!("{}.device", PathBuf::from(named).display())),
        None => spatiand_app::config::local_file("spatireel-device", None),
    }
}

impl Store {
    pub fn load() -> Store {
        let text = std::fs::read_to_string(path()).ok();
        let mut store: Store = text.as_deref().and_then(|text| serde_json::from_str(text).ok()).unwrap_or_default();
        store.device = std::fs::read_to_string(device_path()).map(|d| d.trim().to_string()).unwrap_or_default();
        if store.device.is_empty() {
            // An older build kept it in the shared file; from now on it is this machine's.
            let kept = text.as_deref().and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok()).and_then(|v| v.get("device")?.as_str().map(str::to_string));
            store.device = kept.filter(|d| !d.is_empty()).unwrap_or_else(|| {
                // Not a secret, only a name a server can tell this device from another by.
                let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
                format!("spatireel-{:016x}", (seed as u64) ^ ((std::process::id() as u64) << 32))
            });
            let _ = std::fs::write(device_path(), &store.device);
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // The device's own name is this machine's, kept apart from what is shared.
        assert!(!text.contains("\"device\":\"d\",\"jellyfin\""));
        assert_eq!(serde_json::from_str::<Store>(&text).unwrap(), Store { device: String::new(), ..store });
    }
}
