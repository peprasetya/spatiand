//! What is open, kept: a profile's windows written down as they are (`spatimesh::session`) and
//! opened again when the profile next opens; and the tabs and windows closed lately, for
//! Control-Shift-T to bring back.

use std::time::{Duration, Instant};

use spatimesh::engine::TabId;
use spatimesh::profiles::PRIVATE;
use spatimesh::session::{self, SavedTab, SavedWindow, Session};
use spatimesh::tabs::{Place, WindowKey};

use super::ext::Told;
use super::{pages, Browser};

/// How often what is open is written down while it changes.
const EVERY: Duration = Duration::from_secs(10);
/// How many closed tabs and windows are kept to bring back.
const KEPT: usize = 25;

/// A tab or a window closed, as it was.
pub(super) enum Closed {
    Tab { profile: String, tab: SavedTab, window: WindowKey, index: usize },
    Window { profile: String, window: SavedWindow },
}

impl Closed {
    fn profile(&self) -> &str {
        match self {
            Closed::Tab { profile, .. } | Closed::Window { profile, .. } => profile,
        }
    }

    /// What its line in a menu says.
    pub(super) fn label(&self) -> String {
        match self {
            Closed::Tab { tab, .. } => if tab.title.trim().is_empty() { tab.url.clone() } else { tab.title.clone() },
            Closed::Window { window, .. } => match window.tabs.len() {
                1 => "A window with 1 tab".to_string(),
                n => format!("A window with {n} tabs"),
            },
        }
    }
}

/// A tab as it is kept: one of SpatiMesh's own pages as its address, a page as its address.
fn kept_tab(t: &spatimesh::tabs::Tab) -> SavedTab {
    SavedTab { url: t.url.clone(), title: t.title.clone(), pinned: t.pinned, muted: t.muted }
}

impl Browser {
    pub(super) fn saved_window(&self, key: WindowKey) -> SavedWindow {
        let Some(window) = self.tabs.window(key) else { return SavedWindow::default() };
        // The developer tools are not kept: they are opened again from their page.
        let tabs: Vec<&spatimesh::tabs::Tab> = window.tabs.iter().filter_map(|&t| self.tabs.tab(t)).filter(|t| t.inspects.is_none()).collect();
        let active = window.active_tab().and_then(|a| tabs.iter().position(|t| t.id == a)).unwrap_or(0);
        SavedWindow { tabs: tabs.into_iter().map(kept_tab).collect(), active }
    }

    /// Every profile's open windows written down, if anything changed: so that a SpatiMesh that
    /// stopped opens them again.
    pub(super) fn session_tick(&mut self) {
        if !self.session_dirty || self.session_saved.elapsed() < EVERY {
            return;
        }
        self.session_dirty = false;
        self.session_saved = Instant::now();
        let mut profiles: Vec<String> = self.tabs.windows().iter().map(|w| w.profile.clone()).filter(|p| p != PRIVATE).collect();
        profiles.sort();
        profiles.dedup();
        for profile in profiles {
            let keys: Vec<WindowKey> = self.tabs.windows().iter().filter(|w| w.profile == profile).map(|w| w.key).collect();
            // A locked profile's windows are kept as they were before it was locked.
            let session = Session { windows: keys.into_iter().map(|k| self.saved_window(k)).collect() };
            session::save(&self.store, &profile, &session);
        }
    }

    /// A window about to close: kept to bring back, and, the last of its profile's, written
    /// down as what the profile opens next time.
    pub(super) fn remember_closing_window(&mut self, key: WindowKey) {
        let Some(profile) = self.tabs.window(key).map(|w| w.profile.clone()) else { return };
        let window = self.saved_window(key);
        let last = self.tabs.windows().iter().filter(|w| w.profile == profile).count() <= 1;
        if last && profile != PRIVATE {
            session::save(&self.store, &profile, &Session { windows: vec![window.clone()] });
        }
        if !window.tabs.is_empty() && profile != PRIVATE {
            self.closed.push(Closed::Window { profile, window });
            self.trim_closed();
        }
    }

