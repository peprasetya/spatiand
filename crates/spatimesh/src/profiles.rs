//! Profiles: whose browsing this is. Each has its own bookmarks and its own website data
//! (cookies, logins, storage), and its pages run in an engine host of their own. The private
//! profile is not one of them: it is never written down, keeps nothing, and every private
//! window shares one host that ends when the last of them closes.

use serde::{Deserialize, Serialize};

use crate::store::{new_id, now_ms, Store};

/// The private profile's id: never a folder, never a file.
pub const PRIVATE: &str = "private";
/// The first profile's id, the same on every machine, so that two machines starting from
/// nothing have one profile between them and not two of the same name.
pub const FIRST: &str = "default";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    #[serde(skip)]
    pub id: String,
    pub name: String,
    /// The colour of its windows' bar, to tell them apart.
    pub colour: [u8; 3],
    #[serde(default)]
    pub created: u64,
    /// What opens its windows when none is open, or after the lock button: a PIN or a
    /// password, kept only as a salted hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock: Option<Lock>,
}

/// A profile's PIN or password, as it is kept: never the thing itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lock {
    pub kind: LockKind,
    salt: String,
    hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LockKind {
    Pin,
    Password,
}

impl LockKind {
    pub fn name(self) -> &'static str {
        match self {
            LockKind::Pin => "PIN",
            LockKind::Password => "password",
        }
    }

    /// Why `secret` will not do, if it will not: a PIN is 4 to 12 digits, a password at least
    /// 6 characters.
    pub fn refuses(self, secret: &str) -> Option<&'static str> {
        match self {
            LockKind::Pin if !(4..=12).contains(&secret.len()) || !secret.bytes().all(|b| b.is_ascii_digit()) => Some("A PIN is 4 to 12 digits"),
            LockKind::Password if secret.chars().count() < 6 => Some("A password is at least 6 characters"),
            _ => None,
        }
    }
}

/// Many rounds of SHA-256, so that a copy of the profile's file (it is synced) does not give a
/// short PIN up in a moment.
fn hash(salt: &str, secret: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::digest(format!("{salt}\u{0}{secret}").as_bytes());
    for _ in 0..100_000 {
        digest = Sha256::digest(digest);
    }
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

impl Lock {
    pub fn new(kind: LockKind, secret: &str) -> Lock {
        let salt = new_id();
        Lock { kind, hash: hash(&salt, secret), salt }
    }

    pub fn opens(&self, secret: &str) -> bool {
        hash(&self.salt, secret) == self.hash
    }
}

impl Profile {
    pub fn is_private(&self) -> bool {
        self.id == PRIVATE
    }
}

/// The colours a new profile is given in turn.
const COLOURS: &[[u8; 3]] = &[[55, 166, 247], [22, 230, 192], [143, 107, 242], [242, 160, 61], [235, 87, 120], [120, 200, 90]];

pub fn private() -> Profile {
    Profile { id: PRIVATE.to_string(), name: "Private".to_string(), colour: [96, 96, 112], created: 0, lock: None }
}

fn file(id: &str) -> String {
    format!("profiles/{id}/profile.json")
}

/// Every profile on this machine (which is every profile on every machine it syncs with), the
/// first one made first. There is always at least one.
pub fn list(store: &Store) -> Vec<Profile> {
    let mut found: Vec<Profile> = folders(store)
        .into_iter()
        .filter(|id| id != PRIVATE)
        .filter_map(|id| store.read::<Profile>(&file(&id)).map(|p| Profile { id, ..p }))
        .collect();
    if found.is_empty() {
        let first = Profile { id: FIRST.to_string(), name: "Personal".to_string(), colour: COLOURS[0], created: 0, lock: None };
        // Every machine makes this one on its own: another's, renamed perhaps, wins over it.
        if let Err(e) = store.write_default(&file(FIRST), &first) {
            log::warn!("the first profile could not be written: {e}");
        }
        found.push(first);
    }
    found.sort_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));
    found
}

fn folders(store: &Store) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(store.shared_dir().join("profiles")) else { return Vec::new() };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    ids.sort();
    ids
}

pub fn get(store: &Store, id: &str) -> Option<Profile> {
    if id == PRIVATE {
        return Some(private());
    }
    store.read::<Profile>(&file(id)).map(|p| Profile { id: id.to_string(), ..p })
}

pub fn add(store: &Store, name: &str) -> Result<Profile, String> {
    let count = list(store).len();
    let profile = Profile { id: new_id(), name: name.trim().to_string(), colour: COLOURS[count % COLOURS.len()], created: now_ms(), lock: None };
    if profile.name.is_empty() {
        return Err("a profile needs a name".into());
    }
    store.write(&file(&profile.id), &profile)?;
    Ok(profile)
}

