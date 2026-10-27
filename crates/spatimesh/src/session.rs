//! What a profile's windows had open, kept on this machine so that they open again as they
//! were: `local/spatimesh/profiles/<profile>/session.json`.
//!
//! Written while windows are open (so that a SpatiMesh that stopped leaves its tabs behind it),
//! and when a profile's last window closes, as that window was: opening the profile again
//! opens what was there. Not synchronised -- what was open is this machine's -- and never for
//! the private profile, which keeps nothing.

use serde::{Deserialize, Serialize};

use crate::profiles::PRIVATE;
use crate::store::Store;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedTab {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SavedWindow {
    pub tabs: Vec<SavedTab>,
    #[serde(default)]
    pub active: usize,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Session {
    pub windows: Vec<SavedWindow>,
}

impl Session {
    /// Whether there is anything worth opening again: a page, not only new tabs.
    pub fn worth_restoring(&self) -> bool {
        self.windows.iter().any(|w| w.tabs.iter().any(|t| !t.url.is_empty() && t.url != "about:blank"))
    }
}

fn file(profile: &str) -> String {
    format!("profiles/{profile}/session.json")
}

pub fn load(store: &Store, profile: &str) -> Option<Session> {
    if profile == PRIVATE {
        return None;
    }
    let session: Session = store.read_local(&file(profile))?;
    let windows: Vec<SavedWindow> = session
        .windows
        .into_iter()
        .filter(|w| !w.tabs.is_empty())
        .map(|w| SavedWindow { active: w.active.min(w.tabs.len() - 1), ..w })
        .collect();
    Some(Session { windows }).filter(Session::worth_restoring)
}

pub fn save(store: &Store, profile: &str, session: &Session) {
    if profile == PRIVATE {
        return;
    }
    if let Err(e) = store.write_local(&file(profile), session) {
        log::warn!("{profile}'s open tabs not kept: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Scratch;

    fn tab(url: &str) -> SavedTab {
        SavedTab { url: url.into(), title: String::new(), pinned: false, muted: false }
    }

    #[test]
    fn kept_and_opened_again() {
        let scratch = Scratch::new("session");
        let store = scratch.store();
        assert!(load(&store, "p").is_none());
        let session = Session { windows: vec![SavedWindow { tabs: vec![tab("https://a/"), tab("https://b/")], active: 9 }, SavedWindow::default()] };
        save(&store, "p", &session);
        let back = load(&store, "p").unwrap();
        assert_eq!(back.windows.len(), 1, "an empty window is not opened again");
        assert_eq!(back.windows[0].active, 1, "the tab in front is one there is");
        // Only new tabs: nothing to open again.
        save(&store, "p", &Session { windows: vec![SavedWindow { tabs: vec![tab("about:blank")], active: 0 }] });
        assert!(load(&store, "p").is_none());
        save(&store, PRIVATE, &session);
        assert!(load(&store, PRIVATE).is_none());
    }
}
