//! Installing an extension in developer mode, and the list of what a profile has installed.
//!
//! A folder is loaded where it is, as Chrome's "Load unpacked" does: changing its files and
//! reloading the extension is how one is developed. A `.zip`, `.crx` or `.xpi` is unpacked into
//! the profile's own folder on this machine. Either way the list is this machine's
//! (`local/spatimesh/profiles/<profile>/extensions.json`): a folder on one machine is nothing
//! on another.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::manifest::Manifest;
use super::{id_of_bytes, id_of_key, id_of_path, zip};
use crate::store::Store;

/// Where an installed extension came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum From {
    /// A folder, loaded where it is.
    Folder,
    /// A `.zip`, `.crx` or `.xpi`, unpacked here.
    Package,
}

/// Whose extension it is: which namespace and URLs it expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Flavor {
    /// `chrome` only, and `chrome-extension://` URLs.
    Chrome,
    /// `browser` (with promises) and `chrome` both, and `moz-extension://` URLs.
    Firefox,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    pub id: String,
    /// Its files.
    pub root: PathBuf,
    pub from: From,
    pub flavor: Flavor,
    pub enabled: bool,
    /// Milliseconds since 1970.
    pub installed: u64,
    /// Whether it runs in the private profile too (off, as in Chrome and Firefox).
    #[serde(default)]
    pub in_private: bool,
}

fn list_file(profile: &str) -> String {
    format!("profiles/{profile}/extensions.json")
}

/// What `profile` has installed.
pub fn list(store: &Store, profile: &str) -> Vec<Installed> {
    store.read_local::<Vec<Installed>>(&list_file(profile)).unwrap_or_default()
}

fn save(store: &Store, profile: &str, all: &[Installed]) -> Result<(), String> {
    store.write_local(&list_file(profile), &all.to_vec())
}

/// Where a package's files are unpacked for `profile`.
pub fn unpacked_dir(store: &Store, profile: &str, id: &str) -> PathBuf {
    store.local_dir(&format!("profiles/{profile}/extensions/{id}"))
}

fn flavor_of(manifest: &Manifest, from_xpi: bool) -> Flavor {
    // An .xpi, or a manifest that says only Firefox's things: Firefox's. Anything with
    // Chrome's own marks (a key, an update_url, a service worker with no scripts beside it) is
    // Chrome's, which is also what an extension made for both works with.
    let chrome_marks = manifest.key.is_some()
        || manifest.raw.get("update_url").is_some()
        || manifest.raw.get("minimum_chrome_version").is_some()
        || matches!(manifest.background, Some(super::manifest::Background::ServiceWorker { .. }));
    if from_xpi || (manifest.gecko_id.is_some() && !chrome_marks) {
        Flavor::Firefox
    } else {
        Flavor::Chrome
    }
}

/// The id an extension has: Chrome's from its key, a .crx's own, Firefox's add-on id made into
/// Chrome's form, or else from where it is.
fn id_for(manifest: &Manifest, crx_id: Option<String>, origin: &Path) -> String {
    if let Some(key) = &manifest.key {
        use base64::Engine as _;
        if let Ok(der) = base64::engine::general_purpose::STANDARD.decode(key.trim()) {
            return id_of_key(&der);
        }
    }
    if let Some(id) = crx_id {
        return id;
    }
    if let Some(gecko) = &manifest.gecko_id {
        use sha2::Digest as _;
        let hash = sha2::Sha256::digest(gecko.as_bytes());
        return id_of_bytes(&hash[..16]);
    }
    id_of_path(origin)
}

/// A path inside an archive that stays inside the folder it is unpacked into.
fn safe_relative(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    let parts: Vec<_> = path.components().collect();
    (!parts.is_empty() && parts.iter().all(|c| matches!(c, Component::Normal(_)))).then(|| path.to_path_buf())
}

