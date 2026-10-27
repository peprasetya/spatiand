//! The one interface every web engine is put behind (docs/spatimesh.md).
//!
//! An [`Engine`] lives in the engine host, the process that holds one profile's pages: WebKitGTK
//! on Linux, Steam's Chromium (CEF) on SteamOS, WKWebView on the Mac, the system WebView on
//! Android. The window process never links one; it says the same things over the wire
//! (`wire.rs`) and is told what [`Happened`].
//!
//! Each tab's picture is drawn by the engine into a [`Canvas`], which the host keeps in memory
//! shared with the window, so that a frame crosses from one process to the other without being
//! copied a second time.

use std::time::Duration;

pub mod fake;

/// A tab, by the number the window process gave it. Numbers are never used twice in a run.
pub type TabId = u64;

/// How a tab's page is to be drawn: the size of its picture in pixels, and how far it is
/// zoomed (1.0 is the page as it asks to be; 1.25 draws everything a quarter larger, and the
/// page sees a viewport that much narrower).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub width: u32,
    pub height: u32,
    pub zoom: f32,
}

impl View {
    pub fn new(width: u32, height: u32, zoom: f32) -> View {
        View { width: width.max(1), height: height.max(1), zoom: if zoom.is_finite() && zoom > 0.1 { zoom } else { 1.0 } }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    Back,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub logo: bool,
}

/// A key pressed or let go, as the room told the window: where it is (`code`, Linux's number
/// for the key), what it does (`key`) and what it typed, if anything.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyPress {
    pub code: u32,
    pub key: spatiand_app::Key,
    pub text: Option<String>,
    pub pressed: bool,
    pub repeat: bool,
    pub modifiers: Modifiers,
}

/// What the pointer looks like over the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Cursor {
    #[default]
    Arrow,
    Hand,
    Text,
    Wait,
    Crosshair,
    Move,
    ResizeHorizontal,
    ResizeVertical,
    NotAllowed,
    Grab,
    Grabbing,
    Hidden,
}

/// A rectangle of a picture, in its pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn whole(width: u32, height: u32) -> Rect {
        Rect { x: 0, y: 0, width, height }
    }

    /// This rectangle cut to a picture of `width` by `height`.
    pub fn clip(&self, width: u32, height: u32) -> Option<Rect> {
        let x = self.x.min(width);
        let y = self.y.min(height);
        let right = self.x.saturating_add(self.width).min(width);
        let bottom = self.y.saturating_add(self.height).min(height);
        (right > x && bottom > y).then(|| Rect { x, y, width: right - x, height: bottom - y })
    }
}

/// How the bytes of a picture are laid out: four to a pixel, rows from the top, no gaps
/// between rows, colour already multiplied by alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Blue, green, red, alpha: cairo's, Chromium's and Core Graphics' own order.
    Bgra,
    /// Red, green, blue, alpha: Android's.
    Rgba,
}

/// Where an engine draws a tab's picture.
///
/// [`Canvas::begin`] hands out memory for a whole picture of the size asked, to be filled and
/// then given to the window with [`Canvas::finish`], saying which parts changed. It may hand
/// back the picture the window had before, so an engine that only redraws what changed copies
/// only that.
pub trait Canvas {
    fn begin(&mut self, tab: TabId, width: u32, height: u32, format: Format) -> Option<&mut [u8]>;
    fn finish(&mut self, tab: TabId, changed: &[Rect]);
    /// A whole picture already on the GPU, by the system's number for it, instead of bytes.
    fn surface(&mut self, tab: TabId, id: u32, width: u32, height: u32) {
        let _ = (tab, id, width, height);
    }
}

/// One line of a `<select>`'s list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    pub enabled: bool,
    pub selected: bool,
    /// The heading of a group (`<optgroup>`), not to be chosen.
    pub heading: bool,
}

/// What a page's script asked the wearer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    /// `alert()`: a message and OK.
    Alert,
    /// `confirm()`: OK or Cancel.
    Confirm,
    /// `prompt()`: a line of text, OK or Cancel.
    Prompt,
    /// Leaving a page that asked to be asked first.
    Leave,
}

/// What is under the pointer where the wearer asked for a menu (a right click): what the
/// window's own menu offers to do with it.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Hit {
    /// A link's address, and the words it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub link_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// A film's or a sound's address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<String>,
    /// A field or anything else that can be typed into.
    #[serde(default)]
    pub editable: bool,
    /// Some of the page's text is chosen.
    #[serde(default)]
    pub selection: bool,
}

