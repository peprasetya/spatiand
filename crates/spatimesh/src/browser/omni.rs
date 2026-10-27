//! The address bar while it is typed into: what it suggests under it (`spatimesh::omnibox`),
//! the likeliest address finished inline, the arrows going through the suggestions, and
//! Shift-Delete taking a page out of the history.

use spatiand_app::ui::{look, theme, Icon, Input, List, Painter, Rect, Row};
use spatiand_app::{Key, Modifiers};
use spatimesh::address::shown;
use spatimesh::omnibox::{self, Kind, Suggestion};
use spatimesh::profiles::PRIVATE;
use spatimesh::store::now_ms;

use super::{command, Browser};

/// How many lines the bar suggests at most.
const MOST: usize = 8;

pub(super) struct Suggest {
    /// What the wearer typed, without what was finished for them.
    pub typed: String,
    pub items: Vec<Suggestion>,
    pub rows: Vec<Row>,
    pub list: List,
    pub rect: Rect,
}

fn row_of(s: &Suggestion, engine: &str) -> Row {
    let short = |url: &str| -> String {
        let plain = omnibox::stripped(url);
        if plain.chars().count() > 60 { format!("{}\u{2026}", plain.chars().take(59).collect::<String>()) } else { plain.to_string() }
    };
    let titled = |s: &Suggestion| if s.title.trim().is_empty() { short(&s.url) } else { s.title.clone() };
    match &s.kind {
        Kind::Search => Row::with(s.title.clone(), format!("Search {engine}")).icon(Icon::Search),
        Kind::Go => Row::with(short(&s.url), "Go").icon(Icon::Globe),
        Kind::History => Row::with(titled(s), short(&s.url)).icon(Icon::SortRecent),
        Kind::Bookmark => Row::with(titled(s), short(&s.url)).icon(Icon::Star),
        Kind::Tab(_) => Row::with(titled(s), "Switch to this tab").icon(Icon::Grid),
    }
}

impl Browser {
    /// The suggestions for what the bar says now; and, if `finish`, the likeliest address
    /// finished in it, the rest chosen so that typing on replaces it.
    pub(super) fn suggest(&mut self, w: usize, finish: bool) {
        let typed = self.wins[w].address.text.clone();
        if typed.trim().is_empty() {
            self.wins[w].suggest = None;
            return;
        }
        let key = self.wins[w].key;
        let profile = self.tabs.window(key).map(|x| x.profile.clone()).unwrap_or_default();
        let engine = self.search_for(&profile);
        let history = if profile == PRIVATE { Vec::new() } else { self.kept.get_mut(&profile).map(|k| k.history.entries(&self.store)).unwrap_or_default() };
        let marks = if profile == PRIVATE { Vec::new() } else { self.bookmarks_of(&profile).clone() };
        let active = self.tabs.active(key);
        let tabs: Vec<(u64, String, String)> = self.tabs.tabs().filter(|t| t.profile == profile && Some(t.id) != active && t.inspects.is_none()).map(|t| (t.id, t.url.clone(), t.title.clone())).collect();
        let found = omnibox::suggest(&typed, &history, &marks, &tabs, &engine, now_ms(), MOST);
        let rows: Vec<Row> = found.list.iter().map(|s| row_of(s, &engine.name)).collect();
        // Where Enter would go (the address finished, or the likeliest suggestion; a search's
        // engine too): connected to while the rest is typed.
        let likeliest = found.inline.as_ref().map(|i| i.url.clone()).or_else(|| found.list.first().filter(|s| !matches!(s.kind, Kind::Tab(_))).map(|s| s.url.clone()));
        if let (Some(url), Some(tab)) = (likeliest, active) {
            self.preconnect(tab, &url);
        }
        if let Some(i) = found.inline.as_ref().filter(|_| finish) {
            let field = &mut self.wins[w].address;
            field.set(i.text.clone());
            field.select_from(typed.chars().count());
        }
        let mut list = List::default();
        list.focused = true;
        self.wins[w].suggest = (!found.list.is_empty()).then(|| Suggest { typed, items: found.list, rows, list, rect: Rect::default() });
        self.wins[w].dirty = true;
    }

