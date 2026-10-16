//! The extension runtime: every extension's API answered, and its events sent, in the window
//! process, whatever the engine (docs/spatimesh.md, "Extensions").
//!
//! An extension context -- a content script in a frame, a background page, a popup, a page of
//! the extension's in a tab -- runs api.js, which says each call as JSON over its engine's
//! bridge; the engine host hands it to the window as it came, with the tab it came from, and it
//! arrives at [`Runtime::call`]. Answers and events go back the other way: code run in that tab,
//! in the context's own world ([`set::deliver`]). What the APIs are about -- tabs, windows, a
//! page's engine -- the runtime asks of the browser through [`BrowserSide`], which SpatiMesh's
//! window implements and tests stand in for.
//!
//! Modelled on Chromium's (`extensions/browser`: the event router, the message service, the
//! API functions) and Firefox's (`toolkit/components/extensions`).

mod action;
mod apis;
mod scripting;
mod tabs;

pub use action::{ActionView, Picture};

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::install::{self, Flavor, Installed};
use super::locale::Messages;
use super::manifest::{Background, Manifest, RunAt, World};
use super::set::{self, Addresses, ExtensionSet, Script, Spec};
use super::storage::{self, Area, Session};
use crate::engine::TabId;
use crate::profiles::PRIVATE;
use crate::store::Store;
use crate::tabs::{Tabs, WindowKey};

/// Tabs that are an extension's and no window's -- background pages, popups, offscreen
/// documents -- are numbered from here, apart from the window's own.
pub const HIDDEN_TABS: TabId = 1 << 40;

/// What a message or a port gets when nothing listens at the other end.
const NO_RECEIVER: &str = "Could not establish connection. Receiving end does not exist.";

/// How long a call waits for an extension's background to load before it is given up.
const BACKGROUND_WAIT: Duration = Duration::from_secs(15);

/// What the runtime asks of the browser: SpatiMesh's window, or a test's stand-in.
pub trait BrowserSide {
    /// Every tab and window there is.
    fn tabs(&self) -> &Tabs;
    /// The window the wearer used last.
    fn last_focused(&self) -> Option<WindowKey>;
    /// A tab of `profile` at `url`: in `window` (a new window if none), at `index` (after the
    /// others if none), in front if `active`.
    fn open_tab(&mut self, profile: &str, window: Option<WindowKey>, url: &str, index: Option<usize>, active: bool, opener: Option<TabId>) -> Option<TabId>;
    /// A window of `profile` with a tab for each address (the start page if there are none), or
    /// with `tab` moved into it.
    fn open_window(&mut self, profile: &str, urls: &[String], tab: Option<TabId>, focused: bool) -> Option<WindowKey>;
    fn load(&mut self, tab: TabId, url: &str);
    fn activate(&mut self, tab: TabId);
    fn close_tab(&mut self, tab: TabId);
    fn close_window(&mut self, window: WindowKey);
    fn focus_window(&mut self, window: WindowKey);
    fn reload(&mut self, tab: TabId);
    fn back(&mut self, tab: TabId);
    fn forward(&mut self, tab: TabId);
    fn mute(&mut self, tab: TabId, muted: bool);
    fn move_tab(&mut self, tab: TabId, window: WindowKey, index: Option<usize>);
    /// How far pages are zoomed (1.0 as they ask to be), and zoomed otherwise.
    fn zoom(&self) -> f32;
    fn set_zoom(&mut self, zoom: f32);
    /// The size of a window's pages, in pixels.
    fn page_size(&self, window: WindowKey) -> (u32, u32);
    /// What `profile`'s engine is to run.
    fn set_extensions(&mut self, profile: &str, set: &ExtensionSet);
    /// Code run in a tab of `profile`'s engine: in `world` ("" for the page's own;
    /// `spatimesh-<id>` for an extension's, its API set up there first if `boot`), in the tab's
    /// top frame or in all its frames. If `request` is not 0, the top frame's result comes back
    /// to [`Runtime::script_result`].
    #[allow(clippy::too_many_arguments)]
    fn run_script(&mut self, profile: &str, tab: TabId, world: &str, all_frames: bool, code: &str, request: u64, boot: bool);
    /// A page of an extension's in `profile`'s engine that no window shows: a background page,
    /// an offscreen document.
    fn open_hidden(&mut self, profile: &str, tab: TabId, url: &str);
    fn close_hidden(&mut self, profile: &str, tab: TabId);
    /// An extension's popup over `window`, in `tab`.
    fn open_popup(&mut self, profile: &str, window: WindowKey, tab: TabId, url: &str);
    fn close_popup(&mut self, profile: &str, tab: TabId);
    /// Something for the wearer to see.
    fn notify(&mut self, title: &str, message: &str);
    /// An extension's toolbar button looks different now.
    fn actions_changed(&mut self, profile: &str);
    /// What the tab in front of a window shows: width, height, and four bytes a pixel (red,
    /// green, blue, alpha).
    fn capture(&mut self, window: WindowKey) -> Option<(u32, u32, Vec<u8>)>;
}

/// What kind of extension context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A content script, in a page that is not the extension's.
    Content,
    Background,
    Popup,
    /// A page of the extension's in a tab of a window's.
    Tab,
    Offscreen,
}

impl Kind {
    /// As `runtime.getContexts` names it.
    fn context_type(self) -> &'static str {
        match self {
            Kind::Content => "CONTENT_SCRIPT",
            Kind::Background => "BACKGROUND",
            Kind::Popup => "POPUP",
            Kind::Tab => "TAB",
            Kind::Offscreen => "OFFSCREEN_DOCUMENT",
        }
    }
}

struct Context {
    profile: String,
    extension: String,
    kind: Kind,
    tab: TabId,
    frame: i64,
    url: String,
    /// The world it runs in: the extension's for a content script, "" (the page's own) for the
    /// extension's own pages.
    world: String,
    /// The events it has listeners for, with each listener's filter, if it gave one.
    listening: HashMap<String, Vec<Value>>,
}

/// Who made a call: what its answer goes to, and what its sender is made of.
#[derive(Debug, Clone)]
struct Caller {
    context: String,
    profile: String,
    extension: String,
    kind: Kind,
    tab: TabId,
    frame: i64,
    url: String,
}

/// A call's answer: now, or later (the runtime keeps who waits for it).
enum Answer {
    Now(Result<Value, String>),
    Later,
}

fn ok(v: Value) -> Answer {
    Answer::Now(Ok(v))
}

fn err(e: impl Into<String>) -> Answer {
    Answer::Now(Err(e.into()))
}

/// What waits for an answer from elsewhere.
enum Request {
    /// A message: the first of `waiting` to answer answers `caller`'s call.
    Message { caller: String, call: u64, waiting: HashSet<String>, heard: bool },
    /// Code run in the extension's world: each frame's result.
    Script(scripting::Pending),
    /// Code run in the page's world: the engine's result for the top frame.
    MainWorld { caller: String, call: u64, legacy: bool, since: Instant },
}

/// A port between contexts: what its opener posts goes to every other end, and theirs to the
/// opener.
struct Port {
    opener: String,
    others: HashSet<String>,
}

/// A call that waits for an extension's background to load.
struct Held {
    context: String,
    call: String,
    args: Vec<Value>,
    id: Option<u64>,
    since: Instant,
}

/// What of an extension is kept between runs, on this machine.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct State {
    /// `scripting.registerContentScripts`'s, as they were given.
    #[serde(default)]
    registered: Vec<Value>,
    /// Optional permissions and hosts granted.
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(default)]
    origins: Vec<String>,
    /// declarativeNetRequest's.
    #[serde(default)]
    dynamic_rules: Vec<Value>,
    #[serde(default)]
    enabled_rulesets: Option<Vec<String>>,
    #[serde(default)]
    disabled_rules: BTreeMap<String, Vec<i64>>,
}

/// An extension loaded in a profile.
struct Ext {
    /// The profile it is installed in (another than the one it runs in, in the private one).
    home: String,
    installed: Installed,
    /// Where its pages are, as the engine serves them ([`Addresses::origin`]).
    origin: String,
    manifest: Manifest,
    messages: Messages,
    content_token: String,
    page_token: String,
    /// The hidden tab its background runs in.
    background: Option<TabId>,
    /// Its background has loaded: its listeners are there.
    loaded: bool,
    restarts: u32,
    held: Vec<Held>,
    /// What `runtime.onInstalled` is to say when its background has loaded.
    reason: Option<(String, Option<String>)>,
    state: State,
    action: action::ActionState,
    menus: Vec<Map<String, Value>>,
    offscreen: Option<TabId>,
    offscreen_waiting: Vec<(String, u64)>,
    /// Tabs `activeTab` was granted for: the wearer used the extension there.
    active_tabs: HashSet<TabId>,
    session_rules: Vec<Value>,
    /// `storage.session` is open to content scripts.
    session_open: bool,
    /// `notifications` shown.
    notifications: HashSet<String>,
}

impl Ext {
    fn id(&self) -> &str {
        &self.installed.id
    }

