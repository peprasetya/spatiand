//! One person's devices, on every machine of theirs: the signed list.
//!
//! Pairing joins a *user* across machines. Paired with any host, a device is paired with all of
//! them; removed on any, it is removed on all. The list is a file per device in the folder that
//! is already shared, `apps/roster/<fingerprint>.json`, carried by the same sync, newest wins, and
//! each machine turns it into what it already had: a host's paired list, a device's hosts.
//!
//! Every entry is signed with the user's key, `apps/roster/user.key`, which travels with the list
//! to whoever is paired, over the pinned links and to nobody else. Removing a device makes a new
//! key, so what it signs afterwards is not believed. It is not proof against a thief who gets
//! there first: whoever has a member's disk can do what a member can, and the owner's shell wins
//! that race (`docs/remove-a-device.md`). Two machines that made keys before they met take the
//! newer, and sign again what they had signed with the old.
//!
//! **A removal is a file of its own**, `<fingerprint>.removed`, because sync keeps whichever copy of
//! a file is newest: a machine that had not heard yet, writing the device's entry again as it
//! signed or filled it in, would otherwise have overwritten the removal with "still a member". Only
//! pairing again, later than the removal, outranks it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};
use serde::{Deserialize, Serialize};

const KEY_FILE: &str = "user.key";
const SEEN_FILE: &str = "roster-seen-key";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Its certificate's fingerprint, whole, in lowercase hex.
    pub fp: String,
    /// What it calls itself. Empty until it has said.
    #[serde(default)]
    pub name: String,
    /// Where it can be reached if it is a host, as the device that paired with it dialled it.
    #[serde(default)]
    pub address: String,
    pub added_ms: u64,
    /// When it was removed; 0 while it is a member.
    #[serde(default)]
    pub removed_ms: u64,
    #[serde(default)]
    pub sig: String,
}

/// That a device was removed, and when.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Removal {
    fp: String,
    removed_ms: u64,
    #[serde(default)]
    sig: String,
}

impl Removal {
    fn signed_text(&self) -> String {
        format!("spatiand roster removal 1\n{}\n{}", self.fp, self.removed_ms)
    }
}

impl Entry {
    fn signed_text(&self) -> String {
        format!("spatiand roster 1\n{}\n{}\n{}\n{}\n{}", self.fp, self.name, self.address, self.added_ms, self.removed_ms)
    }

    pub fn removed(&self) -> bool {
        self.removed_ms != 0
    }

    /// What to call it: its name, or the start of its fingerprint.
    pub fn label(&self) -> String {
        if self.name.is_empty() { format!("unnamed {}", &self.fp[..self.fp.len().min(8)]) } else { self.name.clone() }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn pair_of(seed: &[u8]) -> Option<Ed25519KeyPair> {
    Ed25519KeyPair::from_seed_unchecked(seed).ok()
}

fn sign(seed: &[u8], entry: &mut Entry) {
    if let Some(pair) = pair_of(seed) {
        entry.sig = hex(pair.sign(entry.signed_text().as_bytes()).as_ref());
    }
}

fn verifies(seed: &[u8], entry: &Entry) -> bool {
    let (Some(pair), Some(sig)) = (pair_of(seed), unhex(&entry.sig)) else { return false };
    UnparsedPublicKey::new(&ED25519, pair.public_key().as_ref()).verify(entry.signed_text().as_bytes(), &sig).is_ok()
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let scratch = path.with_extension("new");
    std::fs::write(&scratch, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&scratch, path)
}

/// The list as this machine has it.
pub struct Roster {
    dir: PathBuf,
    seen: PathBuf,
}

impl Roster {
    pub fn open(shared: &Path, local: &Path) -> Roster {
        Roster { dir: shared.join("roster"), seen: local.join(SEEN_FILE) }
    }

    pub fn open_default() -> Roster {
        let (shared, local) = crate::default_folders();
        Roster::open(&shared, &local)
    }

    fn entry_path(&self, fp: &str) -> PathBuf {
        self.dir.join(format!("{fp}.json"))
    }

    fn read_seed(path: &Path) -> Option<Vec<u8>> {
        unhex(&std::fs::read_to_string(path).ok()?).filter(|s| s.len() == 32)
    }

    fn new_key(&self) -> Option<Vec<u8>> {
        let mut seed = [0u8; 32];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut seed).ok()?;
        write_private(&self.dir.join(KEY_FILE), hex(&seed).as_bytes()).ok()?;
        Some(seed.to_vec())
    }

    fn entries_on_disk(&self) -> Vec<Entry> {
        std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|f| f.path().extension().is_some_and(|e| e == "json"))
            .filter_map(|f| serde_json::from_slice::<Entry>(&std::fs::read(f.path()).ok()?).ok())
            .filter(|e| !e.fp.is_empty() && e.fp.bytes().all(|b| b.is_ascii_hexdigit()))
            .collect()
    }

