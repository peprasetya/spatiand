//! Remote applications, shown as ordinary windows in the room.
//!
//! A host somewhere else runs the application; this receives its pictures and puts them on a
//! surface. What makes that cheap to build is the last step: **the remote side is a Wayland
//! client of this compositor**, connected over a socket pair inside the same process.
//!
//! ```text
//!   host ── QUIC ──▶ net thread ──▶ decode ──▶ convert ──▶ wl_surface ──▶ the room
//!                         (this module, on its own thread)      ▲
//!                                                    the compositor sees only a client
//! ```
//!
//! Everything a window already has — a title bar, being moved and resized, focus, its own
//! sound, its own controller layout, the fade when nobody is looking at it — applies to a
//! remote window without a line of new code, because from the compositor's side it *is* a
//! window. The alternative, teaching the scene about a second kind of window, would have
//! touched the renderer, the pointer, the layout and the audio router at once.
//!
//! It costs one hop through a socket, and no pixels go through it: frames arrive as dmabufs
//! and are attached as dmabufs.
//!
//! ## What runs where
//!
//! One thread per host, holding the network, the decoders and the client connection. The
//! compositor thread is untouched: it discovers remote windows the same way it discovers any
//! other application's, by being asked for a surface.

/// The Mac's own windows are shown through the same client; see `spatiand-mac`'s `local`.
pub(crate) mod client;
/// A platform's own clipboard joined to the session's: the Beam Pro's.
#[allow(dead_code)]
pub mod local_board;
pub mod menu;
mod microphone;
mod pairing;
mod session;
pub mod sound;

use std::sync::{Arc, Mutex};

pub use session::Config;

use smithay::reexports::wayland_server::DisplayHandle;
use spatiand_shell::{HostRow, HostStatus, HostTab, PairingView, RemoteEntry, Shell};

use crate::state::ClientState;

/// What this session knows about one host right now. Written by the host's thread, read by
/// the compositor to fill in the settings page and the launcher.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostView {
    /// What the host calls itself, once it has said.
    pub name: Option<String>,
    pub link: Link,
    /// What it offers, as of the last time it said.
    pub apps: Vec<RemoteApp>,
    /// A game there asked the pad to rumble, and nothing has played it yet. Taken by the
    /// compositor, which owns the only motors.
    pub rumble: Option<(u16, u16)>,
    /// What this host has said about the clipboard and nothing has acted on yet. A queue
    /// rather than the latest, because an announcement, a paste's question and its answer are
    /// three different things and losing any of them loses a paste.
    pub clipboard: Vec<spatiand_stream::control::Clipboard>,
    /// Connected, but nothing at all has come from the host for a while. See `QUIET_AFTER` in
    /// `session`: an application that is the room is hidden while this is true.
    pub quiet: bool,
    /// The host is locked, and shows nothing of its windows until it is unlocked there: how it
    /// is unlocked, in its own words ("Touch ID or its password"). See `spatiand_stream::Held`.
    pub held: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Link {
    #[default]
    Connecting,
    Online,
    Offline(String),
    Refused(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteApp {
    pub id: String,
    pub name: String,
    /// The icon the host sent, saved to a file. Its name changes when the picture does, so a
    /// replaced icon is loaded afresh rather than found in the icon cache under the old name.
    pub icon: Option<String>,
}

/// Where the icons hosts send are kept.
fn icon_dir() -> std::path::PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("spatiand")
        .join("remote-icons")
}

/// The app id a remote window is given: `remote.<host>.<app>`. One definition, because the
/// controller layouts, the title bar's icon and the window itself all key on it.
pub fn app_id(host: &str, app: &str) -> String {
    format!("{}{app}", app_id_prefix(host))
}

fn app_id_prefix(host: &str) -> String {
    format!("remote.{host}.")
}

/// How many pixels of each host's pictures make one of its points, as it last said
/// (`HostMessage::Density`). One for the host, not each window: it is the host's screen's, and
/// a host with screens of two densities is not one this has met.
static DENSITY: std::sync::Mutex<Option<std::collections::HashMap<String, f64>>> = std::sync::Mutex::new(None);

fn density_said(host: &str, pixels_per_point: f64) {
    if pixels_per_point.is_finite() && pixels_per_point > 0.1 {
        if let Ok(mut d) = DENSITY.lock() {
            d.get_or_insert_with(std::collections::HashMap::new).insert(host.to_string(), pixels_per_point);
        }
    }
}

/// Pixels to a point of a window whose application is `app_id`, when it is a host's and the
/// host has said.
pub fn pixels_per_point(app_id: &str) -> Option<f64> {
    let d = DENSITY.lock().ok()?;
    d.as_ref()?.iter().find(|(host, _)| app_id.starts_with(&app_id_prefix(host))).map(|(_, p)| *p)
}

/// How wide each remote application's sound is, by app id, as its host's catalogue said.
fn sound_widths() -> &'static std::sync::Mutex<std::collections::HashMap<String, u16>> {
    static WIDTHS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, u16>>> =
        std::sync::OnceLock::new();
    WIDTHS.get_or_init(Default::default)
}

/// Remember how many channels a host's application sends. Called as its catalogue arrives, which
/// is before any of its windows do.
pub fn set_sound_channels(app_id: &str, channels: u16) {
    if let Ok(mut widths) = sound_widths().lock() {
        widths.insert(app_id.to_string(), channels);
    }
}

/// The width of sink a remote application's window should get.
///
/// **Stereo unless its host's catalogue asked for a surround layout.** A remote application's
/// sound is placed at its window by its sink, so a stereo one is given the stereo sink it always
/// had, and a VR viewer -- which plays its own head-turned mix and is not placed at all -- never
/// reaches here. Only an application set to 5.1, 7.1 or 7.1.4 is given the full sink.
pub fn sound_width(app_id: &str) -> spatiand_audio::server::Width {
    let channels = sound_widths()
        .lock()
        .ok()
        .and_then(|w| w.get(app_id).copied())
        .unwrap_or(2);
    if channels > 2 {
        spatiand_audio::server::Width::Full
    } else {
        spatiand_audio::server::Width::Stereo
    }
}

/// A remote window's icon, for its title bar: the latest picture its host sent for it.
pub fn icon_for(app_id: &str) -> Option<std::path::PathBuf> {
    let path = icon_dir().join(format!("{}.png", file_safe(app_id)));
    path.exists().then_some(path)
}

