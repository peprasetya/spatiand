//! Tabs and the windows they are in, with nothing of drawing or engines: which tab is where,
//! which is in front, and what happens when one is closed, dragged along the bar, torn off into
//! a window of its own or dropped onto another window.
//!
//! A window belongs to one profile, and so do its tabs: a tab goes only to a window of its own
//! profile (its page lives in that profile's engine host), so a tab dropped on another
//! profile's window opens a window of its own instead.

use std::collections::HashMap;

use crate::engine::TabId;

/// A window, by the model's own number for it (the window process pairs it with the room's).
pub type WindowKey = u32;

#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    pub id: TabId,
    pub profile: String,
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub progress: f32,
    pub can_back: bool,
    pub can_forward: bool,
    pub playing: bool,
    pub muted: bool,
    /// Kept at the start of its window's bar, small, and opened again with it.
    pub pinned: bool,
    /// What the page ran in stopped, and why.
    pub crashed: Option<String>,
    /// The page could not be loaded, and why.
    pub failed: Option<String>,
    /// The tab that opened it, while it is still where it was opened.
    pub opener: Option<TabId>,
    /// The developer tools of this tab: no page of its own, and not kept in the history.
    pub inspects: Option<TabId>,
}

impl Tab {
    /// What its place on the bar says.
    pub fn label(&self) -> String {
        if self.crashed.is_some() {
            return "Stopped".to_string();
        }
        if self.url.is_empty() || self.url == "about:blank" {
            return "New tab".to_string();
        }
        let title = self.title.trim();
        if !title.is_empty() {
            return title.to_string();
        }
        let url = self.url.as_str();
        url.split_once("://").map_or(url, |(_, rest)| rest).trim_end_matches('/').to_string()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub key: WindowKey,
    pub profile: String,
    pub tabs: Vec<TabId>,
    pub active: usize,
}

impl Window {
    pub fn active_tab(&self) -> Option<TabId> {
        self.tabs.get(self.active).copied()
    }
}

/// Where a new tab goes on its window's bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    End,
    /// After the tab in front, and after any it opened since: links opened in the background
    /// line up in the order they were opened.
    AfterActive,
}

/// What closing a tab did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Closed {
    pub window: WindowKey,
    /// It was the window's last tab, and the window is gone too.
    pub window_closed: bool,
    /// The tab now in front of the window, if it changed.
    pub now_active: Option<TabId>,
}

#[derive(Debug, Default)]
pub struct Tabs {
    windows: Vec<Window>,
    tabs: HashMap<TabId, Tab>,
    next_tab: TabId,
    next_window: WindowKey,
}

impl Tabs {
    pub fn new() -> Tabs {
        Tabs { next_tab: 1, next_window: 1, ..Default::default() }
    }

    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    pub fn window(&self, key: WindowKey) -> Option<&Window> {
        self.windows.iter().find(|w| w.key == key)
    }

