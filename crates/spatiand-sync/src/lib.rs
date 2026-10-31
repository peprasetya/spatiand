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

pub mod roster;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use spatiand_stream::{ConfigFile, ConfigSync};

/// Larger than this is not a setting, and is not sent.
const LARGEST: u64 = 4 << 20;

/// A scan that finds this share of what was here gone, of at least [`MANY_GONE`] files, is taken
/// for something wrong here (a folder not mounted, a restored backup) and not as deletions.
const MOST_GONE: f32 = 0.5;
const MANY_GONE: usize = 8;

/// What was deleted here because another machine said so is kept this long, in this much, in
/// `local/sync-bin`: nothing of it is sent anywhere. A whole folder of settings is under a megabyte.
const BIN_DAYS: u64 = 30;
const BIN_BYTES: u64 = 20 << 20;

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

/// One copy in the bin. See [`Sync::in_bin`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binned {
    pub name: String,
    pub path: String,
    pub deleted_ms: u64,
    pub bytes: u64,
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
    bin: PathBuf,
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
        let bin = local.join("sync-bin");
        Sync { shared, bin, state_path, state, said: None }
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

    /// Every file here now, with its time and hash; and what could not be looked at, or is too
    /// large to send, which is not the same as gone. A folder that could not be read is all of it.
    fn scan(&self) -> (BTreeMap<String, (u64, u64)>, Vec<String>) {
        let mut found = BTreeMap::new();
        let mut unsure = Vec::new();
        let mut stack = vec![PathBuf::new()];
        let text = |path: &Path| path.to_string_lossy().replace('\\', "/");
        while let Some(rel) = stack.pop() {
            let read = match std::fs::read_dir(self.shared.join(&rel)) {
                Ok(read) => read,
                Err(e) => {
                    log::warn!("settings: could not read {}: {e}", rel.display());
                    unsure.push(text(&rel));
                    continue;
                }
            };
            for entry in read.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if ignored(&name) {
                    continue;
                }
                let rel_path = rel.join(&name);
                let Ok(meta) = entry.metadata() else {
                    unsure.push(text(&rel_path));
                    continue;
                };
                if meta.is_dir() {
                    stack.push(rel_path);
                } else if meta.is_file() && meta.len() <= LARGEST {
                    let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0);
                    match std::fs::read(self.shared.join(&rel_path)) {
                        Ok(bytes) => {
                            found.insert(text(&rel_path), (modified, hash(&bytes)));
                        }
                        Err(_) => unsure.push(text(&rel_path)),
                    }
                } else if meta.is_file() {
                    unsure.push(text(&rel_path));
                }
            }
        }
        (found, unsure)
    }

    /// What is here, deletions included: noticing, first, what has gone since last looked.
    pub fn manifest(&mut self) -> Vec<ConfigFile> {
        let (found, unsure) = self.scan();
        let mut changed = false;
        // Only what is certainly gone is a deletion: a deletion is passed to every machine.
        let is_unsure = |path: &str| {
            unsure.iter().any(|u| u.is_empty() || path == u || path.strip_prefix(u.as_str()).is_some_and(|rest| rest.starts_with('/')))
        };
        let gone: Vec<String> = self.state.known.iter().filter(|k| !found.contains_key(*k) && !is_unsure(k)).cloned().collect();
        let most = gone.len() >= MANY_GONE && gone.len() as f32 >= self.state.known.len() as f32 * MOST_GONE;
        if most {
            log::warn!(
                "settings: {} of {} files are gone from {} at once: not taken as deletions, and the other machines' copies come back",
                gone.len(),
                self.state.known.len(),
                self.shared.display()
            );
        } else {
            for gone in gone {
                self.state.deleted.insert(gone, now_ms());
                changed = true;
            }
        }
        for back in found.keys() {
            if self.state.deleted.remove(back).is_some() {
                changed = true;
            }
        }
        let mut known: BTreeSet<String> = found.keys().cloned().collect();
        // Still known: not seen, but not deleted either.
        known.extend(self.state.known.iter().filter(|k| !self.state.deleted.contains_key(*k) && !found.contains_key(*k)).cloned());
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

    /// Into the bin, and out of the oldest of it what no longer fits.
    fn to_bin(&self, path: &str, file: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.bin)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.bin, std::fs::Permissions::from_mode(0o700));
        }
        let kept = self.bin.join(format!("{}~{}", now_ms(), path.replace('%', "%25").replace('/', "%2F")));
        std::fs::rename(file, &kept)?;
        self.trim_bin();
        Ok(())
    }

    fn trim_bin(&self) {
        let oldest = now_ms().saturating_sub(BIN_DAYS * 24 * 60 * 60 * 1000);
        let mut kept = self.in_bin();
        let mut total: u64 = kept.iter().map(|k| k.bytes).sum();
        kept.sort_by_key(|k| k.deleted_ms);
        for k in kept {
            if k.deleted_ms < oldest || total > BIN_BYTES {
                total = total.saturating_sub(k.bytes);
                let _ = std::fs::remove_file(self.bin.join(&k.name));
            }
        }
    }

    /// What is in the bin, newest first.
    pub fn in_bin(&self) -> Vec<Binned> {
        let mut kept: Vec<Binned> = std::fs::read_dir(&self.bin)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let (ms, path) = name.split_once('~')?;
                Some(Binned {
                    deleted_ms: ms.parse().ok()?,
                    path: path.replace("%2F", "/").replace("%25", "%"),
                    bytes: entry.metadata().ok()?.len(),
                    name,
                })
            })
            .collect();
        kept.sort_by_key(|k| std::cmp::Reverse(k.deleted_ms));
        kept
    }

    /// Put the newest copies of every file deleted at or after `since_ms` back, or only those whose
    /// path starts with `prefix`, and give them the time now: so they are the newest version on
    /// every machine, and the deletion is undone there too. Says how many came back.
    pub fn restore(&mut self, since_ms: u64, prefix: &str) -> usize {
        let mut seen = BTreeSet::new();
        let mut back = 0;
        for k in self.in_bin() {
            if k.deleted_ms < since_ms || !k.path.starts_with(prefix) || !seen.insert(k.path.clone()) {
                continue;
            }
            let Some(rel) = safe(&k.path) else { continue };
            let full = self.shared.join(rel);
            if full.exists() {
                continue;
            }
            if let Some(parent) = full.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::copy(self.bin.join(&k.name), &full).is_ok() {
                // A copy may keep the old time, which the deletion would beat.
                if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&full) {
                    let _ = file.set_modified(SystemTime::now());
                }
                self.state.deleted.remove(&k.path);
                self.state.known.insert(k.path.clone());
                back += 1;
            }
        }
        self.save_state();
        back
    }

    fn delete(&mut self, path: &str, at: u64) {
        if let Some(rel) = safe(path) {
            let file = self.shared.join(rel);
            if file.is_file() {
                match self.to_bin(path, &file) {
                    Ok(()) => log::info!("settings: {path} was deleted at the other end; a copy is in the bin"),
                    Err(e) => {
                        log::warn!("settings: {path} was deleted at the other end, and could not be kept: {e}");
                        let _ = std::fs::remove_file(&file);
                    }
                }
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

    #[test]
    fn a_file_too_large_to_send_is_not_a_deletion() {
        let (mut a, dir) = end("large");
        let file = dir.join("apps/grown.json");
        std::fs::write(&file, b"small").unwrap();
        a.manifest();
        std::fs::write(&file, vec![0u8; (LARGEST + 1) as usize]).unwrap();
        let files = a.manifest();
        assert!(files.iter().all(|f| !f.deleted), "it would have been deleted on every machine");
    }

    #[test]
    fn most_of_the_folder_gone_at_once_is_no_deletion() {
        let (mut a, dir) = end("most");
        for i in 0..10 {
            std::fs::write(dir.join(format!("apps/{i}.json")), b"x").unwrap();
        }
        a.manifest();
        for i in 0..9 {
            std::fs::remove_file(dir.join(format!("apps/{i}.json"))).unwrap();
        }
        assert!(a.manifest().iter().all(|f| !f.deleted), "a folder not mounted would delete everywhere");
        // One gone, on purpose, still is.
        let (mut b, dir) = end("one");
        for i in 0..10 {
            std::fs::write(dir.join(format!("apps/{i}.json")), b"x").unwrap();
        }
        b.manifest();
        std::fs::remove_file(dir.join("apps/3.json")).unwrap();
        assert_eq!(b.manifest().iter().filter(|f| f.deleted).count(), 1);
    }

    #[test]
    fn a_deletion_from_elsewhere_can_be_undone_everywhere() {
        let (mut a, dir_a) = end("bin-a");
        let (mut b, dir_b) = end("bin-b");
        std::fs::create_dir_all(dir_a.join("apps/some")).unwrap();
        std::fs::write(dir_a.join("apps/some/thing.json"), b"precious").unwrap();
        converse(&mut a, &mut b);
        assert!(dir_b.join("apps/some/thing.json").exists());
        std::fs::remove_file(dir_a.join("apps/some/thing.json")).unwrap();
        converse(&mut a, &mut b);
        assert!(!dir_b.join("apps/some/thing.json").exists());
        let kept = b.in_bin();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].path, "some/thing.json");
        assert_eq!(b.restore(0, ""), 1);
        std::thread::sleep(std::time::Duration::from_millis(5));
        converse(&mut a, &mut b);
        assert_eq!(std::fs::read(dir_a.join("apps/some/thing.json")).unwrap(), b"precious");
    }

    #[test]
    fn the_bin_keeps_to_its_size() {
        let (a, dir) = end("trim");
        std::fs::create_dir_all(&a.bin).unwrap();
        let old = now_ms() - (BIN_DAYS + 1) * 24 * 60 * 60 * 1000;
        std::fs::write(a.bin.join(format!("{old}~old.json")), b"x").unwrap();
        std::fs::write(a.bin.join(format!("{}~big.json", now_ms() - 5)), vec![0u8; (BIN_BYTES + 1) as usize]).unwrap();
        std::fs::write(dir.join("apps/new.json"), b"y").unwrap();
        a.to_bin("new.json", &dir.join("apps/new.json")).unwrap();
        let names: Vec<String> = a.in_bin().into_iter().map(|k| k.path).collect();
        assert_eq!(names, vec!["new.json"], "too old and too much go; the newest stays");
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
