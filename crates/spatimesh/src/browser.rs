//! The window process: every SpatiMesh window, what is drawn around the page, and where the
//! wearer's input goes. The pages themselves are the engine hosts' (`engines.rs`).

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::net::UnixListener;
use std::time::{Duration, Instant};

use spatiand_app::ui::{draw_icon, look, theme, Field, Icon, Input, List, Painter, Rect, Row};
use spatiand_app::{App, Button as PointerButton, Event, Key, Modifiers, WindowId};
use spatimesh::address::{self, shown};
use spatimesh::bookmarks::{self, At, Bookmark};
use spatimesh::engine::{self, Choice, DialogKind, Happened, KeyPress, TabId, View};
use spatimesh::profiles::{self, Profile, PRIVATE};
use spatimesh::settings::Settings;
use spatimesh::store::Store;
use spatimesh::tabs::{Place, Tabs, WindowKey};
use spatimesh::wire::{FromHost, Pictures, ToHost};

use crate::engines::{Host, Program};
use crate::page::PageTexture;

pub const WINDOW: (u32, u32) = (1600, 1000);
const TAB_TOP: f32 = 10.0;
const TAB_H: f32 = 46.0;
const TOOL_TOP: f32 = 62.0;
const TOOL_H: f32 = 52.0;
const PAGE_TOP: f32 = 122.0;
const MARGIN: f32 = 10.0;

/// What is under the pointer.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Hover {
    Nothing,
    Tab(TabId),
    Close(TabId),
    Plus,
    Back,
    Forward,
    Reload,
    Address,
    Star,
    Menu,
    Page,
}

/// A tab being dragged by its place on the bar.
struct Drag {
    tab: TabId,
    from: (f32, f32),
    at: (f32, f32),
    moved: bool,
    /// Off the bar far enough that letting go takes it out of this window.
    torn: bool,
}

/// What a menu's lines do.
#[derive(Debug, Clone, PartialEq)]
enum Action {
    NewTab,
    NewWindow,
    NewPrivateWindow,
    Bookmark,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    SearchWith(String),
    OpenProfile(String),
    NewProfile,
    Open(String),
    Nothing,
}

/// A list over the page: the menu, a `<select>`'s choices, the bookmarks of the start page.
struct Popup {
    rows: Vec<Row>,
    actions: Vec<Action>,
    rect: Rect,
    list: List,
    /// A `<select>`'s, which the page waits on.
    choose_for: Option<TabId>,
}

/// A script's question, waiting for its answer.
struct Question {
    kind: DialogKind,
    message: String,
    field: Field,
    ok: Rect,
    cancel: Rect,
}

/// The window process's view of a tab: its picture, and what it waits on.
#[derive(Default)]
struct TabView {
    texture: Option<PageTexture>,
    /// The size and zoom the engine was last told.
    told: Option<View>,
    question: Option<Question>,
    choices: Option<(Vec<Choice>, engine::Rect)>,
    focused: bool,
}

struct Win {
    key: WindowKey,
    id: WindowId,
    size: (u32, u32),
    ready: bool,
    dirty: bool,
    focused: bool,
    pointer: (f32, f32),
    hover: Hover,
    address: Field,
    /// The address bar has the keyboard.
    editing: bool,
    /// The bar's text is the page's address, to be replaced whole by the first thing typed.
    fresh: bool,
    drag: Option<Drag>,
    placed: Vec<(TabId, Rect, Rect)>,
    plus: Rect,
    buttons: [(Hover, Rect); 6],
    page: Rect,
    /// A button went down on the page and has not come up: moves go to the page meanwhile.
    pressed_in_page: Option<engine::Button>,
    in_page: bool,
    popup: Option<Popup>,
    start: Option<Popup>,
}

pub struct Browser {
    app: App,
    painter: Option<Painter>,
    store: Store,
    settings: Settings,
    profiles: Vec<Profile>,
    bookmarks: HashMap<String, Vec<Bookmark>>,
    tabs: Tabs,
    wins: Vec<Win>,
    views: HashMap<TabId, TabView>,
    hosts: HashMap<String, Host>,
    program: Program,
    /// A tab let go of off its window: where from and when. If the pointer arrives on another
    /// SpatiMesh window straight after, it was dropped there; if not, it becomes a window.
    in_flight: Option<(TabId, WindowKey, Instant)>,
    listener: Option<UnixListener>,
    checked: Instant,
    seen_changes: Option<std::time::SystemTime>,
}

fn command(m: Modifiers) -> bool {
    m.control || m.logo
}

fn modifiers(m: Modifiers) -> engine::Modifiers {
    engine::Modifiers { shift: m.shift, control: m.control, alt: m.alt, logo: m.logo }
}

impl Browser {
    pub fn new(app: App, listener: Option<UnixListener>) -> Browser {
        let store = Store::open();
        let settings = Settings::load(&store);
        let profiles = profiles::list(&store);
        let mut browser = Browser {
            app,
            painter: None,
            store,
            settings,
            profiles,
            bookmarks: HashMap::new(),
            tabs: Tabs::new(),
            wins: Vec::new(),
            views: HashMap::new(),
            hosts: HashMap::new(),
            program: crate::engines::program(),
            in_flight: None,
            listener,
            checked: Instant::now(),
            seen_changes: None,
        };
        if let Some(l) = browser.listener.as_ref() {
            use std::os::fd::AsRawFd;
            let fd = l.as_raw_fd();
            browser.app.watch(fd);
        }
        log::info!("SpatiMesh: pages by {:?}", browser.program);
        browser
    }

    fn profile(&self, id: &str) -> Profile {
        if id == PRIVATE {
            return profiles::private();
        }
        self.profiles.iter().find(|p| p.id == id).cloned().unwrap_or_else(|| self.profiles[0].clone())
    }

    fn bookmarks_of(&mut self, profile: &str) -> &Vec<Bookmark> {
        if !self.bookmarks.contains_key(profile) {
            let all = if profile == PRIVATE { Vec::new() } else { bookmarks::all(&self.store, profile) };
            self.bookmarks.insert(profile.to_string(), all);
        }
        &self.bookmarks[profile]
    }

    fn win_index(&self, id: WindowId) -> Option<usize> {
        self.wins.iter().position(|w| w.id == id)
    }

    fn win_of_key(&self, key: WindowKey) -> Option<usize> {
        self.wins.iter().position(|w| w.key == key)
    }

    // ---- engines ----------------------------------------------------------------------------

    fn host(&mut self, profile: &str) -> Option<&mut Host> {
        if !self.hosts.contains_key(profile) {
            match Host::start(&self.program, &self.store, profile) {
                Ok(host) => {
                    self.app.watch(host.link.raw_fd());
                    self.hosts.insert(profile.to_string(), host);
                }
                Err(e) => {
                    log::error!("no engine for {profile}: {e}");
                    return None;
                }
            }
        }
        self.hosts.get_mut(profile)
    }

    fn tell(&mut self, tab: TabId, message: ToHost) {
        let Some(profile) = self.tabs.tab(tab).map(|t| t.profile.clone()) else { return };
        if let Some(host) = self.hosts.get_mut(&profile) {
            host.send(&message);
        }
    }

    /// The size a window's pages are drawn at.
    fn view_of(&self, w: usize) -> View {
        let page = self.wins[w].page;
        View::new(page.w.max(1.0) as u32, page.h.max(1.0) as u32, self.settings.zoom())
    }

    /// Ask the engine for a tab's page, in the window it is in.
    fn open_in_engine(&mut self, tab: TabId) {
        let Some(t) = self.tabs.tab(tab) else { return };
        let (profile, url) = (t.profile.clone(), t.url.clone());
        let Some(w) = self.tabs.window_of(tab).and_then(|k| self.win_of_key(k)) else { return };
        let view = self.view_of(w);
        let window = self.wins[w].key;
        if let Some(host) = self.host(&profile) {
            host.send(&ToHost::Open { tab, url, view });
            host.send(&ToHost::Window { tab, window });
        }
        self.views.entry(tab).or_default().told = Some(view);
    }

    /// Tell every tab of a window its page's size, where that changed.
    fn size_pages(&mut self, w: usize) {
        let view = self.view_of(w);
        let Some(window) = self.tabs.window(self.wins[w].key).cloned() else { return };
        for tab in window.tabs {
            let told = self.views.get(&tab).and_then(|v| v.told);
            if told != Some(view) && told.is_some() {
                self.views.entry(tab).or_default().told = Some(view);
                self.tell(tab, ToHost::Resize { tab, view });
            }
        }
    }

