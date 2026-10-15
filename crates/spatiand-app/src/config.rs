//! Where Spatiand's applications keep what they keep: **one folder for all of them**.
//!
//! ```text
//! ~/.config/spatiand/apps/     what every machine shares: servers signed in to, remotes,
//!                              where films were left -- synchronised between this machine
//!                              and every Spatiand it is paired with (crates/spatiand/src/sync.rs)
//! ~/.config/spatiand/local/    what is this machine's alone: tokens, a device's name
//! ```
//!
//! `XDG_CONFIG_HOME` moves both (on Android it is the app's own files). `SPATIAND_APPS_CONFIG`
//! moves the whole of it, for a test that must not touch the wearer's own.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    if let Some(named) = std::env::var_os("SPATIAND_APPS_CONFIG") {
        return PathBuf::from(named);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into())).join(".config"));
    base.join("spatiand")
}

/// The folder every machine shares, made if it is not there.
pub fn shared() -> PathBuf {
    let dir = root().join("apps");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// This machine's own folder, made if it is not there.
pub fn local() -> PathBuf {
    let dir = root().join("local");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// A file in the shared folder, moved there from where an older build kept it (`legacy`,
/// relative to `~/.config/spatiand`) the first time it is asked for.
pub fn shared_file(name: &str, legacy: Option<&str>) -> PathBuf {
    let path = shared().join(name);
    adopt(&path, legacy);
    path
}

/// A file in this machine's own folder, moved there likewise.
pub fn local_file(name: &str, legacy: Option<&str>) -> PathBuf {
    let path = local().join(name);
    adopt(&path, legacy);
    path
}

fn adopt(path: &Path, legacy: Option<&str>) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Some(old) = legacy.map(|l| root().join(l)) {
        if !path.exists() && old.is_file() {
            let _ = std::fs::rename(&old, path);
        }
    }
}