/// Keep an icon a host sent -- or, on the Beam Pro, an Android app's (`spatiand-android`'s `apps`).
/// Returns the path the launcher should load it from.
pub(crate) fn store_icon(app_id: &str, png: &[u8]) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let dir = icon_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    png.hash(&mut hasher);
    let name = file_safe(app_id);
    let versioned = dir.join(format!("{name}-{:016x}.png", hasher.finish()));
    if !versioned.exists() {
        std::fs::write(&versioned, png).ok()?;
    }
    // And under the plain name, for the title bar, which only knows the app id.
    let _ = std::fs::write(dir.join(format!("{name}.png")), png);
    Some(versioned.to_string_lossy().to_string())
}

fn file_safe(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect()
}

/// Something asked of a host's thread.
#[derive(Debug)]
pub enum Command {
    Launch(String),
    /// Kill an application on the host, by catalogue id.
    ForceQuit(String),
    /// The whole state of the gamepad, for whatever is being played there.
    Pad(spatiand_stream::Pad),
    /// Something about the clipboard: what has been copied, a paste asking for bytes, or the
    /// bytes themselves. See `crate::clipboard`.
    Clipboard(spatiand_stream::control::Clipboard),
    /// Where the head is now. Sent as a datagram, never down the ordered stream.
    Viewport(spatiand_stream::Viewport),
    /// The glasses have come on, or off: an application that can be a room or a window is
    /// whichever suits.
    Glasses(bool),
    /// Show a window the host has offered.
    Show(u32),
    /// The wearer is using a window's application menus. See [`menu`].
    Menu(spatiand_stream::MenuAsk),
    /// The wearer pressed the card this host's notification is on.
    NoticePress,
}

/// Windows that hosts have open and are not showing: which host, its number there, what it is
/// called, and a number of this session's own for the list of windows.
static OFFERED: std::sync::Mutex<Vec<(String, u32, String, usize, bool)>> = std::sync::Mutex::new(Vec::new());
/// Clear of anything the compositor numbers its own windows with.
const OFFERED_FROM: usize = 1 << 40;

/// Whether what hosts offer has changed since the list of windows was last made from it.
static OFFERED_CHANGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn offered_said(host: &str, window: u32, title: String, screen: bool) {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(OFFERED_FROM);
    let mut offered = OFFERED.lock().unwrap();
    match offered.iter_mut().find(|o| o.0 == host && o.1 == window) {
        Some(known) if known.2 == title => return,
        Some(known) => known.2 = title,
        None => offered.push((host.to_string(), window, title, NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed), screen)),
    }
    OFFERED_CHANGED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Shown now, or closed there: off the list either way. `None` is every window of that host's.
