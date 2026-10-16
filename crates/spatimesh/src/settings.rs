//! SpatiMesh's settings, the same on every machine: `apps/spatimesh/settings.json`.

use serde::{Deserialize, Serialize};

use crate::address::{search_engines, SearchEngine};
use crate::store::Store;

const FILE: &str = "settings.json";

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
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { search: search_engines()[0].name.clone(), custom_search: None, home: "about:blank".to_string(), zoom: 1.25, developer: false }
    }
}

impl Settings {
    pub fn load(store: &Store) -> Settings {
        store.read(FILE).unwrap_or_default()
    }

    pub fn save(&self, store: &Store) -> Result<(), String> {
        store.write(FILE, self)
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
    fn unknown_fields_and_engines_are_survived() {
        let scratch = Scratch::new("settings-old");
        let store = scratch.store();
        std::fs::create_dir_all(store.shared_dir()).unwrap();
        std::fs::write(store.shared_dir().join(FILE), br#"{"search":"Nowhere","from_a_newer_build":1}"#).unwrap();
        let settings = Settings::load(&store);
        assert_eq!(settings.search_engine().name, "DuckDuckGo");
        assert_eq!(settings.home, "about:blank");
        let custom = Settings { custom_search: Some("https://example.com/s?q={}".into()), ..Settings::default() };
        assert_eq!(custom.search_engine().url_for("a b"), "https://example.com/s?q=a+b");
    }
}