    fn removals_on_disk(&self) -> Vec<Removal> {
        std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|f| f.path().extension().is_some_and(|e| e == "removed"))
            .filter_map(|f| serde_json::from_slice::<Removal>(&std::fs::read(f.path()).ok()?).ok())
            .filter(|r| !r.fp.is_empty() && r.fp.bytes().all(|b| b.is_ascii_hexdigit()))
            .collect()
    }

    fn write_removal(&self, seed: &[u8], mut removal: Removal) {
        let Some(pair) = pair_of(seed) else { return };
        removal.sig = hex(pair.sign(removal.signed_text().as_bytes()).as_ref());
        let path = self.dir.join(format!("{}.removed", removal.fp));
        match serde_json::to_vec_pretty(&removal).map(|b| write_private(&path, &b)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => log::warn!("roster: could not write {}: {e}", path.display()),
            Err(e) => log::warn!("roster: {e}"),
        }
    }

    /// Whether this removal was signed with this key.
    fn removal_verifies(seed: &[u8], removal: &Removal) -> bool {
        let (Some(pair), Some(sig)) = (pair_of(seed), unhex(&removal.sig)) else { return false };
        UnparsedPublicKey::new(&ED25519, pair.public_key().as_ref()).verify(removal.signed_text().as_bytes(), &sig).is_ok()
    }

    /// When this device was removed, if it was and has not been paired since.
    fn removed_at(&self, seed: &[u8], fp: &str, added_ms: u64) -> Option<u64> {
        let removal = std::fs::read(self.dir.join(format!("{fp}.removed"))).ok().and_then(|b| serde_json::from_slice::<Removal>(&b).ok())?;
        (Self::removal_verifies(seed, &removal) && removal.removed_ms >= added_ms).then_some(removal.removed_ms)
    }

    fn write(&self, seed: &[u8], mut entry: Entry) {
        sign(seed, &mut entry);
        let path = self.entry_path(&entry.fp);
        let same = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Entry>(&b).ok()).is_some_and(|old| old == entry);
        if same {
            return;
        }
        match serde_json::to_vec_pretty(&entry).map(|b| write_private(&path, &b)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => log::warn!("roster: could not write {}: {e}", path.display()),
            Err(e) => log::warn!("roster: {e}"),
        }
    }

    /// The key, made if there is none; and what was signed with the key before, signed again.
    fn settle(&self) -> Option<Vec<u8>> {
        let seed = Self::read_seed(&self.dir.join(KEY_FILE)).or_else(|| self.new_key())?;
        if let Some(old) = Self::read_seed(&self.seen).filter(|old| *old != seed) {
            for entry in self.entries_on_disk() {
                if !verifies(&seed, &entry) && verifies(&old, &entry) {
                    self.write(&seed, entry);
                }
            }
            for removal in self.removals_on_disk() {
                if !Self::removal_verifies(&seed, &removal) && Self::removal_verifies(&old, &removal) {
                    self.write_removal(&seed, removal);
                }
            }
        }
        if Self::read_seed(&self.seen).as_deref() != Some(&seed) {
            let _ = write_private(&self.seen, hex(&seed).as_bytes());
        }
        Some(seed)
    }

    /// Everyone, the removed too; only what is signed with the key this machine holds. Sorted by name.
    pub fn all(&self) -> Vec<Entry> {
        let Some(seed) = self.settle() else { return Vec::new() };
        let mut entries: Vec<Entry> = self
            .entries_on_disk()
            .into_iter()
            .filter(|e| {
                let good = verifies(&seed, e);
                if !good {
                    log::debug!("roster: {} is not signed with this key yet", &e.fp[..e.fp.len().min(8)]);
                }
                good
            })
            .collect();
        for e in &mut entries {
            if let Some(at) = self.removed_at(&seed, &e.fp, e.added_ms) {
                e.removed_ms = at;
            }
        }
        // Removed, with its entry out of date or never seen here: still removed.
        for r in self.removals_on_disk() {
            if Self::removal_verifies(&seed, &r) && !entries.iter().any(|e| e.fp == r.fp) {
                entries.push(Entry { fp: r.fp, removed_ms: r.removed_ms, ..Entry::default() });
            }
        }
        entries.sort_by_key(|e| (e.removed(), e.label().to_lowercase()));
        entries
    }

    /// A member this machine knows of. A removed one stays removed; what is already known of one
    /// stays as it is, except what it says of itself (`own`).
    pub fn note(&self, fp: &str, name: &str, address: &str, own: bool) {
        let fp = fp.trim().to_lowercase();
        let Some(seed) = self.settle() else { return };
        if fp.is_empty() || !fp.bytes().all(|b| b.is_ascii_hexdigit()) {
            return;
        }
        let on_disk = std::fs::read(self.entry_path(&fp)).ok().and_then(|b| serde_json::from_slice::<Entry>(&b).ok());
        // Signed with a key not here yet: not this machine's to write over.
        if on_disk.as_ref().is_some_and(|e| !verifies(&seed, e)) {
            return;
        }
        let old_added = on_disk.as_ref().map_or(0, |e| e.added_ms);
        if self.removed_at(&seed, &fp, old_added).is_some() {
            return;
        }
        let mut entry = match on_disk {
            Some(old) if old.removed() => return,
            Some(old) => old,
            None => Entry { fp: fp.clone(), added_ms: now_ms(), ..Entry::default() },
        };
        if !name.is_empty() && (own || entry.name.is_empty()) {
            entry.name = name.to_string();
        }
        if !address.is_empty() && entry.address.is_empty() {
            entry.address = address.to_string();
        }
        self.write(&seed, entry);
    }

    /// Paired, by the owner, now: a member again even if it was removed.
    pub fn pair(&self, fp: &str, name: &str, address: &str) {
        let fp = fp.trim().to_lowercase();
        let Some(seed) = self.settle() else { return };
        let mut entry = std::fs::read(self.entry_path(&fp))
            .ok()
            .and_then(|b| serde_json::from_slice::<Entry>(&b).ok())
            .filter(|e| verifies(&seed, e))
            .unwrap_or_else(|| Entry { fp: fp.clone(), ..Entry::default() });
        entry.removed_ms = 0;
        entry.added_ms = now_ms();
        if !name.is_empty() {
            entry.name = name.to_string();
        }
        if !address.is_empty() {
            entry.address = address.to_string();
        }
        self.write(&seed, entry);
    }

    /// The one member that `which` means. People know their devices by name: its whole name, then
    /// part of one; and for a device that has none, the start of its fingerprint.
    fn find(&self, which: &str) -> Result<Entry, String> {
        let which = which.trim().to_lowercase();
        if which.is_empty() {
            return Err("say which device: --devices lists them".into());
        }
        let members: Vec<Entry> = self.all().into_iter().filter(|e| !e.removed()).collect();
        let tries: [&dyn Fn(&Entry) -> bool; 3] = [
            &|e| e.name.to_lowercase() == which,
            &|e| e.name.to_lowercase().contains(&which),
            &|e| which.len() >= 4 && e.fp.starts_with(&which),
        ];
        for matches in tries {
            let found: Vec<&Entry> = members.iter().filter(|e| matches(e)).collect();
            match found[..] {
                [] => continue,
                [one] => return Ok(one.clone()),
                _ => {
                    let names: Vec<String> = found.iter().map(|e| format!("{} ({})", e.label(), &e.fp[..e.fp.len().min(8)])).collect();
                    return Err(format!("{which} could be any of: {}. Say which by the part in brackets", names.join(", ")));
                }
            }
        }
        let names: Vec<String> = members.iter().map(Entry::label).collect();
        Err(format!("no device on the list is called {which}. There are: {}", names.join(", ")))
    }

    /// Take a device off every machine's list, by its name.
    pub fn remove(&self, which: &str) -> Result<Entry, String> {
        let mut entry = self.find(which)?;
        // A new key first, and everything signed with it, so what the removed one signs is not.
        let seed = self.new_key().ok_or("could not make a new key")?;
        self.settle();
        entry.removed_ms = now_ms();
        self.write_removal(&seed, Removal { fp: entry.fp.clone(), removed_ms: entry.removed_ms, sig: String::new() });
        Ok(entry)
    }

    /// Keeps the list up on a thread of its own, every two seconds, and says when it has changed.
    /// `known` is what this machine already trusts, which goes on the list: (fingerprint, address).
    pub fn watch(mut known: impl FnMut() -> Vec<(String, String)> + Send + 'static) -> std::sync::mpsc::Receiver<Vec<Entry>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let run = move || {
            let roster = Roster::open_default();
            let mut said: Option<Vec<Entry>> = None;
            loop {
                for (fp, address) in known() {
                    roster.note(&fp, "", &address, false);
                }
                let entries = roster.all();
                if said.as_ref() != Some(&entries) {
                    if tx.send(entries.clone()).is_err() {
                        return;
                    }
                    said = Some(entries);
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        };
        if let Err(e) = std::thread::Builder::new().name("roster".into()).spawn(run) {
            log::error!("roster: no thread: {e}");
        }
        rx
    }
}

