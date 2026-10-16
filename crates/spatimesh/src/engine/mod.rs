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
}

/// Something a page did that the window should know.
#[derive(Debug, Clone, PartialEq)]
pub enum Happened {
    Title { tab: TabId, title: String },
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
}

impl Happened {
    pub fn tab(&self) -> TabId {
        match self {
            Happened::Title { tab, .. }
            | Happened::Url { tab, .. }
            | Happened::Progress { tab, .. }
            | Happened::History { tab, .. }
            | Happened::Picture { tab }
            | Happened::Cursor { tab, .. }
            | Happened::Sound { tab, .. }
            | Happened::CloseTab { tab }
            | Happened::Crashed { tab, .. }
            | Happened::Failed { tab, .. } => *tab,
            Happened::OpenTab { opener, .. } => *opener,
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
    /// Let the engine work for at most `wait`, drawing into `canvas`, and say what happened.
    /// It returns sooner when the descriptor given to [`Engine::wake_on`] has something to read.
    fn pump(&mut self, wait: Duration, canvas: &mut dyn Canvas) -> Vec<Happened>;
    /// The window's socket: a reason for [`Engine::pump`] to stop waiting.
    fn wake_on(&mut self, _fd: std::os::fd::RawFd) {}
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