pub(crate) fn offered_gone(host: &str, window: Option<u32>) {
    let mut offered = OFFERED.lock().unwrap();
    let before = offered.len();
    offered.retain(|o| o.0 != host || window.is_some_and(|w| o.1 != w));
    if offered.len() != before {
        OFFERED_CHANGED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Whether what hosts offer has changed since this was last asked (or since the list was last
/// made, [`with_offered`]). **The list of windows on show is made again when it has**: a host
/// that went away while the list was open took its windows off it only at the next opening, so
/// the wearer could choose one nobody was there to show; and one just unlocked, or just
/// connected to, offered its windows to a list that went on showing none.
pub fn offered_changed() -> bool {
    OFFERED_CHANGED.swap(false, std::sync::atomic::Ordering::Relaxed)
}

/// The compositor's list of windows, with what hosts are offering after it: put away, to be
/// asked for like any hidden window.
pub fn with_offered(mut windows: Vec<spatiand_shell::WindowEntry>) -> Vec<spatiand_shell::WindowEntry> {
    OFFERED_CHANGED.store(false, std::sync::atomic::Ordering::Relaxed);
    for (_, _, title, id, screen) in OFFERED.lock().unwrap().iter() {
        windows.push(spatiand_shell::WindowEntry {
            id: *id,
            title: title.clone(),
            hidden: true,
            screen: *screen,
            ..Default::default()
        });
    }
    windows
}

/// The last notification a host sent, which host, and when: shown for a glance and then let go of.
static HOST_NOTICE: std::sync::Mutex<Option<HostNotice>> = std::sync::Mutex::new(None);

struct HostNotice {
    /// The host it is from, by the name it was added as; empty for a word of the room's own
    /// about a host (that it is unlocked), which pressing does nothing to.
    host: String,
    text: String,
    at: std::time::Instant,
    lasts: std::time::Duration,
}

/// How long a host's notification stays on the card unless the host takes it away first (it
/// does, after its own eight seconds).
const NOTICE_FOR: std::time::Duration = std::time::Duration::from_secs(9);
/// How long the room's own last word about a host stays.
const WORD_FOR: std::time::Duration = std::time::Duration::from_secs(6);

fn notice_slot() -> std::sync::MutexGuard<'static, Option<HostNotice>> {
    HOST_NOTICE.lock().unwrap_or_else(|e| e.into_inner())
}

/// A host's notification, or (empty) its going. One from a host that is locked is passed over:
/// a host that also says it is locked in a notice, for a session older than
/// [`spatiand_stream::Held`], is not shown twice.
pub(crate) fn host_notice_said(host: &str, text: String) {
    if !text.is_empty() && held().iter().any(|h| h.host == host) {
        return;
    }
    let mut notice = notice_slot();
    if text.is_empty() {
        // Only its own is taken away: not another host's, and not the room's word about one.
        if notice.as_ref().is_some_and(|n| n.host == host) {
            *notice = None;
        }
    } else {
        log::info!("remote {host}: a notification there, shown on the card");
        *notice = Some(HostNotice { host: host.to_string(), text, at: std::time::Instant::now(), lasts: NOTICE_FOR });
    }
}

/// The room's own word about a host, for a few seconds.
fn word(text: String) {
    *notice_slot() = Some(HostNotice { host: String::new(), text, at: std::time::Instant::now(), lasts: WORD_FOR });
}

/// The room's own word about itself, on the same card for the same few seconds, on every
/// platform: a recording that stopped by itself, and why.
pub fn room_word(text: String) {
    word(text)
}

/// What the card says of the hosts, if anything: that one the wearer asked something of is
/// locked, for as long as it is waited for; else what one has just notified of, if it is recent
/// enough to be worth a glance.
pub fn host_notice() -> String {
    if let Some(waiting) = waiting_words(std::time::Instant::now()) {
        return waiting;
    }
    notice_slot().as_ref().filter(|n| n.at.elapsed() < n.lasts).map(|n| n.text.clone()).unwrap_or_default()
}

/// The host whose notification is on the card now, for a press on it.
fn notice_from() -> Option<String> {
    if waiting_words(std::time::Instant::now()).is_some() {
        return None;
    }
    notice_slot().as_ref().filter(|n| n.at.elapsed() < n.lasts && !n.host.is_empty()).map(|n| n.host.clone())
}

// --- a host that is locked ---

/// A host that has said it is locked ([`spatiand_stream::Held`]).
#[derive(Debug, Clone)]
struct HeldHost {
    /// What it was added as.
    host: String,
    /// What it calls itself.
    name: String,
    /// How it is unlocked there, in its own words.
    unlock: String,
    /// When the wearer last asked something of it that it cannot do until it is unlocked: one
    /// of its applications, one of its windows, or to be connected to at all. From then the card
    /// says what is waited for; a host that is locked and not being asked anything says nothing.
    asked: Option<std::time::Instant>,
}

/// How long the card says a locked host is waited for. After it the card says once that it is
/// still locked and goes: the wait is not over (what was asked for still opens when the host is
/// unlocked, and that is still said), but nobody looks at a card for longer than this.
const WAIT_FOR: std::time::Duration = std::time::Duration::from_secs(120);
/// How long before a host says it is locked an asking still counts as what is waited for:
/// connecting to it and being told takes a moment.
const ASKED_WITHIN: std::time::Duration = std::time::Duration::from_secs(20);

static HELD: std::sync::Mutex<Vec<HeldHost>> = std::sync::Mutex::new(Vec::new());
/// The host the wearer last asked something of, and when.
static LAST_ASKED: std::sync::Mutex<Option<(String, std::time::Instant)>> = std::sync::Mutex::new(None);

fn held() -> std::sync::MutexGuard<'static, Vec<HeldHost>> {
    HELD.lock().unwrap_or_else(|e| e.into_inner())
}

/// The wearer asked something of `host`: if it is locked, that is what is waited for from now.
pub(crate) fn asked_of(host: &str) {
    let now = std::time::Instant::now();
    *LAST_ASKED.lock().unwrap_or_else(|e| e.into_inner()) = Some((host.to_string(), now));
    if let Some(h) = held().iter_mut().find(|h| h.host == host) {
        h.asked = Some(now);
    }
}

/// `host` says it is locked (`Some`: how it is unlocked) or no longer (`None`).
pub(crate) fn held_said(host: &str, name: &str, unlock: Option<String>) {
    let mut all = held();
    match unlock {
        Some(unlock) => {
            let asked = LAST_ASKED
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .filter(|(h, at)| h == host && at.elapsed() < ASKED_WITHIN)
                .map(|(_, at)| *at);
            match all.iter_mut().find(|h| h.host == host) {
                Some(h) => {
                    h.name = name.to_string();
                    h.unlock = unlock;
                    h.asked = h.asked.or(asked);
                }
                None => all.push(HeldHost { host: host.to_string(), name: name.to_string(), unlock, asked }),
            }
            drop(all);
            log::info!("remote {host}: it is locked; its windows wait until it is unlocked there");
            // Its notice of the same, from a host that says both, is not also shown.
            let mut notice = notice_slot();
            if notice.as_ref().is_some_and(|n| n.host == host) {
                *notice = None;
            }
        }
        None => {
            let was = all.iter().position(|h| h.host == host).map(|i| all.remove(i));
            drop(all);
            // **Said, when it was being waited for**: the finger was on a sensor in another
            // room, and nothing here said it had been taken.
            if let Some(was) = was {
                log::info!("remote {host}: unlocked");
                if was.asked.is_some() {
                    word(format!("{} is unlocked\nWhat you asked of it opens here now", was.name));
                }
            }
        }
    }
}

/// `host`'s link has gone: it is not waited for any more, and that is said if it was.
pub(crate) fn held_gone(host: &str) {
    let mut all = held();
    let was = all.iter().position(|h| h.host == host).map(|i| all.remove(i));
    drop(all);
    if let Some(HeldHost { name, asked: Some(at), .. }) = was {
        if at.elapsed() < WAIT_FOR {
            word(format!("{name} is no longer connected\nIt was locked, and nothing of it was shown"));
        }
    }
}

/// What the card says while a locked host is waited for.
fn waiting_words(now: std::time::Instant) -> Option<String> {
    let all = held();
    // The one asked last.
    let h = all.iter().filter(|h| h.asked.is_some()).max_by_key(|h| h.asked)?;
    let waited = now.saturating_duration_since(h.asked?);
    if waited < WAIT_FOR {
        Some(format!("{} is locked\nUnlock it with {}: what you asked of it opens here by itself", h.name, h.unlock))
    } else if waited < WAIT_FOR + WORD_FOR {
        Some(format!("{} is still locked\nNothing of it is shown here until it is unlocked there", h.name))
    } else {
        None
    }
}

/// Why a host that is only connected to when the wearer asks is not connected yet.
pub(crate) const NOT_ASKED_FOR: &str = "connects when you use it";

/// What a host's tab in the launcher says after its name, from its link and whether it is locked.
fn tab_note(link: &Link, held: Option<&str>) -> String {
    match (link, held) {
        (Link::Online, Some(unlock)) => format!("locked: unlock it with {unlock}"),
        (Link::Online, None) => String::new(),
        (Link::Connecting, _) => "connecting\u{2026}".into(),
        // Not asked for yet: its one bubble says what to do.
        (Link::Offline(reason), _) if reason == NOT_ASKED_FOR => String::new(),
        (Link::Offline(reason) | Link::Refused(reason), _) => reason.clone(),
    }
}

/// Whether this session has glasses on, as last noticed: what a host is told when it is
/// connected to, before anything is started there.
pub static GLASSES_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// A host being shown in this session.
pub struct Remote {
    /// Kept so the thread can be asked to stop; dropping it closes the connection and the
    /// windows go with it.
    stop: Arc<std::sync::atomic::AtomicBool>,
    /// What it was added as. The key it is known by everywhere.
    pub host: String,
    view: Arc<Mutex<HostView>>,
    commands: tokio::sync::mpsc::UnboundedSender<Command>,
}

impl Remote {
    /// Connect to a host and show whatever it opens.
    ///
    /// Returns as soon as the thread is running: connecting, pairing failures and everything
    /// else are reported in the log rather than here, because a host that is asleep is a normal
    /// thing for a session to start with.
    pub fn start(display: &mut DisplayHandle, config: Config) -> Result<Remote, String> {
        let (server, client) = std::os::unix::net::UnixStream::pair()
            .map_err(|e| format!("could not make a socket pair: {e}"))?;
        display
            .insert_client(server, Arc::new(ClientState::default()))
            .map_err(|e| format!("could not attach the remote client: {e}"))?;

        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let host = config.host.clone();
        let view: Arc<Mutex<HostView>> = Arc::default();
        let (commands, inbox) = tokio::sync::mpsc::unbounded_channel();
        {
            let stop = stop.clone();
            let view = view.clone();
            std::thread::Builder::new()
                .name(format!("remote-{host}"))
                .spawn(move || {
                    // A panic here closes this host's windows and says why. It must never take
                    // the session with it: the wearer is wearing the session.
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        session::run(client, config, stop, view, inbox);
                    }));
                    if let Err(panic) = result {
                        let what = panic
                            .downcast_ref::<&str>()
                            .map(|s| s.to_string())
                            .or_else(|| panic.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "something unprintable".into());
                        log::error!("the remote session died: {what}");
                    }
                })
                .map_err(|e| format!("could not start the remote thread: {e}"))?;
        }
        menu::host_up(&host, commands.clone());
        Ok(Remote {
            stop,
            host,
            view,
            commands,
        })
    }

    pub fn view(&self) -> HostView {
        self.view.lock().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn launch(&self, app: &str) {
        asked_of(&self.host);
        let _ = self.commands.send(Command::Launch(app.to_string()));
    }

    pub fn force_quit(&self, app: &str) {
        let _ = self.commands.send(Command::ForceQuit(app.to_string()));
    }

    fn pad(&self, state: spatiand_stream::Pad) {
        let _ = self.commands.send(Command::Pad(state));
    }

    fn glasses(&self, on: bool) {
        let _ = self.commands.send(Command::Glasses(on));
    }

    fn show(&self, window: u32) {
        asked_of(&self.host);
        let _ = self.commands.send(Command::Show(window));
    }

    fn notice_press(&self) {
        let _ = self.commands.send(Command::NoticePress);
    }

    fn viewport(&self, viewport: spatiand_stream::Viewport) {
        let _ = self.commands.send(Command::Viewport(viewport));
    }

    /// Tell this host something about the clipboard.
    fn clipboard(&self, what: spatiand_stream::control::Clipboard) {
        let _ = self.commands.send(Command::Clipboard(what));
    }

    /// Whatever it has said about the clipboard since the last look.
    fn take_clipboard(&self) -> Vec<spatiand_stream::control::Clipboard> {
        self.view
            .lock()
            .map(|mut v| std::mem::take(&mut v.clipboard))
            .unwrap_or_default()
    }

    /// Whether an application id belongs to this host.
    fn owns(&self, app_id: &str) -> bool {
        app_id.starts_with(&app_id_prefix(&self.host))
    }

    /// A game there asked for rumble, if one has since the last look.
    fn take_rumble(&self) -> Option<(u16, u16)> {
        self.view.lock().ok()?.rumble.take()
    }
}