/// `--devices`, `--forget-device <which>`, `--settings-bin` and `--restore-settings [hours] [path]`,
/// the same on every platform. Whether one of them was asked for, having done it.
pub fn command_line(args: &[String]) -> bool {
    let after = |flag: &str| args.iter().position(|a| a == flag).map(|i| args[i + 1..].to_vec());
    if args.iter().any(|a| a == "--devices") {
        let entries = Roster::open_default().all();
        if entries.is_empty() {
            println!("no devices on the list yet");
        }
        for e in entries {
            let state = if e.removed() { "removed" } else { "member" };
            println!("{:<24} {state:8} {}{}", e.label(), &e.fp[..e.fp.len().min(8)], if e.address.is_empty() { String::new() } else { format!("  {}", e.address) });
        }
        return true;
    }
    if let Some(rest) = after("--forget-device") {
        let Some(which) = rest.first() else {
            eprintln!("--forget-device <its name>; --devices lists them");
            std::process::exit(2);
        };
        match Roster::open_default().remove(which) {
            Ok(e) => println!("removed {} ({}) from this machine's list; it leaves every other machine as they sync", e.label(), e.fp),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return true;
    }
    if args.iter().any(|a| a == "--settings-bin") {
        let kept = crate::Sync::open_default().in_bin();
        if kept.is_empty() {
            println!("nothing has been deleted here by another machine");
        }
        let now = now_ms();
        for k in kept {
            println!("{:>6.1} h ago  {:>8} bytes  {}", now.saturating_sub(k.deleted_ms) as f64 / 3_600_000.0, k.bytes, k.path);
        }
        return true;
    }
    if let Some(rest) = after("--restore-settings") {
        let hours: u64 = rest.first().and_then(|h| h.parse().ok()).unwrap_or(24 * 30);
        let prefix = rest.get(1).map(String::as_str).unwrap_or("");
        let since = now_ms().saturating_sub(hours * 3_600_000);
        let back = crate::Sync::open_default().restore(since, prefix);
        println!("{back} file(s) put back; they reach the other machines as they sync");
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aa00000000000000000000000000000000000000000000000000000000000000";
    const B: &str = "bb00000000000000000000000000000000000000000000000000000000000000";

    fn roster(name: &str) -> (Roster, PathBuf) {
        let dir = std::env::temp_dir().join(format!("spatiand-roster-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (Roster::open(&dir.join("apps"), &dir.join("local")), dir)
    }

    /// What sync does: the newer machine's roster files over the older's.
    fn copy(from: &Path, to: &Path) {
        let (from, to) = (from.join("apps/roster"), to.join("apps/roster"));
        std::fs::create_dir_all(&to).unwrap();
        for file in std::fs::read_dir(from).unwrap().flatten() {
            if file.path().extension().is_some_and(|e| e == "new") {
                continue;
            }
            std::fs::copy(file.path(), to.join(file.file_name())).unwrap();
        }
    }

    #[test]
    fn what_a_device_says_of_itself_fills_in_what_a_host_knew() {
        let (r, _) = roster("fill");
        r.note(A, "", "", false);
        assert_eq!(r.all()[0].label(), "unnamed aa000000");
        r.note(A, "Steam Deck", "", true);
        r.note(A, "someone else's word", "deepmagpie:47600", false);
        let e = &r.all()[0];
        assert_eq!((e.name.as_str(), e.address.as_str()), ("Steam Deck", "deepmagpie:47600"));
    }

    #[test]
    fn an_entry_not_signed_with_the_key_is_not_believed() {
        let (r, dir) = roster("forged");
        r.note(A, "ok", "", true);
        let forged = Entry { fp: B.into(), name: "planted".into(), added_ms: 1, sig: "00".repeat(64), ..Entry::default() };
        std::fs::write(dir.join(format!("apps/roster/{B}.json")), serde_json::to_vec(&forged).unwrap()).unwrap();
        let path = dir.join(format!("apps/roster/{A}.json"));
        let mut edited: Entry = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(r.all().len(), 1, "the planted one");
        edited.name = "changed on the way".into();
        std::fs::write(&path, serde_json::to_vec(&edited).unwrap()).unwrap();
        assert!(r.all().is_empty(), "the edited one");
    }

    #[test]
    fn a_removal_reaches_the_other_machine_and_stays() {
        let (a, dir_a) = roster("rm-a");
        let (b, dir_b) = roster("rm-b");
        a.note(A, "deck", "", true);
        a.note(B, "beam", "", true);
        copy(&dir_a, &dir_b);
        assert_eq!(b.all().iter().filter(|e| !e.removed()).count(), 2);
        let gone = b.remove("beam").unwrap();
        assert_eq!(gone.fp, B);
        copy(&dir_b, &dir_a);
        let on_a = a.all();
        assert_eq!(on_a.iter().filter(|e| !e.removed()).count(), 1, "the other member, signed again");
        assert!(on_a.iter().any(|e| e.fp == B && e.removed()));
        // Noticed again by a machine that has not caught up: it stays removed.
        a.note(B, "", "", false);
        assert!(a.all().iter().any(|e| e.fp == B && e.removed()));
        // The owner pairing it again brings it back.
        a.pair(B, "beam", "");
        assert!(a.all().iter().any(|e| e.fp == B && !e.removed()));
    }

    #[test]
    fn a_machine_that_has_not_heard_cannot_overwrite_a_removal() {
        let (a, dir_a) = roster("late-a");
        let (b, dir_b) = roster("late-b");
        a.note(A, "deck", "", true);
        a.note(B, "fake", "", true);
        copy(&dir_a, &dir_b);
        b.all();
        // A removes it. Everything of A's reaches B in the worst order: the removal, before the key
        // that signs it. Meanwhile B goes on noting what it trusts, and signing.
        a.remove("fake").unwrap();
        let (from, to) = (dir_a.join("apps/roster"), dir_b.join("apps/roster"));
        std::fs::copy(from.join(format!("{B}.removed")), to.join(format!("{B}.removed"))).unwrap();
        b.note(B, "", "", false);
        assert!(b.all().iter().any(|e| e.fp == B && !e.removed()), "not believed yet: the key has not come");
        std::fs::copy(from.join("user.key"), to.join("user.key")).unwrap();
        b.note(B, "", "", false);
        assert!(b.all().iter().any(|e| e.fp == B && e.removed()), "got {:?}", b.all());
        // And B's copy of the entry, newer than A's, going back to A changes nothing.
        copy(&dir_b, &dir_a);
        assert!(a.all().iter().any(|e| e.fp == B && e.removed()));
    }

    #[test]
    fn what_a_removed_device_signs_afterwards_is_not_believed() {
        let (home, dir_home) = roster("thief-home");
        let (stolen, dir_stolen) = roster("thief-stolen");
        home.note(A, "deck", "", true);
        home.note(B, "stolen", "", true);
        copy(&dir_home, &dir_stolen);
        stolen.all();
        home.remove("stolen").unwrap();
        // It still has the old key, and tries: undoing its removal, adding a friend.
        let mut undo = stolen.all().into_iter().find(|e| e.fp == B).unwrap();
        undo.added_ms = u64::MAX / 2;
        sign(&Roster::read_seed(&dir_stolen.join("apps/roster/user.key")).unwrap(), &mut undo);
        std::fs::write(dir_home.join(format!("apps/roster/{B}.json")), serde_json::to_vec(&undo).unwrap()).unwrap();
        let seen = home.all();
        assert!(!seen.iter().any(|e| e.fp == B && !e.removed()), "got {seen:?}");
    }

    #[test]
    fn two_machines_that_made_keys_before_they_met_agree() {
        let (a, dir_a) = roster("keys-a");
        let (b, dir_b) = roster("keys-b");
        a.note(A, "a", "", true);
        b.note(B, "b", "", true);
        // B's key is the newer, and comes to A with everything of B's.
        copy(&dir_b, &dir_a);
        assert_eq!(a.all().len(), 2, "A signed its own again with B's key");
        copy(&dir_a, &dir_b);
        assert_eq!(b.all().len(), 2);
    }

    #[test]
    fn a_device_is_removed_by_what_people_call_it() {
        let (r, _) = roster("name");
        r.note(A, "Steam Deck", "", true);
        r.note(B, "Beam Pro", "", true);
        assert_eq!(r.remove("steam deck").unwrap().fp, A, "its name, in any case");
        assert_eq!(r.remove("beam").unwrap().fp, B, "part of it");
    }

    #[test]
    fn a_name_that_could_be_two_devices_removes_neither() {
        let (r, _) = roster("two");
        r.note(A, "Paulus's Deck", "", true);
        r.note(B, "Office Deck", "", true);
        let why = r.remove("deck").unwrap_err();
        assert!(why.contains("Paulus's Deck") && why.contains("Office Deck"), "got {why}");
        assert_eq!(r.all().iter().filter(|e| !e.removed()).count(), 2);
        // The whole name is not ambiguous, though a shorter name is inside a longer.
        r.note(A, "Deck", "", true);
        assert_eq!(r.remove("deck").unwrap().fp, A);
    }

    #[test]
    fn one_with_no_name_goes_by_its_fingerprint_and_what_is_not_there_says_what_is() {
        let (r, _) = roster("unnamed");
        r.note(A, "", "", false);
        r.note(B, "Beam Pro", "", true);
        assert!(r.remove("aa").is_err(), "too little of a fingerprint");
        let why = r.remove("toaster").unwrap_err();
        assert!(why.contains("Beam Pro") && why.contains("unnamed aa000000"), "got {why}");
        assert_eq!(r.remove("aa00").unwrap().fp, A);
    }
}