    /// Which tab the keyboard goes to, told to the engines where it changed.
    fn focus_pages(&mut self) {
        let mut wanted: Vec<(TabId, bool)> = Vec::new();
        for win in &self.wins {
            let Some(window) = self.tabs.window(win.key) else { continue };
            for &tab in &window.tabs {
                let active = window.active_tab() == Some(tab);
                let overlay = win.popup.is_some() || self.views.get(&tab).is_some_and(|v| v.question.is_some());
                wanted.push((tab, active && win.focused && !win.editing && !overlay));
            }
        }
        for (tab, focused) in wanted {
            let view = self.views.entry(tab).or_default();
            if view.focused != focused {
                view.focused = focused;
                self.tell(tab, ToHost::Focus { tab, focused });
            }
        }
    }

    /// What the engine hosts have said.
    fn hear(&mut self) {
        let profiles: Vec<String> = self.hosts.keys().cloned().collect();
        for profile in profiles {
            let messages = match self.hosts.get_mut(&profile) {
                Some(host) => host.receive(),
                None => continue,
            };
            for (message, fds) in messages {
                self.heard(&profile, message, fds);
            }
            let alive = self.hosts.get_mut(&profile).is_some_and(|h| h.alive());
            if !alive {
                self.engine_gone(&profile);
            }
        }
    }

    fn engine_gone(&mut self, profile: &str) {
        let Some(host) = self.hosts.remove(profile) else { return };
        self.app.unwatch(host.link.raw_fd());
        let why = match (&host.failed, &host.engine) {
            (Some(why), _) => format!("The engine for this profile's pages could not be started: {why}"),
            (None, Some(e)) if e.starts_with("failed") => e.clone(),
            _ => "The engine showing this profile's pages stopped".to_string(),
        };
        log::warn!("the engine for {profile} has gone: {why}");
        let ids: Vec<TabId> = self.tabs.tabs().filter(|t| t.profile == profile).map(|t| t.id).collect();
        for tab in ids {
            if let Some(t) = self.tabs.tab_mut(tab) {
                t.crashed = Some(why.clone());
                t.loading = false;
            }
            if let Some(view) = self.views.get_mut(&tab) {
                view.told = None;
                view.focused = false;
            }
        }
        for win in &mut self.wins {
            win.dirty = true;
        }
    }

    fn heard(&mut self, profile: &str, message: FromHost, fds: Vec<std::os::fd::OwnedFd>) {
        match message {
            FromHost::Hello { engine } => {
                log::info!("{profile}'s pages: {engine}");
                if let Some(host) = self.hosts.get_mut(profile) {
                    host.engine = Some(engine);
                }
            }
            FromHost::Buffers { tab, width, height, format, .. } => match Pictures::map(width, height, format, fds) {
                Ok(pictures) => {
                    if let Some(host) = self.hosts.get_mut(profile) {
                        host.pictures.insert(tab, pictures);
                    }
                }
                Err(e) => log::warn!("tab {tab}'s pictures could not be mapped: {e}"),
            },
            FromHost::Picture { tab, buffer, changed } => {
                self.picture(profile, tab, buffer, &changed);
                if let Some(host) = self.hosts.get_mut(profile) {
                    host.send(&ToHost::Release { tab, buffer });
                }
                self.dirty_tab(tab);
            }
            FromHost::Happened(happened) => self.happened(happened),
        }
    }

    fn picture(&mut self, profile: &str, tab: TabId, buffer: u8, changed: &[engine::Rect]) {
        let Some(host) = self.hosts.get(profile) else { return };
        let Some(pictures) = host.pictures.get(&tab) else { return };
        let Some(bytes) = pictures.buffer(buffer) else { return };
        let (width, height, format) = (pictures.width, pictures.height, pictures.format);
        let Ok(gl) = self.app.gl() else { return };
        let view = self.views.entry(tab).or_default();
        let fits = view.texture.as_ref().is_some_and(|t| t.width == width && t.height == height);
        let changed: Vec<engine::Rect> = if fits {
            changed.to_vec()
        } else {
            if let Some(old) = view.texture.take() {
                old.destroy(gl);
            }
            match PageTexture::new(gl, width, height, format) {
                Ok(texture) => view.texture = Some(texture),
                Err(e) => {
                    log::warn!("no texture for tab {tab}: {e}");
                    return;
                }
            }
            // A new texture has nothing in it: all of the picture goes up.
            vec![engine::Rect::whole(width, height)]
        };
        if let Some(texture) = view.texture.as_ref() {
            texture.upload(gl, bytes, &changed);
        }
    }

    fn dirty_tab(&mut self, tab: TabId) {
        if let Some(w) = self.tabs.window_of(tab).and_then(|k| self.win_of_key(k)) {
            if self.tabs.active(self.wins[w].key) == Some(tab) {
                self.wins[w].dirty = true;
            }
        }
    }

    fn happened(&mut self, happened: Happened) {
        let tab = happened.tab();
        let engine = self.settings.search_engine();
        match happened {
            Happened::Title { tab, title } => {
                if let Some(t) = self.tabs.tab_mut(tab) {
                    t.title = title;
                }
                self.dirty_bar(tab);
            }
            Happened::Url { tab, url } => {
                if let Some(t) = self.tabs.tab_mut(tab) {
                    t.url = url.clone();
                    t.failed = None;
                }
                if let Some(w) = self.tabs.window_of(tab).and_then(|k| self.win_of_key(k)) {
                    if self.tabs.active(self.wins[w].key) == Some(tab) && !self.wins[w].editing {
                        self.wins[w].address.set(shown(&url, &engine));
                    }
                }
                self.dirty_bar(tab);
            }
            Happened::Progress { tab, progress, loading } => {
                if let Some(t) = self.tabs.tab_mut(tab) {
                    t.progress = progress;
                    t.loading = loading;
                    if loading {
                        t.crashed = None;
                    }
                }
                self.dirty_bar(tab);
            }
            Happened::History { tab, back, forward } => {
                if let Some(t) = self.tabs.tab_mut(tab) {
                    t.can_back = back;
                    t.can_forward = forward;
                }
                self.dirty_bar(tab);
            }
            Happened::Picture { .. } | Happened::Cursor { .. } => {}
            Happened::Sound { tab, playing } => {
                if let Some(t) = self.tabs.tab_mut(tab) {
                    t.playing = playing;
                }
                self.dirty_bar(tab);
            }
            Happened::OpenTab { opener, url, background } => {
                if let Some(window) = self.tabs.window_of(opener) {
                    if let Some(new) = self.tabs.new_tab(window, &url, Place::AfterActive, !background, Some(opener)) {
                        self.open_in_engine(new);
                        self.after_tab_change(window);
                    }
                }
            }
            Happened::CloseTab { tab } => self.close_tab(tab),
            Happened::Crashed { tab, why } => {
                if let Some(t) = self.tabs.tab_mut(tab) {
                    t.crashed = Some(why);
                    t.loading = false;
                }
                self.dirty_bar(tab);
            }
            Happened::Failed { tab, why, .. } => {
                if let Some(t) = self.tabs.tab_mut(tab) {
                    t.failed = Some(why);
                }
            }
            Happened::Choose { tab, choices, at } => {
                self.views.entry(tab).or_default().choices = Some((choices, at));
                self.show_choices(tab);
            }
            Happened::Dialog { tab, kind, message, text } => {
                let mut field = Field::default();
                field.set(text);
                field.focused = kind == DialogKind::Prompt;
                self.views.entry(tab).or_default().question = Some(Question { kind, message, field, ok: Rect::default(), cancel: Rect::default() });
                self.dirty_tab(tab);
                self.focus_pages();
            }
        }
        let _ = tab;
    }

    fn dirty_bar(&mut self, tab: TabId) {
        if let Some(w) = self.tabs.window_of(tab).and_then(|k| self.win_of_key(k)) {
            self.wins[w].dirty = true;
        }
    }

    // ---- windows and tabs -------------------------------------------------------------------

    pub fn new_window(&mut self, profile: &str, url: Option<&str>) {
        let key = self.tabs.new_window(profile);
        let title = if profile == PRIVATE { "SpatiMesh (Private)" } else { "SpatiMesh" };
        let id = self.app.open(title, WINDOW);
        let mut win = Win {
            key,
            id,
            size: WINDOW,
            ready: false,
            dirty: true,
            focused: false,
            pointer: (0.0, 0.0),
            hover: Hover::Nothing,
            address: Field::default(),
            editing: false,
            fresh: false,
            drag: None,
            placed: Vec::new(),
            plus: Rect::default(),
            buttons: [(Hover::Back, Rect::default()); 6],
            page: Rect::default(),
            pressed_in_page: None,
            in_page: false,
            popup: None,
            start: None,
        };
        win.address.hint = "Search or type an address".into();
        win.page = page_rect(WINDOW);
        self.wins.push(win);
        let home = self.settings.home.clone();
        let url = url.unwrap_or(&home).to_string();
        if let Some(tab) = self.tabs.new_tab(key, &url, Place::End, true, None) {
            self.open_in_engine(tab);
        }
        self.after_tab_change(key);
    }

