//! SpatiMesh's menus: the main menu and its submenus, opened beside the line that names them;
//! the menu of what is under the pointer in a page (a link, an image, a film, chosen text, a
//! field), of a tab on the bar, of a profile; the bookmark's own; and what every line does.

use spatiand_app::ui::{theme, Icon, Input, List, Rect, Row};
use spatiand_app::Key;
use spatimesh::address::{self, Typed};
use spatimesh::bookmarks::{self, At};
use spatimesh::engine::{Hit, TabId};
use spatimesh::extensions::store as stores;
use spatimesh::profiles::{self, LockKind, PRIVATE};
use spatimesh::tabs::Place;
use spatimesh::wire::ToHost;

use super::downloads::Purpose;
use super::ext::{self, Told};
use super::{is_start, pages, Action, Browser, MediaDo, Own, Popup, Sub, TOOL_H};

/// Script requests of the page menu's, told apart from an extension's and the clipboard's.
pub(super) const PAGE_PROBE: u64 = 1 << 61;

/// A film's or a sound's state, as the page said it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(super) struct MediaState {
    pub paused: bool,
    pub muted: bool,
    pub looped: bool,
    pub controls: bool,
    pub video: bool,
}

/// The page menu open now.
#[derive(Debug, Clone)]
pub(super) struct Context {
    pub tab: TabId,
    pub hit: Hit,
    /// Where it was asked for: in the window, and in the page's own (CSS) pixels.
    pub at: (f32, f32),
    pub css: (f32, f32),
    /// What the page said when asked: its chosen text, and the film under the pointer.
    pub selection: String,
    pub media: Option<MediaState>,
}

/// What the page is asked when its menu opens: the text chosen in it, and the film or sound
/// under the pointer (X, Y in CSS pixels), as JSON.
const PROBE: &str = "(function(x,y){var r={sel:'',media:null};try{var s=getSelection();r.sel=s?String(s).slice(0,400):'';\
var e=document.elementFromPoint(x,y);while(e&&!(e instanceof HTMLMediaElement))e=e.parentElement;\
if(e)r.media={paused:e.paused,muted:e.muted,loop:e.loop,controls:e.controls,video:e instanceof HTMLVideoElement};}catch(_){}\
return JSON.stringify(r)})(X,Y)";

/// What a film's menu does to it, at X, Y.
const MEDIA: &str = "(function(x,y,w){var e=document.elementFromPoint(x,y);while(e&&!(e instanceof HTMLMediaElement))e=e.parentElement;if(!e)return;\
if(w==='play'){if(e.paused)e.play();else e.pause()}else if(w==='mute'){e.muted=!e.muted}else if(w==='loop'){e.loop=!e.loop}\
else if(w==='controls'){e.controls=!e.controls}})(X,Y,'W')";

fn rows_height(rows: &[Row]) -> f32 {
    rows.len() as f32 * (theme::ROW_HEIGHT + theme::ROW_GAP) + 24.0
}

/// A line that opens a submenu: what it is, what it is set to, and the mark that says so.
fn sub_row(label: &str, now: &str) -> Row {
    Row::with(label, if now.is_empty() { "\u{203A}".to_string() } else { format!("{now}  \u{203A}") })
}

/// Words chosen in a page, for a menu's line: short, on one line.
fn quoted(text: &str) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > 28 { format!("\u{201C}{}\u{2026}\u{201D}", one.chars().take(27).collect::<String>()) } else { format!("\u{201C}{one}\u{201D}") }
}

impl Browser {
    /// A list over the window, at `rect` (kept inside the window), the keys going to it.
    pub(super) fn open_popup(&mut self, w: usize, rows: Vec<Row>, actions: Vec<Action>, rect: Rect) {
        let size = self.wins[w].size;
        let height = rect.h.min(size.1 as f32 - 16.0);
        let x = rect.x.clamp(8.0, (size.0 as f32 - rect.w - 8.0).max(8.0));
        let y = rect.y.clamp(8.0, (size.1 as f32 - height - 8.0).max(8.0));
        let mut list = List::default();
        list.focused = true;
        self.wins[w].flyout = None;
        self.wins[w].suggest = None;
        self.wins[w].popup = Some(Popup { rows, actions, rect: Rect::new(x, y, rect.w, height), list, choose_for: None });
        self.wins[w].dirty = true;
        self.focus_pages();
    }

    /// A menu under the toolbar's right end, where its button is.
    fn open_menu_at_right(&mut self, w: usize, rows: Vec<Row>, actions: Vec<Action>, width: f32, from_right: f32) {
        let page = self.wins[w].page;
        let height = rows_height(&rows).min(page.h);
        let rect = Rect::new(page.right() - width - 8.0 - from_right, page.y + 4.0, width, height);
        self.open_popup(w, rows, actions, rect);
    }

    // ---- the main menu -------------------------------------------------------------------

    pub(super) fn menu(&mut self, w: usize) {
        let key = self.wins[w].key;
        let profile = self.tabs.window(key).map(|x| x.profile.clone()).unwrap_or_default();
        let url = self.tabs.active(key).and_then(|t| self.tabs.tab(t)).map(|t| t.url.clone()).unwrap_or_default();
        let engine = self.search_for(&profile).name;
        let mut rows = vec![
            Row::with("New tab", "Ctrl-T").icon(Icon::Plus),
            Row::with("New window", "Ctrl-N").icon(Icon::Desktop),
            Row::with("New private window", "Ctrl-Shift-N").icon(Icon::Eye),
        ];
        let mut actions = vec![Action::NewTab, Action::NewWindow, Action::NewPrivateWindow];
        rows.push(sub_row("History", "").icon(Icon::SortRecent));
        actions.push(Action::Submenu(Sub::History));
        if profile != PRIVATE {
            rows.push(sub_row("Bookmarks", "").icon(Icon::Star));
            actions.push(Action::Submenu(Sub::Bookmarks));
        }
        let going = self.downloads.going();
        rows.push(Row::with("Downloads", if going > 0 { format!("{going} going") } else { "Ctrl-J".into() }).icon(Icon::Download));
        actions.push(Action::Open(pages::DOWNLOADS.into()));
        rows.push(sub_row("Zoom", &format!("{:.0} %", self.settings.zoom() * 80.0)).icon(Icon::Search));
        actions.push(Action::Submenu(Sub::Zoom));
        // Twelve lines at most: the room's lines are tall, and a menu fits its window. Saving
        // the page and reopening a tab are in the page's menu, History's and the keys.
        if url.starts_with("http") {
            rows.push(Row::with("Find in page\u{2026}", "Ctrl-F").icon(Icon::Search));
            actions.push(Action::FindInPage);
        }
        rows.push(sub_row("Search engine", &engine).icon(Icon::Globe));
        actions.push(Action::Submenu(Sub::SearchEngine));
        if profile != PRIVATE {
            rows.push(sub_row("Extensions", "").icon(Icon::Puzzle));
            actions.push(Action::Submenu(Sub::Extensions));
        }
        rows.push(Row::with("Developer tools", "Ctrl-Shift-I").icon(Icon::Terminal));
        actions.push(Action::DeveloperTools);
        rows.push(Row::new("Settings").icon(Icon::Options));
        actions.push(Action::Open(pages::SETTINGS.into()));
        self.open_menu_at_right(w, rows, actions, 680.0, 0.0);
    }

