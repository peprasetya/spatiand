//! `storage`: what an extension keeps, in three places.
//!
//! * `local`: this machine's, one file per extension (`local/spatimesh/profiles/<p>/
//!   extension-storage/<id>.json`).
//! * `sync`: every machine's -- the shared folder spatiand-sync keeps the same everywhere, one
//!   file per key (`profiles/<p>/extension-sync/<id>/<key in hex>.json`), so that two machines
//!   setting two keys while apart both keep theirs, as with bookmarks.
//! * `session`: memory only, gone when SpatiMesh is (held by the runtime, [`Session`]).

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::store::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Area {
    Local,
    Sync,
    Session,
    /// What an administrator set: nothing, here.
    Managed,
}

impl Area {
    pub fn parse(name: &str) -> Option<Area> {
        Some(match name {
            "local" => Area::Local,
            "sync" => Area::Sync,
            "session" => Area::Session,
            "managed" => Area::Managed,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Area::Local => "local",
            Area::Sync => "sync",
            Area::Session => "session",
            Area::Managed => "managed",
        }
    }
}

/// Every extension's `session` storage, by profile and extension.
#[derive(Default)]
pub struct Session(HashMap<(String, String), Map<String, Value>>);

fn local_file(profile: &str, id: &str) -> String {
    format!("profiles/{profile}/extension-storage/{id}.json")
}

fn sync_folder(profile: &str, id: &str) -> String {
    format!("profiles/{profile}/extension-sync/{id}")
}

fn hex(key: &str) -> String {
    key.bytes().map(|b| format!("{b:02x}")).collect()
}

/// One synced key's file: the key itself is kept in it, as its name is only its hex.
#[derive(serde::Serialize, serde::Deserialize)]
struct SyncedItem {
    key: String,
    value: Value,
}

/// The whole of one area.
pub fn all(store: &Store, session: &Session, profile: &str, id: &str, area: Area) -> Map<String, Value> {
    match area {
        Area::Local => store.read_local::<Map<String, Value>>(&local_file(profile, id)).unwrap_or_default(),
        Area::Sync => {
            let folder = sync_folder(profile, id);
            store
                .list(&folder)
                .into_iter()
                .filter_map(|name| store.read::<SyncedItem>(&format!("{folder}/{name}.json")))
                .map(|item| (item.key, item.value))
                .collect()
        }
        Area::Session => session.0.get(&(profile.to_string(), id.to_string())).cloned().unwrap_or_default(),
        Area::Managed => Map::new(),
    }
}

/// `get(keys)`: null for everything, a key, a list of keys, or an object of keys with defaults.
pub fn get(store: &Store, session: &Session, profile: &str, id: &str, area: Area, keys: &Value) -> Map<String, Value> {
    let everything = all(store, session, profile, id, area);
    match keys {
        Value::Null => everything,
        Value::String(k) => everything.get(k).map(|v| Map::from_iter([(k.clone(), v.clone())])).unwrap_or_default(),
        Value::Array(ks) => ks.iter().filter_map(|k| k.as_str()).filter_map(|k| Some((k.to_string(), everything.get(k)?.clone()))).collect(),
        Value::Object(defaults) => defaults.iter().map(|(k, d)| (k.clone(), everything.get(k).cloned().unwrap_or_else(|| d.clone()))).collect(),
        _ => Map::new(),
    }
}

/// What changed, as `storage.onChanged` tells it: `{key: {oldValue, newValue}}`.
pub type Changes = Map<String, Value>;

fn change(old: Option<&Value>, new: Option<&Value>) -> Value {
    let mut c = Map::new();
    if let Some(o) = old {
        c.insert("oldValue".into(), o.clone());
    }
    if let Some(n) = new {
        c.insert("newValue".into(), n.clone());
    }
    Value::Object(c)
}

fn write(store: &Store, session: &mut Session, profile: &str, id: &str, area: Area, before: &Map<String, Value>, after: &Map<String, Value>) -> Result<(), String> {
    match area {
        Area::Local => store.write_local(&local_file(profile, id), after),
        Area::Session => {
            session.0.insert((profile.to_string(), id.to_string()), after.clone());
            Ok(())
        }
        Area::Sync => {
            let folder = sync_folder(profile, id);
            for (k, v) in after {
                if before.get(k) != Some(v) {
                    store.write(&format!("{folder}/{}.json", hex(k)), &SyncedItem { key: k.clone(), value: v.clone() })?;
                }
            }
            for k in before.keys().filter(|k| !after.contains_key(*k)) {
                store.remove(&format!("{folder}/{}.json", hex(k)));
            }
            Ok(())
        }
        Area::Managed => Err("managed storage is read-only".into()),
    }
}

/// `set(items)`.
pub fn set(store: &Store, session: &mut Session, profile: &str, id: &str, area: Area, items: &Map<String, Value>) -> Result<Changes, String> {
    let before = all(store, session, profile, id, area);
    let mut after = before.clone();
    let mut changes = Changes::new();
    for (k, v) in items {
        if before.get(k) != Some(v) {
            changes.insert(k.clone(), change(before.get(k), Some(v)));
        }
        after.insert(k.clone(), v.clone());
    }
    if area == Area::Sync {
        // Chrome's limits: what a synced store may hold.
        let size: usize = after.iter().map(|(k, v)| k.len() + v.to_string().len()).sum();
        if size > 102_400 || after.len() > 512 || items.iter().any(|(k, v)| k.len() + v.to_string().len() > 8192) {
            return Err("QUOTA_BYTES quota exceeded".into());
        }
    }
    write(store, session, profile, id, area, &before, &after)?;
    Ok(changes)
}

/// `remove(keys)`.
pub fn remove(store: &Store, session: &mut Session, profile: &str, id: &str, area: Area, keys: &Value) -> Result<Changes, String> {
    let names: Vec<String> = match keys {
        Value::String(k) => vec![k.clone()],
        Value::Array(ks) => ks.iter().filter_map(|k| k.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    };
    let before = all(store, session, profile, id, area);
    let mut after = before.clone();
    let mut changes = Changes::new();
    for k in names {
        if let Some(old) = after.remove(&k) {
            changes.insert(k, change(Some(&old), None));
        }
    }
    write(store, session, profile, id, area, &before, &after)?;
    Ok(changes)
}

/// `clear()`.
pub fn clear(store: &Store, session: &mut Session, profile: &str, id: &str, area: Area) -> Result<Changes, String> {
    let before = all(store, session, profile, id, area);
    let changes = before.iter().map(|(k, v)| (k.clone(), change(Some(v), None))).collect();
    write(store, session, profile, id, area, &before, &Map::new())?;
    Ok(changes)
}

/// `getBytesInUse(keys)`: what the keys take, as JSON.
pub fn bytes_in_use(store: &Store, session: &Session, profile: &str, id: &str, area: Area, keys: &Value) -> usize {
    let items = match keys {
        Value::Null => all(store, session, profile, id, area),
        other => get(store, session, profile, id, area, other),
    };
    items.iter().map(|(k, v)| k.len() + v.to_string().len()).sum()
}

/// What an uninstalled extension kept, gone.
pub fn forget(store: &Store, profile: &str, id: &str) {
    let _ = std::fs::remove_file(store.local_dir(&local_file(profile, id)));
    store.remove(&sync_folder(profile, id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Scratch;
    use serde_json::json;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn every_area_keeps_and_tells_what_changed() {
        let scratch = Scratch::new("ext-storage");
        let store = scratch.store();
        let mut session = Session::default();
        for area in [Area::Local, Area::Sync, Area::Session] {
            let changes = set(&store, &mut session, "default", "abc", area, &obj(json!({"a": 1, "b": {"deep": [1, 2]}}))).unwrap();
            assert_eq!(changes.get("a"), Some(&json!({"newValue": 1})), "{area:?}");
            let changes = set(&store, &mut session, "default", "abc", area, &obj(json!({"a": 2, "b": {"deep": [1, 2]}}))).unwrap();
            assert_eq!(Value::Object(changes), json!({"a": {"oldValue": 1, "newValue": 2}}), "only what changed: {area:?}");
            assert_eq!(Value::Object(get(&store, &session, "default", "abc", area, &json!("a"))), json!({"a": 2}));
            assert_eq!(Value::Object(get(&store, &session, "default", "abc", area, &json!({"a": 0, "z": "default"}))), json!({"a": 2, "z": "default"}));
            assert_eq!(get(&store, &session, "default", "abc", area, &Value::Null).len(), 2);
            assert!(bytes_in_use(&store, &session, "default", "abc", area, &Value::Null) > 0);
            let changes = remove(&store, &mut session, "default", "abc", area, &json!(["a", "missing"])).unwrap();
            assert_eq!(Value::Object(changes), json!({"a": {"oldValue": 2}}));
            clear(&store, &mut session, "default", "abc", area).unwrap();
            assert!(get(&store, &session, "default", "abc", area, &Value::Null).is_empty(), "{area:?}");
        }
        // Another extension's are its own.
        set(&store, &mut session, "default", "abc", Area::Local, &obj(json!({"mine": true}))).unwrap();
        assert!(get(&store, &session, "default", "xyz", Area::Local, &Value::Null).is_empty());
        // Synced keys are each a file of their own, under the shared folder.
        set(&store, &mut session, "default", "abc", Area::Sync, &obj(json!({"k/1": "v"}))).unwrap();
        assert!(store.shared_dir().join("profiles/default/extension-sync/abc").join(format!("{}.json", hex("k/1"))).exists());
        forget(&store, "default", "abc");
        assert!(get(&store, &session, "default", "abc", Area::Sync, &Value::Null).is_empty());
        assert!(get(&store, &session, "default", "abc", Area::Local, &Value::Null).is_empty());
    }

    #[test]
    fn synced_storage_has_chromes_limits() {
        let scratch = Scratch::new("ext-quota");
        let store = scratch.store();
        let mut session = Session::default();
        let big = "x".repeat(9000);
        assert!(set(&store, &mut session, "default", "abc", Area::Sync, &obj(json!({"big": big}))).is_err());
        assert!(set(&store, &mut session, "default", "abc", Area::Local, &obj(json!({"big": "x".repeat(9000)}))).is_ok());
        assert!(set(&store, &mut session, "default", "abc", Area::Managed, &obj(json!({"a": 1}))).is_err());
    }
}