    fn new_tab(&mut self, w: usize, url: &str) {
        let key = self.wins[w].key;
        if let Some(tab) = self.tabs.new_tab(key, url, Place::AfterActive, true, None) {
            self.open_in_engine(tab);
        }
        self.after_tab_change(key);
    }

    /// The tab in front of a window changed: the bar shows its address, the keyboard goes to it.
    fn after_tab_change(&mut self, key: WindowKey) {
        let Some(w) = self.win_of_key(key) else { return };
        let engine = self.settings.search_engine();
        let active = self.tabs.active(key).and_then(|t| self.tabs.tab(t)).cloned();
        let win = &mut self.wins[w];
        win.dirty = true;
        win.popup = None;
        match active {
            Some(tab) if is_start(&tab.url) => {
                // A new tab: the bar takes the keyboard, as in every browser.
                win.address.set("");
                win.editing = true;
                win.address.focused = true;
                win.fresh = false;
            }
            Some(tab) => {
                win.address.set(shown(&tab.url, &engine));
                win.editing = false;
                win.address.focused = false;
            }
            None => {}
        }
        if let Some(tab) = self.tabs.active(key) {
            if self.views.get(&tab).is_some_and(|v| v.choices.is_some()) {
                self.show_choices(tab);
            }
        }
        self.focus_pages();
    }

    fn close_tab(&mut self, tab: TabId) {
        self.tell(tab, ToHost::Close { tab });
        let profile = self.tabs.tab(tab).map(|t| t.profile.clone());
        if let Some(view) = self.views.remove(&tab) {
            if let (Some(texture), Ok(gl)) = (view.texture, self.app.gl()) {
                texture.destroy(gl);
            }
        }
        if let Some(profile) = profile.as_deref() {
            if let Some(host) = self.hosts.get_mut(profile) {
                host.pictures.remove(&tab);
            }
        }
        if let Some(closed) = self.tabs.close_tab(tab) {
            if closed.window_closed {
                if let Some(w) = self.win_of_key(closed.window) {
                    let win = self.wins.remove(w);
                    self.app.close(win.id);
                }
            } else {
                self.after_tab_change(closed.window);
            }
        }
        if let Some(profile) = profile {
            self.end_idle_host(&profile);
        }
    }

    /// A profile with no tabs left has no engine left: the private one's goes, and with it
    /// everything it kept.
    fn end_idle_host(&mut self, profile: &str) {
        if !self.tabs.tabs().any(|t| t.profile == profile) {
            if let Some(host) = self.hosts.remove(profile) {
                self.app.unwatch(host.link.raw_fd());
            }
        }
    }

    fn close_window(&mut self, w: usize) {
        let win = self.wins.remove(w);
        let profile = self.tabs.window(win.key).map(|x| x.profile.clone());
        for tab in self.tabs.close_window(win.key) {
            self.tell_profile(profile.as_deref(), ToHost::Close { tab });
            if let Some(view) = self.views.remove(&tab) {
                if let (Some(texture), Ok(gl)) = (view.texture, self.app.gl()) {
                    texture.destroy(gl);
                }
            }
        }
        self.app.close(win.id);
        if let Some(profile) = profile {
            self.end_idle_host(&profile);
        }
    }

    fn tell_profile(&mut self, profile: Option<&str>, message: ToHost) {
        if let Some(host) = profile.and_then(|p| self.hosts.get_mut(p)) {
            host.send(&message);
        }
    }

    fn move_tab(&mut self, tab: TabId, to: WindowKey, place: Option<usize>) {
        let from = self.tabs.window_of(tab);
        match self.tabs.move_tab(tab, to, place) {
            Ok(emptied) => {
                if emptied {
                    if let Some(w) = from.and_then(|k| self.win_of_key(k)) {
                        let win = self.wins.remove(w);
                        self.app.close(win.id);
                    }
                } else if let Some(from) = from {
                    self.after_tab_change(from);
                }
                // Its sound now comes from where it is.
                self.tell(tab, ToHost::Window { tab, window: to });
                self.after_tab_change(to);
                if let Some(w) = self.win_of_key(to) {
                    self.size_pages(w);
                }
            }
            Err(e) => log::info!("{e}"),
        }
    }

    fn tear_off(&mut self, tab: TabId) {
        let Some(profile) = self.tabs.tab(tab).map(|t| t.profile.clone()) else { return };
        let key = self.tabs.new_window(&profile);
        let title = if profile == PRIVATE { "SpatiMesh (Private)" } else { "SpatiMesh" };
        let id = self.app.open(title, WINDOW);
        let mut win = Win {
            key,
            id,
            size: WINDOW,
            ready: false,
            dirty: true,
            focused: false,
            pointer: (0.0, 0.0),
            hover: Hover::Nothing,
            address: Field::default(),
            editing: false,
            fresh: false,
            drag: None,
            placed: Vec::new(),
            plus: Rect::default(),
            buttons: [(Hover::Back, Rect::default()); 6],
            page: page_rect(WINDOW),
            pressed_in_page: None,
            in_page: false,
            popup: None,
            start: None,
        };
        win.address.hint = "Search or type an address".into();
        self.wins.push(win);
        self.move_tab(tab, key, None);
    }

    // ---- what the wearer does ---------------------------------------------------------------

    fn go(&mut self, w: usize, typed: &str) {
        let Some(understood) = address::understand(typed) else { return };
        let url = understood.url(&self.settings.search_engine());
        let key = self.wins[w].key;
        self.wins[w].editing = false;
        self.wins[w].address.focused = false;
        let Some(tab) = self.tabs.active(key) else { return };
        let crashed = self.tabs.tab(tab).is_some_and(|t| t.crashed.is_some());
        if let Some(t) = self.tabs.tab_mut(tab) {
            t.url = url.clone();
            t.crashed = None;
            t.loading = true;
        }
        let profile = self.tabs.tab(tab).map(|t| t.profile.clone()).unwrap_or_default();
        if crashed || !self.hosts.contains_key(&profile) || self.views.get(&tab).map_or(true, |v| v.told.is_none()) {
            self.reopen(tab);
        } else {
            self.tell(tab, ToHost::Load { tab, url: url.clone() });
        }
        self.wins[w].address.set(shown(&url, &self.settings.search_engine()));
        self.wins[w].dirty = true;
        self.focus_pages();
    }

    /// A tab whose page stopped, loaded again; its engine started again if that is what went.
    fn reopen(&mut self, tab: TabId) {
        let Some(t) = self.tabs.tab(tab) else { return };
        let profile = t.profile.clone();
        let engine_was_gone = !self.hosts.contains_key(&profile);
        if engine_was_gone || self.views.get(&tab).map_or(true, |v| v.told.is_none()) {
            if engine_was_gone {
                // Every tab of the profile goes back to its engine, not only this one.
                let all: Vec<TabId> = self.tabs.tabs().filter(|t| t.profile == profile).map(|t| t.id).collect();
                for other in all {
                    if let Some(t) = self.tabs.tab_mut(other) {
                        t.crashed = None;
                    }
                    self.open_in_engine(other);
                }
            } else {
                self.open_in_engine(tab);
            }
        } else {
            if let Some(t) = self.tabs.tab_mut(tab) {
                t.crashed = None;
            }
            self.tell(tab, ToHost::Reload { tab });
        }
        self.focus_pages();
    }

    fn toggle_bookmark(&mut self, w: usize) {
        let key = self.wins[w].key;
        let Some(tab) = self.tabs.active(key).and_then(|t| self.tabs.tab(t)).cloned() else { return };
        if tab.profile == PRIVATE || is_start(&tab.url) {
            return;
        }
        let existing = bookmarks::find(self.bookmarks_of(&tab.profile), &tab.url).map(|b| b.id.clone());
        match existing {
            Some(id) => bookmarks::remove(&self.store, &tab.profile, &id),
            None => {
                let title = if tab.title.trim().is_empty() { tab.label() } else { tab.title.clone() };
                if let Err(e) = bookmarks::add(&self.store, &tab.profile, "", &title, Some(&tab.url), At::End) {
                    log::warn!("the bookmark could not be kept: {e}");
                }
            }
        }
        self.bookmarks.remove(&tab.profile);
        self.wins[w].dirty = true;
        self.wins[w].start = None;
    }

    fn zoom(&mut self, by: Option<f32>) {
        self.settings.zoom = match by {
            Some(by) => (self.settings.zoom() * by).clamp(0.5, 3.0),
            None => 1.25,
        };
        let _ = self.settings.save(&self.store);
        for w in 0..self.wins.len() {
            self.size_pages(w);
        }
    }