    /// A submenu's lines.
    fn sub_rows(&mut self, w: usize, sub: Sub) -> (Vec<Row>, Vec<Action>) {
        let key = self.wins[w].key;
        let profile = self.tabs.window(key).map(|x| x.profile.clone()).unwrap_or_default();
        let url = self.tabs.active(key).and_then(|t| self.tabs.tab(t)).map(|t| t.url.clone()).unwrap_or_default();
        let mut rows = Vec::new();
        let mut actions = Vec::new();
        let mut add = |row: Row, action: Action| {
            rows.push(row);
            actions.push(action);
        };
        match sub {
            Sub::SearchEngine => {
                add(Row::heading("Search with, in this profile"), Action::Nothing);
                let current = self.search_for(&profile).name;
                for engine in address::search_engines() {
                    let mark = if engine.name == current { "\u{2713}" } else { "" };
                    add(Row::with(engine.name.clone(), mark).icon(Icon::Search), Action::SearchWith(engine.name));
                }
                if profile != PRIVATE {
                    add(Row::with("An address of your own\u{2026}", if current == "Custom" { "\u{2713}" } else { "" }), Action::CustomSearch);
                }
            }
            Sub::Zoom => {
                add(Row::with("Bigger", "Ctrl-+"), Action::ZoomIn);
                add(Row::with("Smaller", "Ctrl--"), Action::ZoomOut);
                add(Row::with("Usual size", "Ctrl-0"), Action::ZoomReset);
            }
            Sub::History => {
                add(Row::with("Show all history", "Ctrl-H").icon(Icon::SortRecent), Action::Open(pages::HISTORY.into()));
                let closed = self.closed_of(&profile);
                if !closed.is_empty() {
                    add(Row::with("Reopen closed tab", "Ctrl-Shift-T").icon(Icon::Rotate), Action::ReopenClosed);
                }
                if !closed.is_empty() {
                    add(Row::heading("Closed lately"), Action::Nothing);
                    for (i, label, window) in closed.into_iter().take(8) {
                        add(Row::new(label).icon(if window { Icon::Desktop } else { Icon::Rotate }), Action::ReopenAt(i));
                    }
                }
                let recent: Vec<(String, String)> = match self.kept.get_mut(&profile) {
                    Some(kept) => kept.history.entries(&self.store).into_iter().take(10).map(|e| (e.url, e.title)).collect(),
                    None => Vec::new(),
                };
                if !recent.is_empty() {
                    add(Row::heading("Visited lately"), Action::Nothing);
                    for (url, title) in recent {
                        let label = if title.trim().is_empty() { spatimesh::omnibox::stripped(&url).to_string() } else { title };
                        add(Row::new(label).icon(Icon::Globe), Action::Open(url));
                    }
                }
            }
            Sub::Bookmarks => {
                let marked = bookmarks::find(self.bookmarks_of(&profile), &url).is_some();
                if url.starts_with("http") {
                    add(Row::with(if marked { "Edit this bookmark" } else { "Bookmark this page" }, "Ctrl-D").icon(Icon::Star), Action::Bookmark);
                }
                add(Row::with("Show all bookmarks", "Ctrl-Shift-O").icon(Icon::List), Action::Open(pages::BOOKMARKS.into()));
                add(Row::with("Bookmarks bar", if self.settings.bookmarks_bar { "\u{2713}  Ctrl-Shift-B" } else { "Ctrl-Shift-B" }), Action::ToggleBookmarksBar);
                let marks: Vec<(String, String)> = bookmarks::children(self.bookmarks_of(&profile), "")
                    .into_iter()
                    .filter(|b| !b.is_folder())
                    .take(15)
                    .map(|b| (b.title.clone(), b.url.clone().unwrap_or_default()))
                    .collect();
                if !marks.is_empty() {
                    add(Row::heading("Bookmarks"), Action::Nothing);
                    for (title, url) in marks {
                        add(Row::new(title).icon(Icon::Link), Action::Open(url));
                    }
                }
            }
            Sub::Extensions => {
                if stores::listing(&url).is_some() {
                    add(Row::new("Add this extension to SpatiMesh").icon(Icon::Plus), Action::AddFromStore);
                }
                for (label, detail, action) in ext::menu_rows(&self.store, &profile).into_iter().filter(|(_, _, a)| matches!(a, Action::ToggleExtension(_))) {
                    add(Row::with(label, if detail == "on" { "\u{2713} on" } else { "off" }).icon(Icon::Puzzle), action);
                }
                add(Row::new("Manage extensions").icon(Icon::Options), Action::Open(pages::EXTENSIONS.into()));
                add(Row::heading("Get extensions"), Action::Nothing);
                add(Row::with("Chrome Web Store", "Chrome's").icon(Icon::Globe), Action::OpenInNewTab(stores::CHROME_WEB_STORE.into()));
                add(Row::with("Firefox Add-ons", "Firefox's").icon(Icon::Globe), Action::OpenInNewTab(stores::FIREFOX_ADD_ONS.into()));
                add(Row::with("Load an unpacked folder\u{2026}", "developer mode").icon(Icon::Folder), Action::PickExtensionFolder);
                add(Row::with("Install from a file\u{2026}", ".crx .xpi .zip").icon(Icon::Document), Action::PickExtensionFile);
            }
        }
        (rows, actions)
    }

    /// The submenu of the popup's line `index`, beside it.
    fn open_flyout(&mut self, w: usize, index: usize, sub: Sub, focus: bool) {
        if self.wins[w].flyout.as_ref().is_some_and(|(i, _)| *i == index) {
            if focus {
                if let Some((_, f)) = self.wins[w].flyout.as_mut() {
                    f.list.focused = true;
                }
                if let Some(p) = self.wins[w].popup.as_mut() {
                    p.list.focused = false;
                }
            }
            return;
        }
        let (rows, actions) = self.sub_rows(w, sub);
        let Some(popup) = self.wins[w].popup.as_ref() else { return };
        let line = popup.list.row_rect(index).unwrap_or(Rect::new(popup.rect.x, popup.rect.y, popup.rect.w, theme::ROW_HEIGHT));
        let width = 560.0;
        let height = rows_height(&rows).min(self.wins[w].size.1 as f32 - 16.0);
        // To the left of a menu at the window's right, as the main menu is; else to its right.
        let size = self.wins[w].size;
        let x = if popup.rect.x - width - 4.0 >= 8.0 { popup.rect.x - width - 4.0 } else { (popup.rect.right() + 4.0).min(size.0 as f32 - width - 8.0) };
        let y = (line.y - 12.0).clamp(8.0, (size.1 as f32 - height - 8.0).max(8.0));
        let mut list = List::default();
        list.focused = focus;
        if let Some(p) = self.wins[w].popup.as_mut() {
            p.list.focused = !focus;
        }
        self.wins[w].flyout = Some((index, Popup { rows, actions, rect: Rect::new(x, y, width, height), list, choose_for: None }));
        self.wins[w].dirty = true;
    }

    fn close_flyout(&mut self, w: usize) {
        if self.wins[w].flyout.take().is_some() {
            if let Some(p) = self.wins[w].popup.as_mut() {
                p.list.focused = true;
            }
            self.wins[w].dirty = true;
        }
    }

    // ---- input to the menus --------------------------------------------------------------

