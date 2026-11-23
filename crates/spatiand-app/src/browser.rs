//! SpatiMesh, for the applications beside it: its profiles, as its shared files name them, and
//! a link opened in one (`App::open_link`).
//!
//! An application that wants the wearer signed in somewhere through a browser -- SpatiReel to a
//! Vibrefy server -- does not know whose browser that is: SpatiMesh has a profile for each of
//! the people who use it, each with its own logins. So it asks, every time, and hands the link
//! to the profile chosen. Only what a profile is called, its colour and whether it has a lock
//! are read here; nothing a profile holds is, and a lock is never given anywhere but in
//! SpatiMesh's own window for it.

use std::path::Path;

/// One of SpatiMesh's profiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// Its folder's name in the shared data: what `App::open_link` is given.
    pub id: String,
    pub name: String,
    pub colour: [u8; 3],
    /// It has a PIN or password. A link is not opened in it from outside SpatiMesh.
    pub locked: bool,
}

/// SpatiMesh's profiles, in the order its own menu has them. None where SpatiMesh has never
/// run with Spatiand's data.
pub fn profiles() -> Vec<Profile> {
    profiles_in(&crate::config::shared().join("spatimesh/profiles"))
}

/// As `profiles_sync` reads them (third_party/chromium, patch 0009): every folder with a
/// `profile.json`, but the hidden ones and `private`.
pub fn profiles_in(folder: &Path) -> Vec<Profile> {
    let mut found: Vec<(i64, f64, Profile)> = Vec::new();
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let id = entry.file_name().to_string_lossy().into_owned();
        if id.starts_with('.') || id == "private" || !entry.path().is_dir() {
            continue;
        }
        let Some(file) = std::fs::read(entry.path().join("profile.json")).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) else {
            continue;
        };
        let Some(dict) = file.as_object() else { continue };
        let name = dict.get("name").and_then(|n| n.as_str()).filter(|n| !n.trim().is_empty()).unwrap_or(&id).to_string();
        let mut colour = [128u8; 3];
        if let Some(c) = dict.get("colour").and_then(|c| c.as_array()).filter(|c| c.len() == 3) {
            for (to, from) in colour.iter_mut().zip(c) {
                *to = from.as_u64().unwrap_or(128).min(255) as u8;
            }
        }
        let locked = dict.get("lock").and_then(|l| l.as_object()).is_some_and(|l| ["kind", "salt", "hash"].iter().all(|k| l.get(*k).is_some_and(|v| v.is_string())));
        // With a place of its own first, in that order; then as they were made, `default` first.
        let position = dict.get("position").and_then(|p| p.as_i64()).unwrap_or(i64::MAX);
        let created = if id == "default" { -1.0 } else { dict.get("created").and_then(|c| c.as_f64()).unwrap_or(0.0) };
        found.push((position, created, Profile { id, name, colour, locked }));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then_with(|| a.2.name.cmp(&b.2.name)));
    found.into_iter().map(|(_, _, profile)| profile).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(folder: &Path, id: &str, json: &str) {
        let dir = folder.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("profile.json"), json).unwrap();
    }

    #[test]
    fn profiles_are_read_as_spatimesh_reads_them() {
        let folder = std::env::temp_dir().join(format!("spatiand-browser-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        write(&folder, "default", r#"{"name":"Ada","colour":[236,206,60],"created":0}"#);
        write(&folder, "bb22", r#"{"name":"Cy","colour":[1,2,3],"created":200,"lock":{"kind":"pin","salt":"s","hash":"h"}}"#);
        write(&folder, "aa11", r#"{"name":"Bo","colour":[9,9,9],"created":100,"lock":{"kind":"pin"}}"#);
        write(&folder, "zz99", r#"{"name":"First","created":900,"position":0}"#);
        write(&folder, "private", r#"{"name":"no"}"#);
        write(&folder, ".hidden", r#"{"name":"no"}"#);
        std::fs::create_dir_all(folder.join("empty")).unwrap();
        let got = profiles_in(&folder);
        let _ = std::fs::remove_dir_all(&folder);
        assert_eq!(got.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["First", "Ada", "Bo", "Cy"]);
        assert_eq!(got.iter().map(|p| p.locked).collect::<Vec<_>>(), [false, false, false, true], "half a lock is no lock, as in SpatiMesh");
        assert_eq!(got[1].colour, [236, 206, 60]);
        assert_eq!(got[0].colour, [128, 128, 128]);
        assert_eq!(got[3].id, "bb22");
    }
}