    /// A tab about to close, kept to bring back: and if it is its window's last, as the window.
    pub(super) fn remember_closing_tab(&mut self, tab: TabId) {
        let Some(t) = self.tabs.tab(tab).filter(|t| t.inspects.is_none()).cloned() else { return };
        let Some(key) = self.tabs.window_of(tab) else { return };
        let alone = self.tabs.window(key).is_some_and(|w| w.tabs.len() == 1);
        if alone {
            self.remember_closing_window(key);
            return;
        }
        if t.profile == PRIVATE || super::is_start(&t.url) {
            return;
        }
        let index = self.tabs.window(key).and_then(|w| w.tabs.iter().position(|&x| x == tab)).unwrap_or(0);
        self.closed.push(Closed::Tab { profile: t.profile.clone(), tab: kept_tab(&t), window: key, index });
        self.trim_closed();
        self.session_dirty = true;
    }

    fn trim_closed(&mut self) {
        let over = self.closed.len().saturating_sub(KEPT);
        self.closed.drain(..over);
    }

    /// The closed tabs and windows of a profile, newest first, for the History menu.
    pub(super) fn closed_of(&self, profile: &str) -> Vec<(usize, String, bool)> {
        self.closed.iter().enumerate().rev().filter(|(_, c)| c.profile() == profile).map(|(i, c)| (i, c.label(), matches!(c, Closed::Window { .. }))).collect()
    }

    /// The newest closed tab or window of a window's profile, back; or the one numbered `which`.
    pub(super) fn reopen_closed(&mut self, w: usize, which: Option<usize>) {
        let key = self.wins[w].key;
        let Some(profile) = self.tabs.window(key).map(|x| x.profile.clone()) else { return };
        let index = which.or_else(|| self.closed.iter().rposition(|c| c.profile() == profile));
        let Some(index) = index.filter(|&i| i < self.closed.len()) else {
            self.say("No tab was closed lately".into());
            return;
        };
        match self.closed.remove(index) {
            Closed::Tab { tab, window, index, .. } => {
                let into = if self.tabs.window(window).is_some() { window } else { key };
                if let Some(id) = self.tabs.new_tab(into, &tab.url, Place::End, true, None) {
                    if let Some(t) = self.tabs.tab_mut(id) {
                        t.title = tab.title;
                        t.pinned = tab.pinned;
                    }
                    // Where it was on its bar, if that is still there.
                    let at = self.tabs.window(into).and_then(|x| x.tabs.iter().position(|&t| t == id)).unwrap_or(0);
                    self.tabs.reorder(into, at, index);
                    self.tabs.activate_tab(id);
                    self.open_in_engine(id);
                    self.told.push(Told::TabCreated(id));
                    self.after_tab_change(into);
                }
            }
            Closed::Window { profile, window } => {
                self.restore_session(&profile, Session { windows: vec![window] });
            }
        }
    }

    /// Windows as they were kept: each tab on its bar, only the one in front loaded now (the
    /// others when they are first shown). The first window's key.
    pub(super) fn restore_session(&mut self, profile: &str, saved: Session) -> WindowKey {
        let mut first = None;
        for window in saved.windows {
            let key = self.open_window(profile);
            first.get_or_insert(key);
            for (i, tab) in window.tabs.iter().enumerate() {
                let Some(id) = self.tabs.new_tab(key, &tab.url, Place::End, i == window.active, None) else { continue };
                if let Some(t) = self.tabs.tab_mut(id) {
                    t.title = tab.title.clone();
                    t.muted = tab.muted;
                    t.loading = false;
                }
                if tab.pinned {
                    self.tabs.set_pinned(id, true);
                }
                self.told.push(Told::TabCreated(id));
            }
            if self.tabs.window(key).is_some_and(|w| w.tabs.is_empty()) {
                if let Some(id) = self.tabs.new_tab(key, "about:blank", Place::End, true, None) {
                    self.told.push(Told::TabCreated(id));
                }
            }
            // The one in front is opened by `after_tab_change`; a start page or one of
            // SpatiMesh's own pages has its blank page in the engine too.
            if let Some(active) = self.tabs.active(key) {
                if self.tabs.tab(active).is_some_and(|t| super::is_start(&t.url) || pages::internal(&t.url).is_some()) {
                    self.open_in_engine(active);
                }
            }
            self.after_tab_change(key);
            if let Some(w) = self.win_of_key(key) {
                if self.wins[w].lock.is_some() {
                    self.hush_window(w);
                }
            }
        }
        first.unwrap_or_else(|| self.open_window(profile))
    }
}