    /// The pointer moved: over a menu, whether it is.
    pub(super) fn popup_pointer(&mut self, w: usize, x: f32, y: f32) -> bool {
        if let Some((_, flyout)) = self.wins[w].flyout.as_mut() {
            flyout.list.input(&Input::Pointer { x, y });
            if flyout.rect.contains(x, y) {
                return true;
            }
        }
        let Some(popup) = self.wins[w].popup.as_mut() else { return false };
        popup.list.input(&Input::Pointer { x, y });
        if !popup.rect.contains(x, y) {
            return false;
        }
        // Over a line with a submenu, the submenu opens; over another, it closes.
        let hovered = popup.list.hovered();
        let sub = hovered.and_then(|i| match popup.actions.get(i) {
            Some(Action::Submenu(sub)) => Some((i, *sub)),
            _ => None,
        });
        match sub {
            Some((i, sub)) => self.open_flyout(w, i, sub, false),
            None if hovered.is_some() => self.close_flyout(w),
            None => {}
        }
        true
    }

    /// The pointer's button over the window while a menu is open: whether the menus took it.
    pub(super) fn popup_press(&mut self, w: usize, x: f32, y: f32, pressed: bool) -> bool {
        if let Some((_, flyout)) = self.wins[w].flyout.as_mut() {
            if flyout.rect.contains(x, y) {
                if let Some(index) = flyout.list.input(&Input::Press { pressed }) {
                    self.picked_in_flyout(w, index);
                }
                return true;
            }
        }
        let Some(popup) = self.wins[w].popup.as_mut() else { return false };
        if popup.rect.contains(x, y) {
            if let Some(index) = popup.list.input(&Input::Press { pressed }) {
                self.picked(w, index);
            }
            return true;
        }
        if pressed {
            self.dismiss_popup(w);
            return true;
        }
        false
    }

    /// A key while a menu is open. Whether it took it (it takes every key but the page's).
    pub(super) fn popup_key(&mut self, w: usize, input: &Input, key: Key) -> bool {
        if self.wins[w].popup.is_none() {
            return false;
        }
        if let Some((_, flyout)) = self.wins[w].flyout.as_mut().filter(|(_, f)| f.list.focused) {
            match key {
                Key::Escape | Key::Left => self.close_flyout(w),
                _ => {
                    if let Some(index) = flyout.list.input(input) {
                        self.picked_in_flyout(w, index);
                    }
                }
            }
            return true;
        }
        if key == Key::Escape {
            self.dismiss_popup(w);
            return true;
        }
        let Some(popup) = self.wins[w].popup.as_mut() else { return true };
        let cursor = popup.list.cursor;
        if key == Key::Right {
            if let Some(Action::Submenu(sub)) = popup.actions.get(cursor).cloned() {
                self.open_flyout(w, cursor, sub, true);
            }
            return true;
        }
        if let Some(index) = popup.list.input(input) {
            self.picked(w, index);
        }
        true
    }

    /// The popup's line `index` was picked.
    pub(super) fn picked(&mut self, w: usize, index: usize) {
        let Some(popup) = self.wins[w].popup.take() else { return };
        if let Some(tab) = popup.choose_for {
            let usable = self.views.get(&tab).and_then(|v| v.choices.as_ref()).and_then(|(c, _)| c.get(index)).is_some_and(|c| c.enabled && !c.heading);
            if !usable {
                self.wins[w].popup = Some(popup);
                return;
            }
            if let Some(view) = self.views.get_mut(&tab) {
                view.choices = None;
            }
            self.tell(tab, ToHost::Choose { tab, index: Some(index) });
        } else if let Some(action) = popup.actions.get(index).cloned() {
            match action {
                Action::Nothing => {
                    self.wins[w].popup = Some(popup);
                    return;
                }
                Action::Submenu(sub) => {
                    self.wins[w].popup = Some(popup);
                    self.open_flyout(w, index, sub, true);
                    return;
                }
                action => {
                    self.wins[w].flyout = None;
                    self.context = None;
                    self.act(w, action);
                }
            }
        }
        self.wins[w].dirty = true;
        self.focus_pages();
    }

    fn picked_in_flyout(&mut self, w: usize, index: usize) {
        let Some(action) = self.wins[w].flyout.as_ref().and_then(|(_, f)| f.actions.get(index).cloned()) else { return };
        if action == Action::Nothing {
            return;
        }
        self.wins[w].flyout = None;
        self.wins[w].popup = None;
        self.act(w, action);
        self.wins[w].dirty = true;
        self.focus_pages();
    }

    // ---- the page's menu -----------------------------------------------------------------

    /// The wearer asked for a menu over a tab's page: what is under the pointer, and the page
    /// asked what it has chosen and whether a film is there.
    pub(super) fn context_menu(&mut self, tab: TabId, hit: Hit) {
        let Some(w) = self.tabs.window_of(tab).and_then(|k| self.win_of_key(k)) else { return };
        if self.tabs.active(self.wins[w].key) != Some(tab) {
            return;
        }
        let at = self.wins[w].pointer;
        let page = self.wins[w].page;
        let zoom = self.settings.zoom();
        let css = ((at.0 - page.x) / zoom, (at.1 - page.y) / zoom);
        self.context = Some(Context { tab, hit, at, css, selection: String::new(), media: None });
        self.show_context(w);
        let code = PROBE.replace("(X,Y)", &format!("({},{})", css.0, css.1));
        self.tell(tab, ToHost::RunScript { tab, world: String::new(), all_frames: false, code, request: PAGE_PROBE | tab, boot: false });
    }

    /// What the page said when its menu opened: the menu again, with it.
    pub(super) fn context_probed(&mut self, tab: TabId, json: &str) {
        let Some(context) = self.context.as_mut().filter(|c| c.tab == tab) else { return };
        let Ok(said) = serde_json::from_str::<serde_json::Value>(json) else { return };
        context.selection = said["sel"].as_str().unwrap_or("").trim().to_string();
        context.media = said["media"].as_object().map(|m| MediaState {
            paused: m.get("paused").and_then(|v| v.as_bool()).unwrap_or(true),
            muted: m.get("muted").and_then(|v| v.as_bool()).unwrap_or(false),
            looped: m.get("loop").and_then(|v| v.as_bool()).unwrap_or(false),
            controls: m.get("controls").and_then(|v| v.as_bool()).unwrap_or(false),
            video: m.get("video").and_then(|v| v.as_bool()).unwrap_or(true),
        });
        let Some(w) = self.tabs.window_of(tab).and_then(|k| self.win_of_key(k)) else { return };
        // Only if the menu is still the one it is about.
        if self.wins[w].popup.is_some() {
            let cursor = self.wins[w].popup.as_ref().map_or(0, |p| p.list.cursor);
            self.show_context(w);
            if let Some(p) = self.wins[w].popup.as_mut() {
                p.list.cursor = cursor;
            }
        }
    }

