//! The window process: every SpatiMesh window, what is drawn around the page, and where the
//! wearer's input goes. The pages themselves are the engine hosts' (`engines.rs`).

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::os::unix::net::UnixListener;
use std::time::{Duration, Instant};

use spatiand_app::ui::{draw_icon, look, theme, Field, Icon, Input, List, Painter, Rect, Row, Texture};
use spatiand_app::{App, Button as PointerButton, Event, Key, Modifiers, WindowId};
use spatimesh::address::{self, shown, SearchEngine};
use spatimesh::bookmarks::{self, Bookmark};
use spatimesh::engine::{self, Choice, DialogKind, Happened, KeyPress, TabId, View};
use spatimesh::profiles::{self, LockKind, Profile, PRIVATE};
use spatimesh::settings::Settings;
use spatimesh::store::Store;
use spatimesh::tabs::{Place, Tabs, WindowKey};
use spatimesh::wire::{FromHost, Pictures, ToHost};

use crate::engines::{Host, Program};
use crate::page::PageTexture;

mod downloads;
mod ext;
mod find;
mod kept;
mod menus;
mod omni;
mod pages;
mod session;
#[cfg(feature = "media")]
mod media;
#[cfg(not(feature = "media"))]
#[path = "browser/media_none.rs"]
mod media;

use ext::{ExtPopup, Told};
use spatimesh::extensions::runtime::Runtime;

/// Script requests of the page clipboard's, told apart from an extension's (which count from 1).
const PAGE_CLIPBOARD: u64 = 1 << 62;

/// What is chosen in the page -- in a field, or in the text -- following the focus into frames
/// of the same origin; taken out of the page too if CUT.
const PAGE_COPY: &str = "(function(cut){function go(d){var e=d.activeElement;\
if(e&&(e.tagName==='IFRAME'||e.tagName==='FRAME')){try{if(e.contentDocument)return go(e.contentDocument)}catch(x){}}\
var t='';if(e&&(e.tagName==='INPUT'||e.tagName==='TEXTAREA')&&typeof e.selectionStart==='number'){\
t=e.value.substring(e.selectionStart,e.selectionEnd)}else{var s=d.getSelection&&d.getSelection();t=s?String(s):''}\
if(cut&&t){d.execCommand('delete')}return t}return go(document)})(CUT)";

/// Typed into whatever has the focus, as a paste is: the page hears it as input.
const PAGE_PASTE: &str = "(function(t){function go(d){var e=d.activeElement;\
if(e&&(e.tagName==='IFRAME'||e.tagName==='FRAME')){try{if(e.contentDocument)return go(e.contentDocument)}catch(x){}}\
if(d.execCommand('insertText',false,t))return;if(e&&(e.tagName==='INPUT'||e.tagName==='TEXTAREA')){\
var a=e.selectionStart,b=e.selectionEnd;e.setRangeText(t,a,b,'end');e.dispatchEvent(new Event('input',{bubbles:true}))}}\
go(document)})(TEXT)";

pub const WINDOW: (u32, u32) = (1600, 1000);
const TAB_TOP: f32 = 10.0;
const TAB_H: f32 = 46.0;
const TOOL_TOP: f32 = 62.0;
const TOOL_H: f32 = 52.0;
/// The address bar's text: smaller than a row's, so more of a long address is in view.
const ADDRESS_EM: f32 = 26.0;
const PAGE_TOP: f32 = 122.0;
/// The bookmarks bar, when it is shown: between the toolbar and the page.
const BAR_H: f32 = 44.0;
const MARGIN: f32 = 10.0;
/// How long something said in a toast stays.
const TOAST: Duration = Duration::from_secs(6);

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
    /// The window's profile, beside the menu.
    Profile,
    /// The padlock beside it: every profile with a PIN or password locked at once.
    Lock,
    Page,
    /// An extension's button in the toolbar, by its place there.
    Action(usize),
    /// An extension's popup.
    ExtPopup,
    /// A tab's speaker: its sound on or off.
    Speaker(TabId),
    /// The padlock or warning before the address: how the page came.
    Security,
    /// The downloads' button, while there are any.
    Downloads,
    /// "Add to SpatiMesh", on an extension store's page for one.
    StoreAdd,
    /// A link of the bookmarks bar, by its place there.
    BarMark(usize),
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

/// A menu of its own, opened beside the line that names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sub {
    SearchEngine,
    Zoom,
    History,
    Bookmarks,
    Extensions,
}

/// What a film or sound in the page is to do, from its menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaDo {
    PlayPause,
    Mute,
    Loop,
    Controls,
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
    DeveloperTools,
    SearchWith(String),
    /// A search address of this profile's own, asked for.
    CustomSearch,
    OpenProfile(String),
    NewProfile,
    /// The colour of this window's profile.
    ProfileColour([u8; 3]),
    /// This window's profile's own settings: its name, its lock, its colour.
    ProfileSettings,
    RenameProfile,
    SetLock(LockKind),
    /// The PIN or password asked for first, then a new one, or none.
    ChangeLock,
    RemoveLock,
    LockAll,
    /// This window's profile, up or down the list of profiles.
    MoveProfile(isize),
    /// This window's profile gone, after asking.
    RemoveProfile,
    Open(String),
    /// An extension turned on or off.
    ToggleExtension(String),
    LoadExtension,
    ReloadExtensions,
    RemoveExtension,
    /// One extension, by its id: its options page, taken out.
    ExtensionOptions(String),
    UninstallExtension(String),
    /// A folder or package to install, chosen from the files.
    PickExtensionFolder,
    PickExtensionFile,
    /// The extension of the store's page in front, installed.
    AddFromStore,
    /// A line that opens a menu of its own.
    Submenu(Sub),
    ReopenClosed,
    /// One of the tabs or windows closed lately, by its place in their list.
    ReopenAt(usize),
    /// The menu closes and nothing else happens.
    Close,
    FindInPage,
    SavePage,
    /// Pages, from a link, an image or a menu.
    OpenInNewTab(String),
    OpenInBackground(String),
    OpenInNewWindow(String),
    OpenInPrivate(String),
    /// Text onto the clipboard.
    Copy(String),
    /// Something downloaded into the wearer's Downloads.
    Download(String),
    /// The page's own editing: `undo`, `redo`, by `document.execCommand`'s name for it.
    PageCommand(&'static str),
    /// Copy, cut, paste or choose all, by its letter, as the keys do.
    PageClipboard(char),
    /// What is chosen in the page, looked up with this profile's search engine.
    SearchSelection,
    /// The film or sound at a point of the page (CSS pixels).
    Media(MediaDo, f32, f32),
    Back,
    Forward,
    Reload,
    /// A tab's own: from its menu on the bar.
    TabNewAfter(TabId),
    TabReload(TabId),
    TabDuplicate(TabId),
    TabPin(TabId, bool),
    TabMute(TabId, bool),
    TabClose(TabId),
    TabCloseOthers(TabId),
    TabCloseRight(TabId),
    TabToWindow(TabId),
    /// A bookmark's own, by its id: renamed, moved up or down among its folder's, removed.
    RenameBookmark(String),
    MoveBookmark(String, isize),
    RemoveBookmark(String),
    /// A page out of the history on every machine; or everything since a time (0: all of it).
    ForgetPage(String),
    ForgetSince(u64),
    /// A download, by its number.
    OpenDownload(u64),
    ShowDownload(u64),
    CancelDownload(u64),
    RetryDownload(u64),
    ForgetDownload(u64),
    ClearDownloads,
    /// What a new window of a profile opens: its tabs as they were, or a new tab.
    SetRestore(bool),
    /// The home page: the page in front, or the start page.
    SetHome(bool),
    ToggleBookmarksBar,
    Nothing,
}

/// A question of SpatiMesh's own, in the question pane: what its answer is for.
#[derive(Debug, Clone, PartialEq)]
enum Own {
    /// Where an extension to install is, for this profile.
    LoadExtension(String),
    /// Which extension of this profile's to remove.
    RemoveExtension(String),
    /// A new name for this profile.
    RenameProfile(String),
    /// A new PIN or password for this profile, and then the same again.
    NewLock(String, LockKind),
    ConfirmLock(String, LockKind, String),
    /// The profile's PIN or password as it is, before it is changed (or, `true`, removed).
    OldLock(String, bool),
    /// A search address of the profile's own, `{}` where the words go.
    CustomSearch(String),
    /// A bookmark's new name, by the profile and the bookmark's id.
    RenameBookmark(String, String),
    /// The profile's name typed again, to remove it.
    RemoveProfile(String),
    /// A downloaded extension package to install in the profile, by its path.
    InstallPackage(String, String),
    /// "yes", to clear all of a profile's history.
    ClearHistory(String),
    /// "yes", to remove an extension, by the profile and its id.
    Uninstall(String, String),
}

/// A locked window: nothing of it is shown, not even its tabs' names, until its profile's PIN
/// or password is typed.
struct LockPane {
    field: Field,
    /// Why the last try did not open it.
    said: Option<String>,
    ok: Rect,
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
    /// SpatiMesh's own question, not the page's.
    own: Option<Own>,
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
    /// The link under the pointer, said at the foot of the page.
    hovered: String,
}

struct Win {
    key: WindowKey,
    id: WindowId,
    /// What this window's pages' sound is handed to the room as (`set_sound_key`): the first
    /// window the process's own name -- which the engine's own sound, all pages' at once, is
    /// handed over as too -- and each after it a name of its own.
    sound: String,
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
    buttons: [(Hover, Rect); 10],
    page: Rect,
    /// The page has the whole window: something in it is fullscreen (`media/page.js`).
    fullscreen: bool,
    /// A button went down on the page and has not come up: moves go to the page meanwhile.
    pressed_in_page: Option<engine::Button>,
    in_page: bool,
    popup: Option<Popup>,
    start: Option<Popup>,
    /// The extensions' buttons in the toolbar, as last drawn.
    action_buttons: Vec<(String, Rect)>,
    /// The tab the extensions were last told is in front.
    told_active: Option<TabId>,
    /// Locked: its profile's PIN or password is wanted before anything else.
    lock: Option<LockPane>,
    /// A menu's submenu, beside the line of it that opened it (`popup`'s line `.0`).
    flyout: Option<(usize, Popup)>,
    /// What the address bar suggests while it is typed into.
    suggest: Option<omni::Suggest>,
    /// The bar for finding in the page.
    find: Option<find::FindBar>,
    /// One of SpatiMesh's own pages, as drawn in front: history, bookmarks, settings...
    internal: Option<pages::Internal>,
    /// Files and folders to choose from, over the page.
    picker: Option<pages::Picker>,
    /// The bookmarks bar's links, as last drawn.
    bar_marks: Vec<(String, Rect)>,
    /// Each sounding tab's speaker on the bar, as last drawn.
    speakers: Vec<(TabId, Rect)>,
    /// The start page's most visited sites, as last drawn.
    top_sites: Vec<(String, Rect)>,
    /// Where the address bar's field was drawn, for its suggestions to hang under.
    address_rect: Rect,
    /// The start page's most visited sites: address and name.
    top_list: Vec<(String, String)>,
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
    /// Windows' sounds named so far (`name_sound`).
    sounds_named: u32,
    /// Each tab's sound's own name, as its engine said (`Happened::SoundName`), and the window
    /// sound name the room was last told it is heard as.
    tab_sounds: HashMap<TabId, (String, Option<String>)>,
    views: HashMap<TabId, TabView>,
    hosts: HashMap<String, Host>,
    program: Program,
    /// A tab let go of off its window: where from and when. If the pointer arrives on another
    /// SpatiMesh window straight after, it was dropped there; if not, it becomes a window.
    in_flight: Option<(TabId, WindowKey, Instant)>,
    listener: Option<UnixListener>,
    checked: Instant,
    seen_changes: Option<std::time::SystemTime>,
    /// Extensions (docs/spatimesh.md, "Extensions"): out of its place while it works.
    extensions: Option<Runtime>,
    /// What the extensions are yet to be told.
    told: Vec<Told>,
    /// Extensions' pages no window shows as a tab (backgrounds, offscreen documents, popups):
    /// their profile.
    hidden: HashMap<TabId, String>,
    /// The extension popup over each window.
    ext_popups: HashMap<WindowKey, ExtPopup>,
    /// The window the wearer used last.
    last_focused: Option<WindowKey>,
    /// Something said for a few seconds: a notification, an extension installed.
    toast: Option<(String, Instant)>,
    /// The profiles with a lock that were opened here, and stay open while any window of
    /// theirs is, until the lock button.
    unlocked: HashSet<String>,
    /// Wrong tries in a row, and when the last was: five, and the next waits half a minute.
    tries: (u32, Instant),
    /// The extensions' buttons' pictures.
    icons: HashMap<(String, String), Option<PageTexture>>,
    /// Which buffer holds each tab's newest picture.
    latest_buffer: HashMap<TabId, u8>,
    /// Pages' films and sounds, played here (`media`).
    media: media::Media,
    /// Each running profile's cookies and history, the same on every machine (`kept`).
    kept: HashMap<String, kept::Kept>,
    /// Each profile's search engine, as its settings say (`search_for`).
    searches: HashMap<String, SearchEngine>,
    /// The tabs and windows closed lately, newest last, to be opened again.
    closed: Vec<session::Closed>,
    /// When the open tabs were last written down, and whether anything changed since.
    session_saved: Instant,
    session_dirty: bool,
    downloads: downloads::Downloads,
    /// The page menu open now: which tab and what was under the pointer, where (CSS pixels).
    context: Option<menus::Context>,
    /// Where files were last chosen from.
    pick_dir: Option<std::path::PathBuf>,
    /// The servers connected to ahead of a load (`preconnect`), and when.
    preconnected: HashMap<String, Instant>,
}

/// How long a server connected to ahead of a load is not connected to again for that.
const PRECONNECT_EVERY: Duration = Duration::from_secs(30);