    /// `runtime.id`: Firefox's add-on id where it has one.
    fn runtime_id(&self) -> String {
        match (self.installed.flavor, &self.manifest.gecko_id) {
            (Flavor::Firefox, Some(g)) => g.clone(),
            _ => self.installed.id.clone(),
        }
    }

    fn origin(&self) -> String {
        self.origin.clone()
    }

    /// One of its pages' address, from a path in it.
    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.origin(), path.trim_start_matches('/'))
    }

    /// An API permission it has: asked for in its manifest, or granted since.
    fn has(&self, permission: &str) -> bool {
        self.manifest.has_permission(permission) || self.state.permissions.iter().any(|p| p == permission)
    }

    /// The hosts it may reach.
    fn hosts(&self) -> Vec<String> {
        let mut all: Vec<String> = self.manifest.host_permissions.clone();
        all.extend(self.manifest.permissions.iter().filter(|p| super::manifest::is_host_permission(p)).cloned());
        all.extend(self.state.origins.iter().cloned());
        all
    }

    /// Whether it may see and change what a tab shows at `url`.
    fn may_access(&self, url: &str, tab: Option<TabId>) -> bool {
        if tab.is_some_and(|t| self.active_tabs.contains(&t)) {
            return true;
        }
        if url.starts_with(&self.origin()) {
            return true;
        }
        super::pattern::Patterns::new(&self.hosts()).matches(url)
    }

    /// Whether it may know a tab's address and title.
    fn may_see(&self, url: &str, tab: TabId) -> bool {
        self.has("tabs") || self.may_access(url, Some(tab))
    }

    fn has_api(&self, namespace: &str) -> bool {
        match namespace {
            "contextMenus" | "menus" => self.has("contextMenus") || self.has("menus"),
            "declarativeNetRequest" => {
                self.has("declarativeNetRequest") || self.has("declarativeNetRequestWithHostAccess") || self.has("declarativeNetRequestFeedback")
            }
            other => self.has(other),
        }
    }
}

/// A profile's extensions, while its engine runs.
struct Loaded {
    private: bool,
    exts: Vec<Ext>,
}

pub struct Runtime {
    store: Store,
    ui_language: String,
    /// How the engines give extensions their addresses.
    addresses: Addresses,
    /// For an engine that is given scripts for the start of every page (Android's WebView):
    /// what its top frames' way in is called, and what calls through it say.
    page_entry: String,
    page_secret: String,
    profiles: HashMap<String, Loaded>,
    /// `storage.session`, and everything the private profile's extensions keep.
    session: Session,
    contexts: HashMap<String, Context>,
    requests: HashMap<u64, Request>,
    ports: HashMap<String, Port>,
    alarms: Vec<apis::Alarm>,
    /// Popups shown: their tab's profile, extension and window.
    popups: HashMap<TabId, (String, String, WindowKey)>,
    next_question: u64,
    next_hidden: TabId,
    /// Profiles whose extensions have been told the browser started (`runtime.onStartup`).
    started: HashSet<String>,
}

/// The wearer's language, as `i18n.getUILanguage` says it: from the system's locale.
fn ui_language() -> String {
    let raw = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().filter_map(|v| std::env::var(v).ok()).find(|v| !v.is_empty()).unwrap_or_default();
    let tag = raw.split(['.', '@']).next().unwrap_or("").replace('_', "-");
    if tag.is_empty() || tag == "C" || tag == "POSIX" {
        "en-US".to_string()
    } else {
        tag
    }
}

/// What `runtime.getPlatformInfo` says the system is.
fn os() -> &'static str {
    if cfg!(target_os = "macos") {
        "mac"
    } else if cfg!(target_os = "android") {
        "android"
    } else {
        "linux"
    }
}

/// A secret no page can guess: what a context's calls are known by.
fn token() -> String {
    format!("{}{}", crate::store::new_id(), crate::store::new_id())
}

/// The world an extension's content scripts run in.
pub fn world_of(extension: &str) -> String {
    format!("spatimesh-{extension}")
}

fn state_file(profile: &str, id: &str) -> String {
    format!("profiles/{profile}/extension-state/{id}.json")
}

fn versions_file(profile: &str) -> String {
    format!("profiles/{profile}/extension-versions.json")
}

/// The address of `path` against `base` (a page of the extension's, or its origin).
fn resolve(base: &str, path: &str) -> String {
    if path.contains("://") || path.starts_with("about:") || path.starts_with("data:") {
        return path.to_string();
    }
    let base = if base.ends_with('/') || base.matches('/').count() > 2 { base.to_string() } else { format!("{base}/") };
    match url::Url::parse(&base).and_then(|b| b.join(path)) {
        Ok(u) => u.to_string(),
        Err(_) => path.to_string(),
    }
}

/// The origin part of an address.
fn origin_of(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) if matches!(u.scheme(), "chrome-extension" | "moz-extension" | set::OWN_SCHEME) => {
            format!("{}://{}", u.scheme(), u.host_str().unwrap_or(""))
        }
        Ok(u) => u.origin().ascii_serialization(),
        Err(_) => "null".to_string(),
    }
}

/// One `content_scripts` entry, its files read from `root` (those that cannot be are left out).
fn script_of(root: &std::path::Path, entry: &super::manifest::ContentScript) -> Script {
    let read = |file: &String| -> Option<(String, String)> {
        let path = set::inside(root, file.trim_start_matches('/'));
        match path.map(std::fs::read_to_string) {
            Some(Ok(code)) => Some((file.clone(), code)),
            _ => {
                log::warn!("{}: content script {file} could not be read", root.display());
                None
            }
        }
    };
    Script {
        matches: entry.matches.clone(),
        exclude_matches: entry.exclude_matches.clone(),
        include_globs: entry.include_globs.clone(),
        exclude_globs: entry.exclude_globs.clone(),
        include: String::new(),
        globs: String::new(),
        exclude: String::new(),
        js: entry.js.iter().filter_map(read).collect(),
        css: entry.css.iter().filter_map(read).collect(),
        run_at: match entry.run_at {
            RunAt::DocumentStart => "document_start",
            RunAt::DocumentEnd => "document_end",
            RunAt::DocumentIdle => "document_idle",
        }
        .to_string(),
        all_frames: entry.all_frames,
        match_about_blank: entry.match_about_blank,
        main_world: entry.world == World::Main,
    }
    .with_regexes()
}

#[derive(Deserialize)]
struct Said {
    /// The token the context was given.
    t: String,
    /// The extension.
    e: String,
    /// The frame (0: the top one).
    #[serde(default)]
    f: i64,
    /// The context.
    x: String,
    /// Its page's address.
    #[serde(default)]
    u: String,
    /// The call.
    c: String,
    #[serde(default)]
    a: Vec<Value>,
    /// The call's number, if it wants an answer.
    #[serde(default)]
    i: Option<u64>,
}

/// What happened to a tab, for `tabs`' events.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Change {
    pub url: Option<String>,
    pub title: Option<String>,
    pub loading: Option<bool>,
    pub audible: Option<bool>,
    pub muted: Option<bool>,
}

/// A tab's top frame going somewhere, for `webNavigation`.
#[derive(Debug, Clone, PartialEq)]
pub enum Navigation {
    Before,
    Committed,
    DomContentLoaded,
    Completed,
    Error(String),
}

impl Runtime {
    pub fn new(store: Store) -> Runtime {
        Runtime {
            store,
            ui_language: ui_language(),
            addresses: Addresses::default(),
            page_entry: format!("_{}", crate::store::new_id()),
            page_secret: token(),
            profiles: HashMap::new(),
            session: Session::default(),
            contexts: HashMap::new(),
            requests: HashMap::new(),
            ports: HashMap::new(),
            alarms: Vec::new(),
            popups: HashMap::new(),
            next_question: 1,
            next_hidden: HIDDEN_TABS,
            started: HashSet::new(),
        }
    }

    /// How the engines give extensions their addresses: told before any profile's extensions
    /// are loaded (the engine program is chosen at start), and kept for those already loaded.
    pub fn set_addresses(&mut self, addresses: Addresses) {
        self.addresses = addresses;
        for ext in self.profiles.values_mut().flat_map(|loaded| loaded.exts.iter_mut()) {
            ext.origin = addresses.origin(ext.installed.flavor, &ext.installed.id);
        }
    }

    fn ext(&self, profile: &str, id: &str) -> Option<&Ext> {
        self.profiles.get(profile)?.exts.iter().find(|e| e.installed.id == id)
    }

    fn ext_mut(&mut self, profile: &str, id: &str) -> Option<&mut Ext> {
        self.profiles.get_mut(profile)?.exts.iter_mut().find(|e| e.installed.id == id)
    }

    /// An extension of the profile's by its id or its `runtime.id`.
    fn find(&self, profile: &str, id: &str) -> Option<String> {
        let loaded = self.profiles.get(profile)?;
        loaded.exts.iter().find(|e| e.installed.id == id || e.runtime_id() == id).map(|e| e.installed.id.clone())
    }