    /// A key while the bar is being typed into. Whether the suggestions took it.
    pub(super) fn suggest_key(&mut self, w: usize, key: Key, m: Modifiers) -> bool {
        let Some(sg) = self.wins[w].suggest.as_mut() else { return false };
        match key {
            Key::Up | Key::Down => {
                let before = sg.list.cursor;
                sg.list.input(&Input::Key { key, text: None, modifiers: m });
                if sg.list.cursor != before || key == Key::Down {
                    // The bar says where the line chosen goes; the first line, what was typed.
                    let item = sg.items.get(sg.list.cursor).cloned();
                    let first = sg.list.cursor == 0;
                    let typed = sg.typed.clone();
                    let profile = self.tabs.window(self.wins[w].key).map(|x| x.profile.clone()).unwrap_or_default();
                    let engine = self.search_for(&profile);
                    let field = &mut self.wins[w].address;
                    match item {
                        Some(_) if first => field.set(typed),
                        Some(s) => field.set(shown(&s.url, &engine)),
                        None => {}
                    }
                }
                true
            }
            // A page taken out of the history, from the bar.
            Key::Delete if m.shift => {
                let Some(s) = sg.items.get(sg.list.cursor).cloned().filter(|s| s.kind == Kind::History) else { return true };
                let typed = sg.typed.clone();
                let profile = self.tabs.window(self.wins[w].key).map(|x| x.profile.clone()).unwrap_or_default();
                if let Some(kept) = self.kept.get_mut(&profile) {
                    kept.history.forget(&self.store, Some(&s.url), 0);
                }
                self.kept_forgot(&profile);
                self.wins[w].address.set(typed);
                self.suggest(w, false);
                true
            }
            _ => false,
        }
    }

    /// Enter in the bar: the line chosen, or what was typed (finished, if it was).
    pub(super) fn suggest_enter(&mut self, w: usize) -> bool {
        let Some(sg) = self.wins[w].suggest.take() else { return false };
        let Some(item) = sg.items.get(sg.list.cursor).cloned() else { return false };
        self.take_suggestion(w, item);
        true
    }

    pub(super) fn take_suggestion(&mut self, w: usize, item: Suggestion) {
        self.wins[w].suggest = None;
        match item.kind {
            Kind::Tab(tab) => {
                self.wins[w].editing = false;
                self.wins[w].address.focused = false;
                if let Some(window) = self.tabs.activate_tab(tab) {
                    self.after_tab_change(window);
                }
                let key = self.wins[w].key;
                self.after_tab_change(key);
            }
            _ => self.go(w, &item.url),
        }
    }

    /// The pointer over the suggestions: whether it is.
    pub(super) fn suggest_pointer(&mut self, w: usize, x: f32, y: f32) -> bool {
        let Some(sg) = self.wins[w].suggest.as_mut() else { return false };
        sg.list.input(&Input::Pointer { x, y });
        sg.rect.contains(x, y)
    }

    /// A press over the suggestions: whether it was.
    pub(super) fn suggest_press(&mut self, w: usize, x: f32, y: f32, pressed: bool) -> bool {
        let Some(sg) = self.wins[w].suggest.as_mut() else { return false };
        if !sg.rect.contains(x, y) {
            return false;
        }
        if let Some(index) = sg.list.input(&Input::Press { pressed }) {
            if let Some(item) = sg.items.get(index).cloned() {
                self.take_suggestion(w, item);
            }
        }
        true
    }

    /// What typing in the bar did: suggestions again, finished inline only when a letter was
    /// added at its end (not when one was taken away, which would finish it again at once).
    pub(super) fn bar_typed(&mut self, w: usize, before: &str, key: Key, text: Option<&str>, m: Modifiers) {
        let after = self.wins[w].address.text.clone();
        if after == before {
            return;
        }
        let typed = text.is_some_and(|t| !t.is_empty() && !t.chars().all(char::is_control));
        let added = typed && !matches!(key, Key::Backspace | Key::Delete) && !command(m) && self.wins[w].address.caret_at_end();
        self.suggest(w, added);
    }
}

/// The suggestions, under the bar.
pub(super) fn draw_suggestions(p: &mut Painter, gl: &spatiand_app::Gl, under: Rect, limit: f32, sg: &mut Suggest) {
    let height = (sg.rows.len() as f32 * (theme::ROW_HEIGHT + theme::ROW_GAP) + 20.0).min(limit - under.bottom() - 8.0);
    let rect = Rect::new(under.x - 8.0, under.bottom() + 6.0, under.w + 16.0, height);
    sg.rect = rect;
    p.fill(gl, rect, 18.0, theme::ground());
    look::glass(p, gl, rect, 18.0, 1.0);
    sg.list.draw(p, gl, rect.inset(10.0), &sg.rows);
}