    fn show_context(&mut self, w: usize) {
        let Some(context) = self.context.clone() else { return };
        let tab = context.tab;
        let Some(t) = self.tabs.tab(tab).cloned() else { return };
        let private = t.profile == PRIVATE;
        let engine = self.search_for(&t.profile);
        let mut rows = Vec::new();
        let mut actions = Vec::new();
        let mut add = |row: Row, action: Action| {
            rows.push(row);
            actions.push(action);
        };
        let hit = &context.hit;
        if let Some(link) = hit.link.clone() {
            add(Row::new("Open link in new tab").icon(Icon::Plus), Action::OpenInBackground(link.clone()));
            add(Row::new("Open link in new window").icon(Icon::Desktop), Action::OpenInNewWindow(link.clone()));
            if !private {
                add(Row::new("Open link in private window").icon(Icon::Eye), Action::OpenInPrivate(link.clone()));
            }
            add(Row::new("Copy link address").icon(Icon::Copy), Action::Copy(link.clone()));
            if !hit.link_text.is_empty() && hit.image.is_none() {
                add(Row::new("Copy link text").icon(Icon::Copy), Action::Copy(hit.link_text.clone()));
            }
            add(Row::new("Save link as\u{2026}").icon(Icon::Download), Action::Download(link.clone()));
            if !private && stores::listing(&link).is_some() {
                add(Row::new("Add this extension to SpatiMesh").icon(Icon::Puzzle), Action::OpenInNewTab(link));
            }
        }
        if let Some(image) = hit.image.clone() {
            add(Row::new("Open image in new tab").icon(Icon::Picture), Action::OpenInBackground(image.clone()));
            add(Row::new("Save image as\u{2026}").icon(Icon::Download), Action::Download(image.clone()));
            add(Row::new("Copy image address").icon(Icon::Copy), Action::Copy(image));
        }
        let media_url = hit.media.clone();
        if media_url.is_some() || context.media.is_some() {
            let state = context.media.unwrap_or_default();
            let what = if context.media.is_some_and(|m| !m.video) { "sound" } else { "video" };
            let (x, y) = context.css;
            add(Row::new(if state.paused { "Play" } else { "Pause" }).icon(if state.paused { Icon::Play } else { Icon::Pause }), Action::Media(MediaDo::PlayPause, x, y));
            add(Row::new(if state.muted { "Unmute" } else { "Mute" }).icon(if state.muted { Icon::Speaker } else { Icon::Muted }), Action::Media(MediaDo::Mute, x, y));
            add(Row::with("Loop", if state.looped { "\u{2713}" } else { "" }).icon(Icon::Rotate), Action::Media(MediaDo::Loop, x, y));
            add(Row::with("Show controls", if state.controls { "\u{2713}" } else { "" }).icon(Icon::Options), Action::Media(MediaDo::Controls, x, y));
            if let Some(url) = media_url.clone().filter(|u| u.starts_with("http")) {
                add(Row::new(format!("Open {what} in new tab")).icon(Icon::Film), Action::OpenInBackground(url.clone()));
                add(Row::new(format!("Save {what} as\u{2026}")).icon(Icon::Download), Action::Download(url.clone()));
                add(Row::new(format!("Copy {what} address")).icon(Icon::Copy), Action::Copy(url));
            }
        }
        if hit.editable {
            add(Row::with("Undo", "Ctrl-Z"), Action::PageCommand("undo"));
            add(Row::with("Redo", "Ctrl-Shift-Z"), Action::PageCommand("redo"));
            add(Row::with("Cut", "Ctrl-X"), Action::PageClipboard('x'));
            add(Row::with("Copy", "Ctrl-C").icon(Icon::Copy), Action::PageClipboard('c'));
            add(Row::with("Paste", "Ctrl-V"), Action::PageClipboard('v'));
            add(Row::with("Select all", "Ctrl-A"), Action::PageClipboard('a'));
        } else if hit.selection || !context.selection.is_empty() {
            add(Row::with("Copy", "Ctrl-C").icon(Icon::Copy), Action::PageClipboard('c'));
            let words = if context.selection.is_empty() { "what is chosen".to_string() } else { quoted(&context.selection) };
            if let Some(Typed::Address(url)) = address::understand(&context.selection).filter(|_| !context.selection.contains(char::is_whitespace)) {
                add(Row::new(format!("Go to {words}")).icon(Icon::Globe), Action::OpenInNewTab(url));
            }
            add(Row::new(format!("Search {} for {words}", engine.name)).icon(Icon::Search), Action::SearchSelection);
        }
        let plain = hit.link.is_none() && hit.image.is_none() && media_url.is_none() && context.media.is_none() && !hit.editable && !hit.selection && context.selection.is_empty();
        if plain {
            if t.can_back {
                add(Row::with("Back", "Alt-Left").icon(Icon::Back), Action::Back);
            }
            if t.can_forward {
                add(Row::with("Forward", "Alt-Right").icon(Icon::Forward), Action::Forward);
            }
            add(Row::with("Reload", "Ctrl-R").icon(Icon::Refresh), Action::Reload);
            if !private && t.url.starts_with("http") {
                let marked = bookmarks::find(self.bookmarks_of(&t.profile), &t.url).is_some();
                add(Row::with(if marked { "Edit bookmark" } else { "Bookmark this page" }, "Ctrl-D").icon(Icon::Star), Action::Bookmark);
            }
            if t.url.starts_with("http") {
                add(Row::with("Save page as\u{2026}", "Ctrl-S").icon(Icon::Import), Action::SavePage);
            }
            add(Row::with("Select all", "Ctrl-A"), Action::PageClipboard('a'));
            add(Row::with("Find in page\u{2026}", "Ctrl-F").icon(Icon::Search), Action::FindInPage);
            if !private && stores::listing(&t.url).is_some() {
                add(Row::new("Add this extension to SpatiMesh").icon(Icon::Puzzle), Action::AddFromStore);
            }
        }
        add(Row::with("Inspect", "Ctrl-Shift-I").icon(Icon::Terminal), Action::DeveloperTools);
        let height = rows_height(&rows);
        let (x, y) = context.at;
        self.open_popup(w, rows, actions, Rect::new(x + 2.0, y + 2.0, 640.0, height));
    }

    // ---- a tab's menu --------------------------------------------------------------------

    pub(super) fn tab_menu(&mut self, w: usize, tab: TabId) {
        let Some(t) = self.tabs.tab(tab).cloned() else { return };
        let key = self.wins[w].key;
        let count = self.tabs.window(key).map_or(0, |x| x.tabs.len());
        let index = self.tabs.window(key).and_then(|x| x.tabs.iter().position(|&id| id == tab)).unwrap_or(0);
        let mut rows = vec![Row::new("New tab to the right").icon(Icon::Plus), Row::with("Reload", "Ctrl-R").icon(Icon::Refresh), Row::new("Duplicate").icon(Icon::Copy)];
        let mut actions = vec![Action::TabNewAfter(tab), Action::TabReload(tab), Action::TabDuplicate(tab)];
        rows.push(Row::new(if t.pinned { "Unpin" } else { "Pin" }).icon(Icon::Pin));
        actions.push(Action::TabPin(tab, !t.pinned));
        rows.push(Row::new(if t.muted { "Unmute tab" } else { "Mute tab" }).icon(if t.muted { Icon::Speaker } else { Icon::Muted }));
        actions.push(Action::TabMute(tab, !t.muted));
        if count > 1 {
            rows.push(Row::new("Move to a new window").icon(Icon::Desktop));
            actions.push(Action::TabToWindow(tab));
        }
        rows.push(Row::with("Close tab", "Ctrl-W").icon(Icon::Close));
        actions.push(Action::TabClose(tab));
        if count > 1 {
            rows.push(Row::new("Close other tabs"));
            actions.push(Action::TabCloseOthers(tab));
        }
        if index + 1 < count {
            rows.push(Row::new("Close tabs to the right"));
            actions.push(Action::TabCloseRight(tab));
        }
        if !self.closed_of(&t.profile).is_empty() {
            rows.push(Row::with("Reopen closed tab", "Ctrl-Shift-T").icon(Icon::Rotate));
            actions.push(Action::ReopenClosed);
        }
        let at = self.wins[w].pointer;
        let height = rows_height(&rows);
        self.open_popup(w, rows, actions, Rect::new(at.0, at.1 + 4.0, 560.0, height));
    }