    fn act(&mut self, w: usize, action: Action) {
        let profile = self.tabs.window(self.wins[w].key).map(|x| x.profile.clone()).unwrap_or_default();
        self.wins[w].popup = None;
        self.wins[w].dirty = true;
        match action {
            Action::NewTab => self.new_tab(w, "about:blank"),
            Action::NewWindow => self.new_window(&profile, None),
            Action::NewPrivateWindow => self.new_window(PRIVATE, None),
            Action::Bookmark => self.toggle_bookmark(w),
            Action::ZoomIn => self.zoom(Some(1.1)),
            Action::ZoomOut => self.zoom(Some(1.0 / 1.1)),
            Action::ZoomReset => self.zoom(None),
            Action::SearchWith(name) => {
                self.settings.search = name;
                let _ = self.settings.save(&self.store);
            }
            Action::OpenProfile(id) => self.new_window(&id, None),
            Action::NewProfile => {
                let name = format!("Profile {}", self.profiles.len() + 1);
                if let Ok(new) = profiles::add(&self.store, &name) {
                    self.profiles = profiles::list(&self.store);
                    self.new_window(&new.id, None);
                }
            }
            Action::Open(url) => self.go(w, &url),
            Action::Nothing => {}
        }
        self.focus_pages();
    }

    fn menu(&mut self, w: usize) {
        let key = self.wins[w].key;
        let profile = self.tabs.window(key).map(|x| x.profile.clone()).unwrap_or_default();
        let url = self.tabs.active(key).and_then(|t| self.tabs.tab(t)).map(|t| t.url.clone()).unwrap_or_default();
        let bookmarked = profile != PRIVATE && bookmarks::find(self.bookmarks_of(&profile), &url).is_some();
        let mut rows = vec![
            Row::with("New tab", "Ctrl-T").icon(Icon::Plus),
            Row::with("New window", "Ctrl-N"),
            Row::with("New private window", "Ctrl-Shift-N"),
        ];
        let mut actions = vec![Action::NewTab, Action::NewWindow, Action::NewPrivateWindow];
        if profile != PRIVATE && !is_start(&url) {
            rows.push(Row::with(if bookmarked { "Remove bookmark" } else { "Bookmark this page" }, "Ctrl-D").icon(Icon::Link));
            actions.push(Action::Bookmark);
        }
        rows.push(Row::with("Bigger", "Ctrl-+"));
        actions.push(Action::ZoomIn);
        rows.push(Row::with("Smaller", "Ctrl--"));
        actions.push(Action::ZoomOut);
        rows.push(Row::with("Usual size", format!("{:.0} %", self.settings.zoom() * 80.0)));
        actions.push(Action::ZoomReset);
        rows.push(Row::heading("Search with"));
        actions.push(Action::Nothing);
        let current = self.settings.search_engine().name;
        for engine in address::search_engines() {
            let mark = if engine.name == current { "\u{2713}" } else { "" };
            rows.push(Row::with(engine.name.clone(), mark).icon(Icon::Search));
            actions.push(Action::SearchWith(engine.name));
        }
        rows.push(Row::heading("Profiles"));
        actions.push(Action::Nothing);
        for p in self.profiles.clone() {
            let mark = if p.id == profile { "\u{2713}" } else { "" };
            rows.push(Row::with(p.name.clone(), mark));
            actions.push(Action::OpenProfile(p.id));
        }
        rows.push(Row::new("New profile").icon(Icon::Plus));
        actions.push(Action::NewProfile);
        let engine = self.hosts.get(&profile).and_then(|h| h.engine.clone()).unwrap_or_else(|| "starting".into());
        rows.push(Row::with("Pages by", engine));
        actions.push(Action::Nothing);
        let win = &self.wins[w];
        let width = 560.0;
        let height = (rows.len() as f32 * (theme::ROW_HEIGHT + theme::ROW_GAP) + 24.0).min(win.page.h);
        let rect = Rect::new(win.page.right() - width - 8.0, win.page.y + 4.0, width, height);
        let mut list = List::default();
        list.focused = true;
        self.wins[w].popup = Some(Popup { rows, actions, rect, list, choose_for: None });
        self.focus_pages();
    }

    fn show_choices(&mut self, tab: TabId) {
        let Some(w) = self.tabs.window_of(tab).and_then(|k| self.win_of_key(k)) else { return };
        if self.tabs.active(self.wins[w].key) != Some(tab) {
            return;
        }
        let Some((choices, at)) = self.views.get(&tab).and_then(|v| v.choices.clone()) else { return };
        let page = self.wins[w].page;
        let rows: Vec<Row> = choices
            .iter()
            .map(|c| if c.heading { Row::heading(c.label.clone()) } else { Row::with(c.label.clone(), if c.selected { "\u{2713}" } else { "" }) })
            .collect();
        let actions: Vec<Action> = (0..choices.len()).map(|_| Action::Nothing).collect();
        let width = (at.width as f32).max(320.0).min(page.w);
        let height = (rows.len() as f32 * (theme::ROW_HEIGHT + theme::ROW_GAP) + 24.0).min(page.h * 0.7);
        let x = (page.x + at.x as f32).min(page.right() - width);
        let below = page.y + at.y as f32 + at.height as f32;
        let y = if below + height > page.bottom() { (page.y + at.y as f32 - height).max(page.y) } else { below };
        let mut list = List::default();
        list.focused = true;
        list.cursor = choices.iter().position(|c| c.selected).unwrap_or(0);
        self.wins[w].popup = Some(Popup { rows, actions, rect: Rect::new(x, y, width, height), list, choose_for: Some(tab) });
        self.wins[w].dirty = true;
        self.focus_pages();
    }

