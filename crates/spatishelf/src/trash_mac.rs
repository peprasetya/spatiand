//! The Mac's trash is Finder's: things are put in it by `NSFileManager`'s `trashItemAtURL`,
//! which is what Finder itself does, so that Finder's Put Back works for them as for its own;
//! what is in it is `~/.Trash` (and a disk's `.Trashes/<uid>`).
//!
//! Where a thing came from Finder keeps in the Trash's `.DS_Store`, which is not read here: what
//! SpatiShelf put in the trash it remembers itself (`spatishelf-trash.json`, this machine's own
//! settings), and Put back works for those; for what Finder put there, SpatiShelf says it does
//! not know where it came from, and Finder's Put Back does it. *The proper whole would read the
//! `.DS_Store`'s `ptbL`/`ptbN` records.*

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use objc2_foundation::{NSFileManager, NSString, NSURL};

use super::Entry;

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into()))
}

/// The trashes: the home one, and each disk's.
fn bins() -> Vec<PathBuf> {
    let mut out = vec![home().join(".Trash")];
    let uid = unsafe { libc::getuid() };
    for entry in std::fs::read_dir("/Volumes").into_iter().flatten().flatten() {
        let bin = entry.path().join(".Trashes").join(uid.to_string());
        if bin.is_dir() {
            out.push(bin);
        }
    }
    out
}

/// What SpatiShelf remembers of what it put in the trash: where each came from, and when.
fn memory_file() -> PathBuf {
    spatiand_app::config::local_file("spatishelf-trash.json", None)
}

fn remembered() -> HashMap<String, (String, i64)> {
    std::fs::read(memory_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn remember(memory: &HashMap<String, (String, i64)>) {
    if let Ok(bytes) = serde_json::to_vec_pretty(memory) {
        let _ = std::fs::write(memory_file(), bytes);
    }
}

pub fn is_within(path: &Path) -> bool {
    bins().iter().any(|b| path.starts_with(b))
}

pub fn put(path: &Path) -> Result<(), String> {
    let manager = NSFileManager::defaultManager();
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let mut landed: Option<objc2::rc::Retained<NSURL>> = None;
    manager.trashItemAtURL_resultingItemURL_error(&url, Some(&mut landed)).map_err(|e| e.localizedDescription().to_string())?;
    if let Some(at) = landed.and_then(|u| u.path()) {
        let mut memory = remembered();
        memory.insert(at.to_string(), (path.to_string_lossy().into_owned(), super::now()));
        remember(&memory);
    }
    Ok(())
}

pub fn list() -> Result<Vec<Entry>, String> {
    let mut memory = remembered();
    let before = memory.len();
    let mut out = Vec::new();
    let mut here = std::collections::HashSet::new();
    for (i, bin) in bins().into_iter().enumerate() {
        let read = match std::fs::read_dir(&bin) {
            Ok(read) => read,
            // The home Trash is kept from applications without Full Disk Access.
            Err(e) if i == 0 && e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err("macOS keeps the Trash from SpatiShelf. Allow Spatiand in System Settings, Privacy & Security, Full Disk Access.".into());
            }
            Err(_) => continue,
        };
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".DS_Store" || name == ".localized" {
                continue;
            }
            let file = entry.path();
            let key = file.to_string_lossy().into_owned();
            here.insert(key.clone());
            let folder = file.symlink_metadata().is_ok_and(|m| m.is_dir());
            let (origin, deleted) = match memory.get(&key) {
                Some((origin, when)) => (Some(PathBuf::from(origin)), Some(*when)),
                // When it came: as near as the file says (it changed when it was moved).
                None => (None, file.symlink_metadata().ok().map(|m| std::os::unix::fs::MetadataExt::ctime(&m))),
            };
            let shown = origin.as_ref().and_then(|o| o.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or(name);
            out.push(Entry { size: super::size_of(&file), file, name: shown, origin, deleted, folder });
        }
    }
    // What has gone from the trash some other way (Finder's Put Back, emptied) is forgotten.
    memory.retain(|k, _| here.contains(k));
    if memory.len() != before {
        remember(&memory);
    }
    Ok(out)
}

pub fn folders() -> Vec<PathBuf> {
    bins()
}

pub fn restore(file: &Path) -> Result<PathBuf, String> {
    let key = file.to_string_lossy().into_owned();
    let mut memory = remembered();
    let (origin, _) = memory.get(&key).cloned().ok_or("Where it came from is not known here: Finder's Put Back knows, or drag it to where it is to go.")?;
    let to = super::put_back(file, Path::new(&origin))?;
    memory.remove(&key);
    remember(&memory);
    Ok(to)
}

pub fn purge(file: &Path) -> Result<(), String> {
    super::remove(file).map_err(|e| e.to_string())?;
    moved_out(file);
    Ok(())
}

pub fn moved_out(file: &Path) {
    let mut memory = remembered();
    if memory.remove(&file.to_string_lossy().into_owned()).is_some() {
        remember(&memory);
    }
}

pub fn empty() -> Result<(), String> {
    for bin in bins() {
        for entry in std::fs::read_dir(&bin).map_err(|e| e.to_string())?.flatten() {
            let name = entry.file_name();
            if name == ".DS_Store" || name == ".localized" {
                continue;
            }
            super::remove(&entry.path()).map_err(|e| e.to_string())?;
        }
    }
    remember(&HashMap::new());
    Ok(())
}
