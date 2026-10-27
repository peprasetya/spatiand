//! SpatiMesh's settings, the same on every machine: `apps/spatimesh/settings/<name>.json`.
//!
//! **One file per setting**, because the synchroniser keeps the newest version of a file: two
//! machines that change two different settings while apart both keep theirs. A setting is
//! written only when it changes, so that one left alone never overrides another machine's.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::address::{search_engines, SearchEngine};
use crate::store::Store;

fn file(name: &str) -> String {
    format!("settings/{name}.json")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The search engine's name, one of [`search_engines`]; or [`Settings::custom_search`].
    pub search: String,
    /// A search address of the wearer's own, `{}` where the words go.
    pub custom_search: Option<String>,
    /// What a new window opens.
    pub home: String,
    /// How large pages are drawn, 1.0 as they ask to be.
    pub zoom: f32,
    /// Extensions may be installed from a folder or a file.
    pub developer: bool,
    /// A profile opened when none of its windows is open opens the tabs it had open.
    pub restore: bool,
    /// The bookmarks bar, under the address bar.
    pub bookmarks_bar: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { search: search_engines()[0].name.clone(), custom_search: None, home: "about:blank".to_string(), zoom: 1.25, developer: false, restore: true, bookmarks_bar: false }
    }
}

/// One setting, read into `value` if it is there and readable.
fn read<T: DeserializeOwned>(store: &Store, name: &str, value: &mut T) {
    if let Some(stored) = store.read(&file(name)) {
        *value = stored;
    }
}

/// One setting, written if it is not what is stored.
fn write<T: Serialize + PartialEq>(store: &Store, name: &str, value: &T, stored: &T) -> Result<(), String> {
    if value == stored { Ok(()) } else { store.write(&file(name), value) }
}

impl Settings {
    pub fn load(store: &Store) -> Settings {
        let mut s = Settings::default();
        read(store, "search", &mut s.search);
        read(store, "custom_search", &mut s.custom_search);
        read(store, "home", &mut s.home);
        read(store, "zoom", &mut s.zoom);
        read(store, "developer", &mut s.developer);
        read(store, "restore", &mut s.restore);
        read(store, "bookmarks_bar", &mut s.bookmarks_bar);
        s
    }

    /// What changed since loaded, written; the rest left as it is, on this machine and others.
    pub fn save(&self, store: &Store) -> Result<(), String> {
        let stored = Settings::load(store);
        write(store, "search", &self.search, &stored.search)?;
        write(store, "custom_search", &self.custom_search, &stored.custom_search)?;
        write(store, "home", &self.home, &stored.home)?;
        write(store, "zoom", &self.zoom, &stored.zoom)?;
        write(store, "developer", &self.developer, &stored.developer)?;
        write(store, "restore", &self.restore, &stored.restore)?;
        write(store, "bookmarks_bar", &self.bookmarks_bar, &stored.bookmarks_bar)
    }

    pub fn search_engine(&self) -> SearchEngine {
        if let Some(template) = self.custom_search.as_ref().filter(|t| t.contains("{}") && t.starts_with("http")) {
            return SearchEngine::new("Custom", template);
        }
        search_engines().into_iter().find(|e| e.name == self.search).unwrap_or_else(|| search_engines().remove(0))
    }

    /// The page zoom, kept to what is sensible.
    pub fn zoom(&self) -> f32 {
        if self.zoom.is_finite() { self.zoom.clamp(0.5, 3.0) } else { 1.0 }
    }
}

/// A profile's own search engine, by name ("Custom" for its own address), if it chose one:
/// `profiles/<profile>/settings/search.json`, synchronised like the rest of the profile.
pub fn profile_search(store: &Store, profile: &str) -> Option<String> {
    store.read(&format!("profiles/{profile}/settings/search.json"))
}

/// A profile's own search address, `{}` where the words go.
pub fn profile_custom_search(store: &Store, profile: &str) -> Option<String> {
    store.read(&format!("profiles/{profile}/settings/custom_search.json"))
}