    /// The popup's line `index` was picked.
    fn picked(&mut self, w: usize, index: usize) {
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
            if action == Action::Nothing {
                self.wins[w].popup = Some(popup);
                return;
            }
            self.act(w, action);
        }
        self.wins[w].dirty = true;
        self.focus_pages();
    }

    fn dismiss_popup(&mut self, w: usize) {
        if let Some(popup) = self.wins[w].popup.take() {
            if let Some(tab) = popup.choose_for {
                if let Some(view) = self.views.get_mut(&tab) {
                    view.choices = None;
                }
                self.tell(tab, ToHost::Choose { tab, index: None });
            }
        }
        self.wins[w].dirty = true;
        self.focus_pages();
    }

    fn answer(&mut self, w: usize, accepted: bool) {
        let Some(tab) = self.tabs.active(self.wins[w].key) else { return };
        let Some(question) = self.views.get_mut(&tab).and_then(|v| v.question.take()) else { return };
        let text = (question.kind == DialogKind::Prompt && accepted).then(|| question.field.text.clone());
        self.tell(tab, ToHost::Answer { tab, accepted, text });
        self.wins[w].dirty = true;
        self.focus_pages();
    }

    fn handle(&mut self, event: Event) -> bool {
        if let Event::Mode(_) | Event::Hands = event {
            return true;
        }
        let Some(id) = event.window() else { return true };
        let Some(w) = self.win_index(id) else { return true };
        match event {
            Event::Ready { .. } => {
                self.wins[w].ready = true;
                self.wins[w].dirty = true;
            }
            Event::Resized { width, height, .. } => {
                let size = (width.max(640), height.max(400));
                self.wins[w].size = size;
                self.wins[w].page = page_rect(size);
                self.app.resize(id, size);
                self.size_pages(w);
                self.wins[w].dirty = true;
                self.wins[w].popup = None;
            }
            Event::CloseRequested { .. } => {
                self.close_window(w);
                return !self.wins.is_empty();
            }
            Event::Focus { focused, .. } => {
                self.wins[w].focused = focused;
                self.wins[w].dirty = true;
                self.focus_pages();
            }
            Event::PointerMoved { x, y, .. } => self.pointer(w, x, y),
            Event::PointerLeft { .. } => {
                self.wins[w].hover = Hover::Nothing;
                if std::mem::take(&mut self.wins[w].in_page) {
                    if let Some(tab) = self.tabs.active(self.wins[w].key) {
                        self.tell(tab, ToHost::PointerLeft { tab });
                    }
                }
                self.wins[w].dirty = true;
            }
            Event::Button { button, pressed, .. } => self.button(w, button, pressed),
            Event::Scroll { dx, dy, .. } => self.scroll(w, dx, dy),
            Event::Key { key, code, pressed, repeat, text, modifiers, .. } => self.key(w, key, code, pressed, repeat, text, modifiers),
            // Pinching zooms the page, as on a touchpad.
            Event::Pinch { scale, .. } if (scale - 1.0).abs() > 0.02 => self.zoom(Some(scale)),
            _ => {}
        }
        true
    }

    fn pointer(&mut self, w: usize, x: f32, y: f32) {
        // A tab let go of over nothing, now over this window: dropped here.
        if let Some((tab, from, at)) = self.in_flight {
            if from != self.wins[w].key && at.elapsed() < Duration::from_millis(900) {
                self.in_flight = None;
                let place = self.wins[w].placed.iter().position(|(_, r, _)| x < r.x + r.w * 0.5);
                let to = self.wins[w].key;
                let same_profile = self.tabs.window(to).map(|x| x.profile.clone()) == self.tabs.tab(tab).map(|t| t.profile.clone());
                if same_profile {
                    self.move_tab(tab, to, place);
                } else {
                    self.tear_off(tab);
                }
                return;
            }
        }
        let key = self.wins[w].key;
        self.wins[w].dirty = true;
        self.wins[w].pointer = (x, y);
        let size = self.wins[w].size;
        if let Some(drag) = self.wins[w].drag.as_mut() {
            drag.at = (x, y);
            if (x - drag.from.0).abs() > 10.0 || (y - drag.from.1).abs() > 10.0 {
                drag.moved = true;
            }
            drag.torn = !(-40.0..=TOOL_TOP + TOOL_H + 60.0).contains(&y) || !(-40.0..=size.0 as f32 + 40.0).contains(&x);
            if drag.moved && !drag.torn {
                let tab = drag.tab;
                let count = self.tabs.window(key).map_or(0, |x| x.tabs.len());
                let target = self.wins[w].placed.iter().position(|(_, r, _)| x < r.x + r.w * 0.5).unwrap_or(count).min(count.saturating_sub(1));
                if let Some(from) = self.tabs.window(key).and_then(|x| x.tabs.iter().position(|&t| t == tab)) {
                    if from != target {
                        self.tabs.reorder(key, from, target);
                    }
                }
            }
            return;
        }
        if let Some(popup) = self.wins[w].popup.as_mut() {
            popup.list.input(&Input::Pointer { x, y });
            if popup.rect.contains(x, y) {
                self.wins[w].hover = Hover::Nothing;
                return;
            }
        }
        if let Some(start) = self.wins[w].start.as_mut() {
            start.list.input(&Input::Pointer { x, y });
        }
        let win = &self.wins[w];
        let hover = if let Some((id, _, close)) = win.placed.iter().find(|(_, r, _)| r.contains(x, y)) {
            if close.inset(-6.0).contains(x, y) { Hover::Close(*id) } else { Hover::Tab(*id) }
        } else if win.plus.contains(x, y) {
            Hover::Plus
        } else if let Some((which, _)) = win.buttons.iter().find(|(_, r)| r.contains(x, y)) {
            *which
        } else if win.address.contains(x, y) {
            Hover::Address
        } else if win.page.contains(x, y) {
            Hover::Page
        } else {
            Hover::Nothing
        };
        self.wins[w].hover = hover;
        let Some(tab) = self.tabs.active(key) else { return };
        let page = self.wins[w].page;
        let over_page = hover == Hover::Page && !self.covered(w, tab);
        if over_page || self.wins[w].pressed_in_page.is_some() {
            self.wins[w].in_page = true;
            let m = modifiers(self.app.modifiers());
            self.tell(tab, ToHost::Pointer { tab, x: x - page.x, y: y - page.y, button: None, modifiers: m });
        } else if std::mem::take(&mut self.wins[w].in_page) {
            self.tell(tab, ToHost::PointerLeft { tab });
        }
    }

    /// Whether something of SpatiMesh's own is over the tab's page: the start page, a question,
    /// a stopped page.
    fn covered(&self, w: usize, tab: TabId) -> bool {
        let t = self.tabs.tab(tab);
        self.wins[w].start.is_some() && t.is_some_and(|t| is_start(&t.url))
            || self.views.get(&tab).is_some_and(|v| v.question.is_some())
            || t.is_some_and(|t| t.crashed.is_some())
    }

    fn button(&mut self, w: usize, button: PointerButton, pressed: bool) {
        let (x, y) = self.wins[w].pointer;
        let key = self.wins[w].key;
        self.wins[w].dirty = true;
        let engine_button = match button {
            PointerButton::Left => engine::Button::Left,
            PointerButton::Right => engine::Button::Right,
            PointerButton::Middle => engine::Button::Middle,
            PointerButton::Other(275) => engine::Button::Back,
            PointerButton::Other(276) => engine::Button::Forward,
            PointerButton::Other(_) => return,
        };
        // A page that has a button held gets its release wherever the pointer is.
        if !pressed {
            if let Some(held) = self.wins[w].pressed_in_page.take() {
                if let Some(tab) = self.tabs.active(key) {
                    let page = self.wins[w].page;
                    let m = modifiers(self.app.modifiers());
                    self.tell(tab, ToHost::Pointer { tab, x: x - page.x, y: y - page.y, button: Some((held, false)), modifiers: m });
                }
                return;
            }
        }
        if let Some(popup) = self.wins[w].popup.as_mut() {
            if button == PointerButton::Left {
                if popup.rect.contains(x, y) {
                    if let Some(index) = popup.list.input(&Input::Press { pressed }) {
                        self.picked(w, index);
                    }
                    return;
                }
                if pressed {
                    self.dismiss_popup(w);
                    return;
                }
            }
        }
        let Some(tab) = self.tabs.active(key) else { return };
        // SpatiMesh's own things over the page.
        if button == PointerButton::Left && self.wins[w].hover == Hover::Page {
            if let Some(question) = self.views.get(&tab).and_then(|v| v.question.as_ref()) {
                if !pressed {
                    if question.ok.contains(x, y) {
                        self.answer(w, true);
                    } else if question.cancel.contains(x, y) {
                        self.answer(w, false);
                    }
                }
                return;
            }
            if self.tabs.tab(tab).is_some_and(|t| t.crashed.is_some()) {
                if !pressed {
                    self.reopen(tab);
                }
                return;
            }
            if self.tabs.tab(tab).is_some_and(|t| is_start(&t.url)) {
                if let Some(start) = self.wins[w].start.as_mut() {
                    if let Some(index) = start.list.input(&Input::Press { pressed }) {
                        if let Some(Action::Open(url)) = start.actions.get(index).cloned() {
                            self.go(w, &url);
                        }
                    }
                    return;
                }
            }
        }
        if self.wins[w].hover == Hover::Page && !self.covered(w, tab) {
            if pressed {
                // The page has the keyboard once it is clicked.
                if self.wins[w].editing {
                    self.wins[w].editing = false;
                    self.wins[w].address.focused = false;
                    let url = self.tabs.tab(tab).map(|t| t.url.clone()).unwrap_or_default();
                    let shown = shown(&url, &self.settings.search_engine());
                    self.wins[w].address.set(shown);
                    self.focus_pages();
                }
                self.wins[w].pressed_in_page = Some(engine_button);
            }
            let page = self.wins[w].page;
            let m = modifiers(self.app.modifiers());
            self.tell(tab, ToHost::Pointer { tab, x: x - page.x, y: y - page.y, button: Some((engine_button, pressed)), modifiers: m });
            return;
        }
        if button == PointerButton::Middle && pressed {
            // A middle click on a tab closes it.
            if let Hover::Tab(t) | Hover::Close(t) = self.wins[w].hover {
                self.close_tab(t);
            }
            return;
        }
        if button != PointerButton::Left {
            return;
        }
        if pressed {
            match self.wins[w].hover {
                Hover::Tab(t) => {
                    if let Some(at) = self.tabs.window(key).and_then(|x| x.tabs.iter().position(|&id| id == t)) {
                        self.tabs.activate(key, at);
                        self.after_tab_change(key);
                    }
                    self.wins[w].drag = Some(Drag { tab: t, from: (x, y), at: (x, y), moved: false, torn: false });
                }
                Hover::Address => {
                    let win = &mut self.wins[w];
                    if !win.editing {
                        win.editing = true;
                        win.address.focused = true;
                        win.fresh = true;
                        self.focus_pages();
                    }
                }
                _ => {}
            }
            return;
        }
        // Let go.
        if let Some(drag) = self.wins[w].drag.take() {
            if drag.torn {
                let alone = self.tabs.window(key).is_some_and(|x| x.tabs.len() == 1);
                if alone && self.wins.len() == 1 {
                    return;
                }
                self.in_flight = Some((drag.tab, key, Instant::now()));
            }
            return;
        }
        match self.wins[w].hover {
            Hover::Close(t) => self.close_tab(t),
            Hover::Plus => self.new_tab(w, "about:blank"),
            Hover::Back => self.tell(tab, ToHost::Back { tab }),
            Hover::Forward => self.tell(tab, ToHost::Forward { tab }),
            Hover::Reload => {
                if self.tabs.tab(tab).is_some_and(|t| t.loading) {
                    self.tell(tab, ToHost::Stop { tab });
                } else {
                    self.reopen(tab);
                }
            }
            Hover::Star => self.toggle_bookmark(w),
            Hover::Menu => {
                if self.wins[w].popup.is_some() {
                    self.dismiss_popup(w);
                } else {
                    self.menu(w);
                }
            }
            _ => {}
        }
    }

    fn scroll(&mut self, w: usize, dx: f32, dy: f32) {
        self.wins[w].dirty = true;
        let (x, y) = self.wins[w].pointer;
        if let Some(popup) = self.wins[w].popup.as_mut() {
            if popup.rect.contains(x, y) {
                popup.list.input(&Input::Scroll { dx, dy });
                return;
            }
        }
        let key = self.wins[w].key;
        let Some(tab) = self.tabs.active(key) else { return };
        if self.tabs.tab(tab).is_some_and(|t| is_start(&t.url)) {
            if let Some(start) = self.wins[w].start.as_mut() {
                start.list.input(&Input::Scroll { dx, dy });
            }
            return;
        }
        if self.wins[w].hover == Hover::Page && !self.covered(w, tab) {
            let page = self.wins[w].page;
            let m = modifiers(self.app.modifiers());
            self.tell(tab, ToHost::Scroll { tab, x: x - page.x, y: y - page.y, dx, dy, modifiers: m });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn key(&mut self, w: usize, key: Key, code: u32, pressed: bool, repeat: bool, text: Option<String>, m: Modifiers) {
        let window = self.wins[w].key;
        self.wins[w].dirty = true;
        let Some(tab) = self.tabs.active(window) else { return };
        let input = Input::Key { key, text: text.clone(), modifiers: m };
        // A question first: the page waits on it.
        if self.views.get(&tab).is_some_and(|v| v.question.is_some()) {
            if !pressed {
                return;
            }
            match key {
                Key::Enter => self.answer(w, true),
                Key::Escape => self.answer(w, false),
                _ => {
                    if let Some(question) = self.views.get_mut(&tab).and_then(|v| v.question.as_mut()) {
                        question.field.input(&input);
                    }
                }
            }
            return;
        }
        if let Some(popup) = self.wins[w].popup.as_mut() {
            if !pressed {
                return;
            }
            if key == Key::Escape {
                self.dismiss_popup(w);
                return;
            }
            if let Some(index) = popup.list.input(&input) {
                self.picked(w, index);
            }
            return;
        }
        // SpatiMesh's own keys: Control, or the Command key on a Mac.
        if pressed && command(m) {
            let letter = match key {
                Key::Char(c) => Some(c.to_ascii_lowercase()),
                _ => None,
            };
            let handled = match (letter, key) {
                (Some('t'), _) => {
                    self.new_tab(w, "about:blank");
                    true
                }
                (Some('w'), _) => {
                    self.close_tab(tab);
                    true
                }
                (Some('n'), _) if m.shift => {
                    self.new_window(PRIVATE, None);
                    true
                }
                (Some('n'), _) => {
                    let profile = self.tabs.window(window).map(|x| x.profile.clone()).unwrap_or_default();
                    self.new_window(&profile, None);
                    true
                }
                (Some('l'), _) => {
                    let win = &mut self.wins[w];
                    win.editing = true;
                    win.address.focused = true;
                    win.fresh = true;
                    self.focus_pages();
                    true
                }
                (Some('r'), _) => {
                    self.reopen(tab);
                    true
                }
                (Some('d'), _) => {
                    self.toggle_bookmark(w);
                    true
                }
                (Some('+' | '='), _) => {
                    self.zoom(Some(1.1));
                    true
                }
                (Some('-'), _) => {
                    self.zoom(Some(1.0 / 1.1));
                    true
                }
                (Some('0'), _) => {
                    self.zoom(None);
                    true
                }
                (Some(d @ '1'..='9'), _) => {
                    let count = self.tabs.window(window).map_or(0, |x| x.tabs.len());
                    let index = if d == '9' { count.saturating_sub(1) } else { (d as usize - '1' as usize).min(count.saturating_sub(1)) };
                    self.tabs.activate(window, index);
                    self.after_tab_change(window);
                    true
                }
                (_, Key::Tab) => {
                    let (count, active) = self.tabs.window(window).map_or((0, 0), |x| (x.tabs.len(), x.active));
                    if count > 0 {
                        let next = if m.shift { (active + count - 1) % count } else { (active + 1) % count };
                        self.tabs.activate(window, next);
                        self.after_tab_change(window);
                    }
                    true
                }
                _ => false,
            };
            if handled {
                return;
            }
        }
        if pressed && m.alt && !self.wins[w].editing {
            match key {
                Key::Left => {
                    self.tell(tab, ToHost::Back { tab });
                    return;
                }
                Key::Right => {
                    self.tell(tab, ToHost::Forward { tab });
                    return;
                }
                _ => {}
            }
        }
        if pressed && code == 63 {
            // F5.
            self.reopen(tab);
            return;
        }
        // The address bar.
        if self.wins[w].editing {
            if !pressed {
                return;
            }
            let win = &mut self.wins[w];
            match key {
                Key::Escape => {
                    win.editing = false;
                    win.address.focused = false;
                    let url = self.tabs.tab(tab).map(|t| t.url.clone()).unwrap_or_default();
                    if !is_start(&url) {
                        self.wins[w].address.set(shown(&url, &self.settings.search_engine()));
                    }
                    self.focus_pages();
                }
                Key::Enter => {
                    let typed = win.address.text.clone();
                    self.go(w, &typed);
                }
                _ => {
                    // The first thing typed into a freshly chosen bar replaces what was there.
                    let types = text.as_deref().is_some_and(|t| !t.is_empty()) && !command(m);
                    if win.fresh && (types || matches!(key, Key::Backspace | Key::Delete)) {
                        win.address.set("");
                    }
                    if !matches!(key, Key::Left | Key::Right | Key::Home | Key::End) || types {
                        win.fresh = false;
                    }
                    win.address.input(&input);
                }
            }
            return;
        }
        if pressed && key == Key::Escape && self.tabs.tab(tab).is_some_and(|t| t.loading) {
            self.tell(tab, ToHost::Stop { tab });
        }
        // Everything else is the page's.
        if self.covered(w, tab) {
            if pressed && key == Key::Enter && self.tabs.tab(tab).is_some_and(|t| t.crashed.is_some()) {
                self.reopen(tab);
            }
            return;
        }
        let key = KeyPress { code, key, text, pressed, repeat, modifiers: modifiers(m) };
        self.tell(tab, ToHost::Key { tab, key });
    }

    // ---- drawing ----------------------------------------------------------------------------

    fn draw(&mut self) -> Result<(), String> {
        for w in 0..self.wins.len() {
            if !self.wins[w].ready || !self.wins[w].dirty {
                continue;
            }
            self.wins[w].dirty = false;
            let key = self.wins[w].key;
            let Some(window) = self.tabs.window(key).cloned() else { continue };
            let profile = self.profile(&window.profile);
            let active = window.active_tab().and_then(|t| self.tabs.tab(t)).cloned();
            // The start page's list: the profile's bookmarks.
            if active.as_ref().is_some_and(|t| is_start(&t.url)) && self.wins[w].start.is_none() {
                let marks: Vec<Bookmark> = bookmarks::children(self.bookmarks_of(&profile.id), "").into_iter().filter(|b| !b.is_folder()).cloned().collect();
                let rows = marks.iter().map(|b| Row::with(b.title.clone(), host_of(b.url.as_deref().unwrap_or(""))).icon(Icon::Link)).collect();
                let actions = marks.iter().map(|b| Action::Open(b.url.clone().unwrap_or_default())).collect();
                self.wins[w].start = Some(Popup { rows, actions, rect: Rect::default(), list: List::default(), choose_for: None });
            }
            let title = match &active {
                Some(t) if profile.is_private() => format!("{} \u{2014} SpatiMesh (Private)", t.label()),
                Some(t) => format!("{} \u{2014} SpatiMesh", t.label()),
                None => "SpatiMesh".to_string(),
            };
            let id = self.wins[w].id;
            self.app.set_title(id, &title);
            let engine_note = self.hosts.get(&window.profile).and_then(|h| h.engine.clone());
            let program_note = match &self.program {
                Program::Fake(why) => Some(format!("No web engine: {why}. Pages are flat colours.")),
                _ => None,
            };
            let starting = engine_note.is_none();
            let (painter, wins, views, tabs) = (&mut self.painter, &mut self.wins, &self.views, &self.tabs);
            let win = &mut wins[w];
            let size = win.size;
            let marked = active.as_ref().is_some_and(|t| profile.id != PRIVATE && self.bookmarks.get(&profile.id).is_some_and(|b| bookmarks::find(b, &t.url).is_some()));
            self.app.draw(id, |gl, picture| {
                let p = painter.get_or_insert_with(|| Painter::new(gl).expect("the painter"));
                p.begin(gl, picture, picture.1 as f32 / size.1 as f32, theme::GROUND);
                look::backdrop(p, gl);
                let all = p.bounds();
                draw_tabs(p, gl, win, &window, tabs, &profile, all);
                draw_toolbar(p, gl, win, active.as_ref(), marked, &profile, all);
                // The page.
                let page = win.page;
                p.fill(gl, page, 12.0, [1.0, 1.0, 1.0, 1.0]);
                if let Some(tab) = active.as_ref() {
                    let view = views.get(&tab.id);
                    if let Some(texture) = view.and_then(|v| v.texture.as_ref()) {
                        // The picture at its own size, from the top left: a page being resized
                        // shows its last picture until the next arrives, not stretched.
                        let shown = Rect::new(page.x, page.y, (texture.width as f32).min(page.w), (texture.height as f32).min(page.h));
                        let uv = [0.0, 0.0, shown.w / texture.width as f32, shown.h / texture.height as f32];
                        p.image(gl, texture.texture(), shown, uv, 0.0, [1.0, 1.0, 1.0, 1.0], true);
                    } else {
                        p.fill(gl, page, 12.0, theme::GROUND);
                        let note = if starting { "Starting the web engine\u{2026}" } else { "" };
                        look::message(p, gl, page, note, theme::INK_HINT);
                    }
                    if is_start(&tab.url) {
                        draw_start(p, gl, win, &profile, program_note.as_deref());
                    } else if let Some(why) = &tab.crashed {
                        draw_notice(p, gl, page, "This page stopped", why, "[Enter] Load it again    The other tabs are as they were.");
                    }
                    if let Some(question) = view.and_then(|v| v.question.as_ref()) {
                        draw_question(p, gl, page, question);
                    }
                }
                if let Some(popup) = win.popup.as_mut() {
                    look::glass(p, gl, popup.rect, 18.0, 1.0);
                    popup.list.draw(p, gl, popup.rect.inset(12.0), &popup.rows);
                }
                if let Some(drag) = win.drag.as_ref().filter(|d| d.torn) {
                    let label = tabs.tab(drag.tab).map(|t| t.label()).unwrap_or_default();
                    let ghost = Rect::new(drag.at.0 - 160.0, drag.at.1 - 26.0, 320.0, 52.0);
                    p.glow(gl, ghost, 26.0, [0.30, 0.62, 1.0, 0.45], 26.0);
                    look::glass(p, gl, ghost, 26.0, 1.0);
                    p.label(gl, &label, ghost.inset(16.0), theme::ROW_EM * 0.8, theme::INK, 0.5);
                }
                p.end(gl);
            })?;
            // What was drawn of the question, for clicks on it.
            if let Some(tab) = active.as_ref() {
                if let Some(q) = self.views.get_mut(&tab.id).and_then(|v| v.question.as_mut()) {
                    let (ok, cancel) = question_buttons(self.wins[w].page, q.kind);
                    q.ok = ok;
                    q.cancel = cancel;
                }
            }
        }
        Ok(())
    }

    fn accept_other_starts(&mut self) {
        let mut asked: Vec<(bool, Option<String>)> = Vec::new();
        if let Some(listener) = self.listener.as_ref() {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut text = String::new();
                let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                let _ = stream.read_to_string(&mut text);
                let mut words = text.split_whitespace();
                if words.next() == Some("window") {
                    let private = words.next() == Some("private");
                    asked.push((private, words.next().map(str::to_string)));
                }
            }
        }
        for (private, url) in asked {
            let profile = if private { PRIVATE.to_string() } else { self.profiles[0].id.clone() };
            self.new_window(&profile, url.as_deref());
        }
    }

    /// What the synchroniser brought from another machine: settings, profiles, bookmarks.
    fn notice_sync(&mut self) {
        if self.checked.elapsed() < Duration::from_secs(3) {
            return;
        }
        self.checked = Instant::now();
        let latest = newest_change(self.store.shared_dir());
        if latest != self.seen_changes {
            if self.seen_changes.is_some() {
                self.settings = Settings::load(&self.store);
                self.profiles = profiles::list(&self.store);
                self.bookmarks.clear();
                for win in &mut self.wins {
                    win.start = None;
                    win.dirty = true;
                }
            }
            self.seen_changes = latest;
        }
    }

    pub fn run(&mut self) -> Result<(), String> {
        loop {
            let busy = self.tabs.tabs().any(|t| t.loading) || self.in_flight.is_some();
            let wait = if busy { 30 } else { 500 };
            for event in self.app.wait(Some(Duration::from_millis(wait)))? {
                if !self.handle(event) {
                    return Ok(());
                }
            }
            self.hear();
            // A tab let go of that no window claimed becomes a window of its own.
            if let Some((tab, _, at)) = self.in_flight {
                if at.elapsed() >= Duration::from_millis(600) {
                    self.in_flight = None;
                    self.tear_off(tab);
                }
            }
            self.accept_other_starts();
            self.notice_sync();
            if self.wins.is_empty() {
                return Ok(());
            }
            self.draw()?;
        }
    }
}