/// What a row of another of the owner's devices is known by on the remote computers page.
const DEVICE_ROW: &str = "device:";

fn config_for(entry: &crate::prefs::RemoteHost, microphone: bool) -> Result<Config, String> {
    Ok(Config {
        host: entry.host.clone(),
        fingerprint: entry.fingerprint.parse()?,
        identity_dir: spatiand_track::config::config_dir(),
        render_node: std::env::var("SPATIAND_RENDER_NODE")
            .unwrap_or_else(|_| "/dev/dri/renderD128".into()),
        launch: entry.launch.clone(),
        microphone,
        on_demand: entry.on_demand,
    })
}

/// Every host this session knows, a pairing if one is under way, and the bookkeeping that
/// keeps the settings page and the launcher in step with them.
///
/// The compositor's whole involvement is [`tick`](Self::tick) once a frame and the handful
/// of calls that answer the shell's events.
#[derive(Default)]
pub struct Remotes {
    hosts: Vec<Remote>,
    /// Which host was last given the pad, and what it was given, so an unchanged pad costs
    /// nothing and a host that loses focus is told to let go exactly once.
    padded: Option<(String, spatiand_stream::Pad)>,
    /// Whether this session sends its microphone to a host that asks. From the preferences,
    /// once, at startup.
    microphone: bool,
    /// This device's fingerprint.
    me: String,
    /// The owner's other devices that are not hosts here, to be forgotten by name: fingerprint, name.
    devices: Vec<(String, String)>,
    pairing: Option<PairingInProgress>,
    /// The owner's devices, as they change. See `spatiand_sync::roster`.
    roster: Option<std::sync::mpsc::Receiver<Vec<spatiand_sync::roster::Entry>>>,
    /// What the shell was last given, so it is only given something when it changed.
    shown: Option<(Vec<HostRow>, Vec<HostTab>)>,
    shown_pairing: Option<PairingView>,
    /// Counts viewports sent, so a picture can say which one it was drawn for.
    viewport_seq: u32,
    /// This machine's own clipboard, where it is not the session's: a Mac's pasteboard. A peer
    /// of the session's clipboard as a host is, by name.
    pub local_clipboard: Option<LocalClipboard>,
}

/// A clipboard of this machine's that the session's is to be kept in step with, as a host's is:
/// what it says is taken as a host's saying, and what the session has to say to everyone is
/// said to it too. See `crate::clipboard`.
pub struct LocalClipboard {
    pub name: &'static str,
    /// What it has said since the last look.
    pub take: fn() -> Vec<spatiand_stream::control::Clipboard>,
    pub say: fn(spatiand_stream::control::Clipboard),
}