/// A profile's search engine from now on, on every machine: one of [`search_engines`]' names,
/// or "Custom" with its address.
pub fn set_profile_search(store: &Store, profile: &str, name: &str, custom: Option<&str>) -> Result<(), String> {
    if let Some(template) = custom {
        if !template.contains("{}") || !template.starts_with("http") {
            return Err("a search address starts with http and has {} where the words go".into());
        }
        store.write(&format!("profiles/{profile}/settings/custom_search.json"), &template)?;
    }
    if profile_search(store, profile).as_deref() != Some(name) {
        store.write(&format!("profiles/{profile}/settings/search.json"), &name)?;
    }
    Ok(())
}

impl Settings {
    /// The search engine of a profile's windows: its own choice, or SpatiMesh's.
    pub fn search_engine_for(&self, store: &Store, profile: &str) -> SearchEngine {
        match profile_search(store, profile).as_deref() {
            Some("Custom") => match profile_custom_search(store, profile).filter(|t| t.contains("{}") && t.starts_with("http")) {
                Some(template) => SearchEngine::new("Custom", &template),
                None => self.search_engine(),
            },
            Some(name) => search_engines().into_iter().find(|e| e.name == name).unwrap_or_else(|| self.search_engine()),
            None => self.search_engine(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Scratch;

    #[test]
    fn defaults_then_saved() {
        let scratch = Scratch::new("settings");
        let store = scratch.store();
        let mut settings = Settings::load(&store);
        assert_eq!(settings, Settings::default());
        assert_eq!(settings.search_engine().name, "DuckDuckGo");
        settings.search = "Google".into();
        settings.developer = true;
        settings.save(&store).unwrap();
        let again = Settings::load(&store);
        assert_eq!(again.search_engine().name, "Google");
        assert!(again.developer);
    }

    #[test]
    fn only_what_changed_is_written() {
        let scratch = Scratch::new("settings-each");
        let store = scratch.store();
        let mut settings = Settings::load(&store);
        settings.zoom = 1.5;
        settings.save(&store).unwrap();
        // The zoom alone: the search engine left alone is no file, which another machine's
        // choice of one then fills.
        assert!(store.shared_dir().join("settings/zoom.json").exists());
        assert!(!store.shared_dir().join("settings/search.json").exists());
        assert_eq!(Settings::load(&store).zoom, 1.5);
    }

    #[test]
    fn each_profile_its_own_search() {
        let scratch = Scratch::new("settings-profile");
        let store = scratch.store();
        let settings = Settings::load(&store);
        assert_eq!(settings.search_engine_for(&store, "work").name, "DuckDuckGo");
        set_profile_search(&store, "work", "Kagi", None).unwrap();
        assert_eq!(settings.search_engine_for(&store, "work").name, "Kagi");
        assert_eq!(settings.search_engine_for(&store, "home").name, "DuckDuckGo", "another profile keeps its own");
        assert!(set_profile_search(&store, "home", "Custom", Some("nowhere")).is_err());
        set_profile_search(&store, "home", "Custom", Some("https://s.example/?q={}")).unwrap();
        assert_eq!(settings.search_engine_for(&store, "home").url_for("a"), "https://s.example/?q=a");
    }

    #[test]
    fn unknown_settings_and_engines_are_survived() {
        let scratch = Scratch::new("settings-old");
        let store = scratch.store();
        std::fs::create_dir_all(store.shared_dir().join("settings")).unwrap();
        std::fs::write(store.shared_dir().join("settings/search.json"), br#""Nowhere""#).unwrap();
        std::fs::write(store.shared_dir().join("settings/from_a_newer_build.json"), br#"1"#).unwrap();
        std::fs::write(store.shared_dir().join("settings/zoom.json"), br#""not a number""#).unwrap();
        let settings = Settings::load(&store);
        assert_eq!(settings.zoom, Settings::default().zoom);
        assert_eq!(settings.search_engine().name, "DuckDuckGo");
        assert_eq!(settings.home, "about:blank");
        let custom = Settings { custom_search: Some("https://example.com/s?q={}".into()), ..Settings::default() };
        assert_eq!(custom.search_engine().url_for("a b"), "https://example.com/s?q=a+b");
    }
}