    fn is_private(&self, profile: &str) -> bool {
        self.profiles.get(profile).is_some_and(|l| l.private) || profile == PRIVATE
    }

    fn question(&mut self) -> u64 {
        let q = self.next_question;
        self.next_question += 1;
        q
    }

    fn hidden_tab(&mut self) -> TabId {
        let t = self.next_hidden;
        self.next_hidden += 1;
        t
    }

    /// Whether a tab is one of the extensions' that no window shows.
    pub fn is_hidden(tab: TabId) -> bool {
        tab >= HIDDEN_TABS
    }

    // ---- loading ----------------------------------------------------------------------------

    fn load(&self, home: &str, installed: Installed) -> Result<Ext, String> {
        let text = std::fs::read_to_string(installed.root.join("manifest.json")).map_err(|e| format!("{}: {e}", installed.root.display()))?;
        let written = Manifest::parse(&text)?;
        let messages = Messages::load(&installed.root, &self.ui_language, written.default_locale.as_deref());
        let id = installed.id.clone();
        let manifest = written.localized(&|name| {
            let text = messages.get(name, &[], &id);
            (!text.is_empty()).then_some(text)
        });
        let state = self.store.read_local::<State>(&state_file(home, &id)).unwrap_or_default();
        Ok(Ext {
            home: home.to_string(),
            origin: self.addresses.origin(installed.flavor, &id),
            installed,
            manifest,
            messages,
            content_token: token(),
            page_token: token(),
            background: None,
            loaded: false,
            restarts: 0,
            held: Vec::new(),
            reason: None,
            state,
            action: action::ActionState::default(),
            menus: Vec::new(),
            offscreen: None,
            offscreen_waiting: Vec::new(),
            active_tabs: HashSet::new(),
            session_rules: Vec::new(),
            session_open: false,
            notifications: HashSet::new(),
        })
    }

    /// What a profile runs: its own enabled extensions, or for the private profile every
    /// profile's that were let into it (once each).
    fn wanted(&self, profile: &str) -> Vec<(String, Installed)> {
        if profile == PRIVATE {
            let mut all: Vec<(String, Installed)> = Vec::new();
            for p in crate::profiles::list(&self.store) {
                for i in install::list(&self.store, &p.id) {
                    if i.enabled && i.in_private && !all.iter().any(|(_, w)| w.id == i.id) {
                        all.push((p.id.clone(), i));
                    }
                }
            }
            all
        } else {
            install::list(&self.store, profile).into_iter().filter(|i| i.enabled).map(|i| (profile.to_string(), i)).collect()
        }
    }

    /// A profile's engine has started: its extensions are loaded, given to the engine, and
    /// their backgrounds started.
    pub fn engine_started(&mut self, side: &mut dyn BrowserSide, profile: &str) {
        self.engine_gone(profile);
        self.profiles.insert(profile.to_string(), Loaded { private: profile == PRIVATE, exts: Vec::new() });
        self.refresh(side, profile, &[]);
    }

    /// A profile's engine has gone, and every context in it.
    pub fn engine_gone(&mut self, profile: &str) {
        let gone: Vec<String> = self.contexts.iter().filter(|(_, c)| c.profile == profile).map(|(id, _)| id.clone()).collect();
        for id in gone {
            self.contexts.remove(&id);
        }
        self.drop_dangling();
        self.popups.retain(|_, (p, _, _)| p != profile);
        self.profiles.remove(profile);
    }

    /// What was installed, removed, turned on or off since, taken in (and `reload`, extensions
    /// loaded again from their files): new ones start, gone ones stop, and the engine is given
    /// the set again.
    pub fn refresh(&mut self, side: &mut dyn BrowserSide, profile: &str, reload: &[&str]) {
        if !self.profiles.contains_key(profile) {
            return;
        }
        let wanted = self.wanted(profile);
        let private = self.is_private(profile);
        // Gone, or to be loaded again: stopped.
        let running: Vec<String> = self.profiles[profile].exts.iter().map(|e| e.installed.id.clone()).collect();
        for id in running {
            let keep = wanted.iter().find(|(_, i)| i.id == id);
            let same = keep.is_some_and(|(_, i)| self.ext(profile, &id).is_some_and(|e| e.installed == *i));
            if !same || reload.contains(&id.as_str()) {
                self.stop(side, profile, &id);
            }
        }
        let mut versions: BTreeMap<String, String> = if private { BTreeMap::new() } else { self.store.read_local(&versions_file(profile)).unwrap_or_default() };
        let mut started = Vec::new();
        for (home, installed) in wanted {
            if self.ext(profile, &installed.id).is_some() {
                continue;
            }
            let id = installed.id.clone();
            match self.load(&home, installed) {
                Ok(mut ext) => {
                    if !private {
                        // Installed or changed since last time: runtime.onInstalled says so.
                        match versions.get(&id) {
                            None => ext.reason = Some(("install".into(), None)),
                            Some(v) if *v != ext.manifest.version || reload.contains(&id.as_str()) => ext.reason = Some(("update".into(), Some(v.clone()))),
                            _ => {}
                        }
                        versions.insert(id.clone(), ext.manifest.version.clone());
                    }
                    if let Some(loaded) = self.profiles.get_mut(profile) {
                        loaded.exts.push(ext);
                    }
                    started.push(id);
                }
                Err(e) => log::warn!("extension {id} could not be loaded: {e}"),
            }
        }
        if !private {
            if let Some(loaded) = self.profiles.get(profile) {
                versions.retain(|id, _| loaded.exts.iter().any(|e| e.installed.id == *id) || install::list(&self.store, profile).iter().any(|i| i.id == *id));
            }
            let _ = self.store.write_local(&versions_file(profile), &versions);
        }
        self.send_set(side, profile);
        for id in started {
            self.start_background(side, profile, &id);
        }
        side.actions_changed(profile);
    }

    /// Every extension of the profile loaded again from its files, as Chrome's developer mode
    /// reloads one.
    pub fn reload_all(&mut self, side: &mut dyn BrowserSide, profile: &str) {
        let ids: Vec<String> = self.profiles.get(profile).map(|l| l.exts.iter().map(|e| e.installed.id.clone()).collect()).unwrap_or_default();
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        self.refresh(side, profile, &ids);
    }

    /// An extension stopped: its background and pages closed, its contexts forgotten.
    fn stop(&mut self, side: &mut dyn BrowserSide, profile: &str, id: &str) {
        let Some(loaded) = self.profiles.get_mut(profile) else { return };
        let Some(at) = loaded.exts.iter().position(|e| e.installed.id == id) else { return };
        let ext = loaded.exts.remove(at);
        for tab in ext.background.into_iter().chain(ext.offscreen) {
            side.close_hidden(profile, tab);
        }
        let popups: Vec<TabId> = self.popups.iter().filter(|(_, (p, e, _))| p == profile && e == id).map(|(t, _)| *t).collect();
        for tab in popups {
            self.popups.remove(&tab);
            side.close_popup(profile, tab);
        }
        let gone: Vec<String> = self.contexts.iter().filter(|(_, c)| c.profile == profile && c.extension == id).map(|(cid, _)| cid.clone()).collect();
        for cid in gone {
            self.forget_context(side, &cid);
        }
        // Its pages in windows' tabs go with it, as in Chrome.
        let origin = ext.origin();
        let tabs: Vec<TabId> = side.tabs().tabs().filter(|t| t.profile == profile && t.url.starts_with(&origin)).map(|t| t.id).collect();
        for tab in tabs {
            side.close_tab(tab);
        }
        self.alarms.retain(|a| !(a.profile == profile && a.extension == id));
    }

    fn start_background(&mut self, side: &mut dyn BrowserSide, profile: &str, id: &str) {
        let tab = self.hidden_tab();
        let Some(ext) = self.ext_mut(profile, id) else { return };
        let path = match &ext.manifest.background {
            None => {
                ext.loaded = true;
                return;
            }
            Some(Background::Page(page)) => page.clone(),
            Some(_) => set::BACKGROUND_PAGE.to_string(),
        };
        let url = ext.url(&path);
        ext.background = Some(tab);
        ext.loaded = false;
        side.open_hidden(profile, tab, &url);
    }

    /// The configuration api.js is booted with, for a content script or for the extension's
    /// own pages.
    fn config(&self, ext: &Ext, content: bool, private: bool) -> Value {
        let mut api = Map::new();
        for p in ext.manifest.permissions.iter().chain(&ext.state.permissions) {
            if !super::manifest::is_host_permission(p) {
                api.insert(p.clone(), Value::Bool(true));
            }
        }
        json!({
            "id": ext.id(),
            "runtimeId": ext.runtime_id(),
            "token": if content { &ext.content_token } else { &ext.page_token },
            "kind": if content { "content" } else { "page" },
            "flavor": match ext.installed.flavor { Flavor::Chrome => "chrome", Flavor::Firefox => "firefox" },
            "manifest": ext.manifest.raw,
            "messages": ext.messages.table(ext.id()),
            "uiLanguage": self.ui_language,
            "origin": ext.origin(),
            "api": api,
            "version": env!("CARGO_PKG_VERSION"),
            "incognito": private,
            "os": os(),
        })
    }