    fn window_mut(&mut self, key: WindowKey) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.key == key)
    }

    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.get(&id)
    }

    pub fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.get_mut(&id)
    }

    pub fn tabs(&self) -> impl Iterator<Item = &Tab> {
        self.tabs.values()
    }

    pub fn window_of(&self, tab: TabId) -> Option<WindowKey> {
        self.windows.iter().find(|w| w.tabs.contains(&tab)).map(|w| w.key)
    }

    pub fn active(&self, window: WindowKey) -> Option<TabId> {
        self.window(window).and_then(Window::active_tab)
    }

    /// A window of `profile`, with no tabs yet.
    pub fn new_window(&mut self, profile: &str) -> WindowKey {
        let key = self.next_window;
        self.next_window += 1;
        self.windows.push(Window { key, profile: profile.to_string(), tabs: Vec::new(), active: 0 });
        key
    }

    /// A tab in `window` at `url`; in front if `front`.
    pub fn new_tab(&mut self, window: WindowKey, url: &str, place: Place, front: bool, opener: Option<TabId>) -> Option<TabId> {
        let id = self.next_tab;
        let profile = self.window(window)?.profile.clone();
        let index = {
            let w = self.window(window)?;
            match place {
                Place::End => w.tabs.len(),
                Place::AfterActive => {
                    // Past the tab in front and the run of tabs it opened.
                    let active = w.active_tab();
                    let mut i = (w.active + 1).min(w.tabs.len());
                    while i < w.tabs.len() && active.is_some() && self.tabs.get(&w.tabs[i]).and_then(|t| t.opener) == active {
                        i += 1;
                    }
                    i
                }
            }
        };
        self.next_tab += 1;
        self.tabs.insert(
            id,
            Tab {
                id,
                profile,
                url: url.to_string(),
                title: String::new(),
                loading: true,
                progress: 0.0,
                can_back: false,
                can_forward: false,
                playing: false,
                muted: false,
                pinned: false,
                crashed: None,
                failed: None,
                opener,
                inspects: None,
            },
        );
        let w = self.window_mut(window)?;
        let had = w.tabs.len();
        w.tabs.insert(index, id);
        if front || had == 0 {
            w.active = index;
        } else if index <= w.active {
            w.active += 1;
        }
        Some(id)
    }

    /// Put a window's `index`th tab in front.
    pub fn activate(&mut self, window: WindowKey, index: usize) -> Option<TabId> {
        let w = self.window_mut(window)?;
        if index < w.tabs.len() {
            w.active = index;
        }
        w.active_tab()
    }

    pub fn activate_tab(&mut self, tab: TabId) -> Option<WindowKey> {
        let window = self.window_of(tab)?;
        let index = self.window(window)?.tabs.iter().position(|&t| t == tab)?;
        self.activate(window, index);
        Some(window)
    }

    /// Close a tab. The tab in front after it is the one that opened it, if that is still in
    /// the window (a link looked at and closed goes back to where it was clicked), else the
    /// one to its right, else the one to its left.
    pub fn close_tab(&mut self, tab: TabId) -> Option<Closed> {
        let window = self.window_of(tab)?;
        let opener = self.tabs.get(&tab).and_then(|t| t.opener);
        self.tabs.remove(&tab);
        for other in self.tabs.values_mut().filter(|t| t.opener == Some(tab)) {
            other.opener = None;
        }
        let w = self.window_mut(window)?;
        let index = w.tabs.iter().position(|&t| t == tab)?;
        let was_active = index == w.active;
        w.tabs.remove(index);
        if w.tabs.is_empty() {
            self.windows.retain(|w| w.key != window);
            return Some(Closed { window, window_closed: true, now_active: None });
        }
        let before = w.active_tab();
        if was_active {
            let opener_here = opener.and_then(|o| w.tabs.iter().position(|&t| t == o));
            w.active = opener_here.unwrap_or(index.min(w.tabs.len() - 1));
        } else if index < w.active {
            w.active -= 1;
        }
        let after = w.active_tab();
        Some(Closed { window, window_closed: false, now_active: (was_active || before != after).then_some(after).flatten() })
    }

    /// Close a window and every tab in it; the tabs that went.
    pub fn close_window(&mut self, window: WindowKey) -> Vec<TabId> {
        let Some(w) = self.window(window).cloned() else { return Vec::new() };
        for tab in &w.tabs {
            self.tabs.remove(tab);
        }
        self.windows.retain(|w| w.key != window);
        w.tabs
    }

    /// Drag a tab along its own window's bar to `to`.
    pub fn reorder(&mut self, window: WindowKey, from: usize, to: usize) {
        let Some(w) = self.window_mut(window) else { return };
        if from >= w.tabs.len() {
            return;
        }
        let active = w.active_tab();
        let tab = w.tabs.remove(from);
        let to = to.min(w.tabs.len());
        w.tabs.insert(to, tab);
        if let Some(active) = active {
            w.active = w.tabs.iter().position(|&t| t == active).unwrap_or(0);
        }
    }

    /// Pin a tab, or let it go: pinned, it goes after the window's other pinned tabs; let go,
    /// after the last of them.
    pub fn set_pinned(&mut self, tab: TabId, pinned: bool) {
        let Some(window) = self.window_of(tab) else { return };
        if let Some(t) = self.tabs.get_mut(&tab) {
            t.pinned = pinned;
        }
        let Some(w) = self.window(window) else { return };
        let from = w.tabs.iter().position(|&t| t == tab).unwrap_or(0);
        let others_pinned = w.tabs.iter().filter(|&&t| t != tab && self.tabs.get(&t).is_some_and(|t| t.pinned)).count();
        self.reorder(window, from, others_pinned);
    }

    /// Move a tab to another window of its profile, at `index` (the end if none), in front
    /// there. The window it left closes if that was its last tab: say so with `true`.
    pub fn move_tab(&mut self, tab: TabId, to: WindowKey, index: Option<usize>) -> Result<bool, String> {
        let from = self.window_of(tab).ok_or("no such tab")?;
        let profile = self.tabs.get(&tab).map(|t| t.profile.clone()).ok_or("no such tab")?;
        let target = self.window(to).ok_or("no such window")?;
        if target.profile != profile {
            return Err("a tab goes only to a window of its own profile".into());
        }
        if from == to {
            let w = self.window(from).ok_or("no such window")?;
            let at = w.tabs.iter().position(|&t| t == tab).ok_or("no such tab")?;
            let end = w.tabs.len() - 1;
            self.reorder(from, at, index.unwrap_or(end).min(end));
            self.activate_tab(tab);
            return Ok(false);
        }
        // Out of the old window, as closing it would, but keeping the tab.
        let w = self.window_mut(from).ok_or("no such window")?;
        let at = w.tabs.iter().position(|&t| t == tab).ok_or("no such tab")?;
        let was_active = at == w.active;
        w.tabs.remove(at);
        let emptied = w.tabs.is_empty();
        if emptied {
            self.windows.retain(|w| w.key != from);
        } else if was_active {
            w.active = at.min(w.tabs.len() - 1);
        } else if at < w.active {
            w.active -= 1;
        }
        if let Some(t) = self.tabs.get_mut(&tab) {
            t.opener = None;
        }
        let w = self.window_mut(to).ok_or("no such window")?;
        let index = index.unwrap_or(w.tabs.len()).min(w.tabs.len());
        w.tabs.insert(index, tab);
        w.active = index;
        Ok(emptied)
    }

    /// Tear a tab off into a window of its own. Nothing to do for a window's only tab, which
    /// is already alone: `None`.
    pub fn tear_off(&mut self, tab: TabId) -> Option<WindowKey> {
        let from = self.window_of(tab)?;
        if self.window(from)?.tabs.len() <= 1 {
            return None;
        }
        let profile = self.tabs.get(&tab)?.profile.clone();
        let new = self.new_window(&profile);
        self.move_tab(tab, new, None).ok()?;
        Some(new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(tabs: &Tabs, window: WindowKey) -> Vec<TabId> {
        tabs.window(window).unwrap().tabs.clone()
    }

    #[test]
    fn opening_and_closing() {
        let mut tabs = Tabs::new();
        let w = tabs.new_window("default");
        let a = tabs.new_tab(w, "https://a/", Place::End, true, None).unwrap();
        let b = tabs.new_tab(w, "https://b/", Place::End, true, None).unwrap();
        let c = tabs.new_tab(w, "https://c/", Place::End, false, None).unwrap();
        assert_eq!(titles(&tabs, w), vec![a, b, c]);
        assert_eq!(tabs.active(w), Some(b), "a background tab does not come to the front");
        // Closing the one in front brings the one to its right.
        let closed = tabs.close_tab(b).unwrap();
        assert_eq!(closed.now_active, Some(c));
        assert_eq!(tabs.active(w), Some(c));
        // The last on the right closing brings the one to its left.
        tabs.close_tab(c);
        assert_eq!(tabs.active(w), Some(a));
        let closed = tabs.close_tab(a).unwrap();
        assert!(closed.window_closed);
        assert!(tabs.window(w).is_none());
    }

    #[test]
    fn links_opened_in_the_background_line_up() {
        let mut tabs = Tabs::new();
        let w = tabs.new_window("default");
        let page = tabs.new_tab(w, "https://news/", Place::End, true, None).unwrap();
        let other = tabs.new_tab(w, "https://other/", Place::End, false, None).unwrap();
        let one = tabs.new_tab(w, "https://news/1", Place::AfterActive, false, Some(page)).unwrap();
        let two = tabs.new_tab(w, "https://news/2", Place::AfterActive, false, Some(page)).unwrap();
        assert_eq!(titles(&tabs, w), vec![page, one, two, other]);
        assert_eq!(tabs.active(w), Some(page));
        // A link looked at and closed goes back to the page it was opened from.
        tabs.activate_tab(two);
        tabs.close_tab(two);
        assert_eq!(tabs.active(w), Some(page));
        // A tab with no opener: the one to its right.
        tabs.activate_tab(other);
        tabs.activate_tab(page);
        tabs.close_tab(page);
        assert_eq!(tabs.active(w), Some(one));
    }

    #[test]
    fn reordering_keeps_the_front_tab() {
        let mut tabs = Tabs::new();
        let w = tabs.new_window("default");
        let a = tabs.new_tab(w, "a", Place::End, true, None).unwrap();
        let b = tabs.new_tab(w, "b", Place::End, false, None).unwrap();
        let c = tabs.new_tab(w, "c", Place::End, false, None).unwrap();
        tabs.reorder(w, 0, 2);
        assert_eq!(titles(&tabs, w), vec![b, c, a]);
        assert_eq!(tabs.active(w), Some(a));
    }

    #[test]
    fn tearing_off_and_dropping_back() {
        let mut tabs = Tabs::new();
        let w1 = tabs.new_window("default");
        let a = tabs.new_tab(w1, "a", Place::End, true, None).unwrap();
        let b = tabs.new_tab(w1, "b", Place::End, true, None).unwrap();
        let w2 = tabs.tear_off(b).unwrap();
        assert_eq!(titles(&tabs, w1), vec![a]);
        assert_eq!(titles(&tabs, w2), vec![b]);
        assert_eq!(tabs.active(w1), Some(a));
        assert_eq!(tabs.window_of(b), Some(w2));
        assert_eq!(tabs.tear_off(a), None, "a window's only tab is already alone");
        // Dropped back onto the first window, at the front of its bar: the second window,
        // left with nothing, is gone.
        assert_eq!(tabs.move_tab(b, w1, Some(0)), Ok(true));
        assert_eq!(titles(&tabs, w1), vec![b, a]);
        assert_eq!(tabs.active(w1), Some(b));
        assert!(tabs.window(w2).is_none());
    }

    #[test]
    fn a_tab_stays_with_its_profile() {
        let mut tabs = Tabs::new();
        let mine = tabs.new_window("default");
        let private = tabs.new_window("private");
        let a = tabs.new_tab(mine, "a", Place::End, true, None).unwrap();
        tabs.new_tab(mine, "b", Place::End, true, None).unwrap();
        tabs.new_tab(private, "p", Place::End, true, None).unwrap();
        assert!(tabs.move_tab(a, private, None).is_err());
        assert_eq!(tabs.window_of(a), Some(mine));
        assert_eq!(tabs.tab(a).unwrap().profile, "default");
    }

    #[test]
    fn closing_a_window_closes_its_tabs() {
        let mut tabs = Tabs::new();
        let w = tabs.new_window("default");
        let a = tabs.new_tab(w, "a", Place::End, true, None).unwrap();
        let b = tabs.new_tab(w, "b", Place::End, true, None).unwrap();
        let mut gone = tabs.close_window(w);
        gone.sort();
        assert_eq!(gone, vec![a, b]);
        assert!(tabs.tab(a).is_none() && tabs.windows().is_empty());
    }

    #[test]
    fn pinned_tabs_go_first() {
        let mut tabs = Tabs::new();
        let w = tabs.new_window("default");
        let a = tabs.new_tab(w, "a", Place::End, true, None).unwrap();
        let b = tabs.new_tab(w, "b", Place::End, false, None).unwrap();
        let c = tabs.new_tab(w, "c", Place::End, false, None).unwrap();
        tabs.set_pinned(c, true);
        tabs.set_pinned(b, true);
        assert_eq!(titles(&tabs, w), vec![c, b, a]);
        assert_eq!(tabs.active(w), Some(a));
        tabs.set_pinned(c, false);
        assert_eq!(titles(&tabs, w), vec![b, c, a]);
    }

    #[test]
    fn labels() {
        let mut tabs = Tabs::new();
        let w = tabs.new_window("default");
        let a = tabs.new_tab(w, "about:blank", Place::End, true, None).unwrap();
        assert_eq!(tabs.tab(a).unwrap().label(), "New tab");
        tabs.tab_mut(a).unwrap().url = "https://example.com/".into();
        assert_eq!(tabs.tab(a).unwrap().label(), "example.com");
        tabs.tab_mut(a).unwrap().title = "Example".into();
        assert_eq!(tabs.tab(a).unwrap().label(), "Example");
        tabs.tab_mut(a).unwrap().crashed = Some("gone".into());
        assert_eq!(tabs.tab(a).unwrap().label(), "Stopped");
    }
}