/// The page's place in a window of `size`.
pub fn page_rect(size: (u32, u32)) -> Rect {
    Rect::new(MARGIN, PAGE_TOP, size.0 as f32 - MARGIN * 2.0, size.1 as f32 - PAGE_TOP - MARGIN)
}

/// A tab with no page in it: SpatiMesh's start page is drawn instead.
pub fn is_start(url: &str) -> bool {
    url.is_empty() || url == "about:blank"
}

fn host_of(url: &str) -> String {
    url.split_once("://").map_or(url, |(_, rest)| rest).split(['/', '?', '#']).next().unwrap_or("").to_string()
}

/// When anything in a folder last changed, however deep.
fn newest_change(dir: &std::path::Path) -> Option<std::time::SystemTime> {
    let mut newest = std::fs::metadata(dir).and_then(|m| m.modified()).ok();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            }
            if let Ok(time) = meta.modified() {
                newest = Some(newest.map_or(time, |n| n.max(time)));
            }
        }
    }
    newest
}

fn draw_tabs(p: &mut Painter, gl: &spatiand_app::Gl, win: &mut Win, window: &spatimesh::tabs::Window, tabs: &Tabs, profile: &Profile, all: Rect) {
    let bar = Rect::new(all.x + 12.0, TAB_TOP, all.w - 24.0, TAB_H);
    let plus_w = 50.0;
    let n = window.tabs.len().max(1);
    let width = ((bar.w - plus_w - 16.0) / n as f32).clamp(150.0, 330.0);
    win.placed.clear();
    let mut x = bar.x;
    for (index, &id) in window.tabs.iter().enumerate() {
        let Some(tab) = tabs.tab(id) else { continue };
        let rect = Rect::new(x, bar.y, width - 8.0, bar.h);
        x += width;
        if rect.right() > bar.right() - plus_w {
            break;
        }
        let active = index == window.active;
        let hover = matches!(win.hover, Hover::Tab(t) | Hover::Close(t) if t == id);
        let rect = match win.drag.as_ref() {
            Some(d) if d.tab == id && d.moved && !d.torn => Rect { x: (d.at.0 - rect.w * 0.5).clamp(bar.x, bar.right() - plus_w - rect.w), ..rect },
            _ => rect,
        };
        if active {
            p.glow(gl, rect, 14.0, [0.30, 0.62, 1.0, if win.focused { 0.18 } else { 0.06 }], 16.0);
            p.gradient(gl, rect, 14.0, [1.0, 1.0, 1.0, 0.11], [1.0, 1.0, 1.0, 0.05]);
            let c = profile.colour;
            p.fill(gl, Rect::new(rect.x + 16.0, rect.bottom() - 3.0, rect.w - 32.0, 2.5), 1.25, [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 0.9]);
        } else if hover {
            p.fill(gl, rect, 14.0, theme::HOVER);
        }
        // How the tab is: loading, sounding, stopped.
        let colour = if tab.crashed.is_some() {
            look::TROUBLE
        } else if tab.loading {
            look::BLUE
        } else if tab.playing {
            look::VIOLET
        } else if active {
            look::TEAL
        } else {
            theme::INK_HINT
        };
        p.fill(gl, Rect::new(rect.x + 16.0, rect.y + rect.h * 0.5 - 4.0, 8.0, 8.0), 4.0, colour);
        let close = Rect::new(rect.right() - 38.0, rect.y + rect.h * 0.5 - 13.0, 26.0, 26.0);
        let label = Rect { x: rect.x + 34.0, w: rect.w - 34.0 - if active || hover { 44.0 } else { 14.0 }, ..rect };
        p.label(gl, &tab.label(), label, theme::ROW_EM * 0.72, if active { theme::INK } else { theme::INK_ROW }, 0.0);
        if active || hover {
            if win.hover == Hover::Close(id) {
                p.fill(gl, close, 13.0, [1.0, 1.0, 1.0, 0.14]);
            }
            draw_icon(p, gl, Icon::Close, close.inset(4.0), theme::INK_DETAIL);
        }
        win.placed.push((id, rect, close));
    }
    win.plus = Rect::new(x.min(bar.right() - plus_w), bar.y + (bar.h - 44.0) * 0.5, 44.0, 44.0);
    if win.hover == Hover::Plus {
        p.fill(gl, win.plus, 22.0, theme::HOVER);
    }
    draw_icon(p, gl, Icon::Plus, win.plus.inset(10.0), theme::INK_DETAIL);
}