impl Hit {
    /// From the lines an engine's native half writes: flags (1 a link, 2 an image, 4 media, 8
    /// editable, 16 chosen text), and the link, its words, the image and the media, a line each.
    pub fn from_native(flags: i32, lines: &str) -> Hit {
        let mut l = lines.split('\n').map(str::to_string);
        let (link, link_text, image, media) = (l.next().unwrap_or_default(), l.next().unwrap_or_default(), l.next().unwrap_or_default(), l.next().unwrap_or_default());
        let some = |on: bool, s: String| (on && !s.is_empty()).then_some(s);
        Hit {
            link: some(flags & 1 != 0, link),
            link_text: link_text.trim().to_string(),
            image: some(flags & 2 != 0, image),
            media: some(flags & 4 != 0, media),
            editable: flags & 8 != 0,
            selection: flags & 16 != 0,
        }
    }
}

/// What became of a download.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Download {
    /// It began: from `url`, into the file `file` (where it is being written, finished or not).
    Started { url: String, file: String },
    /// So many bytes have come, of `total` (0 if not known).
    Progress { received: u64, total: u64 },
    Finished { file: String },
    Failed { why: String },
}

/// The first number of the downloads an engine begins by itself (a page's file it cannot show,
/// a link with `download`): below it are the window's own (`Engine::download`).
pub const ENGINE_DOWNLOADS: u64 = 1 << 30;

/// Something a page did that the window should know.
#[derive(Debug, Clone, PartialEq)]
pub enum Happened {
    Title { tab: TabId, title: String },
    /// The name the tab's sound is handed to the room under (`<app id>/page<n>`), for the window
    /// to say which window's place it is to be heard at -- and again when the tab moves.
    SoundName { tab: TabId, name: String },
    /// The address the tab is at now: after a redirect, a link followed, history.
    Url { tab: TabId, url: String },
    /// How much of the page has come, from 0 to 1, and whether it is still coming.
    Progress { tab: TabId, progress: f32, loading: bool },
    /// Whether going back and forward would go anywhere.
    History { tab: TabId, back: bool, forward: bool },
    /// A new picture is in the tab's canvas.
    Picture { tab: TabId },
    Cursor { tab: TabId, cursor: Cursor },
    /// The tab started or stopped making a sound.
    Sound { tab: TabId, playing: bool },
    /// The page asked for a page of its own: a link with `target="_blank"`, `window.open`, a
    /// middle click. The window decides where it goes, and opens it with [`Engine::open`].
    OpenTab { opener: TabId, url: String, background: bool },
    /// The page asked to be closed (`window.close()`).
    CloseTab { tab: TabId },
    /// What the tab's page ran in stopped: it shows nothing until loaded again.
    Crashed { tab: TabId, why: String },
    /// The page could not be loaded at all: no such host, refused, a certificate.
    Failed { tab: TabId, url: String, why: String },
    /// A `<select>` was opened: the window shows the list at `at` and says what was chosen
    /// with [`Engine::choose`].
    Choose { tab: TabId, choices: Vec<Choice>, at: Rect },
    /// A script asks something; the page waits for [`Engine::answer`].
    Dialog { tab: TabId, kind: DialogKind, message: String, text: String },
    /// A call from an extension context in the tab, as its API said it (JSON): for the
    /// window's extension runtime (docs/spatimesh.md, "Extensions").
    Extension { tab: TabId, json: String },
    /// What code run with [`Engine::run_script`] gave in the tab's top frame: its value as
    /// JSON, or why it failed.
    ScriptResult { tab: TabId, request: u64, result: Result<String, String> },
    /// What SpatiMesh's media script in the tab's top frame says to the window (JSON): its
    /// films, sounds and MediaSources, played by Spatiand's own player (`crate::media`).
    Media { tab: TabId, json: String },
    /// The wearer asked for a menu over the page (a right click the page did not take for
    /// itself): the window shows its own, for what is under the pointer.
    ContextMenu { tab: TabId, hit: Hit },
    /// The link under the pointer now, or none (empty): said at the foot of the page.
    Hovered { tab: TabId, link: String },
    /// What [`Engine::find`] found: how many times it is in the page (0: not at all).
    Found { tab: TabId, matches: u32 },
    /// A page's `<input type=file>` wants files: the window asks the wearer which, and says
    /// with [`Engine::choose_files`]. `accept` is what the page said it takes (MIME types).
    ChooseFiles { tab: TabId, multiple: bool, accept: Vec<String> },
    /// A download's news: the window's own ([`Engine::download`]) by its number, or one the
    /// engine began (numbered from [`ENGINE_DOWNLOADS`]).
    Download { tab: TabId, id: u64, event: Download },
    /// The profile's cookies, all of them, as asked for with [`Engine::ask_cookies`].
    Cookies { cookies: Vec<Cookie> },
    /// What an engine that cannot list its cookies (Android's) sends to each address it was
    /// asked about, as a page there would get it: `name=value; name=value`, by address.
    CookiesSent { sent: Vec<(String, String)> },
}