    // ---- profiles ------------------------------------------------------------------------

    /// The profile button's list: which profile this window is, the others to open a window in,
    /// and the colour this one's windows are.
    pub(super) fn profile_menu(&mut self, w: usize) {
        let key = self.wins[w].key;
        let profile = self.tabs.window(key).map(|x| x.profile.clone()).unwrap_or_default();
        let mut rows = vec![Row::heading("This window's profile")];
        let mut actions = vec![Action::Nothing];
        for p in self.profiles.clone() {
            let (label, mark) = if p.id == profile { (p.name.clone(), "\u{2713} this window") } else { (p.name.clone(), "open a window") };
            rows.push(Row::with(label, mark));
            actions.push(if p.id == profile { Action::Nothing } else { Action::OpenProfile(p.id) });
        }
        rows.push(Row::with("Private", if profile == PRIVATE { "\u{2713} this window" } else { "open a window" }).icon(Icon::Eye));
        actions.push(if profile == PRIVATE { Action::Nothing } else { Action::NewPrivateWindow });
        rows.push(Row::new("New profile").icon(Icon::Plus));
        actions.push(Action::NewProfile);
        if profile != PRIVATE {
            let mine = self.profile(&profile);
            rows.push(Row::new(format!("Settings of {}\u{2026}", mine.name)).icon(Icon::Options));
            actions.push(Action::ProfileSettings);
        }
        rows.push(Row::new("Lock profiles now").icon(Icon::Lock));
        actions.push(Action::LockAll);
        self.open_menu_at_right(w, rows, actions, 560.0, TOOL_H);
    }

    /// This window's profile's settings, in a list as the menu is: its name, its place among
    /// the profiles, its PIN or password, its colour.
    pub(super) fn profile_settings(&mut self, w: usize) {
        let key = self.wins[w].key;
        let id = self.tabs.window(key).map(|x| x.profile.clone()).unwrap_or_default();
        if id == PRIVATE {
            return;
        }
        let mine = self.profile(&id);
        let mut rows = vec![Row::heading(format!("Settings of {}", mine.name)), Row::with("Name", format!("{}  \u{270E}", mine.name))];
        let mut actions = vec![Action::Nothing, Action::RenameProfile];
        let place = self.profiles.iter().position(|p| p.id == id).unwrap_or(0);
        if self.profiles.len() > 1 {
            rows.push(Row::heading(format!("Place in the list: {} of {}", place + 1, self.profiles.len())));
            actions.push(Action::Nothing);
            if place > 0 {
                rows.push(Row::new("Move up").icon(Icon::Up));
                actions.push(Action::MoveProfile(-1));
            }
            if place + 1 < self.profiles.len() {
                rows.push(Row::new("Move down").icon(Icon::Down));
                actions.push(Action::MoveProfile(1));
            }
        }
        rows.push(Row::heading("Asked for when a window opens"));
        actions.push(Action::Nothing);
        match mine.lock.as_ref() {
            None => {
                rows.push(Row::with("Nothing", "\u{2713}"));
                actions.push(Action::Nothing);
                rows.push(Row::new("A PIN\u{2026}").icon(Icon::Lock));
                actions.push(Action::SetLock(LockKind::Pin));
                rows.push(Row::new("A password\u{2026}").icon(Icon::Lock));
                actions.push(Action::SetLock(LockKind::Password));
            }
            Some(lock) => {
                rows.push(Row::with(format!("Its {}", lock.kind.name()), "\u{2713}").icon(Icon::Lock));
                actions.push(Action::Nothing);
                rows.push(Row::new(format!("Change the {}\u{2026}", lock.kind.name())));
                actions.push(Action::ChangeLock);
                rows.push(Row::new("Ask for nothing\u{2026}"));
                actions.push(Action::RemoveLock);
            }
        }
        rows.push(Row::heading("Colour of its windows"));
        actions.push(Action::Nothing);
        for (name, colour) in profiles::COLOUR_CHOICES {
            rows.push(Row::with(*name, if *colour == mine.colour { "\u{2713}" } else { "" }));
            actions.push(Action::ProfileColour(*colour));
        }
        if self.profiles.len() > 1 {
            rows.push(Row::heading("Gone"));
            actions.push(Action::Nothing);
            rows.push(Row::new(format!("Remove {}\u{2026}", mine.name)).icon(Icon::Trash));
            actions.push(Action::RemoveProfile);
        }
        self.open_menu_at_right(w, rows, actions, 600.0, TOOL_H);
    }

    // ---- bookmarks -----------------------------------------------------------------------

    /// The star: the page bookmarked if it is not, and its bookmark's own menu either way --
    /// its name, the bookmarks, and taking it out again.
    pub(super) fn bookmark_page(&mut self, w: usize) {
        let key = self.wins[w].key;
        let Some(tab) = self.tabs.active(key).and_then(|t| self.tabs.tab(t)).cloned() else { return };
        if tab.profile == PRIVATE || is_start(&tab.url) || pages::internal(&tab.url).is_some() {
            return;
        }
        let existing = bookmarks::find(self.bookmarks_of(&tab.profile), &tab.url).cloned();
        let (mark, added) = match existing {
            Some(mark) => (mark, false),
            None => {
                let title = if tab.title.trim().is_empty() { tab.label() } else { tab.title.clone() };
                match bookmarks::add(&self.store, &tab.profile, "", &title, Some(&tab.url), At::End) {
                    Ok(mark) => (mark, true),
                    Err(e) => {
                        self.say(format!("The bookmark could not be kept: {e}"));
                        return;
                    }
                }
            }
        };
        self.bookmarks.remove(&tab.profile);
        self.wins[w].start = None;
        let rows = vec![
            Row::heading(if added { "Bookmarked" } else { "This page's bookmark" }),
            Row::with(mark.title.clone(), "\u{270E} rename").icon(Icon::Star),
            Row::new("Remove the bookmark").icon(Icon::Trash),
            Row::with("All bookmarks", "Ctrl-Shift-O").icon(Icon::List),
            Row::new("Done"),
        ];
        let actions = vec![Action::Nothing, Action::RenameBookmark(mark.id.clone()), Action::RemoveBookmark(mark.id.clone()), Action::Open(pages::BOOKMARKS.into()), Action::Close];
        let star = self.wins[w].buttons.iter().find(|(h, _)| *h == super::Hover::Star).map(|(_, r)| *r).unwrap_or_default();
        let height = rows_height(&rows);
        self.open_popup(w, rows, actions, Rect::new(star.right() - 640.0, star.bottom() + 8.0, 640.0, height));
    }

