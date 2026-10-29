//! Where SpatiKey keeps things, on every platform.
//!
//! The vault is in the applications' shared folder (`spatiand_app::config::shared()`, synced to
//! every linked Spatiand); what is this machine's alone -- its Secure Enclave key, its settings,
//! who may ask it -- in the local one, never sent.

use std::path::PathBuf;

/// `~/.config/spatiand`, as `spatiand_app::config` has it: `SPATIAND_APPS_CONFIG` moves all of
/// it (tests), `XDG_CONFIG_HOME` the base (Android sets it to the app's files).
pub fn root() -> PathBuf {
    if let Some(named) = std::env::var_os("SPATIAND_APPS_CONFIG") {
        return PathBuf::from(named);
    }
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".config"))
        .join("spatiand")
}

/// The vault: `apps/spatikey`. Not made here.
pub fn shared() -> PathBuf {
    root().join("apps").join("spatikey")
}

/// This machine's own: `local/spatikey`. Not made here.
pub fn local() -> PathBuf {
    root().join("local").join("spatikey")
}

/// The service's socket: `SPATIKEY_SOCKET`, else `$XDG_RUNTIME_DIR/spatikey.sock`, else on the
/// Mac `~/Library/Application Support/Spatiand/spatikey.sock` (the browsers start their native
/// host with no XDG_RUNTIME_DIR, and the Mac's runtime folder is under a temporary folder they
/// may not share).
pub fn socket() -> PathBuf {
    if let Some(named) = std::env::var_os("SPATIKEY_SOCKET") {
        return PathBuf::from(named);
    }
    if cfg!(target_os = "macos") {
        return home().join("Library/Application Support/Spatiand/spatikey.sock");
    }
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute()) {
        return runtime.join("spatikey.sock");
    }
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/spatikey-{uid}.sock"))
}

/// The folder holding this machine's link certificate (`identity.cert`, `identity.key`): the
/// same identity the machine pairs with hosts as. `SPATIKEY_IDENTITY` moves it (tests).
pub fn identity_dir() -> PathBuf {
    if let Some(named) = std::env::var_os("SPATIKEY_IDENTITY") {
        return PathBuf::from(named);
    }
    if cfg!(target_os = "macos") {
        return home().join("Library/Application Support/Spatiand/identity");
    }
    root()
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into()))
}