    fn spec(&self, ext: &Ext, private: bool) -> Spec {
        let root = &ext.installed.root;
        let mut scripts: Vec<Script> = ext.manifest.content_scripts.iter().map(|entry| script_of(root, entry)).collect();
        scripts.extend(ext.state.registered.iter().filter_map(|r| scripting::registered_script(root, r)));
        let (background, background_html) = match &ext.manifest.background {
            None => (None, None),
            Some(Background::Page(page)) => (Some(page.trim_start_matches('/').to_string()), None),
            Some(Background::Scripts(list)) => (Some(set::BACKGROUND_PAGE.to_string()), Some(set::background_html(list, None))),
            Some(Background::ServiceWorker { script, module }) => {
                (Some(set::BACKGROUND_PAGE.to_string()), Some(set::background_html(&[], Some((script.as_str(), *module)))))
            }
        };
        let messages = ext.messages.table(ext.id()).keys().map(|name| (name.clone(), ext.messages.get(name, &[], ext.id()))).collect();
        Spec {
            id: ext.id().to_string(),
            root: root.clone(),
            flavor: ext.installed.flavor,
            origin: ext.origin(),
            scripts,
            web_accessible: ext.manifest.web_accessible.iter().map(|w| (w.resources.clone(), w.matches.clone())).collect(),
            background,
            background_html,
            content_config: self.config(ext, true, private).to_string(),
            page_config: self.config(ext, false, private).to_string(),
            messages,
        }
    }

    /// What a profile's engine runs now.
    pub fn set_of(&self, profile: &str) -> ExtensionSet {
        let Some(loaded) = self.profiles.get(profile) else { return ExtensionSet::default() };
        let mut set = ExtensionSet { extensions: loaded.exts.iter().map(|e| self.spec(e, loaded.private)).collect(), page_start: None };
        if self.addresses == Addresses::Https {
            set.page_start = Some(set::page_starts(&set, &self.page_secret, &self.page_entry));
        }
        set
    }

    fn send_set(&mut self, side: &mut dyn BrowserSide, profile: &str) {
        let set = self.set_of(profile);
        side.set_extensions(profile, &set);
    }

    fn save_state(&self, profile: &str, id: &str) {
        let Some(ext) = self.ext(profile, id) else { return };
        if self.is_private(profile) {
            return;
        }
        if let Err(e) = self.store.write_local(&state_file(&ext.home, id), &ext.state) {
            log::warn!("extension {id}'s state could not be kept: {e}");
        }
    }

    // ---- calls ------------------------------------------------------------------------------

    /// A call from an extension context in `tab` of `profile`'s engine, as api.js said it.
    pub fn call(&mut self, side: &mut dyn BrowserSide, profile: &str, tab: TabId, json: &str) {
        let said: Said = match serde_json::from_str(json) {
            Ok(s) => s,
            Err(e) => {
                log::debug!("not an extension call ({e}): {json}");
                return;
            }
        };
        let Some(ext) = self.ext(profile, &said.e) else {
            log::debug!("a call for extension {}, which {profile} does not run", said.e);
            return;
        };
        let content = said.t == ext.content_token;
        if !content && said.t != ext.page_token {
            log::warn!("extension {}: a call with the wrong token, from tab {tab}: ignored", said.e);
            return;
        }
        // The extension's own pages' token works only in its own pages.
        if !content && !said.u.starts_with(&format!("{}/", ext.origin())) {
            log::warn!("extension {}: its pages' token from {}: ignored", said.e, said.u);
            return;
        }
        let world = if content { world_of(&said.e) } else { String::new() };
        match self.contexts.get_mut(&said.x) {
            Some(c) => {
                if c.tab != tab || c.extension != said.e || c.profile != profile || c.world != world {
                    log::warn!("extension {}: context {} claimed from elsewhere: ignored", said.e, said.x);
                    return;
                }
                c.url = said.u.clone();
            }
            None => {
                if said.c == "gone" {
                    return;
                }
                let kind = if content { Kind::Content } else { self.kind_of(profile, &said.e, tab) };
                self.contexts.insert(
                    said.x.clone(),
                    Context { profile: profile.to_string(), extension: said.e.clone(), kind, tab, frame: said.f, url: said.u.clone(), world, listening: HashMap::new() },
                );
            }
        }
        let c = &self.contexts[&said.x];
        let caller = Caller { context: said.x.clone(), profile: c.profile.clone(), extension: c.extension.clone(), kind: c.kind, tab, frame: c.frame, url: c.url.clone() };
        if let (Answer::Now(result), Some(id)) = (self.dispatch(side, &caller, &said.c, &said.a, said.i), said.i) {
            self.reply(side, &caller.context, id, result);
        }
    }

    fn kind_of(&self, profile: &str, extension: &str, tab: TabId) -> Kind {
        let Some(ext) = self.ext(profile, extension) else { return Kind::Tab };
        if ext.background == Some(tab) {
            Kind::Background
        } else if ext.offscreen == Some(tab) {
            Kind::Offscreen
        } else if self.popups.contains_key(&tab) {
            Kind::Popup
        } else {
            Kind::Tab
        }
    }

    /// Calls a content script may make: the rest are the extension's own pages'.
    fn content_may(call: &str) -> bool {
        matches!(
            call,
            "ready" | "loaded" | "gone" | "listen" | "runtime.sendMessage" | "runtime.connect" | "port.post" | "port.disconnect" | "message.respond" | "message.none" | "message.error" | "event.result" | "scripting.done"
        ) || call.starts_with("storage.")
    }

    fn dispatch(&mut self, side: &mut dyn BrowserSide, caller: &Caller, call: &str, args: &[Value], id: Option<u64>) -> Answer {
        if caller.kind == Kind::Content && !Self::content_may(call) {
            return err(format!("{call} is not available to content scripts"));
        }
        let arg = |n: usize| args.get(n).cloned().unwrap_or(Value::Null);
        let text = |n: usize| args.get(n).and_then(Value::as_str).unwrap_or("").to_string();
        let number = |n: usize| args.get(n).and_then(Value::as_u64).unwrap_or(0);
        match call {
            "ready" => ok(Value::Null),
            "loaded" => {
                self.loaded(side, caller);
                ok(Value::Null)
            }
            "gone" => {
                self.forget_context(side, &caller.context);
                Answer::Later
            }
            "listen" => {
                if let Some(c) = self.contexts.get_mut(&caller.context) {
                    let name = text(0);
                    if arg(1).as_bool().unwrap_or(false) {
                        c.listening.entry(name).or_default().push(arg(2));
                    } else {
                        c.listening.remove(&name);
                    }
                }
                Answer::Later
            }
            "runtime.sendMessage" => self.send_message(side, caller, call, args, id, arg(0), arg(1)),
            "tabs.sendMessage" => self.send_to_tab(side, caller, call, args, id),
            "message.respond" => {
                self.respond(side, caller, number(0), Ok(arg(1)));
                Answer::Later
            }
            "message.error" => {
                self.respond(side, caller, number(0), Err(text(1)));
                Answer::Later
            }
            "message.none" => {
                self.no_answer(side, &caller.context, number(0), arg(1).as_bool().unwrap_or(false));
                Answer::Later
            }
            "runtime.connect" | "tabs.connect" => {
                self.connect(side, caller, call, args, id);
                Answer::Later
            }
            "port.post" => {
                self.port_post(side, caller, &text(0), arg(1));
                Answer::Later
            }
            "port.disconnect" => {
                self.port_disconnect(side, &caller.context, &text(0));
                Answer::Later
            }
            // An answer to an event that wanted one: none do, yet (webRequest's will).
            "event.result" => Answer::Later,
            "scripting.done" => {
                self.script_done(side, caller, number(0), arg(1), arg(2), arg(3));
                Answer::Later
            }
            "runtime.openOptionsPage" => self.open_options(side, caller),
            "runtime.reload" => {
                let (profile, id) = (caller.profile.clone(), caller.extension.clone());
                self.refresh(side, &profile, &[&id]);
                Answer::Later
            }
            "runtime.getContexts" => ok(self.get_contexts(side, caller, &arg(0))),
            c if c.starts_with("storage.") => self.storage_call(side, caller, c, args),
            c if c.starts_with("tabs.") => tabs::tab_call(self, side, caller, c, args, id),
            c if c.starts_with("windows.") => tabs::window_call(self, side, caller, c, args),
            c if c.starts_with("scripting.") => scripting::call(self, side, caller, c, args, id),
            c if c.starts_with("action.") || c.starts_with("browserAction.") || c.starts_with("pageAction.") => action::call(self, side, caller, c, args),
            c => apis::call(self, side, caller, c, args, id),
        }
    }

