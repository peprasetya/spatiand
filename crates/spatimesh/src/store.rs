//! Where SpatiMesh keeps what it keeps (docs/spatimesh.md, "Settings").
//!
//! ```text
//! ~/.config/spatiand/apps/spatimesh/    shared by every machine (spatiand-sync): settings,
//!                                       profiles, bookmarks
//! ~/.config/spatiand/local/spatimesh/   this machine's: history, extensions
//! ~/.local/share/spatiand/spatimesh/    this machine's: each profile's website data
//! ~/.cache/spatiand/spatimesh/          thrown away freely: cache, downloads in progress
//! ```
//!
//! A file is written whole or not at all (to a hidden file, then renamed over), so that a
//! machine syncing at the wrong moment never takes half of one; the synchroniser leaves hidden
//! files alone.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;

#[derive(Debug, Clone)]
pub struct Store {
    shared: PathBuf,
    local: PathBuf,
    data: PathBuf,
    cache: PathBuf,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into()))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(fallback))
}

impl Store {
    /// The wearer's own.
    pub fn open() -> Store {
        // A test or a second person's session moves all of it at once.
        if let Some(root) = std::env::var_os("SPATIMESH_HOME") {
            return Store::at(Path::new(&root));
        }
        Store {
            shared: spatiand_app::config::shared().join("spatimesh"),
            local: spatiand_app::config::local().join("spatimesh"),
            data: xdg("XDG_DATA_HOME", ".local/share").join("spatiand/spatimesh"),
            cache: xdg("XDG_CACHE_HOME", ".cache").join("spatiand/spatimesh"),
        }
    }

    /// Everything under one folder: for tests.
    pub fn at(root: &Path) -> Store {
        Store { shared: root.join("apps/spatimesh"), local: root.join("local/spatimesh"), data: root.join("data"), cache: root.join("cache") }
    }

    pub fn shared_dir(&self) -> &Path {
        &self.shared
    }

    /// A profile's website data, on this machine only.
    pub fn data_dir(&self, profile: &str) -> PathBuf {
        self.data.join(profile)
    }

    /// A profile's cache: thrown away freely.
    pub fn cache_dir(&self, profile: &str) -> PathBuf {
        self.cache.join(profile)
    }

    pub fn cache_root(&self) -> &Path {
        &self.cache
    }

    fn path(&self, shared: bool, name: &str) -> PathBuf {
        if shared { self.shared.join(name) } else { self.local.join(name) }
    }

    /// A shared file, read; `None` if it is not there or is not what it should be.
    pub fn read<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        read_json(&self.path(true, name))
    }

    pub fn write<T: Serialize>(&self, name: &str, value: &T) -> Result<(), String> {
        write_json(&self.path(true, name), value)
    }

    /// A shared file made because there was none, such as the first profile: written as if
    /// long ago, so that the synchroniser takes any version of it from another machine over
    /// this one. Left alone if it is there.
    pub fn write_default<T: Serialize>(&self, name: &str, value: &T) -> Result<(), String> {
        let path = self.path(true, name);
        if path.exists() {
            return Ok(());
        }
        write_json(&path, value)?;
        let long_ago = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
        std::fs::File::options().write(true).open(&path).and_then(|f| f.set_modified(long_ago)).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// A shared file gone; synchronised as a deletion.
    pub fn remove(&self, name: &str) {
        let path = self.path(true, name);
        let _ = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
    }

    /// The names of the files in a shared folder that end in `.json`, without it.
    pub fn list(&self, folder: &str) -> Vec<String> {
        list_json(&self.path(true, folder))
    }

    /// When a shared folder or file last changed, to notice what the synchroniser brought.
    pub fn changed(&self, name: &str) -> Option<std::time::SystemTime> {
        std::fs::metadata(self.path(true, name)).and_then(|m| m.modified()).ok()
    }

    pub fn read_local<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        read_json(&self.path(false, name))
    }

    pub fn write_local<T: Serialize>(&self, name: &str, value: &T) -> Result<(), String> {
        write_json(&self.path(false, name), value)
    }

    pub fn local_dir(&self, name: &str) -> PathBuf {
        self.path(false, name)
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(value) => Some(value),
        Err(e) => {
            log::warn!("{} could not be read: {e}", path.display());
            None
        }
    }
}

/// Write a file whole: to a hidden one beside it, then renamed over it.
fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let dir = path.parent().ok_or("a file with no folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let name = path.file_name().ok_or("a file with no name")?.to_string_lossy();
    let temporary = dir.join(format!(".{name}.{}.new", std::process::id()));
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    // Unchanged is not written: a write is a change to every machine that syncs it.
    if std::fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(());
    }
    std::fs::write(&temporary, &bytes).map_err(|e| format!("{}: {e}", temporary.display()))?;
    std::fs::rename(&temporary, path).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        format!("{}: {e}", path.display())
    })
}

fn list_json(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .filter_map(|n| n.strip_suffix(".json").map(str::to_string))
        .collect();
    names.sort();
    names
}

/// A new name for something: sixteen hex digits from the system's randomness.
pub fn new_id() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 8];
    let random = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
    if random.is_err() {
        // Nowhere without /dev/urandom is expected; the time and the process are enough to
        // keep two names apart then.
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        bytes = (now.as_nanos() as u64 ^ ((std::process::id() as u64) << 40)).to_le_bytes();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Milliseconds since 1970.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A folder of its own under the system's temporary folder, removed when dropped: for tests.
#[cfg(test)]
pub struct Scratch(pub PathBuf);

#[cfg(test)]
impl Scratch {
    pub fn new(what: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("spatimesh-test-{what}-{}-{}", std::process::id(), new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    pub fn store(&self) -> Store {
        Store::at(&self.0)
    }
}

#[cfg(test)]
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_whole_and_read_back() {
        let scratch = Scratch::new("store");
        let store = scratch.store();
        store.write("a/b.json", &vec![1, 2, 3]).unwrap();
        assert_eq!(store.read::<Vec<i32>>("a/b.json"), Some(vec![1, 2, 3]));
        assert_eq!(store.list("a"), vec!["b".to_string()]);
        // Nothing hidden is left behind for the synchroniser to see.
        let left: Vec<_> = std::fs::read_dir(store.shared_dir().join("a")).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 1);
        store.remove("a/b.json");
        assert_eq!(store.read::<Vec<i32>>("a/b.json"), None);
    }

    #[test]
    fn unchanged_is_not_written() {
        let scratch = Scratch::new("unchanged");
        let store = scratch.store();
        store.write("x.json", &"same").unwrap();
        let path = store.shared_dir().join("x.json");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(old).unwrap();
        store.write("x.json", &"same").unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), old);
    }

    #[test]
    fn a_broken_file_is_none() {
        let scratch = Scratch::new("broken");
        let store = scratch.store();
        std::fs::create_dir_all(store.shared_dir()).unwrap();
        std::fs::write(store.shared_dir().join("y.json"), b"{not json").unwrap();
        assert_eq!(store.read::<Vec<i32>>("y.json"), None);
    }

    #[test]
    fn ids_differ() {
        assert_ne!(new_id(), new_id());
        assert_eq!(new_id().len(), 16);
    }
}