struct PairingInProgress {
    pairing: pairing::Pairing,
    /// Over: kept only so the page can say how it ended.
    finished: Option<Result<String, String>>,
}

impl Remotes {
    /// Start every host in the preferences.
    ///
    /// A host that is asleep, unreachable or has not paired with this session costs one line
    /// in the log and nothing else — the session carries on, and keeps trying.
    pub fn start(display: &mut DisplayHandle, prefs: &crate::prefs::Prefs) -> Remotes {
        let mut remotes = Remotes {
            microphone: prefs.remote_microphone,
            ..Remotes::default()
        };
        let dir = spatiand_track::config::config_dir();
        let identity = spatiand_stream::Identity::load_or_create(&dir).ok();
        if let Some(me) = &identity {
            remotes.me = me.fingerprint().to_string();
        }
        for entry in &prefs.remotes {
            remotes.add(display, entry);
        }
        if let Some(me) = identity {
            let fp = me.fingerprint().to_string();
            let name = crate::system::machine_name();
            // Read again only when the file has changed: reading it says so in the log.
            let prefs_file = dir.join("prefs.toml");
            let mut read: Option<(Option<std::time::SystemTime>, Vec<(String, String)>)> = None;
            remotes.roster = Some(spatiand_sync::roster::Roster::watch(move || {
                spatiand_sync::roster::Roster::open_default().note(&fp, &name, "", true);
                let changed = std::fs::metadata(&prefs_file).and_then(|m| m.modified()).ok();
                if read.as_ref().is_none_or(|(when, _)| *when != changed) {
                    let hosts = crate::prefs::Prefs::load().remotes.into_iter().map(|r| (r.fingerprint, r.host)).collect();
                    read = Some((changed, hosts));
                }
                read.as_ref().map(|(_, hosts)| hosts.clone()).unwrap_or_default()
            }));
        }
        remotes
    }

    /// Whether a host on the list is this machine itself: a Mac is a host and a room under one
    /// identity, and a list of hosts copied from elsewhere (the Mac app's paired computers) can
    /// name it. It is not a remote of its own: its own host refuses it, and it would be shown
    /// as one more computer, offline.
    fn is_me(&self, entry: &crate::prefs::RemoteHost) -> bool {
        !self.me.is_empty() && entry.fingerprint.eq_ignore_ascii_case(&self.me)
    }

    fn add(&mut self, display: &mut DisplayHandle, entry: &crate::prefs::RemoteHost) {
        if self.is_me(entry) {
            log::info!("remote {}: this machine itself, not a remote of its own", entry.host);
            return;
        }
        let config = match config_for(entry, self.microphone) {
            Ok(config) => config,
            Err(e) => {
                log::error!("remote {}: {e}", entry.host);
                return;
            }
        };
        match Remote::start(display, config) {
            Ok(remote) => {
                log::info!("remote: watching {}", entry.host);
                self.hosts.push(remote);
            }
            Err(e) => log::error!("remote {}: {e}", entry.host),
        }
    }

    /// Start an application the launcher asked for.
    pub fn launch(&self, host: &str, app: &str) {
        match self.hosts.iter().find(|r| r.host == host) {
            Some(remote) => remote.launch(app),
            None => log::warn!("remote: no host called {host} to start {app} on"),
        }
    }

    /// Kill the application behind a remote window, named by the window's app id.
    pub fn force_quit(&self, app_id: &str) {
        for remote in &self.hosts {
            if let Some(app) = app_id.strip_prefix(&app_id_prefix(&remote.host)) {
                remote.force_quit(app);
                return;
            }
        }
        log::warn!("no host for {app_id}");
    }

    /// Pair with the host at `address`, proving `code`: the digits it shows, as the wearer typed them.
    pub fn pair(&mut self, address: String, code: String) {
        log::info!("pairing with {address}");
        self.pairing = Some(PairingInProgress {
            pairing: pairing::Pairing::start(address, code, spatiand_track::config::config_dir()),
            finished: None,
        });
    }

    pub fn cancel(&mut self) {
        if self.pairing.take().is_some() {
            log::info!("pairing cancelled");
        }
    }

    /// Stop knowing a host: stop showing it, and take it out of the preferences.
    pub fn forget(&mut self, address: &str, prefs: &mut crate::prefs::Prefs) {
        if let Some(fp) = address.strip_prefix(DEVICE_ROW) {
            match spatiand_sync::roster::Roster::open_default().remove(fp) {
                Ok(e) => log::info!("forgot {} on every machine", e.label()),
                Err(e) => log::warn!("{e}"),
            }
            self.devices.retain(|(known, _)| known != fp);
            return;
        }
        // From every machine of the owner's, as the host is.
        if let Some(entry) = prefs.remotes.iter().find(|r| r.host == address) {
            let _ = spatiand_sync::roster::Roster::open_default().remove(&entry.fingerprint);
        }
        // Dropping the handle stops its thread, and its windows go with the connection.
        self.hosts.retain(|r| r.host != address);
        prefs.remotes.retain(|r| r.host != address);
        prefs.save();
        log::info!("forgot {address}");
    }

    /// Hand the gamepad to whichever host owns the window in front of the wearer.
    ///
    /// Called every frame with the focused application's id, or `None` for a local one.
    /// Whatever the mapper made of the wearer's controller — thumbsticks, the head on the
    /// spare axes, a Bluetooth pad — is the same report a local game would be given, so a
    /// remote application is played exactly as a local one is, including its own layout.
    ///
    /// Sent only when it differs from the last one, and a host losing focus is told the pad
    /// is at rest: a stick left pushed over walks an avatar into a wall.
    pub fn pad(&mut self, focused: Option<&str>, state: spatiand_stream::Pad) {
        let now = focused
            .filter(|app_id| self.hosts.iter().any(|h| h.owns(app_id)))
            .map(|app_id| app_id.to_string());
        let was = self.padded.as_ref().map(|(host, _)| host.clone());
        if was != now {
            // Whoever had it, let go.
            if let Some(app_id) = was {
                if let Some(host) = self.hosts.iter().find(|h| h.owns(&app_id)) {
                    host.pad(spatiand_stream::Pad::default());
                }
            }
            self.padded = now.clone().map(|app_id| (app_id, spatiand_stream::Pad::default()));
        }
        let Some(app_id) = now else { return };
        if self.padded.as_ref().map(|(_, last)| *last) == Some(state) {
            return;
        }
        if let Some(host) = self.hosts.iter().find(|h| h.owns(&app_id)) {
            host.pad(state);
        }
        self.padded = Some((app_id, state));
    }