    /// Where a new tab opened by SpatiMesh's own menus goes: the start page in front is used,
    /// anything else is kept and a tab is opened after it.
    pub(super) fn open_page(&mut self, w: usize, url: &str) {
        let key = self.wins[w].key;
        let on_start = self.tabs.active(key).and_then(|t| self.tabs.tab(t)).is_some_and(|t| is_start(&t.url));
        // A page of SpatiMesh's own already open in this window is brought to the front.
        if pages::internal(url).is_some() {
            if let Some(open) = self.tabs.window(key).and_then(|x| x.tabs.iter().copied().find(|&t| self.tabs.tab(t).is_some_and(|t| t.url == url))) {
                self.tabs.activate_tab(open);
                self.after_tab_change(key);
                return;
            }
        }
        if on_start {
            self.go(w, url);
        } else {
            self.new_tab(w, url);
        }
    }

    // ---- what every line does ------------------------------------------------------------

    pub(super) fn act(&mut self, w: usize, action: Action) {
        let key = self.wins[w].key;
        let profile = self.tabs.window(key).map(|x| x.profile.clone()).unwrap_or_default();
        let front = self.tabs.active(key);
        self.wins[w].popup = None;
        self.wins[w].flyout = None;
        self.wins[w].dirty = true;
        match action {
            Action::NewTab => self.new_tab(w, "about:blank"),
            Action::NewWindow => {
                self.new_window(&profile, None);
            }
            Action::NewPrivateWindow => {
                self.new_window(PRIVATE, None);
            }
            Action::Bookmark => self.bookmark_page(w),
            Action::ZoomIn => self.zoom(Some(1.1)),
            Action::ZoomOut => self.zoom(Some(1.0 / 1.1)),
            Action::ZoomReset => self.zoom(None),
            Action::DeveloperTools => self.developer_tools(w),
            Action::SearchWith(name) => {
                if profile == PRIVATE {
                    self.settings.search = name;
                    let _ = self.settings.save(&self.store);
                } else if let Err(e) = spatimesh::settings::set_profile_search(&self.store, &profile, &name, None) {
                    self.say(format!("Not changed: {e}"));
                }
                self.searches.clear();
            }
            Action::CustomSearch => {
                let now = spatimesh::settings::profile_custom_search(&self.store, &profile).unwrap_or_else(|| "https://".into());
                self.ask_own_with(w, Own::CustomSearch(profile), "A search address of this profile's own, with {} where the words go -- https://example.com/search?q={}", false, &now);
            }
            Action::OpenProfile(id) => {
                self.new_window(&id, None);
            }
            Action::ProfileColour(colour) => match profiles::set_colour(&self.store, &profile, colour) {
                Ok(()) => {
                    self.profiles = profiles::list(&self.store);
                    self.dirty_all();
                }
                Err(e) => self.say(format!("Not changed: {e}")),
            },
            Action::ProfileSettings => self.profile_settings(w),
            Action::RenameProfile => {
                let name = self.profile(&profile).name;
                self.ask_own_with(w, Own::RenameProfile(profile), "This profile's new name:", false, &name);
            }
            Action::SetLock(kind) => {
                let message = match kind {
                    LockKind::Pin => "A PIN for this profile, 4 to 12 digits. A window of it opened when none is open asks for it, and so does every window of it after the lock button:",
                    LockKind::Password => "A password for this profile, at least 6 characters. A window of it opened when none is open asks for it, and so does every window of it after the lock button:",
                };
                self.ask_own_with(w, Own::NewLock(profile, kind), message, true, "");
            }
            Action::ChangeLock | Action::RemoveLock => {
                let kind = self.profile(&profile).lock.map_or(LockKind::Pin, |l| l.kind);
                let remove = action == Action::RemoveLock;
                self.ask_own_with(w, Own::OldLock(profile, remove), &format!("This profile's {} as it is now:", kind.name()), true, "");
            }
            Action::LockAll => self.lock_all(),
            Action::MoveProfile(by) => {
                if let Err(e) = profiles::move_by(&self.store, &profile, by) {
                    self.say(format!("Not moved: {e}"));
                }
                self.profiles = profiles::list(&self.store);
                self.profile_settings(w);
            }
            Action::RemoveProfile => {
                let name = self.profile(&profile).name;
                let message = format!(
                    "Remove {name}, with its bookmarks and history on every machine, and its logins and website data on this one? Its windows close. Type its name to say so:"
                );
                self.ask_own_with(w, Own::RemoveProfile(profile), &message, false, "");
            }
            Action::NewProfile => {
                let name = format!("Profile {}", self.profiles.len() + 1);
                if let Ok(new) = profiles::add(&self.store, &name) {
                    self.profiles = profiles::list(&self.store);
                    self.new_window(&new.id, None);
                }
            }
            Action::Open(url) => {
                if pages::internal(&url).is_some() {
                    self.open_page(w, &url);
                } else {
                    self.go(w, &url);
                }
            }
            Action::ToggleExtension(id) => {
                let now = spatimesh::extensions::install::list(&self.store, &profile).into_iter().find(|i| i.id == id).map(|i| !i.enabled);
                if let Some(on) = now {
                    if let Err(e) = spatimesh::extensions::install::set(&self.store, &profile, &id, Some(on), None) {
                        self.say(format!("Not changed: {e}"));
                    }
                    self.with_extensions(|rt, b| rt.refresh(b, &profile, &[]));
                }
                self.refresh_internal();
            }
            Action::LoadExtension => self.ask_own(
                w,
                Own::LoadExtension(profile),
                "Where is the extension? A folder (loaded where it is, as in developer mode), or a .zip, .crx or .xpi file:",
            ),
            Action::ReloadExtensions => {
                self.with_extensions(|rt, b| rt.reload_all(b, &profile));
                self.say("The extensions are loaded again from their files".into());
            }
            Action::RemoveExtension => self.ask_own(w, Own::RemoveExtension(profile), "Which extension is to go? Its name:"),
            Action::ExtensionOptions(id) => self.open_extension_options(w, &profile, &id),
            Action::UninstallExtension(id) => {
                let installed = spatimesh::extensions::install::list(&self.store, &profile).into_iter().find(|i| i.id == id);
                if let Some(installed) = installed {
                    let name = ext::extension_name(&installed);
                    match spatimesh::extensions::install::uninstall(&self.store, &profile, &id) {
                        Ok(()) => {
                            self.with_extensions(|rt, b| rt.refresh(b, &profile, &[]));
                            self.say(format!("{name} is removed"));
                        }
                        Err(e) => self.say(format!("Not removed: {e}")),
                    }
                }
                self.refresh_internal();
            }
            Action::PickExtensionFolder => self.pick(w, pages::PickFor::ExtensionFolder(profile)),
            Action::PickExtensionFile => self.pick(w, pages::PickFor::ExtensionFile(profile)),
            Action::AddFromStore => self.add_from_store(w),
            Action::Submenu(sub) => {
                let (rows, actions) = self.sub_rows(w, sub);
                let height = rows_height(&rows);
                let page = self.wins[w].page;
                self.open_popup(w, rows, actions, Rect::new(page.right() - 568.0, page.y + 4.0, 560.0, height));
            }
            Action::ReopenClosed => self.reopen_closed(w, None),
            Action::ReopenAt(i) => self.reopen_closed(w, Some(i)),
            Action::Close => {}
            Action::FindInPage => self.open_find(w),
            Action::SavePage => {
                if let Some(t) = front.and_then(|t| self.tabs.tab(t)).filter(|t| t.url.starts_with("http")).cloned() {
                    self.start_download(t.id, &t.url, None, Purpose::Keep);
                }
            }
            Action::OpenInNewTab(url) => self.new_tab(w, &url),
            Action::OpenInBackground(url) => {
                if let Some(opener) = front {
                    if let Some(tab) = self.tabs.new_tab(key, &url, Place::AfterActive, false, Some(opener)) {
                        self.open_in_engine(tab);
                        self.told.push(Told::TabCreated(tab));
                        self.after_tab_change(key);
                    }
                }
            }
            Action::OpenInNewWindow(url) => {
                self.new_window(&profile, Some(&url));
            }
            Action::OpenInPrivate(url) => {
                self.new_window(PRIVATE, Some(&url));
            }
            Action::Copy(text) => spatiand_app::clipboard::copy(text),
            Action::Download(url) => {
                if let Some(tab) = front {
                    self.start_download(tab, &url, None, Purpose::Keep);
                }
            }
            Action::PageCommand(command) => {
                if let Some(tab) = front {
                    let code = format!("document.execCommand('{command}')");
                    self.tell(tab, ToHost::RunScript { tab, world: String::new(), all_frames: false, code, request: 0, boot: false });
                }
            }
            Action::PageClipboard(letter) => {
                if let Some(tab) = front {
                    self.page_clipboard(tab, letter);
                }
            }
            Action::SearchSelection => {
                let words = self.context.as_ref().map(|c| c.selection.clone()).unwrap_or_default();
                if !words.is_empty() {
                    let url = self.search_for(&profile).url_for(&words);
                    self.act(w, Action::OpenInBackground(url));
                }
            }
            Action::Media(what, x, y) => {
                if let Some(tab) = front {
                    let word = match what {
                        MediaDo::PlayPause => "play",
                        MediaDo::Mute => "mute",
                        MediaDo::Loop => "loop",
                        MediaDo::Controls => "controls",
                    };
                    let code = MEDIA.replace("(X,Y,'W')", &format!("({x},{y},'{word}')"));
                    self.tell(tab, ToHost::RunScript { tab, world: String::new(), all_frames: false, code, request: 0, boot: false });
                }
            }
            Action::Back => {
                if let Some(tab) = front {
                    self.tell(tab, ToHost::Back { tab });
                }
            }
            Action::Forward => {
                if let Some(tab) = front {
                    self.tell(tab, ToHost::Forward { tab });
                }
            }
            Action::Reload => {
                if let Some(tab) = front {
                    self.reopen(tab);
                }
            }
            Action::TabNewAfter(tab) => {
                self.tabs.activate_tab(tab);
                self.new_tab(w, "about:blank");
            }
            Action::TabReload(tab) => self.reopen(tab),
            Action::TabDuplicate(tab) => {
                if let Some(url) = self.tabs.tab(tab).map(|t| t.url.clone()) {
                    self.tabs.activate_tab(tab);
                    self.new_tab(w, &url);
                }
            }
            Action::TabPin(tab, on) => {
                self.tabs.set_pinned(tab, on);
                self.session_dirty = true;
            }
            Action::TabMute(tab, muted) => self.mute_tab(tab, muted),
            Action::TabClose(tab) => self.close_tab(tab),
            Action::TabCloseOthers(tab) => {
                let others: Vec<TabId> = self.tabs.window(key).map(|x| x.tabs.iter().copied().filter(|&t| t != tab && !self.tabs.tab(t).is_some_and(|t| t.pinned)).collect()).unwrap_or_default();
                for other in others {
                    self.close_tab(other);
                }
            }
            Action::TabCloseRight(tab) => {
                let right: Vec<TabId> = self.tabs.window(key).map(|x| x.tabs.iter().copied().skip_while(|&t| t != tab).skip(1).collect()).unwrap_or_default();
                for other in right {
                    self.close_tab(other);
                }
            }
            Action::TabToWindow(tab) => self.tear_off(tab),
            Action::RenameBookmark(id) => {
                let title = self.bookmarks_of(&profile).iter().find(|b| b.id == id).map(|b| b.title.clone()).unwrap_or_default();
                self.ask_own_with(w, Own::RenameBookmark(profile, id), "The bookmark's name:", false, &title);
            }
            Action::MoveBookmark(id, by) => {
                let all = self.bookmarks_of(&profile).clone();
                if let Some(mark) = all.iter().find(|b| b.id == id) {
                    let siblings = bookmarks::children(&all, &mark.parent);
                    let at = siblings.iter().position(|b| b.id == id).unwrap_or(0);
                    let place = match by {
                        b if b < 0 && at > 0 => Some(At::Before(siblings[at - 1].id.as_str())),
                        b if b > 0 && at + 1 < siblings.len() => Some(At::After(siblings[at + 1].id.as_str())),
                        _ => None,
                    };
                    if let Some(place) = place {
                        let _ = bookmarks::move_to(&self.store, &profile, &id, &mark.parent.clone(), place);
                    }
                }
                self.bookmarks.remove(&profile);
                self.refresh_internal();
            }
            Action::RemoveBookmark(id) => {
                bookmarks::remove(&self.store, &profile, &id);
                self.bookmarks.remove(&profile);
                for win in &mut self.wins {
                    win.start = None;
                }
                self.refresh_internal();
                self.say("The bookmark is removed".into());
            }
            Action::ForgetPage(url) => {
                if let Some(kept) = self.kept.get_mut(&profile) {
                    kept.history.forget(&self.store, Some(&url), 0);
                }
                self.refresh_internal();
            }
            Action::ForgetSince(from) => {
                match self.kept.get_mut(&profile) {
                    Some(kept) => kept.history.forget(&self.store, None, from),
                    None => spatimesh::history::History::load(&self.store, &profile).forget(&self.store, None, from),
                }
                self.refresh_internal();
                self.say("The history is cleared, on every machine".into());
            }
            Action::OpenDownload(k) => {
                if let Some(item) = self.downloads.get(k).cloned() {
                    if let Err(e) = super::downloads::open_with_system(std::path::Path::new(&item.file)) {
                        self.say(format!("Not opened: {e}"));
                    }
                }
            }
            Action::ShowDownload(k) => {
                if let Some(folder) = self.downloads.get(k).and_then(|i| std::path::Path::new(&i.file).parent().map(|p| p.to_path_buf())) {
                    if let Err(e) = super::downloads::open_with_system(&folder) {
                        self.say(format!("Not opened: {e}"));
                    }
                }
            }
            Action::CancelDownload(k) => self.cancel_download(k),
            Action::RetryDownload(k) => self.retry_download(w, k),
            Action::ForgetDownload(k) => self.forget_download(k),
            Action::ClearDownloads => self.clear_downloads(&profile),
            Action::SetRestore(on) => {
                self.settings.restore = on;
                let _ = self.settings.save(&self.store);
                self.refresh_internal();
            }
            Action::SetHome(current) => {
                let url = front.and_then(|t| self.tabs.tab(t)).map(|t| t.url.clone()).filter(|u| current && u.starts_with("http"));
                self.settings.home = url.unwrap_or_else(|| "about:blank".into());
                let _ = self.settings.save(&self.store);
                self.refresh_internal();
            }
            Action::ToggleBookmarksBar => {
                self.settings.bookmarks_bar = !self.settings.bookmarks_bar;
                let _ = self.settings.save(&self.store);
                for i in 0..self.wins.len() {
                    self.lay_out_page(i);
                }
            }
            Action::Nothing => {}
        }
        self.focus_pages();
    }

