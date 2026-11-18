//! The Mac's trash is Finder's: things are put in it by `NSFileManager`'s `trashItemAtURL`,
//! which is what Finder itself does, so that Finder's Put Back works for them as for its own;
//! what is in it is `~/.Trash` (and a disk's `.Trashes/<uid>`).
//!
//! Where each thing came from is read from the Trash's `.DS_Store` (`ds_store.rs`), where
//! Finder keeps it (`ptbL`, the folder from the top of the disk, and `ptbN`, the name), so Put
//! back works for what Finder or any other program put there. SpatiShelf remembers what it put
//! there itself too (`spatishelf-trash.json`, this machine's own settings): that is known at
//! once, before Finder has written its record, and when it was put there. Each memory is of one
//! file, by its inode, so that something else later in the Trash by the same name is not taken
//! for it.
//!
//! Put back by SpatiShelf leaves Finder's record of the thing in the `.DS_Store`, which is never
//! written here: Finder tolerates a record of something no longer there (it is dropped when
//! Finder next writes the file, and a new thing by the name has a new record), and so does this
//! reader, which only looks up what is in the Trash.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use objc2_foundation::{NSFileManager, NSString, NSURL};

use super::{ds_store, Entry};

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

/// The top of the disk a trash is for, which Finder's folders are written from: `/` for the
/// home one, the disk for `/Volumes/<disk>/.Trashes/<uid>`.
fn disk_of(bin: &Path) -> PathBuf {
    if bin == home().join(".Trash") {
        return PathBuf::from("/");
    }
    bin.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/"))
}

/// Where Finder says each thing in this trash came from, by its name there.
fn finders(bin: &Path) -> HashMap<String, PathBuf> {
    let Ok(bytes) = std::fs::read(bin.join(".DS_Store")) else { return HashMap::new() };
    match ds_store::put_back_records(&bytes) {
        Ok(records) => {
            let disk = disk_of(bin);
            records.into_iter().map(|(name, o)| (name.clone(), ds_store::origin_path(&disk, &o, &name))).collect()
        }
        Err(e) => {
            log::warn!("{}: {e}", bin.join(".DS_Store").display());
            HashMap::new()
        }
    }
}

/// What SpatiShelf remembers of something it put in the trash.
#[derive(Debug, Clone, PartialEq)]
struct Kept {
    origin: String,
    when: i64,
    /// The thing's inode once in the trash (not had in the memories of older builds).
    inode: Option<u64>,
}

fn memory_file() -> PathBuf {
    spatiand_app::config::local_file("spatishelf-trash.json", None)
}

/// Read as `[origin, when, inode]` per place in the trash; older builds wrote `[origin, when]`.
fn remembered() -> HashMap<String, Kept> {
    let raw: HashMap<String, Vec<serde_json::Value>> = std::fs::read(memory_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    raw.into_iter()
        .filter_map(|(k, v)| {
            let origin = v.first()?.as_str()?.to_string();
            let when = v.get(1).and_then(|w| w.as_i64()).unwrap_or(0);
            Some((k, Kept { origin, when, inode: v.get(2).and_then(|i| i.as_u64()) }))
        })
        .collect()
}

fn remember(memory: &HashMap<String, Kept>) {
    let raw: HashMap<&String, serde_json::Value> = memory.iter().map(|(k, m)| (k, serde_json::json!([m.origin, m.when, m.inode]))).collect();
    if let Ok(bytes) = serde_json::to_vec_pretty(&raw) {
        let _ = std::fs::write(memory_file(), bytes);
    }
}

/// Whether a memory is of the thing now at its place in the trash.
fn is_of(kept: &Kept, file: &Path) -> bool {
    match kept.inode {
        Some(inode) => file.symlink_metadata().is_ok_and(|m| m.ino() == inode),
        None => true,
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
        let at = at.to_string();
        let inode = Path::new(&at).symlink_metadata().ok().map(|m| m.ino());
        let mut memory = remembered();
        memory.insert(at, Kept { origin: path.to_string_lossy().into_owned(), when: super::now(), inode });
        remember(&memory);
    }
    Ok(())
}

pub fn list() -> Result<Vec<Entry>, String> {
    let mut memory = remembered();
    let before = memory.clone();
    let mut out = Vec::new();
    let mut here = std::collections::HashSet::new();
    // The trashes read this time: what is remembered of another (a disk not plugged in now,
    // one that could not be read) is kept for when it is back.
    let mut read_bins: Vec<PathBuf> = Vec::new();
    for (i, bin) in bins().into_iter().enumerate() {
        let read = match std::fs::read_dir(&bin) {
            Ok(read) => read,
            // The home Trash is kept from applications without Full Disk Access.
            Err(e) if i == 0 && e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err("macOS keeps the Trash from SpatiShelf. Allow Spatiand in System Settings, Privacy & Security, Full Disk Access.".into());
            }
            Err(_) => continue,
        };
        read_bins.push(bin.clone());
        let finders = finders(&bin);
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".DS_Store" || name == ".localized" {
                continue;
            }
            let file = entry.path();
            let key = file.to_string_lossy().into_owned();
            let ours = memory.get(&key).filter(|k| is_of(k, &file)).cloned();
            if ours.is_some() {
                here.insert(key);
            }
            let folder = file.symlink_metadata().is_ok_and(|m| m.is_dir());
            // When it came, where SpatiShelf did not put it: as near as the file says (it
            // changed when it was moved).
            let changed = || file.symlink_metadata().ok().map(|m| m.ctime());
            let (origin, deleted) = match ours {
                Some(k) => (Some(PathBuf::from(k.origin)), Some(k.when)),
                None => (finders.get(&name).cloned(), changed()),
            };
            let shown = origin.as_ref().and_then(|o| o.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or(name);
            out.push(Entry { size: super::size_of(&file), file, name: shown, origin, deleted, folder });
        }
    }
    // What has gone from the trash some other way (Finder's Put Back, emptied), or is not the
    // thing remembered any more, is forgotten.
    memory.retain(|k, _| here.contains(k) || !read_bins.iter().any(|b| Path::new(k).parent() == Some(b.as_path())));
    if memory != before {
        remember(&memory);
    }
    Ok(out)
}

pub fn folders() -> Vec<PathBuf> {
    bins()
}

/// Where a thing in the trash came from: SpatiShelf's memory of it, else Finder's record.
fn origin_of(file: &Path) -> Option<PathBuf> {
    let key = file.to_string_lossy().into_owned();
    if let Some(k) = remembered().get(&key).filter(|k| is_of(k, file)) {
        return Some(PathBuf::from(&k.origin));
    }
    let bin = file.parent()?;
    let name = file.file_name()?.to_string_lossy().into_owned();
    finders(bin).remove(&name)
}

pub fn restore(file: &Path) -> Result<PathBuf, String> {
    let origin = origin_of(file).ok_or("Where it came from is not known: drag it to where it is to go.")?;
    let to = super::put_back(file, &origin)?;
    // Finder's record stays in the .DS_Store (see the top of this file).
    moved_out(file);
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
