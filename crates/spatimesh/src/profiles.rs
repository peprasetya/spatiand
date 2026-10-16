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
}

impl Profile {
    pub fn is_private(&self) -> bool {
        self.id == PRIVATE
    }
}

/// The colours a new profile is given in turn.
const COLOURS: &[[u8; 3]] = &[[55, 166, 247], [22, 230, 192], [143, 107, 242], [242, 160, 61], [235, 87, 120], [120, 200, 90]];

pub fn private() -> Profile {
    Profile { id: PRIVATE.to_string(), name: "Private".to_string(), colour: [96, 96, 112], created: 0 }
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
        let first = Profile { id: FIRST.to_string(), name: "Personal".to_string(), colour: COLOURS[0], created: 0 };
        if let Err(e) = store.write(&file(FIRST), &first) {
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
    let profile = Profile { id: new_id(), name: name.trim().to_string(), colour: COLOURS[count % COLOURS.len()], created: now_ms() };
    if profile.name.is_empty() {
        return Err("a profile needs a name".into());
    }
    store.write(&file(&profile.id), &profile)?;
    Ok(profile)
}

pub fn rename(store: &Store, id: &str, name: &str) -> Result<(), String> {
    let mut profile = get(store, id).filter(|p| !p.is_private()).ok_or("no such profile")?;
    profile.name = name.trim().to_string();
    if profile.name.is_empty() {
        return Err("a profile needs a name".into());
    }
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
    fn private_is_never_written() {
        let scratch = Scratch::new("profiles-private");
        let store = scratch.store();
        assert!(get(&store, PRIVATE).unwrap().is_private());
        assert!(rename(&store, PRIVATE, "x").is_err());
        assert!(remove(&store, PRIVATE).is_err());
        assert!(!store.shared_dir().join("profiles").join(PRIVATE).exists());
    }
}