    /// A call's answer, to the context that made it.
    fn reply(&self, side: &mut dyn BrowserSide, context: &str, call: u64, result: Result<Value, String>) {
        let message = match result {
            Ok(v) => json!({ "to": context, "r": call, "v": v }),
            Err(e) => json!({ "to": context, "r": call, "x": e }),
        };
        self.send(side, context, &message);
    }

    /// Anything for a context: into its tab, its world, its frame (all of them for a frame
    /// other than the top one; api.js takes only what is for it).
    fn send(&self, side: &mut dyn BrowserSide, context: &str, message: &Value) {
        let Some(c) = self.contexts.get(context) else { return };
        side.run_script(&c.profile, c.tab, &c.world, c.frame != 0, &set::deliver(&message.to_string()), 0, false);
    }

    fn event(&self, side: &mut dyn BrowserSide, context: &str, name: &str, args: Vec<Value>, question: Option<u64>) {
        let mut message = json!({ "to": context, "ev": name, "a": args });
        if let Some(q) = question {
            message["q"] = json!(q);
        }
        self.send(side, context, &message);
    }

    /// An event to every context listening for it in `profile` -- of `extension` only, if one is
    /// named -- that `contexts` lets have it, with what `args` makes of it for each extension
    /// (nothing: that extension does not get it).
    fn emit(&self, side: &mut dyn BrowserSide, profile: &str, extension: Option<&str>, name: &str, args: &dyn Fn(&Ext) -> Option<Vec<Value>>, contexts: &dyn Fn(&Context) -> bool) {
        let Some(loaded) = self.profiles.get(profile) else { return };
        for ext in loaded.exts.iter().filter(|e| extension.map_or(true, |x| x == e.id())) {
            let mut targets: Vec<&String> = self
                .contexts
                .iter()
                .filter(|(_, c)| c.profile == profile && c.extension == ext.id() && c.listening.contains_key(name) && contexts(c))
                .map(|(id, _)| id)
                .collect();
            if targets.is_empty() {
                continue;
            }
            targets.sort();
            let Some(a) = args(ext) else { continue };
            for id in targets {
                self.event(side, id, name, a.clone(), None);
            }
        }
    }

    /// The background has loaded: its listeners are there, and what waited for it goes to it.
    fn loaded(&mut self, side: &mut dyn BrowserSide, caller: &Caller) {
        match caller.kind {
            Kind::Background => {}
            Kind::Offscreen => {
                let waiting = self.ext_mut(&caller.profile, &caller.extension).map(|e| std::mem::take(&mut e.offscreen_waiting)).unwrap_or_default();
                for (context, call) in waiting {
                    self.reply(side, &context, call, Ok(Value::Null));
                }
                return;
            }
            _ => return,
        }
        let profile = caller.profile.clone();
        let first_start = !self.started.contains(&profile);
        let Some(ext) = self.ext_mut(&profile, &caller.extension) else { return };
        ext.loaded = true;
        let reason = ext.reason.take();
        let held = std::mem::take(&mut ext.held);
        match reason {
            Some((reason, previous)) => {
                let mut details = json!({ "reason": reason });
                if let Some(p) = previous {
                    details["previousVersion"] = json!(p);
                }
                self.event(side, &caller.context, "runtime.onInstalled", vec![details], None);
            }
            None if first_start => self.event(side, &caller.context, "runtime.onStartup", vec![], None),
            None => {}
        }
        for h in held {
            let Some(c) = self.contexts.get(&h.context) else { continue };
            let held_caller = Caller { context: h.context.clone(), profile: c.profile.clone(), extension: c.extension.clone(), kind: c.kind, tab: c.tab, frame: c.frame, url: c.url.clone() };
            if let (Answer::Now(result), Some(id)) = (self.dispatch(side, &held_caller, &h.call, &h.args, h.id), h.id) {
                self.reply(side, &h.context, id, result);
            }
        }
    }

    /// Every extension of the profile has had its start: `runtime.onStartup` is for the next
    /// time the profile's engine starts with the browser, not for each extension loaded later.
    pub fn startup_done(&mut self, profile: &str) {
        self.started.insert(profile.to_string());
    }

    /// Whether an extension's background is still to load: what is for it waits.
    fn background_coming(&self, profile: &str, extension: &str) -> bool {
        self.ext(profile, extension).is_some_and(|e| e.background.is_some() && !e.loaded)
    }

    fn hold(&mut self, caller: &Caller, target: &str, call: &str, args: &[Value], id: Option<u64>) {
        let held = Held { context: caller.context.clone(), call: call.to_string(), args: args.to_vec(), id, since: Instant::now() };
        if let Some(ext) = self.ext_mut(&caller.profile, target) {
            ext.held.push(held);
        }
    }

    /// A context gone (its page went, its frame, its tab): its ports closed, what it waited for
    /// forgotten, and what waited for it answered without it.
    fn forget_context(&mut self, side: &mut dyn BrowserSide, context: &str) {
        if self.contexts.remove(context).is_none() {
            return;
        }
        let ports: Vec<String> = self.ports.iter().filter(|(_, p)| p.opener == context || p.others.contains(context)).map(|(id, _)| id.clone()).collect();
        for port in ports {
            self.port_disconnect(side, context, &port);
        }
        let questions: Vec<u64> = self.requests.keys().copied().collect();
        for q in questions {
            let (mine, waits_on_it) = match &self.requests[&q] {
                Request::Message { caller, waiting, .. } => (caller == context, waiting.contains(context)),
                Request::Script(p) => (p.caller == context, false),
                Request::MainWorld { caller, .. } => (caller == context, false),
            };
            if mine {
                self.requests.remove(&q);
            } else if waits_on_it {
                self.no_answer(side, context, q, true);
            }
        }
    }

    /// Ports and questions whose contexts are all gone.
    fn drop_dangling(&mut self) {
        let contexts = &self.contexts;
        self.ports.retain(|_, p| contexts.contains_key(&p.opener));
        self.requests.retain(|_, r| match r {
            Request::Message { caller, .. } => contexts.contains_key(caller),
            Request::Script(p) => contexts.contains_key(&p.caller),
            Request::MainWorld { caller, .. } => contexts.contains_key(caller),
        });
    }

    // ---- messages and ports -----------------------------------------------------------------

    /// `runtime.MessageSender` for a context.
    fn sender(&self, side: &dyn BrowserSide, caller: &Caller) -> Value {
        let mut s = Map::new();
        if let Some(ext) = self.ext(&caller.profile, &caller.extension) {
            s.insert("id".into(), json!(ext.runtime_id()));
        }
        s.insert("url".into(), json!(caller.url));
        s.insert("origin".into(), json!(origin_of(&caller.url)));
        if let Some(tab) = tabs::tab_json(self, side, caller.tab, None) {
            s.insert("tab".into(), tab);
            s.insert("frameId".into(), json!(caller.frame));
        }
        s.insert("documentId".into(), json!(caller.context));
        s.insert("documentLifecycle".into(), json!("active"));
        if let Some(ext) = self.ext(&caller.profile, &caller.extension) {
            if ext.installed.flavor == Flavor::Firefox {
                s.insert("envType".into(), json!(if caller.kind == Kind::Content { "content_child" } else { "addon_child" }));
            }
        }
        Value::Object(s)
    }

    /// The contexts of `target` a message or port from `caller` goes to: its own pages (not
    /// the caller), listening for `event`.
    fn receivers(&self, caller: &Caller, target: &str, event: &str) -> Vec<String> {
        let mut all: Vec<String> = self
            .contexts
            .iter()
            .filter(|(id, c)| c.profile == caller.profile && c.extension == target && c.kind != Kind::Content && **id != caller.context && c.listening.contains_key(event))
            .map(|(id, _)| id.clone())
            .collect();
        all.sort();
        all
    }

    /// The content scripts of `extension` in a tab (one frame of it, if named), listening for
    /// `event`.
    fn tab_receivers(&self, profile: &str, extension: &str, tab: TabId, frame: Option<i64>, event: &str) -> Vec<String> {
        let mut all: Vec<String> = self
            .contexts
            .iter()
            .filter(|(_, c)| c.profile == profile && c.extension == extension && c.kind == Kind::Content && c.tab == tab && frame.map_or(true, |f| f == c.frame) && c.listening.contains_key(event))
            .map(|(id, _)| id.clone())
            .collect();
        all.sort();
        all
    }