fn draw_toolbar(p: &mut Painter, gl: &spatiand_app::Gl, win: &mut Win, tab: Option<&spatimesh::tabs::Tab>, marked: bool, profile: &Profile, all: Rect) {
    let mut bar = Rect::new(all.x + 12.0, TOOL_TOP, all.w - 24.0, TOOL_H);
    let side = TOOL_H;
    let button = |p: &mut Painter, rect: Rect, which: Hover, icon: Icon, enabled: bool, lit: bool| {
        if win.hover == which && enabled {
            p.fill(gl, rect, side * 0.5, [1.0, 1.0, 1.0, 0.11]);
        }
        let ink = if !enabled {
            [1.0, 1.0, 1.0, 0.25]
        } else if lit {
            look::BLUE
        } else {
            theme::INK_DETAIL
        };
        draw_icon(p, gl, icon, rect.inset(12.0), ink);
        rect
    };
    let loading = tab.is_some_and(|t| t.loading);
    let back = button(p, bar.take_left(side), Hover::Back, Icon::Back, tab.is_some_and(|t| t.can_back), false);
    bar.take_left(4.0);
    let forward = button(p, bar.take_left(side), Hover::Forward, Icon::Forward, tab.is_some_and(|t| t.can_forward), false);
    bar.take_left(4.0);
    let reload = button(p, bar.take_left(side), Hover::Reload, if loading { Icon::Close } else { Icon::Refresh }, tab.is_some(), false);
    bar.take_left(12.0);
    let menu = button(p, bar.take_right(side), Hover::Menu, Icon::Options, true, false);
    bar.take_right(4.0);
    let private = profile.is_private();
    let star = button(p, bar.take_right(side), Hover::Star, Icon::Link, !private && tab.is_some_and(|t| !is_start(&t.url)), marked);
    bar.take_right(12.0);
    if private {
        let note = bar.take_right(130.0);
        look::chip(p, gl, note.x, note.y + (note.h - 34.0) * 0.5, "Private", look::VIOLET);
    }
    win.address.draw(p, gl, bar);
    if loading {
        // How much of the page has come, along the foot of the bar.
        let progress = tab.map_or(0.0, |t| t.progress).clamp(0.05, 1.0);
        let line = Rect::new(bar.x + 8.0, bar.bottom() - 3.0, (bar.w - 16.0) * progress, 3.0);
        p.fill(gl, line, 1.5, look::BLUE);
    }
    win.buttons = [(Hover::Back, back), (Hover::Forward, forward), (Hover::Reload, reload), (Hover::Star, star), (Hover::Menu, menu), (Hover::Nothing, Rect::default())];
}