/// Install, or install again (an update, or a folder reloaded), the extension at `source`.
pub fn install(store: &Store, profile: &str, source: &Path) -> Result<(Installed, Manifest), String> {
    let source = std::fs::canonicalize(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let (installed, manifest) = if source.is_dir() {
        let text = std::fs::read_to_string(source.join("manifest.json")).map_err(|_| format!("{} has no manifest.json", source.display()))?;
        let manifest = Manifest::parse(&text)?;
        let id = id_for(&manifest, None, &source);
        let flavor = flavor_of(&manifest, false);
        (Installed { id, root: source.clone(), from: From::Folder, flavor, enabled: true, installed: crate::store::now_ms(), in_private: false }, manifest)
    } else {
        let bytes = std::fs::read(&source).map_err(|e| format!("{}: {e}", source.display()))?;
        let (archive, crx_id) = if bytes.starts_with(b"Cr24") { zip::crx(&bytes)? } else { (&bytes[..], None) };
        let entries = zip::read(archive)?;
        // Some packages have everything in one folder; the manifest says where the top is.
        let top = entries
            .iter()
            .filter_map(|e| e.name.strip_suffix("manifest.json"))
            .filter(|prefix| prefix.is_empty() || prefix.ends_with('/'))
            .min_by_key(|p| p.len())
            .ok_or("the package has no manifest.json")?
            .to_string();
        let manifest_entry = entries.iter().find(|e| e.name == format!("{top}manifest.json")).ok_or("the package has no manifest.json")?;
        let manifest = Manifest::parse(&String::from_utf8_lossy(&manifest_entry.bytes))?;
        let from_xpi = source.extension().is_some_and(|e| e.eq_ignore_ascii_case("xpi"));
        let id = id_for(&manifest, crx_id, &source);
        let flavor = flavor_of(&manifest, from_xpi);
        let dir = unpacked_dir(store, profile, &id);
        let fresh = dir.with_extension(format!("new{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&fresh);
        for entry in &entries {
            let Some(name) = entry.name.strip_prefix(&top) else { continue };
            let Some(relative) = safe_relative(name) else {
                log::warn!("{}: {} is outside the extension, left out", source.display(), entry.name);
                continue;
            };
            let path = fresh.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            std::fs::write(&path, &entry.bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::rename(&fresh, &dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        (Installed { id, root: dir, from: From::Package, flavor, enabled: true, installed: crate::store::now_ms(), in_private: false }, manifest)
    };
    let mut all = list(store, profile);
    match all.iter_mut().find(|i| i.id == installed.id) {
        // Again: the same id keeps whether it was turned off or let into the private profile.
        Some(old) => {
            let (enabled, in_private) = (old.enabled, old.in_private);
            *old = Installed { enabled, in_private, ..installed.clone() };
        }
        None => all.push(installed.clone()),
    }
    save(store, profile, &all)?;
    let kept = all.into_iter().find(|i| i.id == installed.id).unwrap_or(installed);
    Ok((kept, manifest))
}

/// An extension gone from `profile`, its unpacked files with it (never a folder it was loaded
/// from) and what it stored.
pub fn uninstall(store: &Store, profile: &str, id: &str) -> Result<(), String> {
    let mut all = list(store, profile);
    let Some(at) = all.iter().position(|i| i.id == id) else { return Err("no such extension".into()) };
    let gone = all.remove(at);
    save(store, profile, &all)?;
    if gone.from == From::Package {
        let _ = std::fs::remove_dir_all(&gone.root);
    }
    super::storage::forget(store, profile, id);
    Ok(())
}

/// Turned on or off, or let into the private profile or not.
pub fn set(store: &Store, profile: &str, id: &str, enabled: Option<bool>, in_private: Option<bool>) -> Result<(), String> {
    let mut all = list(store, profile);
    let entry = all.iter_mut().find(|i| i.id == id).ok_or("no such extension")?;
    if let Some(e) = enabled {
        entry.enabled = e;
    }
    if let Some(p) = in_private {
        entry.in_private = p;
    }
    save(store, profile, &all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Scratch;

    const V3: &str = r#"{"manifest_version":3,"name":"Tidy","version":"1","background":{"service_worker":"sw.js"}}"#;
    const V2: &str = r#"{"manifest_version":2,"name":"Fox","version":"1","background":{"scripts":["bg.js"]},
                         "browser_specific_settings":{"gecko":{"id":"fox@example.org"}}}"#;

    #[test]
    fn a_folder_is_loaded_where_it_is() {
        let scratch = Scratch::new("ext-folder");
        let store = scratch.store();
        let folder = scratch.0.join("tidy");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("manifest.json"), V3).unwrap();
        let (installed, manifest) = install(&store, "default", &folder).unwrap();
        assert_eq!(manifest.name, "Tidy");
        assert_eq!(installed.from, From::Folder);
        assert_eq!(installed.flavor, Flavor::Chrome);
        assert_eq!(installed.root, std::fs::canonicalize(&folder).unwrap());
        assert_eq!(installed.id.len(), 32);
        assert!(installed.id.chars().all(|c| ('a'..='p').contains(&c)));
        // Loaded again: the same one, still where it was turned off.
        set(&store, "default", &installed.id, Some(false), None).unwrap();
        let (again, _) = install(&store, "default", &folder).unwrap();
        assert_eq!(again.id, installed.id);
        assert!(!again.enabled);
        assert_eq!(list(&store, "default").len(), 1);
        uninstall(&store, "default", &installed.id).unwrap();
        assert!(folder.join("manifest.json").exists(), "a folder loaded is never deleted");
        assert!(list(&store, "default").is_empty());
    }

    #[test]
    fn an_xpi_is_unpacked_and_firefoxs() {
        let scratch = Scratch::new("ext-xpi");
        let store = scratch.store();
        let file = scratch.0.join("fox.xpi");
        std::fs::write(&file, zip::tests::make(&[("manifest.json", V2.as_bytes()), ("bg.js", b"1"), ("../escape.js", b"no")])).unwrap();
        let (installed, manifest) = install(&store, "default", &file).unwrap();
        assert_eq!(manifest.gecko_id.as_deref(), Some("fox@example.org"));
        assert_eq!(installed.flavor, Flavor::Firefox);
        assert_eq!(installed.from, From::Package);
        assert!(installed.root.join("bg.js").exists());
        assert!(!installed.root.parent().unwrap().join("escape.js").exists(), "nothing lands outside its folder");
        // Its id follows its add-on id, wherever the file was.
        let elsewhere = scratch.0.join("copy.xpi");
        std::fs::copy(&file, &elsewhere).unwrap();
        assert_eq!(install(&store, "default", &elsewhere).unwrap().0.id, installed.id);
        uninstall(&store, "default", &installed.id).unwrap();
        assert!(!installed.root.exists(), "an unpacked package goes with it");
    }

    #[test]
    fn a_zip_with_everything_in_one_folder() {
        let scratch = Scratch::new("ext-zip");
        let store = scratch.store();
        let file = scratch.0.join("tidy.zip");
        std::fs::write(&file, zip::tests::make(&[("tidy-1.0/manifest.json", V3.as_bytes()), ("tidy-1.0/sw.js", b"x")])).unwrap();
        let (installed, _) = install(&store, "default", &file).unwrap();
        assert!(installed.root.join("sw.js").exists());
        assert!(install(&store, "default", &scratch.0.join("missing.zip")).is_err());
    }
}