/// The I key, where it is on the keyboard (Linux's number for it).
const KEY_I: u32 = 23;

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
            sounds_named: 0,
            tab_sounds: HashMap::new(),
            views: HashMap::new(),
            hosts: HashMap::new(),
            program: crate::engines::program(),
            in_flight: None,
            listener,
            checked: Instant::now(),
            seen_changes: None,
            extensions: None,
            told: Vec::new(),
            hidden: HashMap::new(),
            ext_popups: HashMap::new(),
            last_focused: None,
            toast: None,
            unlocked: HashSet::new(),
            tries: (0, Instant::now()),
            icons: HashMap::new(),
            latest_buffer: HashMap::new(),
            media: media::Media::default(),
            kept: HashMap::new(),
            searches: HashMap::new(),
            closed: Vec::new(),
            session_saved: Instant::now(),
            session_dirty: false,
            downloads: downloads::Downloads::default(),
            context: None,
            pick_dir: None,
            preconnected: HashMap::new(),
        };
        browser.downloads.load(&browser.store);
        let mut extensions = Runtime::new(browser.store.clone());
        extensions.set_addresses(crate::engines::extension_addresses(&browser.program));
        browser.extensions = Some(extensions);
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

    /// A profile's search engine: its own choice, or SpatiMesh's.
    fn search_for(&mut self, profile: &str) -> SearchEngine {
        if !self.searches.contains_key(profile) {
            let engine = if profile == PRIVATE { self.settings.search_engine() } else { self.settings.search_engine_for(&self.store, profile) };
            self.searches.insert(profile.to_string(), engine);
        }
        self.searches[profile].clone()
    }

    /// The search engine of a window's profile.
    fn search_of_win(&mut self, w: usize) -> SearchEngine {
        let profile = self.tabs.window(self.wins[w].key).map(|x| x.profile.clone()).unwrap_or_default();
        self.search_for(&profile)
    }

    /// The search engine of a tab's profile.
    fn search_of_tab(&mut self, tab: TabId) -> SearchEngine {
        let profile = self.tabs.tab(tab).map(|t| t.profile.clone()).unwrap_or_default();
        self.search_for(&profile)
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

    /// Each tab's sound heard at the window it is in now: the room is told whenever that
    /// changes (a tab dragged to another window, a window closed), and forgets a closed tab's.
    fn route_sounds(&mut self) {
        let Some(path) = spatiand_sound_socket() else { return };
        let mut tell = Vec::new();
        self.tab_sounds.retain(|tab, (name, told)| {
            let Some(key) = self.tabs.window_of(*tab) else {
                tell.push((name.clone(), String::new()));
                return false;
            };
            let Some(win) = self.wins.iter().find(|w| w.key == key) else { return true };
            if told.as_deref() != Some(win.sound.as_str()) {
                *told = Some(win.sound.clone());
                tell.push((name.clone(), win.sound.clone()));
            }
            true
        });
        for (feed, to) in tell {
            let mut said = b"SPRT".to_vec();
            for part in [&feed, &to] {
                said.extend_from_slice(&(part.len() as u16).to_le_bytes());
                said.extend_from_slice(part.as_bytes());
            }
            if let Ok(mut socket) = std::os::unix::net::UnixStream::connect(&path) {
                use std::io::Write;
                let _ = socket.write_all(&said);
            }
        }
    }

    /// A new window's sound's name, said to the room.
    fn name_sound(&mut self, id: WindowId) -> String {
        self.sounds_named += 1;
        let name = if self.sounds_named == 1 { self.app.sound_name().to_string() } else { format!("{}#{}", self.app.sound_name(), self.sounds_named) };
        self.app.set_sound_key(id, &name);
        name
    }

    fn win_of_key(&self, key: WindowKey) -> Option<usize> {
        self.wins.iter().position(|w| w.key == key)
    }

    // ---- engines ----------------------------------------------------------------------------

    fn host(&mut self, profile: &str) -> Option<&mut Host> {
        if !self.hosts.contains_key(profile) {
            match Host::start(&self.program, &self.store, profile) {
                Ok(mut host) => {
                    self.app.watch(host.link.raw_fd());
                    // Pages' media played here, from the engine's first page on.
                    if spatimesh::media::ours() {
                        host.send(&ToHost::PageScript { code: spatimesh::media::page_js() });
                    }
                    self.hosts.insert(profile.to_string(), host);
                    self.extensions_to_new_engine(profile);
                    self.kept_started(profile);
                }
                Err(e) => {
                    log::error!("no engine for {profile}: {e}");
                    return None;
                }
            }
        }
        self.hosts.get_mut(profile)
    }

    /// Copy, cut, paste and choose all in a page, by `key`'s letter. Whether it was one of them.
    fn page_clipboard(&mut self, tab: TabId, key: char) -> bool {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let (code, request) = match key {
            'c' | 'x' => {
                let request = PAGE_CLIPBOARD | NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                (PAGE_COPY.replace("CUT", if key == 'x' { "true" } else { "false" }), request)
            }
            'v' => {
                let Some(text) = spatiand_app::clipboard::text() else { return true };
                (PAGE_PASTE.replace("TEXT", &serde_json::to_string(&text).unwrap_or_default()), 0)
            }
            'a' => ("document.execCommand('selectAll')".to_string(), 0),
            _ => return false,
        };
        self.tell(tab, ToHost::RunScript { tab, world: String::new(), all_frames: false, code, request, boot: false });
        true
    }

    /// A load the wearer is about to start in `tab` -- a link pointed at, the address the bar
    /// would go to: the engine connects to its server ahead of it (`Engine::preconnect`), so that
    /// the click does not wait for the name, TCP and TLS. Once in a while a server.
    pub(super) fn preconnect(&mut self, tab: TabId, url: &str) {
        let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) else { return };
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if host.is_empty() {
            return;
        }
        let now = Instant::now();
        self.preconnected.retain(|_, at| now.duration_since(*at) < PRECONNECT_EVERY);
        if self.preconnected.contains_key(host) {
            return;
        }
        self.preconnected.insert(host.to_string(), now);
        self.tell(tab, ToHost::Preconnect { tab, url: url.to_string() });
    }

    fn tell(&mut self, tab: TabId, message: ToHost) {
        let profile = self.tabs.tab(tab).map(|t| t.profile.clone()).or_else(|| self.hidden.get(&tab).cloned());
        let Some(profile) = profile else { return };
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
        let (profile, url, muted) = (t.profile.clone(), pages::engine_url(&t.url), t.muted);
        let Some(w) = self.tabs.window_of(tab).and_then(|k| self.win_of_key(k)) else { return };
        let view = self.view_of(w);
        let window = self.wins[w].key;
        if let Some(host) = self.host(&profile) {
            log::info!("tab {tab} opens {url} in {profile}'s engine");
            host.send(&ToHost::Open { tab, url, view });
            host.send(&ToHost::Window { tab, window });
            if muted {
                host.send(&ToHost::Mute { tab, muted });
            }
        }
        self.views.entry(tab).or_default().told = Some(view);
    }

    /// Tell every tab of a window its page's size, where that changed.
    /// Where the page is in a window: under the bars, or all of it while fullscreen.
    fn lay_out_page(&mut self, w: usize) {
        let size = self.wins[w].size;
        let private = self.tabs.window(self.wins[w].key).is_some_and(|x| x.profile == PRIVATE);
        self.wins[w].page = if self.wins[w].fullscreen { Rect::new(0.0, 0.0, size.0 as f32, size.1 as f32) } else { page_rect(size, self.settings.bookmarks_bar && !private) };
        self.size_pages(w);
        self.wins[w].dirty = true;
    }

    /// Something in a tab's page went fullscreen, or came out: the window gives the page all of
    /// itself, or its bars back. `tell` says it to the page (the wearer left it, by Escape or by
    /// going to another tab).
    fn set_fullscreen(&mut self, tab: TabId, on: bool, tell: bool) {
        let Some(key) = self.tabs.window_of(tab) else { return };
        let Some(w) = self.win_of_key(key) else { return };
        if on && self.tabs.active(key) != Some(tab) {
            return;
        }
        if self.wins[w].fullscreen == on {
            return;
        }
        self.wins[w].fullscreen = on;
        self.wins[w].popup = None;
        self.lay_out_page(w);
        if tell && !on {
            let code = "window.__spatimeshMediaIn&&window.__spatimeshMediaIn({\"t\":\"fullscreen-exit\"})".to_string();
            self.tell(tab, ToHost::RunScript { tab, world: String::new(), all_frames: false, code, request: 0, boot: false });
        }
    }

    fn set_fullscreen_off_quietly(&mut self, w: usize, tab: TabId) {
        let _ = w;
        let code = "window.__spatimeshMediaIn&&window.__spatimeshMediaIn({\"t\":\"fullscreen-exit\"})".to_string();
        self.tell(tab, ToHost::RunScript { tab, world: String::new(), all_frames: false, code, request: 0, boot: false });
    }

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
            let ext_popup = self.ext_popups.get(&win.key).map(|p| p.tab);
            for &tab in &window.tabs {
                let active = window.active_tab() == Some(tab);
                let overlay = win.popup.is_some()
                    || ext_popup.is_some()
                    || win.lock.is_some()
                    || win.picker.is_some()
                    || win.find.as_ref().is_some_and(|f| f.field.focused)
                    || self.views.get(&tab).is_some_and(|v| v.question.is_some());
                wanted.push((tab, active && win.focused && !win.editing && !overlay));
            }
            // An extension's popup has the keyboard while it is open.
            if let Some(tab) = ext_popup {
                wanted.push((tab, win.focused && win.popup.is_none()));
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
        self.extensions_engine_gone(profile);
        self.kept_ended(profile);
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
            FromHost::Surface { tab, id, width, height } => {
                // The engine's own picture on the GPU: used where it is; the one before it given
                // back, and this one too if it cannot be.
                let given_back = self.borrow_picture(tab, id, width, height);
                if let Some(host) = self.hosts.get_mut(profile) {
                    for old in given_back {
                        host.send(&ToHost::ReleaseSurface { tab, id: old });
                    }
                }
                if self.hidden.contains_key(&tab) {
                    self.dirty_popup(tab);
                } else {
                    self.dirty_tab(tab);
                }
            }
            FromHost::Picture { tab, buffer, changed } => {
                self.picture(profile, tab, buffer, &changed);
                self.latest_buffer.insert(tab, buffer);
                if let Some(host) = self.hosts.get_mut(profile) {
                    host.send(&ToHost::Release { tab, buffer });
                }
                if self.hidden.contains_key(&tab) {
                    self.dirty_popup(tab);
                } else {
                    self.dirty_tab(tab);
                }
            }
            FromHost::Happened(Happened::Extension { tab, json }) => {
                let profile = profile.to_string();
                self.with_extensions(|rt, b| rt.call(b, &profile, tab, &json));
            }
            // What was chosen in a page, asked for by Command-C or -X (`page_clipboard`).
            FromHost::Happened(Happened::ScriptResult { request, result, .. }) if request & PAGE_CLIPBOARD != 0 => {
                let chosen = result.ok().and_then(|json| serde_json::from_str::<String>(&json).ok()).filter(|t| !t.is_empty());
                if let Some(text) = chosen {
                    spatiand_app::clipboard::copy(text);
                }
            }
            // What a page said when its menu opened (`menus::PAGE_PROBE`).
            FromHost::Happened(Happened::ScriptResult { tab, request, result }) if request & menus::PAGE_PROBE != 0 => {
                if let Some(json) = result.ok().and_then(|json| serde_json::from_str::<String>(&json).ok()) {
                    self.context_probed(tab, &json);
                }
            }
            FromHost::Happened(Happened::ScriptResult { request, result, .. }) => {
                self.with_extensions(|rt, b| rt.script_result(b, request, result));
            }
            FromHost::Happened(Happened::Media { tab, json }) => {
                // Fullscreen is the window's to do; a new document in the tab ends it.
                if json.contains("\"t\":\"fullscreen\"") {
                    let on = json.contains("\"on\":true");
                    self.set_fullscreen(tab, on, false);
                } else if json.contains("\"t\":\"hello\"") && json.contains("\"top\":true") {
                    self.set_fullscreen(tab, false, false);
                }
                // Its sound is its window's, wherever the tab is now.
                if let Some(w) = self.tabs.window_of(tab).and_then(|k| self.win_of_key(k)) {
                    let key = self.wins[w].sound.clone();
                    self.media.set_tab_key(tab, &key);
                }
                self.media.said(tab, &json);
            }
            // A download is the profile's: its engine numbers the ones it begins itself.
            FromHost::Happened(Happened::Download { tab, id, event }) => self.download_news(profile, tab, id, event),
            FromHost::Happened(Happened::Cookies { cookies }) => self.kept_cookies(profile, Some(cookies), None),
            FromHost::Happened(Happened::CookiesSent { sent }) => self.kept_cookies(profile, None, Some(sent)),
            FromHost::Happened(happened) if self.hidden.contains_key(&happened.tab()) => self.hidden_happened(profile, happened),
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
        // Not into the engine's own picture on the GPU, if that is what the tab had.
        let fits = view.texture.as_ref().is_some_and(|t| t.width == width && t.height == height && t.borrowed_id().is_none());
        let mut given_back = None;
        let changed: Vec<engine::Rect> = if fits {
            changed.to_vec()
        } else {
            if let Some(old) = view.texture.take() {
                given_back = old.borrowed_id();
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
        if let (Some(id), Some(host)) = (given_back, self.hosts.get_mut(profile)) {
            host.send(&ToHost::ReleaseSurface { tab, id });
        }
    }

    /// A tab's picture is now the engine host's `IOSurface` numbered `id`. What is to be given
    /// back: the picture it replaces, or this one where it cannot be used.
    fn borrow_picture(&mut self, tab: TabId, id: u32, width: u32, height: u32) -> Vec<u32> {
        let Ok(gl) = self.app.gl() else { return vec![id] };
        if !self.tabs.tab(tab).is_some() && !self.hidden.contains_key(&tab) {
            return vec![id];
        }
        #[cfg(target_os = "macos")]
        {
            match PageTexture::borrow(gl, id, width, height) {
                Ok(texture) => {
                    let view = self.views.entry(tab).or_default();
                    let mut back = Vec::new();
                    if let Some(old) = view.texture.replace(texture) {
                        back.extend(old.borrowed_id());
                        old.destroy(gl);
                    }
                    back
                }
                Err(e) => {
                    log::warn!("tab {tab}'s picture on the GPU could not be used: {e}");
                    vec![id]
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (gl, width, height);
            vec![id]
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
        let engine = self.search_of_tab(tab);
        if let Some(t) = self.tabs.tab(tab) {
            let told = ext::told_of(&happened, t.loading, &t.url);
            self.told.extend(told);
        }
        match happened {
            Happened::SoundName { tab, name } => {
                self.tab_sounds.insert(tab, (name, None));
                self.route_sounds();
            }
            // SpatiMesh's own page keeps its own name.
            Happened::Title { tab, .. } if self.tabs.tab(tab).is_some_and(|t| pages::internal(&t.url).is_some()) => {}
            Happened::Title { tab, title } => {
                let mut loaded = false;
                if let Some(t) = self.tabs.tab_mut(tab) {
                    t.title = title;
                    loaded = !t.loading;
                }
                if loaded {
                    self.kept_page(tab, false);
                }
                self.dirty_bar(tab);
            }
            Happened::Url { tab, url } => {
                // One of SpatiMesh's own pages is a blank page to the engine: it stays itself.
                if url == "about:blank" && self.tabs.tab(tab).is_some_and(|t| pages::internal(&t.url).is_some()) {
                    return;
                }
                if let Some(t) = self.tabs.tab_mut(tab) {
                    t.url = url.clone();
                    t.failed = None;
                }
                self.session_dirty = true;
                if let Some(view) = self.views.get_mut(&tab) {
                    view.hovered.clear();
                }
                if let Some(w) = self.tabs.window_of(tab).and_then(|k| self.win_of_key(k)) {
                    if self.tabs.active(self.wins[w].key) == Some(tab) && !self.wins[w].editing {
                        self.wins[w].address.set(shown(&url, &engine));
                    }
                }
                self.dirty_bar(tab);
            }
            Happened::Progress { tab, progress, loading } => {
                let mut finished = false;
                if let Some(t) = self.tabs.tab_mut(tab) {
                    finished = t.loading && !loading;
                    t.progress = progress;
                    t.loading = loading;
                    if loading {
                        t.crashed = None;
                    }
                }
                if finished {
                    self.kept_page(tab, true);
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
                        self.told.push(Told::TabCreated(new));
                        self.after_tab_change(window);
                    }
                }
            }
            Happened::CloseTab { tab } => self.close_tab(tab),
            Happened::Crashed { tab, why } => {
                self.media.forget(tab);
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
                self.views.entry(tab).or_default().question = Some(Question { kind, message, field, ok: Rect::default(), cancel: Rect::default(), own: None });
                self.dirty_tab(tab);
                self.focus_pages();
            }
            // Taken by `heard` before they come here.
            Happened::Extension { .. } | Happened::ScriptResult { .. } | Happened::Media { .. } => {}
            Happened::Cookies { .. } | Happened::CookiesSent { .. } => {}
            Happened::ContextMenu { tab, hit } => self.context_menu(tab, hit),
            Happened::Hovered { tab, link } => {
                self.preconnect(tab, &link);
                self.views.entry(tab).or_default().hovered = link;
                self.dirty_tab(tab);
            }
            Happened::Found { tab, matches } => self.found(tab, matches),
            Happened::ChooseFiles { tab, multiple, accept } => self.pick_files_for_page(tab, multiple, accept),
            // Taken by `heard`, with the profile it is of.
            Happened::Download { .. } => {}
        }
        let _ = tab;
    }

    fn dirty_bar(&mut self, tab: TabId) {
        if let Some(w) = self.tabs.window_of(tab).and_then(|k| self.win_of_key(k)) {
            self.wins[w].dirty = true;
        }
    }

    // ---- windows and tabs -------------------------------------------------------------------

    /// A window of `profile` at `url`, or at its home page -- or, for a profile none of whose
    /// windows is open, with the tabs it had open when its last window closed, if it is to.
    pub fn new_window(&mut self, profile: &str, url: Option<&str>) -> WindowKey {
        if url.is_none() && self.settings.restore && !self.tabs.windows().iter().any(|w| w.profile == profile) {
            if let Some(saved) = spatimesh::session::load(&self.store, profile) {
                return self.restore_session(profile, saved);
            }
        }
        let key = self.open_window(profile);
        let home = self.settings.home.clone();
        let url = url.unwrap_or(&home).to_string();
        if let Some(tab) = self.tabs.new_tab(key, &url, Place::End, true, None) {
            self.open_in_engine(tab);
            self.told.push(Told::TabCreated(tab));
        }
        self.after_tab_change(key);
        if let Some(w) = self.win_of_key(key) {
            if self.wins[w].lock.is_some() {
                self.hush_window(w);
            }
        }
        key
    }

    /// A window of `profile` with nothing in it yet: locked, if its profile is.
    fn open_window(&mut self, profile: &str) -> WindowKey {
        let key = self.tabs.new_window(profile);
        let title = if profile == PRIVATE { "SpatiMesh (Private)" } else { "SpatiMesh" };
        let id = self.app.open(title, WINDOW);
        let sound = self.name_sound(id);
        let mut win = Win {
            key,
            id,
            sound,
            size: WINDOW,
            ready: false,
            dirty: true,
            focused: false,
            pointer: (0.0, 0.0),
            hover: Hover::Nothing,
            address: Field::sized(ADDRESS_EM),
            editing: false,
            fresh: false,
            drag: None,
            placed: Vec::new(),
            plus: Rect::default(),
            buttons: [(Hover::Back, Rect::default()); 10],
            page: page_rect(WINDOW, self.settings.bookmarks_bar && profile != PRIVATE),
            fullscreen: false,
            pressed_in_page: None,
            in_page: false,
            popup: None,
            start: None,
            action_buttons: Vec::new(),
            told_active: None,
            lock: None,
            flyout: None,
            suggest: None,
            find: None,
            internal: None,
            picker: None,
            bar_marks: Vec::new(),
            speakers: Vec::new(),
            top_sites: Vec::new(),
            address_rect: Rect::default(),
            top_list: Vec::new(),
        };
        win.address.hint = "Search or type an address".into();
        // A profile with a lock, none of whose windows is open here: its PIN first.
        if self.profile(profile).lock.is_some() && !self.unlocked.contains(profile) {
            win.lock = Some(lock_pane(self.profile(profile).lock.as_ref().map_or(LockKind::Pin, |l| l.kind)));
        }
        self.wins.push(win);
        self.told.push(Told::WindowCreated(key));
        self.session_dirty = true;
        key
    }

    fn new_tab(&mut self, w: usize, url: &str) {
        let key = self.wins[w].key;
        if let Some(tab) = self.tabs.new_tab(key, url, Place::AfterActive, true, None) {
            if let (Some(page), Some(t)) = (pages::internal(url), self.tabs.tab_mut(tab)) {
                t.title = page.title().to_string();
            }
            self.open_in_engine(tab);
            self.told.push(Told::TabCreated(tab));
        }
        self.after_tab_change(key);
    }

    /// The developer tools (the engine's Web Inspector) of the tab in front, as a tab after it:
    /// the one they have already, or new ones. Asked of the developer tools' own tab, it is
    /// itself.
    fn developer_tools(&mut self, w: usize) {
        let key = self.wins[w].key;
        let Some(front) = self.tabs.active(key) else { return };
        let page = self.tabs.tab(front).and_then(|t| t.inspects).unwrap_or(front);
        let open = self.tabs.tabs().find(|t| t.inspects == Some(page)).map(|t| t.id);
        if let Some(open) = open {
            if let Some(window) = self.tabs.activate_tab(open) {
                self.after_tab_change(window);
            }
            return;
        }
        let url = self.tabs.tab(page).map(|t| t.url.clone()).unwrap_or_default();
        let Some(tools) = self.tabs.new_tab(key, &url, Place::AfterActive, true, Some(page)) else { return };
        if let Some(t) = self.tabs.tab_mut(tools) {
            t.inspects = Some(page);
            t.title = "Developer tools".to_string();
            t.loading = false;
        }
        let view = self.view_of(w);
        self.tell(page, ToHost::Inspect { tab: page, inspector: tools, view });
        self.views.entry(tools).or_default().told = Some(view);
        self.after_tab_change(key);
    }

    /// The tab in front of a window changed: the bar shows its address, the keyboard goes to it.
    fn after_tab_change(&mut self, key: WindowKey) {
        let Some(w) = self.win_of_key(key) else { return };
        // Another tab: the one that was fullscreen is not any more.
        if self.wins[w].fullscreen {
            self.wins[w].fullscreen = false;
            if let Some(was) = self.wins[w].told_active {
                self.set_fullscreen_off_quietly(w, was);
            }
            self.lay_out_page(w);
        }
        let engine = self.search_of_win(w);
        let active = self.tabs.active(key).and_then(|t| self.tabs.tab(t)).cloned();
        // A tab opened again with its window but not yet shown: its page now.
        if let Some(tab) = active.as_ref().filter(|t| t.crashed.is_none() && t.inspects.is_none()) {
            if self.views.get(&tab.id).map_or(true, |v| v.told.is_none()) {
                self.open_in_engine(tab.id);
            }
        }
        self.session_dirty = true;
        let w = match self.win_of_key(key) {
            Some(w) => w,
            None => return,
        };
        // Another tab: the old one's finding ends.
        if let Some(bar) = self.wins[w].find.take() {
            if active.as_ref().is_some_and(|t| t.id == bar.tab) {
                self.wins[w].find = Some(bar);
            } else {
                self.tell(bar.tab, ToHost::Find { tab: bar.tab, text: String::new(), forward: true });
            }
        }
        let win = &mut self.wins[w];
        win.dirty = true;
        win.popup = None;
        win.flyout = None;
        win.suggest = None;
        match active {
            Some(tab) if is_start(&tab.url) && tab.inspects.is_none() => {
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
            if self.wins[w].told_active != Some(tab) {
                self.wins[w].told_active = Some(tab);
                self.told.push(Told::TabActivated(tab));
            }
        }
        // Another tab in front: the extension popup over the window was for the one before.
        if self.ext_popups.contains_key(&key) && self.wins[w].told_active != self.tabs.active(key) {
            self.close_ext_popup(key);
        }
        self.focus_pages();
    }

    fn close_tab(&mut self, tab: TabId) {
        self.remember_closing_tab(tab);
        self.media.forget(tab);
        self.tell(tab, ToHost::Close { tab });
        let profile = self.tabs.tab(tab).map(|t| t.profile.clone());
        let window = self.tabs.window_of(tab);
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
            if let (Some(profile), Some(window)) = (profile.clone(), window) {
                self.told.push(Told::TabRemoved { profile: profile.clone(), tab, window, closing: closed.window_closed });
                if closed.window_closed {
                    self.ext_popups.remove(&window);
                    self.told.push(Told::WindowRemoved { profile, window });
                }
            }
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
            self.relock_if_gone(&profile);
        }
    }

    /// A profile with no tabs left has no engine left: the private one's goes, and with it
    /// everything it kept.
    fn end_idle_host(&mut self, profile: &str) {
        if !self.tabs.tabs().any(|t| t.profile == profile) {
            if let Some(host) = self.hosts.remove(profile) {
                self.app.unwatch(host.link.raw_fd());
                self.extensions_engine_gone(profile);
                self.kept_ended(profile);
            }
        }
    }

    fn close_window(&mut self, w: usize) {
        self.remember_closing_window(self.wins[w].key);
        let win = self.wins.remove(w);
        let profile = self.tabs.window(win.key).map(|x| x.profile.clone());
        if let Some(popup) = self.ext_popups.remove(&win.key) {
            self.with_extensions(|rt, b| rt.popup_closed(b, popup.tab));
            self.hidden.remove(&popup.tab);
            self.tell_profile(Some(&popup.profile), ToHost::Close { tab: popup.tab });
            self.forget_view(popup.tab);
        }
        let closing = self.tabs.window(win.key).map(|x| x.tabs.clone()).unwrap_or_default();
        if let Some(profile) = profile.clone() {
            for &tab in &closing {
                self.told.push(Told::TabRemoved { profile: profile.clone(), tab, window: win.key, closing: true });
            }
            self.told.push(Told::WindowRemoved { profile, window: win.key });
        }
        for tab in self.tabs.close_window(win.key) {
            // Its films and sounds stop with it, as a tab closed alone does.
            self.media.forget(tab);
            self.tell_profile(profile.as_deref(), ToHost::Close { tab });
            self.forget_view(tab);
        }
        self.app.close(win.id);
        if let Some(profile) = profile {
            self.end_idle_host(&profile);
            self.relock_if_gone(&profile);
        }
    }

    /// A profile none of whose windows is open any more is locked again: the next window of
    /// it asks.
    fn relock_if_gone(&mut self, profile: &str) {
        if !self.wins.iter().any(|w| self.tabs.window(w.key).is_some_and(|x| x.profile == profile)) {
            self.unlocked.remove(profile);
        }
    }

    /// A window's pages unheard while it is locked, and heard again when it opens.
    fn hush_window(&mut self, w: usize) {
        let on = self.wins[w].lock.is_some();
        let tabs = self.tabs.window(self.wins[w].key).map(|x| x.tabs.clone()).unwrap_or_default();
        for tab in tabs {
            let muted = self.tabs.tab(tab).is_some_and(|t| t.muted);
            self.media.hush(tab, on || muted);
        }
    }

    /// The lock button: every profile with a PIN or password is locked, and every window of
    /// theirs shows nothing until it is typed.
    fn lock_all(&mut self) {
        let locked: Vec<Profile> = self.profiles.iter().filter(|p| p.lock.is_some()).cloned().collect();
        if locked.is_empty() {
            self.say("No profile has a PIN or password yet: the profile button's settings give one".into());
            return;
        }
        self.unlocked.clear();
        for w in 0..self.wins.len() {
            let Some(profile) = self.tabs.window(self.wins[w].key).map(|x| x.profile.clone()) else { continue };
            if let Some(lock) = locked.iter().find(|p| p.id == profile).and_then(|p| p.lock.as_ref()) {
                let win = &mut self.wins[w];
                win.popup = None;
                win.editing = false;
                win.lock = Some(lock_pane(lock.kind));
                win.dirty = true;
                self.hush_window(w);
            }
        }
        self.focus_pages();
    }

    /// What was typed into a locked window: its profile's windows open if it is right.
    fn try_unlock(&mut self, w: usize) {
        let Some(profile) = self.tabs.window(self.wins[w].key).map(|x| x.profile.clone()) else { return };
        let Some(lock) = self.profile(&profile).lock else {
            self.wins[w].lock = None;
            return;
        };
        let (wrong, last) = self.tries;
        let wait = Duration::from_secs(30).saturating_sub(last.elapsed());
        let Some(pane) = self.wins[w].lock.as_mut() else { return };
        pane.said = None;
        if wrong >= 5 && !wait.is_zero() {
            pane.said = Some(format!("Too many wrong tries: wait {} s", wait.as_secs() + 1));
            return;
        }
        let typed = std::mem::take(&mut pane.field.text);
        pane.field.set(String::new());
        if lock.opens(&typed) {
            self.tries = (0, Instant::now());
            self.unlocked.insert(profile.clone());
            for i in 0..self.wins.len() {
                if self.tabs.window(self.wins[i].key).is_some_and(|x| x.profile == profile) {
                    self.wins[i].lock = None;
                    self.wins[i].dirty = true;
                    self.hush_window(i);
                }
            }
            self.focus_pages();
        } else {
            self.tries = (if wrong >= 5 { 1 } else { wrong + 1 }, Instant::now());
            pane.said = Some(format!("That is not the {}", lock.kind.name()));
        }
    }

    /// Input to a locked window: only to its field and its button.
    fn locked_input(&mut self, w: usize, event: Event) {
        self.wins[w].dirty = true;
        self.wins[w].hover = Hover::Nothing;
        match event {
            Event::PointerMoved { x, y, .. } => self.wins[w].pointer = (x, y),
            Event::Button { button: PointerButton::Left, pressed: false, .. } => {
                let (x, y) = self.wins[w].pointer;
                if self.wins[w].lock.as_ref().is_some_and(|l| l.ok.contains(x, y)) {
                    self.try_unlock(w);
                }
            }
            Event::Key { key, pressed: true, text, modifiers, .. } => {
                if key == Key::Enter {
                    self.try_unlock(w);
                } else if let Some(pane) = self.wins[w].lock.as_mut() {
                    pane.field.input(&Input::Key { key, text, modifiers });
                }
            }
            _ => {}
        }
    }

    fn tell_profile(&mut self, profile: Option<&str>, message: ToHost) {
        if let Some(host) = profile.and_then(|p| self.hosts.get_mut(p)) {
            host.send(&message);
        }
    }

    fn move_tab(&mut self, tab: TabId, to: WindowKey, place: Option<usize>) {
        let from = self.tabs.window_of(tab);
        let from_index = from.and_then(|k| self.tabs.window(k)).and_then(|x| x.tabs.iter().position(|&t| t == tab)).unwrap_or(0);
        match self.tabs.move_tab(tab, to, place) {
            Ok(emptied) => {
                let to_index = self.tabs.window(to).and_then(|x| x.tabs.iter().position(|&t| t == tab)).unwrap_or(0);
                if let Some(from) = from {
                    self.told.push(Told::TabMoved { tab, from: (from, from_index), to: (to, to_index) });
                }
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
        let key = self.open_window(&profile);
        self.move_tab(tab, key, None);
    }

    // ---- what the wearer does ---------------------------------------------------------------

    fn go(&mut self, w: usize, typed: &str) {
        let Some(understood) = address::understand(typed) else { return };
        let engine = self.search_of_win(w);
        let url = understood.url(&engine);
        let key = self.wins[w].key;
        self.wins[w].editing = false;
        self.wins[w].address.focused = false;
        let Some(tab) = self.tabs.active(key) else { return };
        let crashed = self.tabs.tab(tab).is_some_and(|t| t.crashed.is_some());
        if let Some(t) = self.tabs.tab_mut(tab) {
            t.url = url.clone();
            t.crashed = None;
            t.loading = true;
            if let Some(page) = pages::internal(&url) {
                t.title = page.title().to_string();
            }
        }
        let profile = self.tabs.tab(tab).map(|t| t.profile.clone()).unwrap_or_default();
        if crashed || !self.hosts.contains_key(&profile) || self.views.get(&tab).map_or(true, |v| v.told.is_none()) {
            self.reopen(tab);
        } else {
            log::info!("tab {tab} loads {url}");
            self.tell(tab, ToHost::Load { tab, url: pages::engine_url(&url) });
        }
        self.wins[w].suggest = None;
        self.session_dirty = true;
        self.wins[w].address.set(shown(&url, &engine));
        self.wins[w].dirty = true;
        self.focus_pages();
    }

    /// A tab whose page stopped, loaded again; its engine started again if that is what went.
    fn reopen(&mut self, tab: TabId) {
        let Some(t) = self.tabs.tab(tab) else { return };
        // The developer tools have no page to load again: they are opened anew from theirs.
        if t.inspects.is_some() {
            self.close_tab(tab);
            return;
        }
        let profile = t.profile.clone();
        let engine_was_gone = !self.hosts.contains_key(&profile);
        if engine_was_gone || self.views.get(&tab).map_or(true, |v| v.told.is_none()) {
            if engine_was_gone {
                // Every tab of the profile goes back to its engine, not only this one.
                let all: Vec<TabId> = self.tabs.tabs().filter(|t| t.profile == profile && t.inspects.is_none()).map(|t| t.id).collect();
                let tools: Vec<TabId> = self.tabs.tabs().filter(|t| t.profile == profile && t.inspects.is_some()).map(|t| t.id).collect();
                for gone in tools {
                    self.close_tab(gone);
                }
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

    /// A question of SpatiMesh's own, in the question pane over the tab in front.
    fn ask_own(&mut self, w: usize, own: Own, message: &str) {
        self.ask_own_with(w, own, message, false, "");
    }

    /// The same, its answer hidden as typed (`secret`), or begun with `text`.
    fn ask_own_with(&mut self, w: usize, own: Own, message: &str, secret: bool, text: &str) {
        let Some(tab) = self.tabs.active(self.wins[w].key) else { return };
        let mut field = Field::default();
        field.focused = true;
        field.secret = secret;
        field.set(text.to_string());
        let question = Question { kind: DialogKind::Prompt, message: message.to_string(), field, ok: Rect::default(), cancel: Rect::default(), own: Some(own) };
        self.views.entry(tab).or_default().question = Some(question);
        self.wins[w].dirty = true;
        self.focus_pages();
    }

    fn answered_own(&mut self, w: usize, own: Own, text: String) {
        match own {
            Own::RenameProfile(profile) => match profiles::rename(&self.store, &profile, &text) {
                Ok(()) => {
                    self.profiles = profiles::list(&self.store);
                    for win in self.wins.iter_mut() {
                        win.dirty = true;
                    }
                }
                Err(e) => self.say(format!("Not renamed: {e}")),
            },
            Own::NewLock(profile, kind) => match kind.refuses(&text) {
                Some(why) => self.say(format!("Not set: {why}")),
                None => self.ask_own_with(w, Own::ConfirmLock(profile, kind, text), &format!("The same {} again, to be sure of it:", kind.name()), true, ""),
            },
            Own::ConfirmLock(profile, kind, first) => {
                if text != first {
                    self.say(format!("Not set: the two were not the same {}", kind.name()));
                } else {
                    match profiles::set_lock(&self.store, &profile, Some((kind, &text))) {
                        Ok(()) => {
                            self.profiles = profiles::list(&self.store);
                            // Whoever set it has the profile open.
                            self.unlocked.insert(profile);
                            self.say(format!("This profile now asks for its {}", kind.name()));
                        }
                        Err(e) => self.say(format!("Not set: {e}")),
                    }
                }
            }
            Own::OldLock(profile, remove) => {
                let Some(lock) = self.profile(&profile).lock else { return };
                if !lock.opens(&text) {
                    self.say(format!("That is not the {}", lock.kind.name()));
                } else if remove {
                    match profiles::set_lock(&self.store, &profile, None) {
                        Ok(()) => {
                            self.profiles = profiles::list(&self.store);
                            self.say("This profile asks for nothing now".into());
                        }
                        Err(e) => self.say(format!("Not changed: {e}")),
                    }
                } else {
                    let kind = lock.kind;
                    self.ask_own_with(w, Own::NewLock(profile, kind), &format!("The new {}:", kind.name()), true, "");
                }
            }
            Own::CustomSearch(profile) => match spatimesh::settings::set_profile_search(&self.store, &profile, "Custom", Some(&text)) {
                Ok(()) => {
                    self.searches.clear();
                    self.refresh_internal();
                    self.say("This profile searches with its own address now".into());
                }
                Err(e) => self.say(format!("Not set: {e}")),
            },
            Own::RenameBookmark(profile, id) => {
                let mark = self.bookmarks_of(&profile).iter().find(|b| b.id == id).cloned();
                if let Some(mut mark) = mark.filter(|_| !text.is_empty()) {
                    mark.title = text;
                    if let Err(e) = bookmarks::update(&self.store, &profile, &mark) {
                        self.say(format!("Not renamed: {e}"));
                    }
                    self.bookmarks.remove(&profile);
                    self.refresh_internal();
                }
            }
            Own::RemoveProfile(profile) => {
                let name = self.profile(&profile).name;
                if text != name {
                    self.say(format!("Not removed: that is not {name}"));
                    return;
                }
                let keys: Vec<WindowKey> = self.tabs.windows().iter().filter(|x| x.profile == profile).map(|x| x.key).collect();
                for key in keys {
                    if let Some(i) = self.win_of_key(key) {
                        self.close_window(i);
                    }
                }
                match profiles::remove(&self.store, &profile) {
                    Ok(()) => {
                        self.profiles = profiles::list(&self.store);
                        self.say(format!("{name} is removed"));
                    }
                    Err(e) => self.say(format!("Not removed: {e}")),
                }
                if self.wins.is_empty() {
                    let first = self.profiles[0].id.clone();
                    self.new_window(&first, None);
                }
            }
            Own::InstallPackage(profile, path) => match self.install_extension(&profile, &path) {
                Ok(name) => {
                    self.say(format!("{name} is installed"));
                    self.refresh_internal();
                }
                Err(e) => self.say(format!("Not installed: {e}")),
            },
            Own::ClearHistory(profile) => {
                if text.eq_ignore_ascii_case("yes") {
                    self.act(w, Action::ForgetSince(0));
                    let _ = profile;
                } else {
                    self.say("The history is as it was".into());
                }
            }
            Own::Uninstall(profile, id) => {
                if text.eq_ignore_ascii_case("yes") {
                    let _ = profile;
                    self.act(w, Action::UninstallExtension(id));
                }
            }
            Own::LoadExtension(profile) => match self.install_extension(&profile, &text) {
                Ok(name) => self.say(format!("{name} is installed")),
                Err(e) => self.say(format!("Not installed: {e}")),
            },
            Own::RemoveExtension(profile) => {
                let wanted = text.to_lowercase();
                let found = spatimesh::extensions::install::list(&self.store, &profile).into_iter().find(|i| i.id == text || ext::extension_name(i).to_lowercase() == wanted);
                match found {
                    Some(installed) => {
                        let name = ext::extension_name(&installed);
                        match spatimesh::extensions::install::uninstall(&self.store, &profile, &installed.id) {
                            Ok(()) => {
                                self.with_extensions(|rt, b| rt.refresh(b, &profile, &[]));
                                self.say(format!("{name} is removed"));
                            }
                            Err(e) => self.say(format!("Not removed: {e}")),
                        }
                    }
                    None => self.say(format!("No extension is called {text}")),
                }
            }
        }
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

    fn dismiss_popup(&mut self, w: usize) {
        self.wins[w].flyout = None;
        self.context = None;
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
        if let Some(own) = question.own {
            if accepted {
                // A PIN or password is taken as typed; anything else without its ends' spaces.
                let secret = question.field.secret;
                let text = if secret { question.field.text.clone() } else { question.field.text.trim().to_string() };
                self.answered_own(w, own, text);
            }
            self.wins[w].dirty = true;
            self.focus_pages();
            return;
        }
        let text = (question.kind == DialogKind::Prompt && accepted).then(|| question.field.text.clone());
        self.tell(tab, ToHost::Answer { tab, accepted, text });
        self.wins[w].dirty = true;
        self.focus_pages();
    }

    fn handle(&mut self, event: Event) -> bool {
        // Dark or light: every window's bars drawn again in the theme's new colours.
        if let Event::Look { .. } = event {
            self.wins.iter_mut().for_each(|w| w.dirty = true);
            return true;
        }
        if let Event::Mode(_) | Event::Hands = event {
            return true;
        }
        let Some(id) = event.window() else { return true };
        let Some(w) = self.win_index(id) else { return true };
        if self.wins[w].lock.is_some() && !matches!(event, Event::Ready { .. } | Event::Resized { .. } | Event::CloseRequested { .. } | Event::Focus { .. }) {
            self.locked_input(w, event);
            return true;
        }
        match event {
            Event::Ready { .. } => {
                self.wins[w].ready = true;
                self.wins[w].dirty = true;
            }
            Event::Resized { width, height, .. } => {
                let size = (width.max(640), height.max(400));
                self.wins[w].size = size;
                self.app.resize(id, size);
                self.lay_out_page(w);
                self.wins[w].dirty = true;
                self.wins[w].popup = None;
                self.wins[w].flyout = None;
                self.wins[w].suggest = None;
            }
            Event::CloseRequested { .. } => {
                self.close_window(w);
                return !self.wins.is_empty();
            }
            Event::Focus { focused, .. } => {
                self.wins[w].focused = focused;
                self.wins[w].dirty = true;
                let key = self.wins[w].key;
                if focused && self.last_focused != Some(key) {
                    self.last_focused = Some(key);
                    self.told.push(Told::WindowFocused(Some(key)));
                }
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
                        self.session_dirty = true;
                    }
                }
            }
            return;
        }
        // What is over everything has the pointer: files to choose, a menu, the suggestions.
        if self.wins[w].picker.is_some() {
            self.wins[w].hover = Hover::Nothing;
            self.picker_pointer(w, x, y);
            return;
        }
        if self.popup_pointer(w, x, y) || self.suggest_pointer(w, x, y) {
            self.wins[w].hover = Hover::Nothing;
            return;
        }
        // An extension's popup: the pointer is its while over it.
        if let Some((tab, rect)) = self.ext_popups.get(&key).map(|p| (p.tab, p.rect)) {
            if rect.contains(x, y) {
                self.wins[w].hover = Hover::ExtPopup;
                let m = modifiers(self.app.modifiers());
                self.tell(tab, ToHost::Pointer { tab, x: x - rect.x, y: y - rect.y, button: None, modifiers: m });
                return;
            }
            if self.wins[w].hover == Hover::ExtPopup {
                self.tell(tab, ToHost::PointerLeft { tab });
            }
        }
        if let Some(start) = self.wins[w].start.as_mut() {
            start.list.input(&Input::Pointer { x, y });
        }
        let win = &self.wins[w];
        let over_find = win.find.as_ref().is_some_and(|f| f.rect.contains(x, y));
        let hover = if let Some((id, _)) = win.speakers.iter().find(|(_, r)| r.inset(-4.0).contains(x, y)) {
            Hover::Speaker(*id)
        } else if let Some((id, _, close)) = win.placed.iter().find(|(_, r, _)| r.contains(x, y)) {
            if close.inset(-6.0).contains(x, y) { Hover::Close(*id) } else { Hover::Tab(*id) }
        } else if win.plus.contains(x, y) {
            Hover::Plus
        } else if let Some((which, _)) = win.buttons.iter().find(|(_, r)| r.contains(x, y)) {
            *which
        } else if let Some(i) = win.action_buttons.iter().position(|(_, r)| r.contains(x, y)) {
            Hover::Action(i)
        } else if let Some(i) = win.bar_marks.iter().position(|(_, r)| r.contains(x, y)) {
            Hover::BarMark(i)
        } else if win.address.contains(x, y) {
            Hover::Address
        } else if over_find {
            Hover::Nothing
        } else if win.page.contains(x, y) {
            Hover::Page
        } else {
            Hover::Nothing
        };
        self.wins[w].hover = hover;
        if hover == Hover::Page && self.wins[w].internal.is_some() {
            self.internal_pointer(w, x, y);
        }
        let Some(tab) = self.tabs.active(key) else { return };
        let page = self.wins[w].page;
        let over_page = hover == Hover::Page && !self.covered(w, tab);
        if over_page || self.wins[w].pressed_in_page.is_some() {
            self.wins[w].in_page = true;
            let zoom = self.settings.zoom();
            if self.media.pointer(tab, (x - page.x) / zoom, (y - page.y) / zoom, false) {
                self.dirty_tab(tab);
            }
            let m = modifiers(self.app.modifiers());
            self.tell(tab, ToHost::Pointer { tab, x: x - page.x, y: y - page.y, button: None, modifiers: m });
        } else if std::mem::take(&mut self.wins[w].in_page) {
            self.tell(tab, ToHost::PointerLeft { tab });
        }
    }

    /// Whether something of SpatiMesh's own is over the tab's page: the start page or another
    /// of its own pages, a question, a stopped page, files to choose.
    fn covered(&self, w: usize, tab: TabId) -> bool {
        let t = self.tabs.tab(tab);
        self.wins[w].start.is_some() && t.is_some_and(|t| is_start(&t.url))
            || t.is_some_and(|t| pages::internal(&t.url).is_some())
            || self.wins[w].picker.is_some()
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
        if self.wins[w].picker.is_some() {
            if button == PointerButton::Left {
                self.picker_press(w, x, y, pressed);
            }
            return;
        }
        if let Some((tab, rect)) = self.ext_popups.get(&key).map(|p| (p.tab, p.rect)) {
            if rect.contains(x, y) {
                let m = modifiers(self.app.modifiers());
                self.tell(tab, ToHost::Pointer { tab, x: x - rect.x, y: y - rect.y, button: Some((engine_button, pressed)), modifiers: m });
                return;
            }
            // A press anywhere else closes it, as a menu is closed (its own button does below).
            if pressed && !matches!(self.wins[w].hover, Hover::Action(_)) {
                self.close_ext_popup(key);
                return;
            }
        }
        if self.wins[w].popup.is_some() {
            if button == PointerButton::Left {
                if self.popup_press(w, x, y, pressed) {
                    return;
                }
            } else if pressed {
                let inside = self.wins[w].popup.as_ref().is_some_and(|p| p.rect.contains(x, y)) || self.wins[w].flyout.as_ref().is_some_and(|(_, f)| f.rect.contains(x, y));
                self.dismiss_popup(w);
                if inside {
                    return;
                }
            } else {
                return;
            }
        }
        if button == PointerButton::Left && self.suggest_press(w, x, y, pressed) {
            return;
        }
        if button == PointerButton::Left && (pressed && self.find_press(w, x, y) || !pressed && self.wins[w].find.as_ref().is_some_and(|f| f.rect.contains(x, y))) {
            return;
        }
        let Some(tab) = self.tabs.active(key) else { return };
        let hover = self.wins[w].hover;
        // A menu of its own, for what the right button was pressed on.
        if button == PointerButton::Right {
            match hover {
                Hover::Tab(t) | Hover::Close(t) | Hover::Speaker(t) => {
                    if pressed {
                        self.tab_menu(w, t);
                    }
                    return;
                }
                Hover::BarMark(i) => {
                    if pressed {
                        self.bar_mark_menu(w, i);
                    }
                    return;
                }
                Hover::Page if self.wins[w].internal.is_some() => {
                    if pressed {
                        self.internal_menu(w);
                    }
                    return;
                }
                Hover::Page if self.tabs.tab(tab).is_some_and(|t| is_start(&t.url)) => {
                    if pressed {
                        self.start_menu(w);
                    }
                    return;
                }
                Hover::Page => {}
                _ => return,
            }
        }
        // SpatiMesh's own things over the page.
        if button == PointerButton::Left && hover == Hover::Page {
            if self.wins[w].internal.is_some() {
                self.internal_press(w, x, y, pressed);
                return;
            }
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
                if !pressed {
                    if let Some(url) = self.wins[w].top_sites.iter().find(|(_, r)| r.contains(x, y)).map(|(u, _)| u.clone()) {
                        self.go(w, &url);
                        return;
                    }
                }
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
        if hover == Hover::Page && !self.covered(w, tab) {
            if pressed {
                // The page has the keyboard once it is clicked.
                if self.wins[w].editing {
                    self.stop_editing(w);
                }
                if let Some(find) = self.wins[w].find.as_mut() {
                    find.field.focused = false;
                }
                self.focus_pages();
                self.wins[w].pressed_in_page = Some(engine_button);
            }
            let page = self.wins[w].page;
            let zoom = self.settings.zoom();
            // A film's own controls (SpatiMesh's, for an element that asked for the browser's).
            if pressed && engine_button == engine::Button::Left && self.media.pointer(tab, (x - page.x) / zoom, (y - page.y) / zoom, true) {
                self.wins[w].pressed_in_page = None;
                self.dirty_tab(tab);
                return;
            }
            let m = modifiers(self.app.modifiers());
            self.tell(tab, ToHost::Pointer { tab, x: x - page.x, y: y - page.y, button: Some((engine_button, pressed)), modifiers: m });
            return;
        }
        if button == PointerButton::Middle && pressed {
            match hover {
                // A middle click on a tab closes it; on a bookmark, opens it behind.
                Hover::Tab(t) | Hover::Close(t) => self.close_tab(t),
                Hover::BarMark(i) => {
                    if let Some(url) = self.wins[w].bar_marks.get(i).map(|(u, _)| u.clone()) {
                        self.act(w, Action::OpenInBackground(url));
                    }
                }
                _ => {}
            }
            return;
        }
        if button != PointerButton::Left {
            return;
        }
        if pressed {
            match hover {
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
                        win.address.select_all();
                        if let Some(find) = win.find.as_mut() {
                            find.field.focused = false;
                        }
                        if let Some(i) = win.internal.as_mut() {
                            i.field.focused = false;
                        }
                        self.focus_pages();
                    }
                }
                Hover::Action(i) => {
                    // An extension's button: its popup, or its onClicked; pressed again, its
                    // popup closes.
                    if let Some((extension, _)) = self.wins[w].action_buttons.get(i).cloned() {
                        let open = self.ext_popups.get(&key).map(|p| p.tab);
                        let same = open.is_some_and(|t| self.extensions.as_ref().and_then(|rt| rt.popup_of(t)).is_some_and(|(_, e, _)| e == extension));
                        if open.is_some() {
                            self.close_ext_popup(key);
                        }
                        if !same {
                            let profile = self.tabs.window(key).map(|x| x.profile.clone()).unwrap_or_default();
                            self.with_extensions(|rt, b| rt.action_clicked(b, &profile, key, &extension));
                        }
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
        match hover {
            Hover::Close(t) => self.close_tab(t),
            Hover::Speaker(t) => {
                let muted = self.tabs.tab(t).is_some_and(|t| t.muted);
                self.mute_tab(t, !muted);
            }
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
            Hover::Star => self.bookmark_page(w),
            Hover::Menu => {
                if self.wins[w].popup.is_some() {
                    self.dismiss_popup(w);
                } else {
                    self.menu(w);
                }
            }
            Hover::Profile => self.profile_menu(w),
            Hover::Lock => self.lock_all(),
            Hover::Security => self.site_menu(w),
            Hover::Downloads => self.downloads_menu(w),
            Hover::StoreAdd => self.add_from_store(w),
            Hover::BarMark(i) => {
                if let Some(url) = self.wins[w].bar_marks.get(i).map(|(u, _)| u.clone()) {
                    self.go(w, &url);
                }
            }
            _ => {}
        }
    }

    /// The address bar let go of: it says the page's address again.
    fn stop_editing(&mut self, w: usize) {
        let key = self.wins[w].key;
        let url = self.tabs.active(key).and_then(|t| self.tabs.tab(t)).map(|t| t.url.clone()).unwrap_or_default();
        let engine = self.search_of_win(w);
        let win = &mut self.wins[w];
        win.editing = false;
        win.address.focused = false;
        win.suggest = None;
        if !is_start(&url) {
            win.address.set(shown(&url, &engine));
        }
        self.focus_pages();
    }

    fn scroll(&mut self, w: usize, dx: f32, dy: f32) {
        self.wins[w].dirty = true;
        let (x, y) = self.wins[w].pointer;
        if self.wins[w].picker.is_some() {
            self.picker_scroll(w, dx, dy);
            return;
        }
        if let Some((tab, rect)) = self.ext_popups.get(&self.wins[w].key).map(|p| (p.tab, p.rect)) {
            if rect.contains(x, y) {
                let m = modifiers(self.app.modifiers());
                self.tell(tab, ToHost::Scroll { tab, x: x - rect.x, y: y - rect.y, dx, dy, modifiers: m });
                return;
            }
        }
        if let Some((_, flyout)) = self.wins[w].flyout.as_mut() {
            if flyout.rect.contains(x, y) {
                flyout.list.input(&Input::Scroll { dx, dy });
                return;
            }
        }
        if let Some(popup) = self.wins[w].popup.as_mut() {
            if popup.rect.contains(x, y) {
                popup.list.input(&Input::Scroll { dx, dy });
                return;
            }
        }
        if let Some(sg) = self.wins[w].suggest.as_mut() {
            if sg.rect.contains(x, y) {
                sg.list.input(&Input::Scroll { dx, dy });
                return;
            }
        }
        let key = self.wins[w].key;
        let Some(tab) = self.tabs.active(key) else { return };
        if self.wins[w].internal.is_some() {
            self.internal_scroll(w, dx, dy);
            return;
        }
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

    /// SpatiMesh's own keys, with Control (Command on a Mac) held. Whether it was one.
    fn command_key(&mut self, w: usize, key: Key, code: u32, m: Modifiers) -> bool {
        let window = self.wins[w].key;
        let Some(tab) = self.tabs.active(window) else { return false };
        let letter = match key {
            Key::Char(c) => Some(c.to_ascii_lowercase()),
            _ => None,
        };
        let count = self.tabs.window(window).map_or(0, |x| x.tabs.len());
        let active = self.tabs.window(window).map_or(0, |x| x.active);
        match (letter, key) {
            (Some('t'), _) if m.shift => self.reopen_closed(w, None),
            (Some('t'), _) => self.new_tab(w, "about:blank"),
            (Some('w'), _) => self.close_tab(tab),
            (Some('n'), _) if m.shift => {
                self.new_window(PRIVATE, None);
            }
            (Some('n'), _) => {
                let profile = self.tabs.window(window).map(|x| x.profile.clone()).unwrap_or_default();
                self.new_window(&profile, None);
            }
            // By the key, not the letter: with Option held a Mac types another one.
            _ if (m.alt || m.shift) && code == KEY_I => self.developer_tools(w),
            (Some('l' | 'e' | 'k'), _) => self.focus_address(w),
            (Some('r'), _) => self.reopen(tab),
            (Some('d'), _) => self.bookmark_page(w),
            (Some('f'), _) => self.open_find(w),
            (Some('g'), _) => self.find_next(w, !m.shift),
            (Some('h'), _) => self.open_page(w, pages::HISTORY),
            (Some('j'), _) => self.open_page(w, pages::DOWNLOADS),
            (Some('o'), _) if m.shift => self.open_page(w, pages::BOOKMARKS),
            (Some('b'), _) if m.shift => self.act(w, Action::ToggleBookmarksBar),
            (Some('s'), _) => self.act(w, Action::SavePage),
            (Some('+' | '='), _) => self.zoom(Some(1.1)),
            (Some('-'), _) => self.zoom(Some(1.0 / 1.1)),
            (Some('0'), _) => self.zoom(None),
            (Some(d @ '1'..='9'), _) => {
                let index = if d == '9' { count.saturating_sub(1) } else { (d as usize - '1' as usize).min(count.saturating_sub(1)) };
                self.tabs.activate(window, index);
                self.after_tab_change(window);
            }
            (_, Key::Tab) | (_, Key::PageUp) | (_, Key::PageDown) if count > 0 => {
                let back = key == Key::PageUp || (key == Key::Tab && m.shift);
                let next = if back { (active + count - 1) % count } else { (active + 1) % count };
                if m.shift && key != Key::Tab {
                    // Control-Shift-PageUp or -PageDown: the tab moves along the bar.
                    self.tabs.reorder(window, active, next);
                    self.session_dirty = true;
                    self.wins[w].dirty = true;
                } else {
                    self.tabs.activate(window, next);
                    self.after_tab_change(window);
                }
            }
            _ => return false,
        }
        true
    }

    /// Control-L: the address bar has the keyboard, its address chosen to be typed over.
    fn focus_address(&mut self, w: usize) {
        let win = &mut self.wins[w];
        win.editing = true;
        win.address.focused = true;
        win.fresh = true;
        win.address.select_all();
        if let Some(find) = win.find.as_mut() {
            find.field.focused = false;
        }
        if let Some(i) = win.internal.as_mut() {
            i.field.focused = false;
        }
        self.focus_pages();
    }

    #[allow(clippy::too_many_arguments)]
    fn key(&mut self, w: usize, key: Key, code: u32, pressed: bool, repeat: bool, text: Option<String>, m: Modifiers) {
        let window = self.wins[w].key;
        self.wins[w].dirty = true;
        let Some(tab) = self.tabs.active(window) else { return };
        // An extension's popup has the keyboard while it is open; Escape closes it.
        if let Some(popup_tab) = self.ext_popups.get(&window).map(|p| p.tab) {
            if pressed && key == Key::Escape {
                self.close_ext_popup(window);
            } else {
                let key = KeyPress { code, key, text, pressed, repeat, modifiers: modifiers(m) };
                self.tell(popup_tab, ToHost::Key { tab: popup_tab, key });
            }
            return;
        }
        if self.wins[w].picker.is_some() {
            if pressed {
                self.picker_key(w, key, text, m);
            }
            return;
        }
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
        if self.wins[w].popup.is_some() {
            if pressed {
                self.popup_key(w, &input, key);
            }
            return;
        }
        if pressed && command(m) && self.command_key(w, key, code, m) {
            return;
        }
        // The function keys, by where they are.
        if pressed && !command(m) {
            match code {
                // F3: the next match, with Shift the one before.
                61 => return self.find_next(w, !m.shift),
                // F5: load again.
                63 => return self.reopen(tab),
                // F6: the address bar.
                64 => return self.focus_address(w),
                // F12: the developer tools.
                88 => return self.developer_tools(w),
                _ => {}
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
                Key::Home => {
                    let home = self.settings.home.clone();
                    self.go(w, &home);
                    return;
                }
                _ => {}
            }
        }
        if pressed && self.find_key(w, key, text.clone(), m) {
            return;
        }
        // The address bar.
        if self.wins[w].editing {
            if !pressed {
                return;
            }
            match key {
                Key::Escape if self.wins[w].suggest.is_some() => {
                    // First the suggestions go, and what was typed is as it was typed.
                    let typed = self.wins[w].suggest.take().map(|s| s.typed).unwrap_or_default();
                    self.wins[w].address.set(typed);
                }
                Key::Escape => self.stop_editing(w),
                Key::Enter => {
                    if !self.suggest_enter(w) {
                        let typed = self.wins[w].address.text.clone();
                        self.go(w, &typed);
                    }
                }
                _ if self.suggest_key(w, key, m) => {}
                _ => {
                    let win = &mut self.wins[w];
                    // A freshly chosen bar has its whole address chosen (`select_all`), so the
                    // first thing typed or pasted replaces it, as in any browser.
                    let types = text.as_deref().is_some_and(|t| !t.is_empty()) && !command(m);
                    if !matches!(key, Key::Left | Key::Right | Key::Home | Key::End) || types {
                        win.fresh = false;
                    }
                    let before = win.address.text.clone();
                    win.address.input(&input);
                    self.bar_typed(w, &before, key, text.as_deref(), m);
                }
            }
            return;
        }
        if pressed && self.internal_key(w, key, text.clone(), m) {
            return;
        }
        // The clipboard's keys over a page: done in the page itself, the same on every engine,
        // through the room's clipboard (and so with the hosts' and this machine's own).
        if pressed && command(m) && !self.covered(w, tab) {
            if let Key::Char(c) = key {
                if self.page_clipboard(tab, c.to_ascii_lowercase()) {
                    return;
                }
            }
        }
        if pressed && key == Key::Escape && self.wins[w].fullscreen {
            self.set_fullscreen(tab, false, true);
            return;
        }
        if pressed && key == Key::Escape && self.wins[w].find.is_some() {
            self.close_find(w);
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
            self.prepare_internal(w);
            // The start page: the profile's bookmarks, and the sites it visits most.
            if active.as_ref().is_some_and(|t| is_start(&t.url)) && self.wins[w].start.is_none() {
                let marks: Vec<Bookmark> = bookmarks::children(self.bookmarks_of(&profile.id), "").into_iter().filter(|b| !b.is_folder()).cloned().collect();
                let rows = marks.iter().map(|b| Row::with(b.title.clone(), host_of(b.url.as_deref().unwrap_or(""))).icon(Icon::Star)).collect();
                let actions = marks.iter().map(|b| Action::Open(b.url.clone().unwrap_or_default())).collect();
                self.wins[w].start = Some(Popup { rows, actions, rect: Rect::default(), list: List::default(), choose_for: None });
                self.wins[w].top_list = self.most_visited(&profile.id, 8);
            }
            let title = match &active {
                Some(t) if profile.is_private() => format!("{} \u{2014} SpatiMesh (Private)", t.label()),
                Some(t) => format!("{} \u{2014} SpatiMesh", t.label()),
                None => "SpatiMesh".to_string(),
            };
            // A speaker before it while any of the window's tabs is playing: the room places
            // SpatiMesh's sound -- one stream for all its windows, as the engine plays it -- at
            // the window that says it plays (`audio::says_it_plays`).
            let playing = window.tabs.iter().any(|t| self.tabs.tab(*t).is_some_and(|t| t.playing && !t.muted) || self.media.playing_in(*t));
            let title = if playing { format!("\u{1F50A} {title}") } else { title };
            // A locked window says nothing of what is in it.
            let title = if self.wins[w].lock.is_some() { format!("{} is locked \u{2014} SpatiMesh", profile.name) } else { title };
            let id = self.wins[w].id;
            self.app.set_title(id, &title);
            let engine_note = self.hosts.get(&window.profile).and_then(|h| h.engine.clone());
            let program_note = match &self.program {
                Program::Fake(why) => Some(format!("No web engine: {why}. Pages are flat colours.")),
                _ => None,
            };
            let starting = engine_note.is_none();
            let actions = self.actions_for(key);
            let icons: Vec<Option<spatiand_app::ui::Texture>> = actions.iter().map(|a| self.icon(a)).collect();
            let ext_popup = self.ext_popups.get(&key).map(|p| (p.rect, p.tab));
            let toast = self.toast.as_ref().filter(|(_, at)| at.elapsed() < TOAST).map(|(t, _)| t.clone());
            let zoom = self.settings.zoom();
            let marked = active.as_ref().is_some_and(|t| profile.id != PRIVATE && bookmarks::find(self.bookmarks_of(&profile.id), &t.url).is_some());
            let store_add = !profile.is_private() && active.as_ref().is_some_and(|t| spatimesh::extensions::store::listing(&t.url).is_some());
            let downloads = (!self.downloads.of(&profile.id).is_empty()).then(|| self.downloads.progress());
            let bar_marks: Vec<(String, String)> = if self.settings.bookmarks_bar && !profile.is_private() {
                bookmarks::children(self.bookmarks_of(&profile.id), "").into_iter().filter(|b| !b.is_folder()).take(40).map(|b| (b.title.clone(), b.url.clone().unwrap_or_default())).collect()
            } else {
                Vec::new()
            };
            let bar = self.settings.bookmarks_bar && !profile.is_private();
            let hovered = active.as_ref().and_then(|t| self.views.get(&t.id)).map(|v| v.hovered.clone()).unwrap_or_default();
            let (painter, wins, views, tabs, media) = (&mut self.painter, &mut self.wins, &self.views, &self.tabs, &mut self.media);
            let win = &mut wins[w];
            let size = win.size;
            self.app.draw(id, |gl, picture| {
                let p = painter.get_or_insert_with(|| Painter::new(gl).expect("the painter"));
                p.begin(gl, picture, picture.1 as f32 / size.1 as f32, theme::ground());
                look::backdrop(p, gl);
                let all = p.bounds();
                if let Some(pane) = win.lock.as_mut() {
                    draw_lock(p, gl, all, &profile, pane);
                    p.end(gl);
                    return;
                }
                if !win.fullscreen {
                    // The window in its profile's colour: the bars washed with it, the page framed in it.
                    p.fill(gl, Rect::new(all.x, all.y, all.w, win.page.y - all.y), 0.0, profile_colour(&profile, 0.22));
                    p.fill(gl, win.page.inset(-4.0), 15.0, profile_colour(&profile, 0.9));
                    draw_tabs(p, gl, win, &window, tabs, &profile, all);
                    let state = Toolbar { tab: active.as_ref(), marked, profile: &profile, actions: &actions, icons: &icons, store_add, downloads };
                    draw_toolbar(p, gl, win, all, &state);
                    if bar {
                        draw_bookmarks_bar(p, gl, win, all, &bar_marks);
                    } else {
                        win.bar_marks.clear();
                    }
                }
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
                        if media.films_in(tab.id) {
                            // Its films under it, where their boxes are (`media`).
                            media.draw(gl, p, tab.id, page, shown, uv, texture.raw(), zoom);
                        } else {
                            p.image(gl, texture.texture(), shown, uv, 0.0, [1.0, 1.0, 1.0, 1.0], true);
                        }
                    } else {
                        p.fill(gl, page, 12.0, theme::ground());
                        let note = if starting { "Starting the web engine\u{2026}" } else { "" };
                        look::message(p, gl, page, note, theme::ink_hint());
                    }
                    if is_start(&tab.url) {
                        draw_start(p, gl, win, &profile, program_note.as_deref());
                    } else if let Some(internal) = win.internal.as_mut() {
                        pages::draw_internal(p, gl, page, internal, &profile.name);
                    } else if let Some(why) = &tab.crashed {
                        draw_notice(p, gl, page, "This page stopped", why, "[Enter] Load it again    The other tabs are as they were.");
                    }
                    if let Some(question) = view.and_then(|v| v.question.as_ref()) {
                        draw_question(p, gl, page, question);
                    }
                    if let Some(bar) = win.find.as_mut().filter(|b| b.tab == tab.id) {
                        find::draw_find(p, gl, page, bar, win.pointer);
                    }
                    if !hovered.is_empty() && !win.editing {
                        draw_status(p, gl, page, &hovered);
                    }
                }
                if let Some((rect, tab)) = ext_popup {
                    look::glass(p, gl, rect.inset(-6.0), 18.0, 1.0);
                    match views.get(&tab).and_then(|v| v.texture.as_ref()) {
                        Some(texture) => {
                            let shown = Rect::new(rect.x, rect.y, (texture.width as f32).min(rect.w), (texture.height as f32).min(rect.h));
                            let uv = [0.0, 0.0, shown.w / texture.width as f32, shown.h / texture.height as f32];
                            p.image(gl, texture.texture(), shown, uv, 12.0, [1.0, 1.0, 1.0, 1.0], true);
                        }
                        None => p.fill(gl, rect, 12.0, [1.0, 1.0, 1.0, 1.0]),
                    }
                }
                if let Some(picker) = win.picker.as_mut() {
                    pages::draw_picker(p, gl, win.page, picker, win.pointer);
                }
                if win.editing {
                    if let Some(sg) = win.suggest.as_mut() {
                        omni::draw_suggestions(p, gl, win.address_rect, size.1 as f32, sg);
                    }
                }
                // Menus are solid: words of the page under them would be read through them.
                if let Some(popup) = win.popup.as_mut() {
                    p.fill(gl, popup.rect, 18.0, theme::ground());
                    look::glass(p, gl, popup.rect, 18.0, 1.0);
                    popup.list.draw(p, gl, popup.rect.inset(12.0), &popup.rows);
                }
                if let Some((_, flyout)) = win.flyout.as_mut() {
                    p.fill(gl, flyout.rect, 18.0, theme::ground());
                    look::glass(p, gl, flyout.rect, 18.0, 1.0);
                    flyout.list.draw(p, gl, flyout.rect.inset(12.0), &flyout.rows);
                }
                if let Some(text) = toast.as_deref() {
                    let page = win.page;
                    let pane = Rect::new(page.x + page.w * 0.5 - 420.0, page.bottom() - 96.0, 840.0, 64.0);
                    look::glass(p, gl, pane, 24.0, 1.0);
                    p.label(gl, text, pane.inset(18.0), theme::ROW_EM * 0.8, theme::ink(), 0.5);
                }
                if let Some(drag) = win.drag.as_ref().filter(|d| d.torn) {
                    let label = tabs.tab(drag.tab).map(|t| t.label()).unwrap_or_default();
                    let ghost = Rect::new(drag.at.0 - 160.0, drag.at.1 - 26.0, 320.0, 52.0);
                    p.glow(gl, ghost, 26.0, [0.30, 0.62, 1.0, 0.45], 26.0);
                    look::glass(p, gl, ghost, 26.0, 1.0);
                    p.label(gl, &label, ghost.inset(16.0), theme::ROW_EM * 0.8, theme::ink(), 0.5);
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

    /// The sites a profile visits most, lately: one page each, by how often and how lately.
    fn most_visited(&mut self, profile: &str, count: usize) -> Vec<(String, String)> {
        let Some(kept) = self.kept.get_mut(profile) else { return Vec::new() };
        let mut entries = kept.history.entries(&self.store);
        let now = spatimesh::store::now_ms();
        let weight = |e: &spatimesh::history::Entry| (e.visits as f32).ln_1p() + 2.0 * (-(now.saturating_sub(e.last) as f32 / 86_400_000.0) / 14.0).exp();
        entries.sort_by(|a, b| weight(b).total_cmp(&weight(a)));
        let mut sites: Vec<(String, String)> = Vec::new();
        for e in entries {
            let host = host_of(&e.url);
            if host.is_empty() || sites.iter().any(|(u, _)| host_of(u) == host) {
                continue;
            }
            sites.push((e.url.clone(), if e.title.trim().is_empty() { host.clone() } else { e.title.clone() }));
            if sites.len() == count {
                break;
            }
        }
        sites
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
                self.searches.clear();
                self.refresh_internal();
                for win in &mut self.wins {
                    win.start = None;
                    win.dirty = true;
                }
                self.kept_synced();
            }
            self.seen_changes = latest;
        }
    }

    pub fn run(&mut self) -> Result<(), String> {
        loop {
            let busy = self.tabs.tabs().any(|t| t.loading) || self.in_flight.is_some();
            let mut wait = if self.media.busy() { 8 } else if busy { 30 } else { 500 };
            let next_tick = self.extensions.as_ref().and_then(Runtime::next_tick);
            if let Some(at) = next_tick {
                wait = wait.min(at.saturating_duration_since(Instant::now()).as_millis() as u64);
            }
            for event in self.app.wait(Some(Duration::from_millis(wait)))? {
                if !self.handle(event) {
                    return Ok(());
                }
            }
            self.hear();
            self.route_sounds();
            self.media.tick();
            if let Ok(gl) = self.app.gl() {
                self.media.capture(gl);
            }
            for (tab, code) in self.media.take_out() {
                self.tell(tab, ToHost::RunScript { tab, world: String::new(), all_frames: false, code, request: 0, boot: false });
            }
            for tab in self.media.take_fresh() {
                self.dirty_tab(tab);
            }
            self.tell_extensions();
            if next_tick.is_some_and(|at| at <= Instant::now()) {
                self.with_extensions(|rt, b| rt.tick(b));
            }
            if self.toast.as_ref().is_some_and(|(_, at)| at.elapsed() >= TOAST) {
                self.toast = None;
                for win in &mut self.wins {
                    win.dirty = true;
                }
            }
            // A tab let go of that no window claimed becomes a window of its own.
            if let Some((tab, _, at)) = self.in_flight {
                if at.elapsed() >= Duration::from_millis(600) {
                    self.in_flight = None;
                    self.tear_off(tab);
                }
            }
            self.accept_other_starts();
            self.notice_sync();
            self.kept_tick();
            self.session_tick();
            if self.wins.is_empty() {
                return Ok(());
            }
            self.draw()?;
        }
    }
}

/// The page's place in a window of `size`: below the bookmarks bar too, if it is shown.
pub fn page_rect(size: (u32, u32), bookmarks_bar: bool) -> Rect {
    let top = if bookmarks_bar { PAGE_TOP + BAR_H } else { PAGE_TOP };
    Rect::new(MARGIN, top, size.0 as f32 - MARGIN * 2.0, size.1 as f32 - top - MARGIN)
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
    let pinned_w = 68.0;
    let pinned = window.tabs.iter().filter(|t| tabs.tab(**t).is_some_and(|t| t.pinned)).count();
    let others = (window.tabs.len() - pinned).max(1);
    let width = ((bar.w - plus_w - 16.0 - pinned as f32 * pinned_w) / others as f32).clamp(120.0, 330.0);
    win.placed.clear();
    win.speakers.clear();
    let mut x = bar.x;
    for (index, &id) in window.tabs.iter().enumerate() {
        let Some(tab) = tabs.tab(id) else { continue };
        let w = if tab.pinned { pinned_w } else { width };
        let rect = Rect::new(x, bar.y, w - 8.0, bar.h);
        x += w;
        if rect.right() > bar.right() - plus_w {
            break;
        }
        let active = index == window.active;
        let hover = matches!(win.hover, Hover::Tab(t) | Hover::Close(t) | Hover::Speaker(t) if t == id);
        let rect = match win.drag.as_ref() {
            Some(d) if d.tab == id && d.moved && !d.torn => Rect { x: (d.at.0 - rect.w * 0.5).clamp(bar.x, bar.right() - plus_w - rect.w), ..rect },
            _ => rect,
        };
        if active {
            p.glow(gl, rect, 14.0, [0.30, 0.62, 1.0, if win.focused { 0.18 } else { 0.06 }], 16.0);
            p.gradient(gl, rect, 14.0, theme::wash(0.11), theme::wash(0.05));
            let c = profile.colour;
            p.fill(gl, Rect::new(rect.x + 16.0, rect.bottom() - 3.0, rect.w - 32.0, 2.5), 1.25, [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 0.9]);
        } else if hover {
            p.fill(gl, rect, 14.0, theme::hover());
        }
        // How the tab is: loading, sounding, stopped.
        let colour = if tab.crashed.is_some() {
            look::trouble()
        } else if tab.loading {
            look::BLUE
        } else if tab.playing {
            look::VIOLET
        } else if active {
            look::TEAL
        } else {
            theme::ink_hint()
        };
        let close = Rect::new(rect.right() - 38.0, rect.y + rect.h * 0.5 - 13.0, 26.0, 26.0);
        if tab.pinned {
            // A pinned tab: its first letter, and how it is under it.
            let initial: String = tab.label().chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_default();
            p.label(gl, &initial, Rect { h: rect.h - 6.0, ..rect }, theme::ROW_EM * 0.72, if active { theme::ink() } else { theme::ink_row() }, 0.5);
            p.fill(gl, Rect::new(rect.x + rect.w * 0.5 - 4.0, rect.bottom() - 10.0, 8.0, 4.0), 2.0, colour);
            win.placed.push((id, rect, Rect::default()));
            continue;
        }
        p.fill(gl, Rect::new(rect.x + 16.0, rect.y + rect.h * 0.5 - 4.0, 8.0, 8.0), 4.0, colour);
        let mut label = Rect { x: rect.x + 34.0, w: rect.w - 34.0 - if active || hover { 44.0 } else { 14.0 }, ..rect };
        // A tab that sounds, or was silenced: its speaker, pressed to silence it or not.
        if tab.playing || tab.muted {
            let speaker = Rect::new(label.x, rect.y + rect.h * 0.5 - 13.0, 26.0, 26.0);
            if win.hover == Hover::Speaker(id) {
                p.fill(gl, speaker.inset(-3.0), 15.0, theme::wash(0.14));
            }
            draw_icon(p, gl, if tab.muted { Icon::Muted } else { Icon::Speaker }, speaker.inset(2.0), if tab.muted { theme::ink_hint() } else { look::VIOLET });
            win.speakers.push((id, speaker));
            label.x += 34.0;
            label.w -= 34.0;
        }
        p.label(gl, &tab.label(), label, theme::ROW_EM * 0.72, if active { theme::ink() } else { theme::ink_row() }, 0.0);
        if active || hover {
            if win.hover == Hover::Close(id) {
                p.fill(gl, close, 13.0, theme::wash(0.14));
            }
            draw_icon(p, gl, Icon::Close, close.inset(4.0), theme::ink_detail());
        }
        win.placed.push((id, rect, close));
    }
    win.plus = Rect::new(x.min(bar.right() - plus_w), bar.y + (bar.h - 44.0) * 0.5, 44.0, 44.0);
    if win.hover == Hover::Plus {
        p.fill(gl, win.plus, 22.0, theme::hover());
    }
    draw_icon(p, gl, Icon::Plus, win.plus.inset(10.0), theme::ink_detail());
}

/// A profile's colour, as the painter takes it.
fn profile_colour(profile: &Profile, alpha: f32) -> [f32; 4] {
    let c = profile.colour;
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, alpha]
}

/// What the toolbar shows of the window's tab in front.
struct Toolbar<'a> {
    tab: Option<&'a spatimesh::tabs::Tab>,
    marked: bool,
    profile: &'a Profile,
    actions: &'a [spatimesh::extensions::runtime::ActionView],
    icons: &'a [Option<spatiand_app::ui::Texture>],
    /// The page is an extension's in its store: "Add to SpatiMesh".
    store_add: bool,
    /// The downloads' button, with how far those going have come, when there are any.
    downloads: Option<Option<f32>>,
}

fn draw_toolbar(p: &mut Painter, gl: &spatiand_app::Gl, win: &mut Win, all: Rect, state: &Toolbar) {
    let mut bar = Rect::new(all.x + 12.0, TOOL_TOP, all.w - 24.0, TOOL_H);
    let side = TOOL_H;
    let tab = state.tab;
    let button = |p: &mut Painter, rect: Rect, which: Hover, icon: Icon, enabled: bool, lit: bool| {
        if win.hover == which && enabled {
            p.fill(gl, rect, side * 0.5, theme::wash(0.11));
        }
        let ink = if !enabled {
            theme::wash(0.25)
        } else if lit {
            look::BLUE
        } else {
            theme::ink_detail()
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
    let lock = button(p, bar.take_right(side), Hover::Lock, Icon::Lock, true, false);
    bar.take_right(4.0);
    // Whose window this is, plainly: the profile's colour and name, next to the menu.
    let profile = state.profile;
    let colour = profile_colour(profile, 1.0);
    let name: String = profile.name.chars().take(14).collect();
    let pill_w = (p.measure(gl, &name, theme::ROW_EM * 0.7, 400.0).0 + 54.0).min(220.0);
    let who = bar.take_right(pill_w);
    let pill = Rect::new(who.x, who.y + (who.h - 40.0) * 0.5, who.w, 40.0);
    p.fill(gl, pill, 20.0, profile_colour(profile, if win.hover == Hover::Profile { 0.55 } else { 0.38 }));
    p.fill(gl, Rect::new(pill.x + 14.0, pill.y + 13.0, 14.0, 14.0), 7.0, colour);
    p.label(gl, &name, Rect::new(pill.x + 36.0, pill.y, pill.w - 46.0, pill.h), theme::ROW_EM * 0.7, theme::ink(), 0.0);
    bar.take_right(8.0);
    // The downloads, while there are any: how far those going have come, under the arrow.
    let mut downloads = Rect::default();
    if let Some(progress) = state.downloads {
        downloads = button(p, bar.take_right(side), Hover::Downloads, Icon::Download, true, progress.is_some());
        if let Some(progress) = progress {
            let track = Rect::new(downloads.x + 12.0, downloads.bottom() - 9.0, downloads.w - 24.0, 4.0);
            p.fill(gl, track, 2.0, theme::wash(0.18));
            p.fill(gl, Rect { w: track.w * progress.clamp(0.04, 1.0), ..track }, 2.0, look::BLUE);
        }
        bar.take_right(4.0);
    }
    // The extensions' buttons, in the order they were installed, with their badges.
    win.action_buttons.clear();
    for (i, view) in state.actions.iter().enumerate().rev() {
        let rect = bar.take_right(side);
        bar.take_right(2.0);
        if win.hover == Hover::Action(win.action_buttons.len()) && view.enabled {
            p.fill(gl, rect, side * 0.5, theme::wash(0.11));
        }
        let tint = if view.enabled { [1.0, 1.0, 1.0, 1.0] } else { [1.0, 1.0, 1.0, 0.35] };
        match state.icons.get(i).and_then(|t| t.as_ref()) {
            Some(texture) => p.image(gl, Texture::from_raw(texture.raw(), texture.width, texture.height), rect.inset(13.0), [0.0, 0.0, 1.0, 1.0], 0.0, tint, false),
            None => {
                let initial: String = view.name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                p.fill(gl, rect.inset(12.0), (side - 24.0) * 0.5, [0.30, 0.62, 1.0, 0.35 * tint[3]]);
                p.label(gl, &initial, rect.inset(12.0), theme::ROW_EM * 0.7, theme::ink(), 0.5);
            }
        }
        if !view.badge.is_empty() {
            let text: String = view.badge.chars().take(4).collect();
            let w = (16.0 + 9.0 * text.chars().count() as f32).min(side);
            let badge = Rect::new(rect.right() - w, rect.bottom() - 22.0, w, 20.0);
            let c = |v: [u8; 4]| [v[0] as f32 / 255.0, v[1] as f32 / 255.0, v[2] as f32 / 255.0, v[3] as f32 / 255.0];
            p.fill(gl, badge, 10.0, c(view.badge_background));
            p.label(gl, &text, badge, theme::ROW_EM * 0.45, c(view.badge_color), 0.5);
        }
        win.action_buttons.push((view.extension.clone(), rect));
    }
    if !state.actions.is_empty() {
        bar.take_right(8.0);
    }
    // An extension's page in its store: added from here, as Chrome and Firefox add from there.
    let mut store_add = Rect::default();
    if state.store_add {
        let label = "Add to SpatiMesh";
        let w = p.measure(gl, label, theme::ROW_EM * 0.66, 600.0).0 + 70.0;
        let slot = bar.take_right(w);
        store_add = Rect::new(slot.x, slot.y + (slot.h - 40.0) * 0.5, slot.w, 40.0);
        let lit = win.hover == Hover::StoreAdd;
        p.fill(gl, store_add, 20.0, if lit { theme::mark() } else { theme::selected() });
        draw_icon(p, gl, Icon::Puzzle, Rect::new(store_add.x + 12.0, store_add.y + 8.0, 24.0, 24.0), theme::ink());
        p.label(gl, label, Rect::new(store_add.x + 44.0, store_add.y, store_add.w - 52.0, store_add.h), theme::ROW_EM * 0.66, theme::ink(), 0.0);
        bar.take_right(8.0);
    }
    let private = profile.is_private();
    let on_page = tab.is_some_and(|t| !is_start(&t.url) && pages::internal(&t.url).is_none());
    let star = button(p, bar.take_right(side), Hover::Star, Icon::Star, !private && on_page, state.marked);
    bar.take_right(8.0);
    // How the page came: a padlock over HTTPS, a warning over plain HTTP.
    let security = bar.take_left(side);
    let url = tab.map(|t| t.url.as_str()).unwrap_or("");
    let (icon, ink) = if url.starts_with("https://") {
        (Icon::Lock, theme::ink_detail())
    } else if url.starts_with("http://") {
        (Icon::Warning, look::trouble())
    } else if is_start(url) {
        (Icon::Search, theme::ink_hint())
    } else if pages::internal(url).is_some() {
        (Icon::Options, theme::ink_hint())
    } else {
        (Icon::Document, theme::ink_hint())
    };
    if win.hover == Hover::Security {
        p.fill(gl, security, side * 0.5, theme::wash(0.11));
    }
    draw_icon(p, gl, icon, security.inset(14.0), ink);
    bar.take_left(4.0);
    win.address.draw(p, gl, bar);
    win.address_rect = bar;
    if loading {
        // How much of the page has come, along the foot of the bar.
        let progress = tab.map_or(0.0, |t| t.progress).clamp(0.05, 1.0);
        let line = Rect::new(bar.x + 8.0, bar.bottom() - 3.0, (bar.w - 16.0) * progress, 3.0);
        p.fill(gl, line, 1.5, look::BLUE);
    }
    win.buttons = [
        (Hover::Back, back),
        (Hover::Forward, forward),
        (Hover::Reload, reload),
        (Hover::Star, star),
        (Hover::Menu, menu),
        (Hover::Profile, pill),
        (Hover::Lock, lock),
        (Hover::Security, security),
        (Hover::Downloads, downloads),
        (Hover::StoreAdd, store_add),
    ];
}

/// The bookmarks bar: the profile's bookmarks, under the toolbar.
fn draw_bookmarks_bar(p: &mut Painter, gl: &spatiand_app::Gl, win: &mut Win, all: Rect, marks: &[(String, String)]) {
    let mut bar = Rect::new(all.x + 18.0, PAGE_TOP - 2.0, all.w - 36.0, BAR_H - 6.0);
    win.bar_marks.clear();
    if marks.is_empty() {
        p.label(gl, "Bookmarks are shown here: the star keeps the page in front (Ctrl-D)", bar, theme::FOOTER_EM, theme::ink_hint(), 0.0);
        return;
    }
    let em = theme::ROW_EM * 0.64;
    for (title, url) in marks {
        let text: String = if title.chars().count() > 24 { format!("{}\u{2026}", title.chars().take(23).collect::<String>()) } else { title.clone() };
        let w = p.measure(gl, &text, em, 600.0).0 + 52.0;
        if w > bar.w {
            break;
        }
        let rect = bar.take_left(w);
        let index = win.bar_marks.len();
        if win.hover == Hover::BarMark(index) {
            p.fill(gl, rect, rect.h * 0.5, theme::wash(0.11));
        }
        draw_icon(p, gl, Icon::Star, Rect::new(rect.x + 12.0, rect.y + (rect.h - 18.0) * 0.5, 18.0, 18.0), theme::ink_hint());
        p.label(gl, &text, Rect::new(rect.x + 36.0, rect.y, rect.w - 40.0, rect.h), em, theme::ink_row(), 0.0);
        win.bar_marks.push((url.clone(), rect));
        bar.take_left(4.0);
    }
}

/// Where the link under the pointer goes, at the foot of the page.
fn draw_status(p: &mut Painter, gl: &spatiand_app::Gl, page: Rect, link: &str) {
    let em = theme::FOOTER_EM * 0.95;
    let text: String = if link.chars().count() > 110 { format!("{}\u{2026}", link.chars().take(109).collect::<String>()) } else { link.to_string() };
    let w = (p.measure(gl, &text, em, page.w).0 + 32.0).min(page.w * 0.7);
    let rect = Rect::new(page.x + 6.0, page.bottom() - 46.0, w, 40.0);
    look::glass(p, gl, rect, 12.0, 1.0);
    p.label(gl, &text, rect.inset(12.0), em, theme::ink_detail(), 0.0);
}

fn draw_start(p: &mut Painter, gl: &spatiand_app::Gl, win: &mut Win, profile: &Profile, note: Option<&str>) {
    let page = win.page;
    p.fill(gl, page, 12.0, theme::ground());
    let mut all = page.inset(theme::PAD);
    let beside = if profile.is_private() { "Private: nothing in this window is kept".to_string() } else { profile.name.clone() };
    look::header(p, gl, &mut all, "SpatiMesh", &beside);
    if let Some(note) = note {
        p.label(gl, note, all.take_top(44.0), theme::DETAIL_EM, look::trouble(), 0.0);
    }
    // The sites visited most: a tile each, its first letter on a colour of its own.
    win.top_sites.clear();
    if !win.top_list.is_empty() {
        look::shelf(p, gl, &mut all, "Most visited", true);
        let row = all.take_top(170.0);
        let tile_w = 230.0;
        let mut x = row.x + 12.0;
        for (url, title) in win.top_list.clone() {
            if x + tile_w > row.right() {
                break;
            }
            let rect = Rect::new(x, row.y, tile_w - 18.0, 150.0);
            x += tile_w;
            let host = host_of(&url);
            let hue = host.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
            let colours = [look::TEAL, look::BLUE, look::VIOLET, [0.95, 0.63, 0.24, 1.0], [0.92, 0.34, 0.47, 1.0], [0.47, 0.78, 0.35, 1.0]];
            let c = colours[(hue as usize) % colours.len()];
            let hover = rect.contains(win.pointer.0, win.pointer.1);
            if hover {
                p.fill(gl, rect.inset(-3.0), 20.0, theme::wash(0.12));
            }
            let face = Rect::new(rect.x, rect.y, rect.w, 96.0);
            p.gradient(gl, face, 16.0, [c[0], c[1], c[2], 0.42], [c[0] * 0.6, c[1] * 0.6, c[2] * 0.6, 0.30]);
            let initial: String = host.trim_start_matches("www.").chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
            p.label(gl, &initial, face, 46.0, [1.0, 1.0, 1.0, 0.9], 0.5);
            p.label(gl, &title, Rect::new(rect.x + 4.0, face.bottom() + 8.0, rect.w - 8.0, 28.0), theme::ROW_EM * 0.6, theme::ink_row(), 0.0);
            p.label(gl, host.trim_start_matches("www."), Rect::new(rect.x + 4.0, face.bottom() + 34.0, rect.w - 8.0, 22.0), theme::FOOTER_EM * 0.85, theme::ink_hint(), 0.0);
            win.top_sites.push((url, rect));
        }
        all.take_top(12.0);
    }
    let Some(start) = win.start.as_mut() else { return };
    if start.rows.is_empty() {
        let hint = if profile.is_private() {
            "Type an address or a search above."
        } else if win.top_list.is_empty() {
            "Type an address or a search above. Bookmarks (Ctrl-D) are kept here, and the sites you visit most."
        } else {
            "Bookmarks (Ctrl-D) are kept here."
        };
        look::message(p, gl, all, hint, theme::ink_hint());
        return;
    }
    look::shelf(p, gl, &mut all, "Bookmarks", true);
    start.rect = all;
    start.list.draw(p, gl, all, &start.rows);
}

fn draw_notice(p: &mut Painter, gl: &spatiand_app::Gl, page: Rect, title: &str, why: &str, hint: &str) {
    p.fill(gl, page, 12.0, theme::ground());
    let pane = Rect::new(page.x + page.w * 0.5 - 380.0, page.y + page.h * 0.5 - 120.0, 760.0, 240.0);
    look::glass(p, gl, pane, 24.0, 1.0);
    let mut inner = pane.inset(28.0);
    p.label(gl, title, inner.take_top(52.0), theme::TITLE_EM * 0.8, look::trouble(), 0.0);
    p.label(gl, why, inner.take_top(44.0), theme::DETAIL_EM, theme::ink_detail(), 0.0);
    inner.take_top(16.0);
    look::keys(p, gl, inner.take_top(40.0), hint, 0.0, 1.0);
}

fn lock_pane(kind: LockKind) -> LockPane {
    let mut field = Field::default();
    field.focused = true;
    field.secret = true;
    field.hint = match kind {
        LockKind::Pin => "PIN".into(),
        LockKind::Password => "Password".into(),
    };
    LockPane { field, said: None, ok: Rect::default() }
}

/// A locked window, whole: its profile's colour and name, and where its PIN goes.
fn draw_lock(p: &mut Painter, gl: &spatiand_app::Gl, all: Rect, profile: &Profile, pane: &mut LockPane) {
    p.fill(gl, all, 0.0, profile_colour(profile, 0.22));
    let card = Rect::new(all.x + all.w * 0.5 - 340.0, all.y + all.h * 0.5 - 190.0, 680.0, 340.0);
    look::glass(p, gl, card, 28.0, 1.0);
    let mut inner = card.inset(36.0);
    let icon = inner.take_top(72.0);
    p.fill(gl, Rect::new(icon.x + icon.w * 0.5 - 36.0, icon.y, 72.0, 72.0), 36.0, profile_colour(profile, 0.9));
    draw_icon(p, gl, Icon::Lock, Rect::new(icon.x + icon.w * 0.5 - 22.0, icon.y + 14.0, 44.0, 44.0), theme::ink());
    inner.take_top(16.0);
    p.label(gl, &format!("{} is locked", profile.name), inner.take_top(52.0), theme::TITLE_EM * 0.7, theme::ink(), 0.5);
    let kind = profile.lock.as_ref().map_or(LockKind::Pin, |l| l.kind);
    let said = pane.said.clone().unwrap_or_else(|| format!("Its {} opens its windows", kind.name()));
    let ink = if pane.said.is_some() { look::trouble() } else { theme::ink_detail() };
    p.label(gl, &said, inner.take_top(40.0), theme::DETAIL_EM, ink, 0.5);
    inner.take_top(16.0);
    let mut row = inner.take_top(theme::ROW_HEIGHT);
    pane.ok = row.take_right(160.0);
    row.take_right(16.0);
    pane.field.draw(p, gl, row);
    p.fill(gl, pane.ok, theme::ROW_RADIUS, theme::selected());
    p.label(gl, "Open", pane.ok, theme::ROW_EM * 0.86, theme::ink(), 0.5);
}

fn question_buttons(page: Rect, kind: DialogKind) -> (Rect, Rect) {
    let pane = question_pane(page, kind);
    let ok = Rect::new(pane.right() - 28.0 - 160.0, pane.bottom() - 28.0 - theme::ROW_HEIGHT, 160.0, theme::ROW_HEIGHT);
    let cancel = if kind == DialogKind::Alert { Rect::default() } else { Rect { x: ok.x - 180.0, ..ok } };
    (ok, cancel)
}

/// Taller with a field to type in, which goes between the words and the buttons.
fn question_pane(page: Rect, kind: DialogKind) -> Rect {
    let height = if kind == DialogKind::Prompt { 300.0 + theme::ROW_HEIGHT } else { 300.0 };
    Rect::new(page.x + page.w * 0.5 - 420.0, page.y + 60.0, 840.0, height)
}

fn draw_question(p: &mut Painter, gl: &spatiand_app::Gl, page: Rect, question: &Question) {
    p.fill(gl, page, 12.0, [0.0, 0.0, 0.0, 0.45]);
    let pane = question_pane(page, question.kind);
    look::glass(p, gl, pane, 24.0, 1.0);
    let mut inner = pane.inset(28.0);
    let heading = match (&question.own, question.kind) {
        (Some(Own::LoadExtension(_) | Own::RemoveExtension(_)), _) => "Extensions",
        (Some(_), _) => "This profile",
        (None, DialogKind::Leave) => "Leave this page?",
        (None, _) => "The page asks",
    };
    p.label(gl, heading, inner.take_top(48.0), theme::TITLE_EM * 0.7, theme::ink(), 0.0);
    p.text(gl, &question.message, inner.x, inner.y, theme::DETAIL_EM, inner.w, theme::ink_detail());
    inner.take_top(110.0);
    if question.kind == DialogKind::Prompt {
        let mut field = Field::default();
        field.focused = true;
        field.secret = question.field.secret;
        field.set(question.field.text.clone());
        field.draw(p, gl, inner.take_top(theme::ROW_HEIGHT));
    }
    let (ok, cancel) = question_buttons(page, question.kind);
    p.fill(gl, ok, theme::ROW_RADIUS, theme::selected());
    p.label(gl, if question.kind == DialogKind::Leave { "Leave" } else { "OK" }, ok, theme::ROW_EM * 0.86, theme::ink(), 0.5);
    if question.kind != DialogKind::Alert {
        p.fill(gl, cancel, theme::ROW_RADIUS, theme::raised());
        p.label(gl, if question.kind == DialogKind::Leave { "Stay" } else { "Cancel" }, cancel, theme::ROW_EM * 0.86, theme::ink_row(), 0.5);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_fills_the_window_below_the_bars() {
        let page = page_rect(WINDOW, false);
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

/// Where the session takes its own applications' sound, and routes for it
/// (`crates/spatiand/src/own.rs`, `sound`).
fn spatiand_sound_socket() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|d| std::path::PathBuf::from(d).join("spatiand-sound"))
}