fn draw_start(p: &mut Painter, gl: &spatiand_app::Gl, win: &mut Win, profile: &Profile, note: Option<&str>) {
    let page = win.page;
    p.fill(gl, page, 12.0, theme::GROUND);
    let mut all = page.inset(theme::PAD);
    let beside = if profile.is_private() { "Private: nothing in this window is kept".to_string() } else { profile.name.clone() };
    look::header(p, gl, &mut all, "SpatiMesh", &beside);
    if let Some(note) = note {
        p.label(gl, note, all.take_top(44.0), theme::DETAIL_EM, look::TROUBLE, 0.0);
    }
    let Some(start) = win.start.as_mut() else { return };
    if start.rows.is_empty() {
        let hint = if profile.is_private() { "Type an address or a search above." } else { "Type an address or a search above. Bookmarks (Ctrl-D) are kept here." };
        look::message(p, gl, all, hint, theme::INK_HINT);
        return;
    }
    look::shelf(p, gl, &mut all, "Bookmarks", true);
    start.rect = all;
    start.list.draw(p, gl, all, &start.rows);
}

fn draw_notice(p: &mut Painter, gl: &spatiand_app::Gl, page: Rect, title: &str, why: &str, hint: &str) {
    p.fill(gl, page, 12.0, theme::GROUND);
    let pane = Rect::new(page.x + page.w * 0.5 - 380.0, page.y + page.h * 0.5 - 120.0, 760.0, 240.0);
    look::glass(p, gl, pane, 24.0, 1.0);
    let mut inner = pane.inset(28.0);
    p.label(gl, title, inner.take_top(52.0), theme::TITLE_EM * 0.8, look::TROUBLE, 0.0);
    p.label(gl, why, inner.take_top(44.0), theme::DETAIL_EM, theme::INK_DETAIL, 0.0);
    inner.take_top(16.0);
    look::keys(p, gl, inner.take_top(40.0), hint, 0.0, 1.0);
}

fn question_buttons(page: Rect, kind: DialogKind) -> (Rect, Rect) {
    let pane = question_pane(page);
    let ok = Rect::new(pane.right() - 28.0 - 160.0, pane.bottom() - 28.0 - theme::ROW_HEIGHT, 160.0, theme::ROW_HEIGHT);
    let cancel = if kind == DialogKind::Alert { Rect::default() } else { Rect { x: ok.x - 180.0, ..ok } };
    (ok, cancel)
}

fn question_pane(page: Rect) -> Rect {
    Rect::new(page.x + page.w * 0.5 - 420.0, page.y + 60.0, 840.0, 300.0)
}

fn draw_question(p: &mut Painter, gl: &spatiand_app::Gl, page: Rect, question: &Question) {
    p.fill(gl, page, 12.0, [0.0, 0.0, 0.0, 0.45]);
    let pane = question_pane(page);
    look::glass(p, gl, pane, 24.0, 1.0);
    let mut inner = pane.inset(28.0);
    let heading = match question.kind {
        DialogKind::Leave => "Leave this page?",
        _ => "The page asks",
    };
    p.label(gl, heading, inner.take_top(48.0), theme::TITLE_EM * 0.7, theme::INK, 0.0);
    p.text(gl, &question.message, inner.x, inner.y, theme::DETAIL_EM, inner.w, theme::INK_DETAIL);
    inner.take_top(110.0);
    if question.kind == DialogKind::Prompt {
        let mut field = Field::default();
        field.focused = true;
        field.set(question.field.text.clone());
        field.draw(p, gl, inner.take_top(theme::ROW_HEIGHT));
    }
    let (ok, cancel) = question_buttons(page, question.kind);
    p.fill(gl, ok, theme::ROW_RADIUS, theme::SELECTED);
    p.label(gl, if question.kind == DialogKind::Leave { "Leave" } else { "OK" }, ok, theme::ROW_EM * 0.86, theme::INK, 0.5);
    if question.kind != DialogKind::Alert {
        p.fill(gl, cancel, theme::ROW_RADIUS, theme::RAISED);
        p.label(gl, if question.kind == DialogKind::Leave { "Stay" } else { "Cancel" }, cancel, theme::ROW_EM * 0.86, theme::INK_ROW, 0.5);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_fills_the_window_below_the_bars() {
        let page = page_rect(WINDOW);
        assert_eq!(page.y, PAGE_TOP);
        assert_eq!(page.x, MARGIN);
        assert_eq!(page.right(), WINDOW.0 as f32 - MARGIN);
        assert_eq!(page.bottom(), WINDOW.1 as f32 - MARGIN);
        const { assert!(PAGE_TOP >= TOOL_TOP + TOOL_H, "the page starts below the toolbar") };
    }

    #[test]
    fn start_pages_and_hosts() {
        assert!(is_start("about:blank") && is_start(""));
        assert!(!is_start("https://example.com/"));
        assert_eq!(host_of("https://example.com:8080/a?b#c"), "example.com:8080");
    }
}