    /// Tell every host where the head is. Called once a frame, while a head is being tracked.
    ///
    /// Every host, not only the focused one: an application that takes the view has to keep
    /// following the head while the wearer is typing into something else, or the world would
    /// freeze the moment a window in front of it got focus. It is a hundred-odd bytes a frame,
    /// unreliable, and a host with nothing that asked for poses simply writes it into a ring
    /// nobody is reading.
    /// The wearer chose something in the list of windows: if it is a window a host is offering,
    /// ask that host to show it, and say so.
    pub fn show_offered(&mut self, id: usize) -> bool {
        if id < OFFERED_FROM {
            return false;
        }
        let found = OFFERED.lock().unwrap().iter().find(|o| o.3 == id).map(|o| (o.0.clone(), o.1));
        let Some((host, window)) = found else { return false };
        for remote in self.hosts.iter().filter(|r| r.host == host) {
            remote.show(window);
        }
        true
    }

    /// Whether a host's notification is on the card now, so that a press on the card is its.
    pub fn notice_showing(&self) -> bool {
        notice_from().is_some()
    }

    /// The card a host's notification is on was pressed: the host is told, and does what
    /// pressing it there does (a Mac presses the banner, and shows the session what that
    /// opens); the card goes.
    pub fn notice_pressed(&mut self) {
        let Some(host) = notice_from() else { return };
        log::info!("remote {host}: its notification was pressed");
        for remote in self.hosts.iter().filter(|r| r.host == host) {
            remote.notice_press();
        }
        *notice_slot() = None;
    }

    /// **Every host is told whenever the glasses come on or go off**, not only when it is
    /// connected to: an application there that can draw a room for a head, or an ordinary window
    /// for nobody's, changes over while it runs (`set_glasses` on its control channel). Called
    /// every frame; says something only when it has changed.
    pub fn glasses(&mut self, on: bool) {
        if GLASSES_ON.swap(on, std::sync::atomic::Ordering::Relaxed) == on {
            return;
        }
        log::info!("remote: telling every host the glasses are {}", if on { "on" } else { "off" });
        for host in &self.hosts {
            host.glasses(on);
        }
    }

    pub fn viewport(&mut self, slot: &spatiand_proto::pose::Slot, render_size: (u32, u32)) {
        if self.hosts.is_empty() {
            return;
        }
        self.viewport_seq = self.viewport_seq.wrapping_add(1);
        let viewport = crate::pose::viewport(slot, self.viewport_seq, render_size);
        for host in &self.hosts {
            host.viewport(viewport);
        }
    }

    /// What a game on any host has asked the motors to do since the last look.
    pub fn rumble(&self) -> Option<(u16, u16)> {
        self.hosts.iter().find_map(|host| host.take_rumble())
    }

    /// Everything the hosts have said about the clipboard since the last look, each with the
    /// host that said it.
    pub fn clipboard_said(&self) -> Vec<(String, spatiand_stream::control::Clipboard)> {
        let mut said: Vec<_> = self
            .hosts
            .iter()
            .flat_map(|host| {
                host.take_clipboard()
                    .into_iter()
                    .map(|what| (host.host.clone(), what))
            })
            .collect();
        if let Some(local) = &self.local_clipboard {
            said.extend((local.take)().into_iter().map(|what| (local.name.to_string(), what)));
        }
        said
    }

    /// Pass on what the session's clipboard has to say.
    pub fn clipboard_say(&self, say: crate::clipboard::Say) {
        match say {
            crate::clipboard::Say::Everyone { except, what } => {
                for host in &self.hosts {
                    if Some(&host.host) == except.as_ref() {
                        continue;
                    }
                    host.clipboard(what.clone());
                }
                if let Some(local) = self.local_clipboard.as_ref().filter(|l| except.as_deref() != Some(l.name)) {
                    (local.say)(what);
                }
            }
            crate::clipboard::Say::Just { host, what } => {
                if let Some(remote) = self.hosts.iter().find(|h| h.host == host) {
                    remote.clipboard(what);
                } else if let Some(local) = self.local_clipboard.as_ref().filter(|l| l.name == host) {
                    (local.say)(what);
                }
            }
        }
    }

    /// The app-id prefixes of hosts that have gone quiet, for [`crate::state::Spatiand::quiet_hosts`].
    pub fn quiet(&self) -> Vec<String> {
        self.hosts
            .iter()
            // Read in place: `view()` copies the catalogue, and this is asked every frame.
            .filter(|host| host.view.lock().is_ok_and(|v| v.quiet))
            .map(|host| app_id_prefix(&host.host))
            .collect()
    }

    /// Which hosts are no longer connected, so the board can drop what they were holding.
    pub fn offline(&self) -> Vec<String> {
        self.hosts
            .iter()
            .filter(|host| !matches!(host.view().link, Link::Online))
            .map(|host| host.host.clone())
            .collect()
    }

    /// Once a frame: finish a pairing that both ends proved, and tell the shell anything
    /// that changed.
    pub fn tick(
        &mut self,
        display: &mut DisplayHandle,
        prefs: &mut crate::prefs::Prefs,
        shell: &mut Shell,
    ) {
        self.settle_pairing(display, prefs);
        self.follow_roster(display, prefs);

        let (rows, tabs) = self.describe(prefs);
        if self.shown.as_ref() != Some(&(rows.clone(), tabs.clone())) {
            shell.set_hosts(rows.clone(), tabs.clone());
            self.shown = Some((rows, tabs));
        }
        if let Some(view) = self.pairing_view() {
            if self.shown_pairing.as_ref() != Some(&view) {
                shell.set_pairing(view.clone());
                self.shown_pairing = Some(view);
            }
        }
    }