/// The colours a profile's windows may be, by name: the first six are the ones new profiles are
/// given in turn.
pub const COLOUR_CHOICES: &[(&str, [u8; 3])] = &[
    ("Blue", [55, 166, 247]),
    ("Teal", [22, 230, 192]),
    ("Violet", [143, 107, 242]),
    ("Orange", [242, 160, 61]),
    ("Pink", [235, 87, 120]),
    ("Green", [120, 200, 90]),
    ("Red", [226, 64, 64]),
    ("Yellow", [236, 206, 60]),
    ("Grey", [150, 156, 168]),
];

/// The colour its windows are drawn in, from now on (on every machine).
pub fn set_colour(store: &Store, id: &str, colour: [u8; 3]) -> Result<(), String> {
    let mut profile = get(store, id).filter(|p| !p.is_private()).ok_or("no such profile")?;
    profile.colour = colour;
    store.write(&file(id), &profile)
}

pub fn rename(store: &Store, id: &str, name: &str) -> Result<(), String> {
    let mut profile = get(store, id).filter(|p| !p.is_private()).ok_or("no such profile")?;
    profile.name = name.trim().to_string();
    if profile.name.is_empty() {
        return Err("a profile needs a name".into());
    }
    store.write(&file(id), &profile)
}

/// Its windows ask for this PIN or password from now on (on every machine), or for nothing.
pub fn set_lock(store: &Store, id: &str, lock: Option<(LockKind, &str)>) -> Result<(), String> {
    let mut profile = get(store, id).filter(|p| !p.is_private()).ok_or("no such profile")?;
    profile.lock = match lock {
        Some((kind, secret)) => {
            if let Some(why) = kind.refuses(secret) {
                return Err(why.into());
            }
            Some(Lock::new(kind, secret))
        }
        None => None,
    };
    store.write(&file(id), &profile)
}

/// A profile gone, with its bookmarks (on every machine) and its website data and cache (on
/// this one). The last one cannot be removed.
pub fn remove(store: &Store, id: &str) -> Result<(), String> {
    if id == PRIVATE || get(store, id).is_none() {
        return Err("no such profile".into());
    }
    if list(store).len() <= 1 {
        return Err("the last profile cannot be removed".into());
    }
    store.remove(&format!("profiles/{id}"));
    let _ = std::fs::remove_dir_all(store.data_dir(id));
    let _ = std::fs::remove_dir_all(store.cache_dir(id));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Scratch;

    #[test]
    fn there_is_always_one() {
        let scratch = Scratch::new("profiles-first");
        let store = scratch.store();
        let all = list(&store);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, FIRST);
        assert_eq!(list(&store).len(), 1, "made once, not again");
        assert!(remove(&store, FIRST).is_err(), "the last cannot go");
    }

    #[test]
    fn add_rename_remove() {
        let scratch = Scratch::new("profiles");
        let store = scratch.store();
        list(&store);
        let work = add(&store, "Work").unwrap();
        assert_eq!(list(&store).iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["Personal", "Work"]);
        rename(&store, &work.id, "Office").unwrap();
        assert_eq!(get(&store, &work.id).unwrap().name, "Office");
        std::fs::create_dir_all(store.data_dir(&work.id)).unwrap();
        remove(&store, &work.id).unwrap();
        assert!(get(&store, &work.id).is_none());
        assert!(!store.data_dir(&work.id).exists(), "its website data went with it");
        assert!(add(&store, "  ").is_err());
    }

    #[test]
    fn a_lock_opens_to_its_own_secret_only() {
        let scratch = Scratch::new("profiles-lock");
        let store = scratch.store();
        list(&store);
        assert!(set_lock(&store, FIRST, Some((LockKind::Pin, "12a4"))).is_err(), "a PIN is digits");
        assert!(set_lock(&store, FIRST, Some((LockKind::Pin, "123"))).is_err(), "and at least four");
        set_lock(&store, FIRST, Some((LockKind::Pin, "2468"))).unwrap();
        let lock = get(&store, FIRST).unwrap().lock.unwrap();
        assert!(lock.opens("2468") && !lock.opens("2469"));
        let kept = std::fs::read_to_string(store.shared_dir().join(file(FIRST))).unwrap();
        assert!(!kept.contains("2468"), "the PIN itself is not written down");
        set_lock(&store, FIRST, None).unwrap();
        assert!(get(&store, FIRST).unwrap().lock.is_none());
    }

    #[test]
    fn private_is_never_written() {
        let scratch = Scratch::new("profiles-private");
        let store = scratch.store();
        assert!(get(&store, PRIVATE).unwrap().is_private());
        assert!(rename(&store, PRIVATE, "x").is_err());
        assert!(remove(&store, PRIVATE).is_err());
        assert!(!store.shared_dir().join("profiles").join(PRIVATE).exists());
    }
}