/// A cookie, as every engine keeps one: a domain that starts with a dot is the domain's and
/// its hosts', one without is that host's alone. `expires` is in seconds since 1970; none is a
/// cookie for the session.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<f64>,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub http_only: bool,
    /// "Lax", "Strict", "None", or empty for the engine's own default.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub same_site: String,
}

impl Cookie {
    /// What makes it the same cookie as another: a new one with these replaces it.
    pub fn key(&self) -> (&str, &str, &str) {
        (&self.name, &self.domain, &self.path)
    }

    /// One line a C engine reads or writes: the fields between tabs, which no cookie has in it.
    pub fn line(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.name,
            self.value,
            self.domain,
            self.path,
            self.expires.map_or(String::from("-1"), |e| format!("{e:.0}")),
            self.secure as u8,
            self.http_only as u8,
            self.same_site
        )
    }

    /// Cookies from the lines of [`Cookie::line`].
    pub fn from_lines(text: &str) -> Vec<Cookie> {
        text.lines()
            .filter_map(|line| {
                let f: Vec<&str> = line.split('\t').collect();
                if f.len() < 7 || f[0].is_empty() || f[2].is_empty() {
                    return None;
                }
                let expires = f[4].parse::<f64>().ok().filter(|e| *e >= 0.0);
                Some(Cookie {
                    name: f[0].into(),
                    value: f[1].into(),
                    domain: f[2].to_ascii_lowercase(),
                    path: if f[3].is_empty() { "/".into() } else { f[3].into() },
                    expires,
                    secure: f[5] == "1",
                    http_only: f[6] == "1",
                    same_site: f.get(7).map_or(String::new(), |s| s.to_string()),
                })
            })
            .collect()
    }
}

impl Happened {
    pub fn tab(&self) -> TabId {
        match self {
            Happened::Title { tab, .. }
            | Happened::SoundName { tab, .. }
            | Happened::Url { tab, .. }
            | Happened::Progress { tab, .. }
            | Happened::History { tab, .. }
            | Happened::Picture { tab }
            | Happened::Cursor { tab, .. }
            | Happened::Sound { tab, .. }
            | Happened::CloseTab { tab }
            | Happened::Crashed { tab, .. }
            | Happened::Failed { tab, .. }
            | Happened::Choose { tab, .. }
            | Happened::Dialog { tab, .. }
            | Happened::Extension { tab, .. }
            | Happened::ScriptResult { tab, .. }
            | Happened::Media { tab, .. }
            | Happened::ContextMenu { tab, .. }
            | Happened::Hovered { tab, .. }
            | Happened::Found { tab, .. }
            | Happened::ChooseFiles { tab, .. }
            | Happened::Download { tab, .. } => *tab,
            Happened::OpenTab { opener, .. } => *opener,
            // The profile's, not a tab's.
            Happened::Cookies { .. } | Happened::CookiesSent { .. } => 0,
        }
    }
}

/// A web engine, holding the pages of one profile.
///
/// Every call is about one tab, named by the window's number for it. A call about a tab the
/// engine does not have is ignored, so that a window and an engine host that have just
/// crossed (a tab closed on one side while the other was busy) do no harm.
pub trait Engine {
    /// What it is, for the log and the About page: "WebKitGTK 2.52.6".
    fn name(&self) -> String;
    /// Make a tab and start loading `url` in it.
    fn open(&mut self, tab: TabId, url: &str, view: View) -> Result<(), String>;
    fn close(&mut self, tab: TabId);
    fn load(&mut self, tab: TabId, url: &str);
    fn back(&mut self, tab: TabId);
    fn forward(&mut self, tab: TabId);
    fn reload(&mut self, tab: TabId);
    fn stop(&mut self, tab: TabId);
    fn resize(&mut self, tab: TabId, view: View);
    /// The pointer is at `x`, `y` in the tab's picture, and a button went down or up.
    fn pointer(&mut self, tab: TabId, x: f32, y: f32, button: Option<(Button, bool)>, modifiers: Modifiers);
    /// The pointer left the page.
    fn pointer_left(&mut self, tab: TabId);
    /// A wheel or two fingers at `x`, `y`: pixels, positive down and to the right.
    fn scroll(&mut self, tab: TabId, x: f32, y: f32, dx: f32, dy: f32, modifiers: Modifiers);
    fn key(&mut self, tab: TabId, key: &KeyPress);
    /// The tab is the one the keyboard goes to, or stops being.
    fn focus(&mut self, tab: TabId, focused: bool);
    /// The window the tab is in now: where its sound is to come from.
    fn set_window(&mut self, tab: TabId, window: u32);
    fn set_muted(&mut self, tab: TabId, muted: bool);
    /// What was chosen from the list of a [`Happened::Choose`]: none if it was closed.
    fn choose(&mut self, tab: TabId, index: Option<usize>);
    /// The answer to a [`Happened::Dialog`]: OK or not, and a prompt's text.
    fn answer(&mut self, tab: TabId, accepted: bool, text: Option<String>);
    /// Let the engine work for at most `wait`, drawing into `canvas`, and say what happened.
    /// It returns sooner when the descriptor given to [`Engine::wake_on`] has something to read.
    fn pump(&mut self, wait: Duration, canvas: &mut dyn Canvas) -> Vec<Happened>;