    /// A tab's sound on or off: the engine's, and SpatiMesh's own player's.
    pub(super) fn mute_tab(&mut self, tab: TabId, muted: bool) {
        if let Some(t) = self.tabs.tab_mut(tab) {
            t.muted = muted;
        }
        self.tell(tab, ToHost::Mute { tab, muted });
        self.media.hush(tab, muted);
        self.session_dirty = true;
        self.dirty_bar(tab);
    }
}

impl Browser {
    /// A bookmark of the bookmarks bar's own menu.
    pub(super) fn bar_mark_menu(&mut self, w: usize, index: usize) {
        let key = self.wins[w].key;
        let profile = self.tabs.window(key).map(|x| x.profile.clone()).unwrap_or_default();
        let Some(url) = self.wins[w].bar_marks.get(index).map(|(u, _)| u.clone()) else { return };
        let Some(mark) = bookmarks::find(self.bookmarks_of(&profile), &url).cloned() else { return };
        self.bookmark_menu(w, &mark);
    }

    /// A bookmark's menu: opening it, renaming, moving, removing.
    fn bookmark_menu(&mut self, w: usize, mark: &bookmarks::Bookmark) {
        let url = mark.url.clone().unwrap_or_default();
        let rows = vec![
            Row::new("Open in new tab").icon(Icon::Plus),
            Row::new("Open in new window").icon(Icon::Desktop),
            Row::new("Rename\u{2026}"),
            Row::new("Move left").icon(Icon::Back),
            Row::new("Move right").icon(Icon::Forward),
            Row::new("Copy address").icon(Icon::Copy),
            Row::new("Remove").icon(Icon::Trash),
        ];
        let actions = vec![
            Action::OpenInBackground(url.clone()),
            Action::OpenInNewWindow(url.clone()),
            Action::RenameBookmark(mark.id.clone()),
            Action::MoveBookmark(mark.id.clone(), -1),
            Action::MoveBookmark(mark.id.clone(), 1),
            Action::Copy(url),
            Action::RemoveBookmark(mark.id.clone()),
        ];
        let at = self.wins[w].pointer;
        let height = rows_height(&rows);
        self.open_popup(w, rows, actions, Rect::new(at.0, at.1 + 4.0, 560.0, height));
    }