    /// Where a message or port goes: its own extension, or another of the profile's.
    fn target(&self, caller: &Caller, target: &Value) -> Result<(String, bool), String> {
        match target.as_str().filter(|t| !t.is_empty()) {
            None => Ok((caller.extension.clone(), false)),
            Some(t) => match self.find(&caller.profile, t) {
                Some(id) => Ok((id.clone(), id != caller.extension)),
                None => Err(NO_RECEIVER.to_string()),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn send_message(&mut self, side: &mut dyn BrowserSide, caller: &Caller, call: &str, args: &[Value], id: Option<u64>, target: Value, message: Value) -> Answer {
        let Some(id) = id else { return Answer::Later };
        let (target, external) = match self.target(caller, &target) {
            Ok(t) => t,
            Err(e) => return err(e),
        };
        if self.background_coming(&caller.profile, &target) {
            self.hold(caller, &target, call, args, Some(id));
            return Answer::Later;
        }
        let event = if external { "runtime.onMessageExternal" } else { "runtime.onMessage" };
        let receivers = self.receivers(caller, &target, event);
        self.ask(side, caller, id, receivers, event, message)
    }

    fn send_to_tab(&mut self, side: &mut dyn BrowserSide, caller: &Caller, call: &str, args: &[Value], id: Option<u64>) -> Answer {
        let Some(id) = id else { return Answer::Later };
        let _ = call;
        let Some(tab) = args.first().and_then(Value::as_u64) else { return err("No tab with id: undefined.") };
        let message = args.get(1).cloned().unwrap_or(Value::Null);
        let frame = args.get(2).and_then(|o| o.get("frameId")).and_then(Value::as_i64);
        let receivers = self.tab_receivers(&caller.profile, &caller.extension, tab, frame, "runtime.onMessage");
        self.ask(side, caller, id, receivers, "runtime.onMessage", message)
    }

    /// A message to `receivers`; the first to answer answers the caller.
    fn ask(&mut self, side: &mut dyn BrowserSide, caller: &Caller, id: u64, receivers: Vec<String>, event: &str, message: Value) -> Answer {
        if receivers.is_empty() {
            return err(NO_RECEIVER);
        }
        let sender = self.sender(side, caller);
        let q = self.question();
        self.requests.insert(q, Request::Message { caller: caller.context.clone(), call: id, waiting: receivers.iter().cloned().collect(), heard: false });
        for r in &receivers {
            self.event(side, r, event, vec![message.clone(), sender.clone()], Some(q));
        }
        Answer::Later
    }

    fn respond(&mut self, side: &mut dyn BrowserSide, caller: &Caller, q: u64, result: Result<Value, String>) {
        let mine = matches!(self.requests.get(&q), Some(Request::Message { waiting, .. }) if waiting.contains(&caller.context));
        if !mine {
            return;
        }
        if let Some(Request::Message { caller: to, call, .. }) = self.requests.remove(&q) {
            self.reply(side, &to, call, result);
        }
    }

    /// A receiver that will not answer: when none is left, the caller is told no answer came
    /// (or that nothing was listening at all).
    fn no_answer(&mut self, side: &mut dyn BrowserSide, receiver: &str, q: u64, had_listeners: bool) {
        let done = match self.requests.get_mut(&q) {
            Some(Request::Message { waiting, heard, .. }) => {
                if !waiting.remove(receiver) {
                    return;
                }
                *heard |= had_listeners;
                waiting.is_empty()
            }
            _ => false,
        };
        if !done {
            return;
        }
        if let Some(Request::Message { caller, call, heard, .. }) = self.requests.remove(&q) {
            if heard {
                self.send(side, &caller, &json!({ "to": caller, "r": call, "v": null, "closed": true }));
            } else {
                self.reply(side, &caller, call, Err(NO_RECEIVER.to_string()));
            }
        }
    }

    fn connect(&mut self, side: &mut dyn BrowserSide, caller: &Caller, call: &str, args: &[Value], id: Option<u64>) {
        let port = args.first().and_then(Value::as_str).unwrap_or("").to_string();
        let name = args.get(1).cloned().unwrap_or(json!(""));
        // A port's id is its opener's context's, and new.
        if !port.starts_with(&format!("{}.", caller.context)) || self.ports.contains_key(&port) {
            return;
        }
        let (receivers, event) = if call == "tabs.connect" {
            let Some(tab) = args.get(2).and_then(Value::as_u64) else {
                self.event(side, &caller.context, "port.disconnect", vec![json!(port), json!(NO_RECEIVER)], None);
                return;
            };
            let frame = args.get(3).and_then(|o| o.get("frameId")).and_then(Value::as_i64);
            (self.tab_receivers(&caller.profile, &caller.extension, tab, frame, "runtime.onConnect"), "runtime.onConnect")
        } else {
            let target = args.get(2).cloned().unwrap_or(Value::Null);
            match self.target(caller, &target) {
                Ok((target, external)) => {
                    if self.background_coming(&caller.profile, &target) {
                        self.hold(caller, &target, call, args, id);
                        return;
                    }
                    let event = if external { "runtime.onConnectExternal" } else { "runtime.onConnect" };
                    (self.receivers(caller, &target, event), event)
                }
                Err(_) => (Vec::new(), "runtime.onConnect"),
            }
        };
        if receivers.is_empty() {
            self.event(side, &caller.context, "port.disconnect", vec![json!(port), json!(NO_RECEIVER)], None);
            return;
        }
        let sender = self.sender(side, caller);
        self.ports.insert(port.clone(), Port { opener: caller.context.clone(), others: receivers.iter().cloned().collect() });
        for r in receivers {
            self.event(side, &r, event, vec![json!(port), name.clone(), sender.clone()], None);
        }
    }

    fn port_post(&mut self, side: &mut dyn BrowserSide, caller: &Caller, port: &str, message: Value) {
        let Some(p) = self.ports.get(port) else { return };
        let to: Vec<String> = if p.opener == caller.context {
            let mut others: Vec<String> = p.others.iter().cloned().collect();
            others.sort();
            others
        } else if p.others.contains(&caller.context) {
            vec![p.opener.clone()]
        } else {
            return;
        };
        for t in to {
            self.event(side, &t, "port.message", vec![json!(port), message.clone()], None);
        }
    }

    fn port_disconnect(&mut self, side: &mut dyn BrowserSide, context: &str, port: &str) {
        let Some(p) = self.ports.get_mut(port) else { return };
        if p.opener == context {
            let p = self.ports.remove(port).unwrap();
            let mut others: Vec<String> = p.others.into_iter().collect();
            others.sort();
            for o in others {
                self.event(side, &o, "port.disconnect", vec![json!(port)], None);
            }
        } else if p.others.remove(context) && p.others.is_empty() {
            let opener = p.opener.clone();
            self.ports.remove(port);
            self.event(side, &opener, "port.disconnect", vec![json!(port)], None);
        }
    }

    // ---- storage ----------------------------------------------------------------------------

    /// Where an area is kept for a profile: the private profile keeps nothing on disk, so its
    /// extensions' local and sync storage live as long as it does, as session storage.
    fn storage_place(&self, profile: &str, area: Area) -> (String, Area) {
        if self.is_private(profile) && matches!(area, Area::Local | Area::Sync) {
            (format!("{PRIVATE}/{}", area.name()), Area::Session)
        } else {
            (profile.to_string(), area)
        }
    }

    fn storage_call(&mut self, side: &mut dyn BrowserSide, caller: &Caller, call: &str, args: &[Value]) -> Answer {
        let mut parts = call.splitn(3, '.').skip(1);
        let (Some(area_name), Some(method)) = (parts.next(), parts.next()) else { return err(format!("no such call: {call}")) };
        let Some(area) = Area::parse(area_name) else { return err(format!("no such storage area: {area_name}")) };
        let Some(ext) = self.ext(&caller.profile, &caller.extension) else { return err("extension gone") };
        if !ext.has("storage") && !ext.has("unlimitedStorage") {
            return err("The storage permission is required.");
        }
        if area == Area::Session && caller.kind == Kind::Content && !ext.session_open {
            return err("Access to storage is not allowed from this context.");
        }
        let id = caller.extension.clone();
        let (place, kept_as) = self.storage_place(&caller.profile, area);
        let keys = args.first().cloned().unwrap_or(Value::Null);
        let changes = match method {
            "get" => return ok(Value::Object(storage::get(&self.store, &self.session, &place, &id, kept_as, &keys))),
            "getKeys" => return ok(json!(storage::all(&self.store, &self.session, &place, &id, kept_as).keys().collect::<Vec<_>>())),
            "getBytesInUse" => return ok(json!(storage::bytes_in_use(&self.store, &self.session, &place, &id, kept_as, &keys))),
            "setAccessLevel" => {
                if caller.kind == Kind::Content {
                    return err("Access to storage is not allowed from this context.");
                }
                let open = keys.get("accessLevel").and_then(Value::as_str) == Some("TRUSTED_AND_UNTRUSTED_CONTEXTS");
                if let Some(ext) = self.ext_mut(&caller.profile, &id) {
                    ext.session_open = open;
                }
                return ok(Value::Null);
            }
            "set" => {
                let Some(items) = keys.as_object() else { return err("storage.set wants an object") };
                storage::set(&self.store, &mut self.session, &place, &id, kept_as, items)
            }
            "remove" => storage::remove(&self.store, &mut self.session, &place, &id, kept_as, &keys),
            "clear" => storage::clear(&self.store, &mut self.session, &place, &id, kept_as),
            other => return err(format!("no such call: storage.{area_name}.{other}")),
        };
        match changes {
            Ok(changes) => {
                if !changes.is_empty() {
                    self.storage_changed(side, &caller.profile, &id, area, changes);
                }
                ok(Value::Null)
            }
            Err(e) => err(e),
        }
    }

    fn storage_changed(&self, side: &mut dyn BrowserSide, profile: &str, id: &str, area: Area, changes: storage::Changes) {
        let session_open = self.ext(profile, id).is_some_and(|e| e.session_open);
        let area_event = format!("storage.{}.onChanged", area.name());
        let Some(ext) = self.ext(profile, id) else { return };
        let mut targets: Vec<&String> = self
            .contexts
            .iter()
            .filter(|(_, c)| c.profile == profile && c.extension == ext.id())
            .filter(|(_, c)| c.listening.contains_key("storage.onChanged") || c.listening.contains_key(&area_event))
            .filter(|(_, c)| area != Area::Session || c.kind != Kind::Content || session_open)
            .map(|(cid, _)| cid)
            .collect();
        targets.sort();
        let args = vec![Value::Object(changes), json!(area.name())];
        for t in targets {
            self.event(side, t, "storage.onChanged", args.clone(), None);
        }
    }

    // ---- the rest of runtime ----------------------------------------------------------------

    fn open_options(&mut self, side: &mut dyn BrowserSide, caller: &Caller) -> Answer {
        let Some(ext) = self.ext(&caller.profile, &caller.extension) else { return err("extension gone") };
        let Some(page) = ext.manifest.options_page.clone() else { return err("Could not create an options page.") };
        let url = ext.url(&page);
        let existing = side.tabs().tabs().find(|t| t.profile == caller.profile && t.url.split('#').next() == Some(url.as_str())).map(|t| t.id);
        match existing {
            Some(tab) => side.activate(tab),
            None => {
                let window = self.current_window(side, caller);
                side.open_tab(&caller.profile, window, &url, None, true, None);
            }
        }
        ok(Value::Null)
    }

    fn get_contexts(&self, side: &dyn BrowserSide, caller: &Caller, filter: &Value) -> Value {
        let wanted = |key: &str| -> Option<Vec<Value>> { filter.get(key).and_then(Value::as_array).cloned() };
        let types = wanted("contextTypes");
        let ids = wanted("contextIds");
        let tab_ids = wanted("tabIds");
        let window_ids = wanted("windowIds");
        let urls = wanted("documentUrls");
        let frames = wanted("frameIds");
        let incognito = filter.get("incognito").and_then(Value::as_bool);
        let private = self.is_private(&caller.profile);
        let mut all: Vec<(&String, &Context)> = self.contexts.iter().filter(|(_, c)| c.profile == caller.profile && c.extension == caller.extension && c.kind != Kind::Content).collect();
        all.sort_by_key(|(id, _)| (*id).clone());
        let out: Vec<Value> = all
            .into_iter()
            .filter_map(|(id, c)| {
                let tab: i64 = if Self::is_hidden(c.tab) { -1 } else { c.tab as i64 };
                let window: i64 = match self.popups.get(&c.tab) {
                    Some((_, _, w)) => *w as i64,
                    None => side.tabs().window_of(c.tab).map_or(-1, |w| w as i64),
                };
                let ty = c.kind.context_type();
                let keep = types.as_ref().map_or(true, |t| t.iter().any(|v| v == ty))
                    && ids.as_ref().map_or(true, |t| t.iter().any(|v| v == id.as_str()))
                    && tab_ids.as_ref().map_or(true, |t| t.iter().any(|v| v.as_i64() == Some(tab)))
                    && window_ids.as_ref().map_or(true, |t| t.iter().any(|v| v.as_i64() == Some(window)))
                    && urls.as_ref().map_or(true, |t| t.iter().any(|v| v.as_str() == Some(c.url.as_str())))
                    && frames.as_ref().map_or(true, |t| t.iter().any(|v| v.as_i64() == Some(c.frame)))
                    && incognito.map_or(true, |i| i == private);
                keep.then(|| {
                    json!({
                        "contextType": ty, "contextId": id, "tabId": tab, "windowId": window, "documentId": id,
                        "frameId": c.frame, "documentUrl": c.url, "documentOrigin": origin_of(&c.url), "incognito": private,
                    })
                })
            })
            .collect();
        Value::Array(out)
    }

    /// The window a context's calls mean by "current": its popup's, its tab's, or the one the
    /// wearer used last.
    fn current_window(&self, side: &dyn BrowserSide, caller: &Caller) -> Option<WindowKey> {
        if let Some((_, _, w)) = self.popups.get(&caller.tab) {
            return Some(*w);
        }
        side.tabs().window_of(caller.tab).or_else(|| side.last_focused())
    }

    // ---- what the browser tells -------------------------------------------------------------

    /// Something happened to a tab: `tabs`' events, to every extension of its profile that may
    /// know.
    pub fn tab_changed(&mut self, side: &mut dyn BrowserSide, tab: TabId, change: &Change) {
        let Some(profile) = side.tabs().tab(tab).map(|t| t.profile.clone()) else { return };
        if change.url.is_some() {
            // A tab gone somewhere else: what the wearer granted for it is no more.
            if let Some(loaded) = self.profiles.get_mut(&profile) {
                for ext in &mut loaded.exts {
                    ext.active_tabs.remove(&tab);
                }
            }
        }
        let Some(full) = tabs::tab_json(self, side, tab, None) else { return };
        let url = full.get("url").and_then(Value::as_str).unwrap_or("").to_string();
        let make = |ext: &Ext| -> Option<Vec<Value>> {
            let see = ext.may_see(&url, tab);
            let mut info = Map::new();
            if let Some(loading) = change.loading {
                info.insert("status".into(), json!(if loading { "loading" } else { "complete" }));
            }
            if let (Some(u), true) = (&change.url, see) {
                info.insert("url".into(), json!(u));
            }
            if let (Some(t), true) = (&change.title, see) {
                info.insert("title".into(), json!(t));
            }
            if let Some(a) = change.audible {
                info.insert("audible".into(), json!(a));
            }
            if let Some(m) = change.muted {
                info.insert("mutedInfo".into(), json!({ "muted": m }));
            }
            if info.is_empty() {
                return None;
            }
            Some(vec![json!(tab), Value::Object(info), tabs::redact(&full, ext, tab)])
        };
        self.emit(side, &profile, None, "tabs.onUpdated", &make, &|_| true);
    }

    pub fn tab_created(&mut self, side: &mut dyn BrowserSide, tab: TabId) {
        let Some(profile) = side.tabs().tab(tab).map(|t| t.profile.clone()) else { return };
        let Some(full) = tabs::tab_json(self, side, tab, None) else { return };
        self.emit(side, &profile, None, "tabs.onCreated", &|ext| Some(vec![tabs::redact(&full, ext, tab)]), &|_| true);
    }

    pub fn tab_activated(&mut self, side: &mut dyn BrowserSide, tab: TabId) {
        let Some(profile) = side.tabs().tab(tab).map(|t| t.profile.clone()) else { return };
        let Some(window) = side.tabs().window_of(tab) else { return };
        let info = json!({ "tabId": tab, "windowId": window });
        self.emit(side, &profile, None, "tabs.onActivated", &|_| Some(vec![info.clone()]), &|_| true);
        side.actions_changed(&profile);
    }

    /// A tab closed (`window_closing`: with its window): its contexts are gone, and so is what
    /// was set for it.
    pub fn tab_removed(&mut self, side: &mut dyn BrowserSide, profile: &str, tab: TabId, window: WindowKey, window_closing: bool) {
        let gone: Vec<String> = self.contexts.iter().filter(|(_, c)| c.tab == tab).map(|(id, _)| id.clone()).collect();
        for id in gone {
            self.forget_context(side, &id);
        }
        if let Some(loaded) = self.profiles.get_mut(profile) {
            for ext in &mut loaded.exts {
                ext.active_tabs.remove(&tab);
                ext.action.forget(tab);
            }
        }
        let info = json!({ "windowId": window, "isWindowClosing": window_closing });
        self.emit(side, profile, None, "tabs.onRemoved", &|_| Some(vec![json!(tab), info.clone()]), &|_| true);
    }

    pub fn tab_moved(&mut self, side: &mut dyn BrowserSide, tab: TabId, from: (WindowKey, usize), to: (WindowKey, usize)) {
        let Some(profile) = side.tabs().tab(tab).map(|t| t.profile.clone()) else { return };
        if from.0 == to.0 {
            let info = json!({ "windowId": to.0, "fromIndex": from.1, "toIndex": to.1 });
            self.emit(side, &profile, None, "tabs.onMoved", &|_| Some(vec![json!(tab), info.clone()]), &|_| true);
        } else {
            let detached = json!({ "oldWindowId": from.0, "oldPosition": from.1 });
            self.emit(side, &profile, None, "tabs.onDetached", &|_| Some(vec![json!(tab), detached.clone()]), &|_| true);
            let attached = json!({ "newWindowId": to.0, "newPosition": to.1 });
            self.emit(side, &profile, None, "tabs.onAttached", &|_| Some(vec![json!(tab), attached.clone()]), &|_| true);
        }
    }

    pub fn window_created(&mut self, side: &mut dyn BrowserSide, window: WindowKey) {
        let Some(profile) = side.tabs().window(window).map(|w| w.profile.clone()) else { return };
        let Some(info) = tabs::window_json(self, side, window, false, None) else { return };
        self.emit(side, &profile, None, "windows.onCreated", &|_| Some(vec![info.clone()]), &|_| true);
    }

    pub fn window_removed(&mut self, side: &mut dyn BrowserSide, profile: &str, window: WindowKey) {
        self.popups.retain(|_, (_, _, w)| *w != window);
        self.emit(side, profile, None, "windows.onRemoved", &|_| Some(vec![json!(window)]), &|_| true);
    }

    /// The wearer went to another window (none: away from SpatiMesh).
    pub fn window_focused(&mut self, side: &mut dyn BrowserSide, window: Option<WindowKey>) {
        let id: i64 = window.map_or(-1, |w| w as i64);
        let profiles: Vec<String> = self.profiles.keys().cloned().collect();
        for profile in profiles {
            self.emit(side, &profile, None, "windows.onFocusChanged", &|_| Some(vec![json!(id)]), &|_| true);
        }
    }

    /// A tab's top frame going somewhere: `webNavigation`'s events.
    pub fn navigation(&mut self, side: &mut dyn BrowserSide, tab: TabId, url: &str, what: &Navigation) {
        let Some(profile) = side.tabs().tab(tab).map(|t| t.profile.clone()) else { return };
        let (name, mut details) = match what {
            Navigation::Before => ("webNavigation.onBeforeNavigate", json!({ "parentFrameId": -1 })),
            Navigation::Committed => ("webNavigation.onCommitted", json!({ "transitionType": "link", "transitionQualifiers": [] })),
            Navigation::DomContentLoaded => ("webNavigation.onDOMContentLoaded", json!({})),
            Navigation::Completed => ("webNavigation.onCompleted", json!({})),
            Navigation::Error(e) => ("webNavigation.onErrorOccurred", json!({ "error": e })),
        };
        details["tabId"] = json!(tab);
        details["url"] = json!(url);
        details["frameId"] = json!(0);
        details["processId"] = json!(-1);
        details["timeStamp"] = json!(crate::store::now_ms() as f64);
        details["documentLifecycle"] = json!("active");
        details["frameType"] = json!("outermost_frame");
        let url = url.to_string();
        let make = |ext: &Ext| -> Option<Vec<Value>> { ext.has("webNavigation").then(|| vec![details.clone()]) };
        // A listener's filter ({url: [UrlFilter]}): only what one of its filters takes.
        let wants = |c: &Context| -> bool {
            c.listening.get(name).is_some_and(|filters| filters.iter().any(|f| apis::url_filter_takes(f, &url)))
        };
        self.emit(side, &profile, None, name, &make, &wants);
    }

    /// A page of an extension's that no window shows went: crashed, or closed.
    pub fn hidden_gone(&mut self, side: &mut dyn BrowserSide, profile: &str, tab: TabId) {
        let gone: Vec<String> = self.contexts.iter().filter(|(_, c)| c.tab == tab).map(|(id, _)| id.clone()).collect();
        for id in gone {
            self.forget_context(side, &id);
        }
        if let Some((_, _, _)) = self.popups.remove(&tab) {
            return;
        }
        let Some(loaded) = self.profiles.get_mut(profile) else { return };
        let mut restart = None;
        for ext in &mut loaded.exts {
            if ext.offscreen == Some(tab) {
                ext.offscreen = None;
            }
            if ext.background == Some(tab) {
                ext.background = None;
                ext.loaded = false;
                // Started again, a few times, as Chrome restarts a crashed background once asked.
                if ext.restarts < 3 {
                    ext.restarts += 1;
                    restart = Some(ext.installed.id.clone());
                } else {
                    log::warn!("extension {}'s background keeps stopping: left stopped", ext.installed.id);
                }
            }
        }
        if let Some(id) = restart {
            self.start_background(side, profile, &id);
        }
    }

    /// The top frame's result of code run in the page's own world.
    pub fn script_result(&mut self, side: &mut dyn BrowserSide, request: u64, result: Result<String, String>) {
        let Some(Request::MainWorld { caller, call, legacy, .. }) = self.requests.remove(&request) else { return };
        let value = match result {
            Ok(json) => serde_json::from_str::<Value>(&json).unwrap_or(Value::Null),
            Err(e) => return self.reply(side, &caller, call, Err(e)),
        };
        let answer = if legacy { json!([value]) } else { json!([{ "frameId": 0, "result": value, "documentId": format!("{caller}-0") }]) };
        self.reply(side, &caller, call, Ok(answer));
    }

    fn script_done(&mut self, side: &mut dyn BrowserSide, caller: &Caller, q: u64, frame: Value, value: Value, error: Value) {
        let finished = match self.requests.get_mut(&q) {
            Some(Request::Script(p)) => p.add(caller, frame.as_i64().unwrap_or(caller.frame), value, error.as_str().map(str::to_string)),
            _ => false,
        };
        if finished {
            if let Some(Request::Script(p)) = self.requests.remove(&q) {
                let (context, call, answer) = p.answer();
                self.reply(side, &context, call, answer);
            }
        }
    }

    /// Time passing: alarms going off, answers waited for too long given up.
    pub fn tick(&mut self, side: &mut dyn BrowserSide) {
        let now = Instant::now();
        // Scripts whose frames have all had their chance, or whose engine never answered.
        let due: Vec<u64> = self
            .requests
            .iter()
            .filter(|(_, r)| match r {
                Request::Script(p) => p.due(now),
                Request::MainWorld { since, .. } => now.duration_since(*since) > scripting::SCRIPT_WAIT,
                Request::Message { .. } => false,
            })
            .map(|(q, _)| *q)
            .collect();
        for q in due {
            match self.requests.remove(&q) {
                Some(Request::Script(p)) => {
                    let (context, call, answer) = p.answer();
                    self.reply(side, &context, call, answer);
                }
                Some(Request::MainWorld { caller, call, .. }) => self.reply(side, &caller, call, Err("The script did not finish.".into())),
                _ => {}
            }
        }
        // Calls that waited for a background that never loaded.
        let mut given_up = Vec::new();
        for loaded in self.profiles.values_mut() {
            for ext in &mut loaded.exts {
                let (old, kept): (Vec<Held>, Vec<Held>) = std::mem::take(&mut ext.held).into_iter().partition(|h| now.duration_since(h.since) > BACKGROUND_WAIT);
                ext.held = kept;
                given_up.extend(old);
            }
        }
        for h in given_up {
            if let Some(id) = h.id {
                self.reply(side, &h.context, id, Err(NO_RECEIVER.to_string()));
            } else if h.call.ends_with("connect") {
                let port = h.args.first().cloned().unwrap_or(Value::Null);
                self.event(side, &h.context, "port.disconnect", vec![port, json!(NO_RECEIVER)], None);
            }
        }
        apis::fire_alarms(self, side);
    }

    /// When [`Runtime::tick`] has something to do next, at the latest.
    pub fn next_tick(&self) -> Option<Instant> {
        let mut next: Option<Instant> = apis::next_alarm(self);
        let mut sooner = |t: Instant| next = Some(next.map_or(t, |n| n.min(t)));
        for r in self.requests.values() {
            match r {
                Request::Script(p) => sooner(p.deadline()),
                Request::MainWorld { since, .. } => sooner(*since + scripting::SCRIPT_WAIT),
                Request::Message { .. } => {}
            }
        }
        for loaded in self.profiles.values() {
            for ext in &loaded.exts {
                for h in &ext.held {
                    sooner(h.since + BACKGROUND_WAIT);
                }
            }
        }
        next
    }

    // ---- what the window shows ---------------------------------------------------------------

    /// The extensions a profile runs: id, name, and whether each has a toolbar button.
    pub fn extensions(&self, profile: &str) -> Vec<(String, String, bool)> {
        self.profiles.get(profile).map_or(Vec::new(), |l| l.exts.iter().map(|e| (e.installed.id.clone(), e.manifest.name.clone(), e.manifest.action.is_some())).collect())
    }

    /// Whether a tab is the popup of an extension's button.
    pub fn popup_of(&self, tab: TabId) -> Option<(&str, &str, WindowKey)> {
        self.popups.get(&tab).map(|(p, e, w)| (p.as_str(), e.as_str(), *w))
    }

    /// The wearer closed a popup.
    pub fn popup_closed(&mut self, side: &mut dyn BrowserSide, tab: TabId) {
        let gone: Vec<String> = self.contexts.iter().filter(|(_, c)| c.tab == tab).map(|(id, _)| id.clone()).collect();
        for id in gone {
            self.forget_context(side, &id);
        }
        self.popups.remove(&tab);
    }
}

#[cfg(test)]
mod tests;