    /// The window is done with a picture handed over as [`Canvas::surface`].
    fn release_surface(&mut self, tab: TabId, id: u32) {
        let _ = (tab, id);
    }
    /// The window's socket: a reason for [`Engine::pump`] to stop waiting.
    fn wake_on(&mut self, _fd: std::os::fd::RawFd) {}
    /// The profile's extensions: which content scripts go into which frames, and what
    /// `chrome-extension://` and `moz-extension://` serve ([`crate::extensions::set::serve`]).
    /// An engine without extensions yet ignores it.
    fn set_extensions(&mut self, _set: &crate::extensions::set::ExtensionSet) {}
    /// Code run in a tab: in `world` ("" for the page's own; `spatimesh-<id>` for an
    /// extension's, its API set up there first if `boot`), in the top frame or in every frame.
    /// If `request` is not 0, the top frame's result is told as [`Happened::ScriptResult`].
    fn run_script(&mut self, _tab: TabId, _world: &str, _all_frames: bool, _code: &str, _request: u64, _boot: bool) {}
    /// SpatiMesh's own script, for the page's world at the start of every frame's every
    /// document from now on, before the page's own. In the top frame it is given, before it
    /// runs, `window.__spatimeshMediaPost(json)`: what it says to the window, as
    /// [`Happened::Media`]. What the window says back reaches it by [`Engine::run_script`].
    fn set_page_script(&mut self, _code: &str) {}
    /// Say all the profile's cookies, as [`Happened::Cookies`], soon. `addresses` are where
    /// an engine that cannot list them all is to look ([`Happened::CookiesSent`]).
    fn ask_cookies(&mut self, _addresses: &[String]) {}
    /// Cookies from another machine: `set` made or replaced, `remove` gone.
    fn set_cookies(&mut self, _set: &[Cookie], _remove: &[Cookie]) {}
    /// Pages visited on another machine, for links to them to look visited.
    fn add_visited(&mut self, _urls: &[String]) {}
    /// Open the developer tools (Web Inspector) for `tab`, as the tab `inspector`: from then on
    /// it is a tab like the others (pictures, input, [`Engine::resize`], [`Engine::close`]),
    /// except that it has no address to load. It ends with [`Happened::CloseTab`] when the
    /// developer tools are closed from inside, and goes when `tab` does. False where the
    /// engine has none.
    fn inspect(&mut self, _tab: TabId, _inspector: TabId, _view: View) -> bool {
        false
    }
    /// Download `url` as the tab would ask for it (its cookies), into the file `to`, or into the
    /// wearer's Downloads under the name the server gives it if `to` is empty. Its news comes as
    /// [`Happened::Download`] with `id`.
    fn download(&mut self, _tab: TabId, _id: u64, _url: &str, _to: &str) {}
    fn cancel_download(&mut self, _id: u64) {}
    /// Find `text` in the tab's page, each match marked, the next one (or the one before)
    /// brought into view: the same text again goes on to the next. Empty, finding ends.
    fn find(&mut self, _tab: TabId, _text: &str, _forward: bool) {}
    /// The files the wearer chose for a [`Happened::ChooseFiles`], by their paths; none if they
    /// chose none.
    fn choose_files(&mut self, _tab: TabId, _files: Option<Vec<String>>) {}
}

/// What a profile is to the engine that holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSetup {
    pub id: String,
    /// Keeps nothing: nothing on disk, nothing after the host ends.
    pub private: bool,
    /// Where the profile's cookies and site storage are kept, on this machine only.
    pub data: std::path::PathBuf,
    /// Where its cache is kept: thrown away freely.
    pub cache: std::path::PathBuf,
}