    /// A right click on the start page: the bookmark under the pointer's menu.
    pub(super) fn start_menu(&mut self, w: usize) {
        let key = self.wins[w].key;
        let profile = self.tabs.window(key).map(|x| x.profile.clone()).unwrap_or_default();
        let url = self.wins[w].start.as_ref().and_then(|s| s.list.hovered().and_then(|i| s.actions.get(i).cloned()));
        if let Some(Action::Open(url)) = url {
            if let Some(mark) = bookmarks::find(self.bookmarks_of(&profile), &url).cloned() {
                self.bookmark_menu(w, &mark);
            }
            return;
        }
        // A most visited site: open it, or take it out of the history.
        let (x, y) = self.wins[w].pointer;
        if let Some(url) = self.wins[w].top_sites.iter().find(|(_, r)| r.contains(x, y)).map(|(u, _)| u.clone()) {
            let rows = vec![Row::new("Open in new tab").icon(Icon::Plus), Row::new("Copy address").icon(Icon::Copy), Row::new("Remove from the history").icon(Icon::Trash)];
            let actions = vec![Action::OpenInBackground(url.clone()), Action::Copy(url.clone()), Action::ForgetPage(url)];
            let height = rows_height(&rows);
            self.open_popup(w, rows, actions, Rect::new(x, y + 4.0, 560.0, height));
        }
    }

    /// The padlock before the address: how the page came, and its site's own.
    pub(super) fn site_menu(&mut self, w: usize) {
        let key = self.wins[w].key;
        let Some(t) = self.tabs.active(key).and_then(|t| self.tabs.tab(t)).cloned() else { return };
        let host = t.url.split_once("://").map_or("", |(_, r)| r).split(['/', '?', '#']).next().unwrap_or("").to_string();
        let mut rows = Vec::new();
        let mut actions = Vec::new();
        let (said, icon) = if t.url.starts_with("https://") {
            ("The connection is private: what is sent and shown cannot be read on the way", Icon::Lock)
        } else if t.url.starts_with("http://") {
            ("Not secure: what is sent to this site and shown from it can be read on the way", Icon::Warning)
        } else if pages::internal(&t.url).is_some() || is_start(&t.url) {
            ("A page of SpatiMesh's own", Icon::Info)
        } else {
            ("A page from this machine or from the page itself", Icon::Document)
        };
        if !host.is_empty() {
            rows.push(Row::heading(host.clone()));
            actions.push(Action::Nothing);
        }
        rows.push(Row::new(said).icon(icon));
        actions.push(Action::Nothing);
        if t.url.starts_with("http") {
            rows.push(Row::new("Copy the address").icon(Icon::Copy));
            actions.push(Action::Copy(t.url.clone()));
            if t.profile != PRIVATE {
                rows.push(Row::new("Take this page out of the history").icon(Icon::Trash));
                actions.push(Action::ForgetPage(t.url.clone()));
            }
        }
        let anchor = self.wins[w].buttons.iter().find(|(h, _)| *h == super::Hover::Security).map(|(_, r)| *r).unwrap_or_default();
        let height = rows_height(&rows);
        self.open_popup(w, rows, actions, Rect::new(anchor.x, anchor.bottom() + 8.0, 720.0, height));
    }

    /// The downloads' button: those of lately, each opened or stopped from here.
    pub(super) fn downloads_menu(&mut self, w: usize) {
        let key = self.wins[w].key;
        let profile = self.tabs.window(key).map(|x| x.profile.clone()).unwrap_or_default();
        let items: Vec<super::downloads::Item> = self.downloads.of(&profile).into_iter().take(8).cloned().collect();
        let mut rows = vec![Row::heading("Downloads")];
        let mut actions = vec![Action::Nothing];
        for item in items {
            let status = match item.progress() {
                Some(p) => format!("{:.0} %", p * 100.0),
                None => item.status(),
            };
            let action = match item.state {
                super::downloads::State::Going => Action::CancelDownload(item.key),
                super::downloads::State::Done => Action::OpenDownload(item.key),
                super::downloads::State::Failed(_) => Action::RetryDownload(item.key),
            };
            let icon = if matches!(item.state, super::downloads::State::Failed(_)) { Icon::Warning } else { Icon::Download };
            rows.push(Row::with(item.name(), status).icon(icon));
            actions.push(action);
        }
        rows.push(Row::with("All downloads", "Ctrl-J").icon(Icon::List));
        actions.push(Action::Open(pages::DOWNLOADS.into()));
        let anchor = self.wins[w].buttons.iter().find(|(h, _)| *h == super::Hover::Downloads).map(|(_, r)| *r).unwrap_or_default();
        let height = rows_height(&rows);
        self.open_popup(w, rows, actions, Rect::new(anchor.right() - 600.0, anchor.bottom() + 8.0, 600.0, height));
    }
}
