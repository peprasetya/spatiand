//! Spatiand's applications' settings, kept the same on every paired machine.
//!
//! The folder is `~/.config/spatiand/apps` (`spatiand_app::config`): SpatiReel's servers, the
//! storage service's remotes, where films were left. Whenever two Spatiands are linked -- a
//! session and a host, in either direction, the Mac being both -- they compare what each has
//! and each takes what is newer at the other. Every machine that is ever linked to another
//! therefore ends up with one set of settings, whichever it was changed on.
//!
//! **The newest version of a file wins**, a deletion being a version too: a deleted file is
//! remembered (`~/.config/spatiand/local/sync-state.json`) with when it went, so that the
//! machine that still has it does not hand it back. A file is compared by when it last
//! changed and, at the same moment, by what is in it, so two ends with the same file agree
//! without sending it. A file taken from the other end is written with the other end's time,
//! so that it is not then taken for a newer one here.
//!
//! What is this machine's alone -- tokens a native application is let in with, a device's name
//! to a media server -- is kept in `local/`, which is never sent.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use spatiand_stream::{ConfigFile, ConfigSync};

/// Larger than this is not a setting, and is not sent.
const LARGEST: u64 = 4 << 20;

/// The folder every machine shares, and this machine's own, as `spatiand_app::config` has them.
pub fn default_folders() -> (PathBuf, PathBuf) {
    let root = match std::env::var_os("SPATIAND_APPS_CONFIG") {
        Some(named) => PathBuf::from(named),
        None => std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into())).join(".config"))
            .join("spatiand"),
    };
    (root.join("apps"), root.join("local"))
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct State {
    /// Files deleted here, or taken as deleted from elsewhere, and when.
    deleted: BTreeMap<String, u64>,
    /// What was here when last looked, to tell a file deleted from one never had.
    known: BTreeSet<String>,
}

pub struct Sync {
    shared: PathBuf,
    state_path: PathBuf,
    state: State,
    /// The manifest last said to the other end, to say it again only when it changes.
    said: Option<Vec<ConfigFile>>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// FNV-1a: what is in a file, for telling two copies apart.
fn hash(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// A path the other end names, as one under the folder: nothing absolute, nothing that climbs.
fn safe(path: &str) -> Option<PathBuf> {
    let p = Path::new(path);
    if path.is_empty() || p.is_absolute() {
        return None;
    }
    p.components().all(|c| matches!(c, Component::Normal(_))).then(|| p.to_path_buf())
}

/// Half-written files and the like, which are nobody's settings.
fn ignored(name: &str) -> bool {
    name.starts_with('.') || name.ends_with(".new") || name.ends_with(".tmp") || name.ends_with('~')
}

impl Sync {
    /// The settings in `shared`, with what is remembered of them in `local`.
    pub fn open(shared: PathBuf, local: PathBuf) -> Sync {
        let _ = std::fs::create_dir_all(&shared);
        let _ = std::fs::create_dir_all(&local);
        let state_path = local.join("sync-state.json");
        let state = std::fs::read_to_string(&state_path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Sync { shared, state_path, state, said: None }
    }

    /// The usual folders.
    pub fn open_default() -> Sync {
        let (shared, local) = default_folders();
        Sync::open(shared, local)
    }

    fn save_state(&self) {
        if let Ok(text) = serde_json::to_string(&self.state) {
            let scratch = self.state_path.with_extension("json.new");
            if std::fs::write(&scratch, text).is_ok() {
                let _ = std::fs::rename(&scratch, &self.state_path);
            }
        }
    }

    /// Every file here now, with its time and hash.
    fn scan(&self) -> BTreeMap<String, (u64, u64)> {
        let mut found = BTreeMap::new();
        let mut stack = vec![PathBuf::new()];
        while let Some(rel) = stack.pop() {
            let Ok(read) = std::fs::read_dir(self.shared.join(&rel)) else { continue };
            for entry in read.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if ignored(&name) {
                    continue;
                }
                let rel_path = rel.join(&name);
                let Ok(meta) = entry.metadata() else { continue };
                if meta.is_dir() {
                    stack.push(rel_path);
                } else if meta.is_file() && meta.len() <= LARGEST {
                    let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0);
                    if let Ok(bytes) = std::fs::read(self.shared.join(&rel_path)) {
                        found.insert(rel_path.to_string_lossy().replace('\\', "/"), (modified, hash(&bytes)));
                    }
                }
            }
        }
        found
    }

    /// What is here, deletions included: noticing, first, what has gone since last looked.
    pub fn manifest(&mut self) -> Vec<ConfigFile> {
        let found = self.scan();
        let mut changed = false;
        for gone in self.state.known.iter().filter(|k| !found.contains_key(*k)).cloned().collect::<Vec<_>>() {
            self.state.deleted.insert(gone, now_ms());
            changed = true;
        }
        for back in found.keys() {
            if self.state.deleted.remove(back).is_some() {
                changed = true;
            }
        }
        let known: BTreeSet<String> = found.keys().cloned().collect();
        if known != self.state.known {
            self.state.known = known;
            changed = true;
        }
        if changed {
            self.save_state();
        }
        let mut files: Vec<ConfigFile> = found.into_iter().map(|(path, (modified_ms, hash))| ConfigFile { path, modified_ms, hash, deleted: false }).collect();
        files.extend(self.state.deleted.iter().map(|(path, at)| ConfigFile { path: path.clone(), modified_ms: *at, hash: 0, deleted: true }));
        files
    }

    /// The manifest, if it has changed since last said: what a link says every few seconds.
    pub fn changed(&mut self) -> Option<ConfigSync> {
        let files = self.manifest();
        if self.said.as_ref() == Some(&files) {
            return None;
        }
        self.said = Some(files.clone());
        Some(ConfigSync::Manifest { files })
    }

    /// The manifest, said now whatever was said before: what a link opens with.
    pub fn hello(&mut self) -> ConfigSync {
        let files = self.manifest();
        self.said = Some(files.clone());
        ConfigSync::Manifest { files }
    }

    fn file_message(&self, path: &str) -> Option<ConfigSync> {
        let rel = safe(path)?;
        if let Some(at) = self.state.deleted.get(path) {
            return Some(ConfigSync::File { path: path.to_string(), modified_ms: *at, bytes: None });
        }
        let full = self.shared.join(&rel);
        let meta = std::fs::metadata(&full).ok()?;
        let modified_ms = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0);
        let bytes = std::fs::read(&full).ok()?;
        Some(ConfigSync::File { path: path.to_string(), modified_ms, bytes: Some(bytes) })
    }