    /// A host on the owner's list is one this device can use, though it was paired from another;
    /// one taken off the list, or this device taken off, is gone.
    fn follow_roster(&mut self, display: &mut DisplayHandle, prefs: &mut crate::prefs::Prefs) {
        let Some(entries) = self.roster.as_ref().and_then(|r| r.try_iter().last()) else { return };
        let i_am_gone = entries.iter().any(|e| e.fp == self.me && e.removed());
        self.devices = entries
            .iter()
            .filter(|e| !e.removed() && e.fp != self.me && e.address.is_empty())
            .map(|e| (e.fp.clone(), e.label()))
            .collect();
        for e in &entries {
            let known = prefs.remotes.iter().find(|r| r.fingerprint.eq_ignore_ascii_case(&e.fp)).map(|r| r.host.clone());
            match known {
                Some(address) if e.removed() || i_am_gone => {
                    log::info!("{} was removed from the owner's devices", e.label());
                    self.hosts.retain(|r| r.host != address);
                    prefs.remotes.retain(|r| r.host != address);
                    prefs.save();
                }
                None if !e.removed() && !i_am_gone && !e.address.is_empty() && e.fp != self.me => {
                    log::info!("{} is on the owner's list: its applications can be used here", e.label());
                    let entry = crate::prefs::RemoteHost {
                        host: e.address.clone(),
                        fingerprint: e.fp.clone(),
                        launch: Vec::new(),
                        on_demand: true,
                    };
                    prefs.remotes.push(entry.clone());
                    prefs.save();
                    self.add(display, &entry);
                }
                _ => {}
            }
        }
    }

    fn settle_pairing(&mut self, display: &mut DisplayHandle, prefs: &mut crate::prefs::Prefs) {
        let Some(p) = self.pairing.as_mut() else { return };
        if p.finished.is_some() {
            return;
        }
        let paired = match p.pairing.state().outcome {
            None => return,
            Some(Err(e)) => {
                p.finished = Some(Err(e));
                return;
            }
            // Both ends proved the code, with this connection's certificates bound in: this is
            // the host whose screen the wearer read it from. Only now is it written down.
            Some(Ok(paired)) => paired,
        };
        let address = p.pairing.address.clone();
        p.finished = Some(Ok(paired.name.clone()));

        let entry = crate::prefs::RemoteHost {
            host: address.clone(),
            fingerprint: paired.fingerprint.to_string(),
            launch: Vec::new(),
            on_demand: false,
        };
        spatiand_sync::roster::Roster::open_default().pair(&entry.fingerprint, &paired.name, &address);
        // Pairing again with a host already known replaces it, which is what pairing again
        // after the host forgot this headset needs; and the same host paired again at another
        // address (its name changed, or reached by its IP address) is the one host, not two.
        let replaced: Vec<String> = prefs
            .remotes
            .iter()
            .filter(|r| same_host(r, &address, &entry.fingerprint))
            .map(|r| r.host.clone())
            .collect();
        self.hosts.retain(|r| r.host != address && !replaced.contains(&r.host));
        prefs.remotes.retain(|r| !same_host(r, &address, &entry.fingerprint));
        prefs.remotes.push(entry.clone());
        prefs.save();
        log::info!("paired with {} at {address}", paired.name);
        self.add(display, &entry);
    }

    fn pairing_view(&self) -> Option<PairingView> {
        let p = self.pairing.as_ref()?;
        let state = p.pairing.state();
        let address = p.pairing.address.clone();
        let (status, finished, succeeded) = match &p.finished {
            Some(Ok(name)) => (
                format!("Paired with {name}. Its apps are in the launcher."),
                true,
                true,
            ),
            Some(Err(e)) => (format!("{}. Press A to type the code again.", e.trim_end_matches('.')), true, false),
            None if state.fingerprint.is_none() => (format!("Looking for {address}…"), false, false),
            None => (format!("Proving the code with {address}…"), false, false),
        };
        Some(PairingView {
            address,
            status,
            finished,
            succeeded,
        })
    }

    /// The settings page's rows and the launcher's tabs, in the order the hosts were added.
    fn describe(&self, prefs: &crate::prefs::Prefs) -> (Vec<HostRow>, Vec<HostTab>) {
        let mut rows = Vec::new();
        let mut tabs = Vec::new();
        for entry in prefs.remotes.iter().filter(|e| !self.is_me(e)) {
            let view = self
                .hosts
                .iter()
                .find(|r| r.host == entry.host)
                .map(|r| r.view())
                .unwrap_or_default();
            let label = view.name.clone().unwrap_or_else(|| entry.host.clone());
            let status = match view.link {
                Link::Online => HostStatus::Online,
                Link::Connecting => HostStatus::Connecting,
                Link::Offline(_) => HostStatus::Offline,
                Link::Refused(_) => HostStatus::Refused,
            };
            rows.push(HostRow {
                label: label.clone(),
                address: entry.host.clone(),
                status,
            });
            tabs.push(HostTab {
                label,
                address: entry.host.clone(),
                online: status == HostStatus::Online,
                // What is in the way of its applications, if anything: said after its name
                // while its tab is open, for as long as it is so.
                note: tab_note(&view.link, view.held.as_deref()),
                apps: if view.apps.is_empty() && view.link == Link::Connecting {
                    // Being connected to: its applications take this bubble's place, where the
                    // wearer is looking, when it has said what they are.
                    vec![RemoteEntry { id: String::new(), name: "Connecting\u{2026}".into(), icon: None }]
                } else if view.apps.is_empty() && !matches!(view.link, Link::Online) {
                    // Its applications are not known until it is connected to, and connecting
                    // takes its windows from whichever device has them: so it is asked for.
                    // One that did not answer is asked again by the same bubble.
                    vec![RemoteEntry { id: String::new(), name: "Use this computer".into(), icon: None }]
                } else {
                    view.apps
                        .iter()
                        .map(|a| RemoteEntry {
                            id: a.id.clone(),
                            name: a.name.clone(),
                            icon: a.icon.clone(),
                        })
                        .collect()
                },
            });
        }
        for (fp, name) in &self.devices {
            if !prefs.remotes.iter().any(|r| r.fingerprint.eq_ignore_ascii_case(fp)) {
                rows.push(HostRow { label: name.clone(), address: format!("{DEVICE_ROW}{fp}"), status: HostStatus::Device });
            }
        }
        (rows, tabs)
    }

    /// For the headless snapshot: whether any host has a window to show yet.
    pub fn len(&self) -> usize {
        self.hosts.len()
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        menu::host_down(&self.host, &self.commands);
    }
}

