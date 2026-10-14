//! Where a file is: on this machine, or on a remote (`remote:folder/file`), reached through
//! the storage service. Everything that reads a file or a folder asks here, so that a remote's
//! pictures are looked at exactly as this machine's are -- read into memory as they are
//! wanted, and kept nowhere.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use spatiand_storage::Storage;

/// A remote and the path in it, if `path` is one.
pub fn remote(path: &Path) -> Option<(String, String)> {
    spatiand_storage::virtual_place(&path.to_string_lossy())
}

fn storage() -> Result<&'static Storage, String> {
    static STORAGE: OnceLock<Result<Storage, String>> = OnceLock::new();
    STORAGE.get_or_init(Storage::connect).as_ref().map_err(|e| e.clone())
}

/// The folder a place is in.
pub fn parent(path: &Path) -> Option<PathBuf> {
    match remote(path) {
        Some((r, p)) if p.is_empty() => {
            let _ = r;
            None
        }
        Some((r, p)) => Some(PathBuf::from(match p.rsplit_once('/') {
            Some((dir, _)) => format!("{r}:{dir}"),
            None => format!("{r}:"),
        })),
        None => path.parent().map(Path::to_path_buf),
    }
}

/// A place's last name.
pub fn name(path: &Path) -> String {
    match remote(path) {
        Some((r, p)) if p.is_empty() => r,
        Some((_, p)) => p.rsplit('/').next().unwrap_or(&p).to_string(),
        None => path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned()),
    }
}

pub fn join(folder: &Path, name: &str) -> PathBuf {
    match remote(folder) {
        Some((r, p)) if p.is_empty() => PathBuf::from(format!("{r}:{name}")),
        Some((r, p)) => PathBuf::from(format!("{r}:{}/{name}", p.trim_end_matches('/'))),
        None => folder.join(name),
    }
}

/// A file, whole, into memory.
pub fn read(path: &Path) -> Result<Vec<u8>, String> {
    match remote(path) {
        Some((r, p)) => storage()?.read(&r, &p, 2 << 30),
        None => std::fs::read(path).map_err(|e| e.to_string()),
    }
}

/// What FFmpeg is to open for a file: its path, or the stream of it.
pub fn source(path: &Path) -> String {
    match remote(path).and_then(|(r, p)| storage().ok().map(|s| s.url(&r, &p))) {
        Some(url) => url,
        None => path.to_string_lossy().into_owned(),
    }
}

/// A folder's things: (name, is a folder, bytes).
pub fn list(folder: &Path) -> Result<Vec<(String, bool, u64)>, String> {
    match remote(folder) {
        Some((r, p)) => Ok(storage()?.list(&r, &p)?.into_iter().map(|e| (e.name, e.is_dir, e.size.max(0) as u64)).collect()),
        None => {
            let read = std::fs::read_dir(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
            Ok(read
                .flatten()
                .filter_map(|entry| {
                    let meta = std::fs::metadata(entry.path()).ok()?;
                    Some((entry.file_name().to_string_lossy().into_owned(), meta.is_dir(), meta.len()))
                })
                .collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_places_go_up_to_the_remote_and_no_further() {
        assert_eq!(parent(Path::new("Drive:Photos/a.jpg")), Some(PathBuf::from("Drive:Photos")));
        assert_eq!(parent(Path::new("Drive:Photos")), Some(PathBuf::from("Drive:")));
        assert_eq!(parent(Path::new("Drive:")), None);
        assert_eq!(join(Path::new("Drive:"), "a.jpg"), PathBuf::from("Drive:a.jpg"));
        assert_eq!(name(Path::new("Drive:Photos/a.jpg")), "a.jpg");
        assert_eq!(parent(Path::new("/Users/me/a.jpg")), Some(PathBuf::from("/Users/me")));
    }
}