    /// What the other end said, and what to say back.
    pub fn handle(&mut self, message: ConfigSync) -> Vec<ConfigSync> {
        match message {
            ConfigSync::Manifest { files } => {
                let mine: BTreeMap<String, ConfigFile> = self.manifest().into_iter().map(|f| (f.path.clone(), f)).collect();
                let theirs: BTreeMap<&str, &ConfigFile> = files.iter().map(|f| (f.path.as_str(), f)).collect();
                let mut want = Vec::new();
                let mut replies = Vec::new();
                for f in &files {
                    if safe(&f.path).is_none() {
                        continue;
                    }
                    match mine.get(&f.path) {
                        // The same: nothing to do.
                        Some(m) if m.deleted == f.deleted && (f.deleted || m.hash == f.hash) => {}
                        // Newer there: a deletion is taken at once, a file asked for.
                        Some(m) if newer(f, m) => {
                            if f.deleted {
                                self.delete(&f.path, f.modified_ms);
                            } else {
                                want.push(f.path.clone());
                            }
                        }
                        // Newer here: sent.
                        Some(_) => replies.extend(self.file_message(&f.path)),
                        None if f.deleted => {
                            // Never had it: the deletion is remembered, so that it is passed on.
                            self.state.deleted.insert(f.path.clone(), f.modified_ms);
                            self.save_state();
                        }
                        None => want.push(f.path.clone()),
                    }
                }
                // What is here and not there at all.
                for (path, m) in &mine {
                    if !theirs.contains_key(path.as_str()) {
                        replies.extend(if m.deleted { Some(ConfigSync::File { path: path.clone(), modified_ms: m.modified_ms, bytes: None }) } else { self.file_message(path) });
                    }
                }
                if !want.is_empty() {
                    replies.insert(0, ConfigSync::Want { paths: want });
                }
                replies
            }
            ConfigSync::Want { paths } => paths.iter().filter_map(|p| self.file_message(p)).collect(),
            ConfigSync::File { path, modified_ms, bytes } => {
                let Some(rel) = safe(&path) else { return Vec::new() };
                let mine = self.manifest().into_iter().find(|f| f.path == path);
                let incoming = ConfigFile { path: path.clone(), modified_ms, hash: bytes.as_deref().map(hash).unwrap_or(0), deleted: bytes.is_none() };
                if mine.as_ref().is_some_and(|m| !newer(&incoming, m)) {
                    return Vec::new();
                }
                match bytes {
                    None => self.delete(&path, modified_ms),
                    Some(bytes) => {
                        if bytes.len() as u64 > LARGEST {
                            return Vec::new();
                        }
                        let full = self.shared.join(&rel);
                        if let Some(parent) = full.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let scratch = full.with_extension("sync.new");
                        let written = std::fs::write(&scratch, &bytes).and_then(|_| {
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::PermissionsExt;
                                let _ = std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o600));
                            }
                            // The other end's time, so that it is the same version here.
                            let file = std::fs::OpenOptions::new().write(true).open(&scratch)?;
                            file.set_modified(UNIX_EPOCH + std::time::Duration::from_millis(modified_ms))?;
                            drop(file);
                            std::fs::rename(&scratch, &full)
                        });
                        match written {
                            Ok(()) => {
                                log::info!("settings: took {path} from the other end");
                                self.state.deleted.remove(&path);
                                self.state.known.insert(path);
                                self.save_state();
                            }
                            Err(e) => log::warn!("settings: could not write {path}: {e}"),
                        }
                    }
                }
                Vec::new()
            }
        }
    }

    fn delete(&mut self, path: &str, at: u64) {
        if let Some(rel) = safe(path) {
            if std::fs::remove_file(self.shared.join(rel)).is_ok() {
                log::info!("settings: {path} was deleted at the other end");
            }
            self.state.known.remove(path);
            self.state.deleted.insert(path.to_string(), at);
            self.save_state();
        }
    }
}