/// Whether a host on the list is the one now paired at `address` with `fingerprint`.
fn same_host(entry: &crate::prefs::RemoteHost, address: &str, fingerprint: &str) -> bool {
    entry.host == address || (!fingerprint.is_empty() && entry.fingerprint.eq_ignore_ascii_case(fingerprint))
}

#[cfg(test)]
mod tests {
    use super::*;
    use spatiand_audio::server::Width;

    fn host(address: &str, fingerprint: &str) -> crate::prefs::RemoteHost {
        crate::prefs::RemoteHost { host: address.into(), fingerprint: fingerprint.into(), launch: Vec::new(), on_demand: false }
    }

    #[test]
    fn this_machine_is_not_a_remote_of_its_own() {
        // The Mac's room and host are one identity; its app's paired computers named it.
        let remotes = Remotes { me: "cec2e365aa".into(), ..Remotes::default() };
        let mut prefs = crate::prefs::Prefs::default();
        prefs.remotes = vec![host("deepmagpie", "afb86951bb"), host("littleotter", "CEC2E365AA")];
        let (rows, tabs) = remotes.describe(&prefs);
        assert_eq!(rows.iter().map(|r| r.address.as_str()).collect::<Vec<_>>(), ["deepmagpie"]);
        assert_eq!(tabs.len(), 1);
        // Not knowing itself yet, it hides nothing.
        let (rows, _) = Remotes::default().describe(&prefs);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn a_host_paired_again_at_another_address_is_the_same_host() {
        let old = host("littleotter", "cec2e365aa");
        assert!(same_host(&old, "192.168.10.20", "CEC2E365AA"), "renamed or by its IP: the same host");
        assert!(same_host(&old, "littleotter", "0b8dfc23"), "the same address: replaced, as before");
        assert!(!same_host(&host("deepmagpie", "afb86951bb"), "192.168.10.20", "cec2e365aa"));
        assert!(!same_host(&host("x", ""), "y", ""), "no fingerprint matches nothing");
    }

    #[test]
    fn a_locked_host_is_waited_for_only_when_asked_and_says_when_it_is_over() {
        // (One test for all of it: the lists are the process's.)
        let now = std::time::Instant::now();
        // Locked, and nothing asked of it: nothing on the card.
        held_said("test-mac", "littleOtter", Some("Touch ID or its password".into()));
        assert_eq!(waiting_words(now), None);
        // A notification of its own saying the same is not shown beside it.
        host_notice_said("test-mac", "littleOtter is locked: unlock it".into());
        assert!(notice_slot().as_ref().is_none_or(|n| n.host != "test-mac"));
        // Asked: the card says what is waited for, and on which machine.
        asked_of("test-mac");
        let words = waiting_words(std::time::Instant::now()).unwrap();
        assert!(words.starts_with("littleOtter is locked\n"), "{words}");
        assert!(words.contains("Touch ID or its password"), "{words}");
        assert_eq!(host_notice(), words);
        // Long after, it says once that it is still locked, and then goes.
        let asked = held().iter().find(|h| h.host == "test-mac").unwrap().asked.unwrap();
        assert!(waiting_words(asked + WAIT_FOR + std::time::Duration::from_secs(1)).unwrap().contains("still locked"));
        assert_eq!(waiting_words(asked + WAIT_FOR + WORD_FOR + std::time::Duration::from_secs(1)), None);
        // Unlocked: said, since it was waited for.
        held_said("test-mac", "littleOtter", None);
        assert!(host_notice().starts_with("littleOtter is unlocked"), "{}", host_notice());
        assert!(notice_from().is_none(), "the room's own word is not a notification to press");
        // A notification from it is shown now, and is its to press; its going takes it away.
        host_notice_said("test-mac", "Messages\nLunch at one?".into());
        assert_eq!(host_notice(), "Messages\nLunch at one?");
        assert_eq!(notice_from().as_deref(), Some("test-mac"));
        host_notice_said("another", String::new());
        assert_eq!(notice_from().as_deref(), Some("test-mac"));
        host_notice_said("test-mac", String::new());
        assert_eq!(host_notice(), "");
        // Asked first and told it is locked a moment after (connecting): waited for at once.
        asked_of("test-mac");
        held_said("test-mac", "littleOtter", Some("its password".into()));
        assert!(host_notice().contains("its password"));
        // Its link goes while it is waited for: said, and the wait is over.
        held_gone("test-mac");
        assert!(host_notice().contains("no longer connected"));
        *notice_slot() = None;
    }

    #[test]
    fn a_tab_says_what_is_in_the_way_of_its_applications() {
        assert_eq!(tab_note(&Link::Online, None), "");
        assert_eq!(tab_note(&Link::Online, Some("Touch ID or its password")), "locked: unlock it with Touch ID or its password");
        assert_eq!(tab_note(&Link::Connecting, None), "connecting\u{2026}");
        assert_eq!(tab_note(&Link::Offline(NOT_ASKED_FOR.into()), None), "");
        assert_eq!(tab_note(&Link::Offline("littleotter does not answer".into()), None), "littleotter does not answer");
    }

    #[test]
    fn the_list_of_windows_is_made_again_when_what_hosts_offer_changes() {
        offered_gone("test-offer", None);
        let _ = offered_changed();
        offered_said("test-offer", 3, "Notes".into(), false);
        assert!(offered_changed());
        assert!(!offered_changed());
        // Said again the same: nothing changed.
        offered_said("test-offer", 3, "Notes".into(), false);
        assert!(!offered_changed());
        offered_gone("test-offer", None);
        assert!(offered_changed());
    }

    #[test]
    fn a_remote_application_is_stereo_unless_its_catalogue_asked_for_more() {
        // Nothing said: the sink it always had, which is what a VR viewer's own mix needs.
        assert_eq!(sound_width("remote.nobody.never-heard-of"), Width::Stereo);
        set_sound_channels("remote.test.stereo", 2);
        set_sound_channels("remote.test.five", 6);
        set_sound_channels("remote.test.seven", 8);
        set_sound_channels("remote.test.atmos", 12);
        assert_eq!(sound_width("remote.test.stereo"), Width::Stereo);
        for app in ["five", "seven", "atmos"] {
            assert_eq!(sound_width(&format!("remote.test.{app}")), Width::Full, "{app}");
        }
    }
}