/// Whether `a` is a newer version than `b`: later, or at the same moment the larger hash, so
/// that both ends make the same choice.
fn newer(a: &ConfigFile, b: &ConfigFile) -> bool {
    (a.modified_ms, a.hash) > (b.modified_ms, b.hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn end(name: &str) -> (Sync, PathBuf) {
        let dir = std::env::temp_dir().join(format!("spatiand-sync-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (Sync::open(dir.join("apps"), dir.join("local")), dir)
    }

    /// Let two ends talk until neither has anything to say.
    fn converse(a: &mut Sync, b: &mut Sync) {
        let mut to_b = vec![a.hello()];
        let mut to_a = vec![b.hello()];
        for _ in 0..10 {
            let mut next_a = Vec::new();
            for m in to_b.drain(..) {
                next_a.extend(b.handle(m));
            }
            let mut next_b = Vec::new();
            for m in to_a.drain(..) {
                next_b.extend(a.handle(m));
            }
            to_a = next_a;
            to_b = next_b;
            if to_a.is_empty() && to_b.is_empty() {
                break;
            }
        }
    }

    fn write_at(dir: &Path, path: &str, text: &str, ms: u64) {
        let full = dir.join("apps").join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, text).unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(&full).unwrap();
        f.set_modified(UNIX_EPOCH + std::time::Duration::from_millis(ms)).unwrap();
    }

    #[test]
    fn two_machines_end_with_the_newest_of_everything() {
        let (mut mac, mac_dir) = end("mac");
        let (mut deck, deck_dir) = end("deck");
        write_at(&mac_dir, "spatireel.json", "mac's servers", 2_000_000);
        write_at(&deck_dir, "spatireel.json", "deck's older servers", 1_000_000);
        write_at(&deck_dir, "storage/remotes.conf", "[drive]", 1_500_000);
        converse(&mut mac, &mut deck);
        for dir in [&mac_dir, &deck_dir] {
            assert_eq!(std::fs::read_to_string(dir.join("apps/spatireel.json")).unwrap(), "mac's servers");
            assert_eq!(std::fs::read_to_string(dir.join("apps/storage/remotes.conf")).unwrap(), "[drive]");
        }
        // And nothing more to say after.
        assert!(mac.handle(deck.hello()).is_empty());
        let _ = std::fs::remove_dir_all(mac_dir);
        let _ = std::fs::remove_dir_all(deck_dir);
    }

    #[test]
    fn a_deletion_travels_and_is_not_handed_back() {
        let (mut mac, mac_dir) = end("mac-del");
        let (mut deck, deck_dir) = end("deck-del");
        write_at(&mac_dir, "old.json", "x", 1_000_000);
        converse(&mut mac, &mut deck);
        assert!(deck_dir.join("apps/old.json").exists());
        std::fs::remove_file(mac_dir.join("apps/old.json")).unwrap();
        converse(&mut mac, &mut deck);
        assert!(!deck_dir.join("apps/old.json").exists());
        assert!(!mac_dir.join("apps/old.json").exists());
        let _ = std::fs::remove_dir_all(mac_dir);
        let _ = std::fs::remove_dir_all(deck_dir);
    }

    #[test]
    fn nothing_outside_the_folder_is_touched() {
        let (mut mac, mac_dir) = end("mac-safe");
        for path in ["../escape", "/etc/passwd", "a/../../b", ""] {
            mac.handle(ConfigSync::File { path: path.into(), modified_ms: u64::MAX / 2, bytes: Some(b"no".to_vec()) });
        }
        assert!(!mac_dir.join("escape").exists());
        assert_eq!(mac.manifest().len(), 0);
        let _ = std::fs::remove_dir_all(mac_dir);
    }
}
