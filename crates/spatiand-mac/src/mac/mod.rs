//! What is the Mac's: where the picture goes, where the input comes from, and the C calls that
//! hand both over from the app.
//!
//! The frame loop is not here. It is `spatiand-android`'s `backend.rs`, compiled by path below,
//! because the Mac is the same case as the Beam Pro: an app hosts the compositor, hands it a
//! window to draw into, and tells it what the pointer and the keys did. What that file asks of
//! "the platform it runs on" is the names in this module:
//!
//! | it asks for | on the Mac |
//! |---|---|
//! | [`NativeWindow`], [`egl`] | a `CALayer` on the glasses' screen, drawn into through ANGLE |
//! | [`controller`] | the Mac's mouse, trackpad and keyboard |
//! | [`pads`] | game controllers, as the app's `GameController` reports them |
//! | [`take_glasses`] | the glasses' USB, through IOKit |
//! | [`remote_video`] | `IOSurface`s: a host's decoded picture, or a Mac window's captured one |
//! | [`local_apps`], [`launch_local`] | this Mac's applications, whose windows come into the room |
//! | [`record`] | the glasses' view to a file |

#[path = "../../../spatiand-android/src/android/backend.rs"]
pub mod backend;
pub mod controller;
pub mod egl;
pub mod ffi;
pub mod keycodes;
/// A character as the key that types it on a US layout: Android's table, which is not Android's.
#[path = "../../../spatiand-android/src/android/keys.rs"]
pub mod keys;
pub mod local;
mod logging;
pub mod pads;
pub mod panel;
pub mod record;
pub mod remote_video;

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Something the app handed over to draw into, let go of when dropped: a `CALayer`, or -- with
/// no layer at all -- a picture of this size that goes nowhere, which is what the tests draw.
pub struct NativeWindow {
    pub layer: *mut c_void,
    pub size: (i32, i32),
}

// Only ever drawn into from the compositor's thread; handed there through `Shared`.
unsafe impl Send for NativeWindow {}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRetain(object: *const c_void) -> *const c_void;
    fn CFRelease(object: *const c_void);
}

impl NativeWindow {
    /// Hold `layer` (the caller keeps its own reference), or nothing if it is null.
    pub fn new(layer: *mut c_void, size: (i32, i32)) -> NativeWindow {
        if !layer.is_null() {
            unsafe { CFRetain(layer) };
        }
        NativeWindow { layer, size }
    }

    pub fn size(&self) -> (i32, i32) {
        self.size
    }
}

impl Drop for NativeWindow {
    fn drop(&mut self) {
        if !self.layer.is_null() {
            unsafe { CFRelease(self.layer) };
        }
    }
}

/// A window handed over by the app, and which one it is, so the compositor can notice a new
/// one and the app can wait until an old one is let go.
#[derive(Default)]
pub struct Slot {
    pub window: Mutex<Option<NativeWindow>>,
    /// Bumped whenever the window is replaced or taken away.
    pub generation: AtomicU64,
    /// The generation the compositor has caught up with: everything older is no longer drawn.
    pub seen: AtomicU64,
}

impl Slot {
    pub fn set(&self, window: Option<NativeWindow>) -> u64 {
        *self.window.lock().unwrap() = window;
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Wait, up to a second, for the compositor to stop drawing into whatever came before
    /// `generation`.
    pub fn wait_released(&self, generation: u64) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while self.seen.load(Ordering::SeqCst) < generation && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// What the app and the compositor's thread share.
#[derive(Default)]
pub struct Shared {
    /// The glasses' screen.
    pub glasses: Slot,
    /// Android's second window, the phone's touch area. The Mac has none: never set.
    pub phone: Slot,
    /// Set when the glasses are to be let go of, so an open handle is closed.
    pub usb_gone: AtomicBool,
    /// A line for the app's menu.
    pub status: Mutex<String>,
    pub running: AtomicBool,
    /// Whether the app wants the glasses held at all: off, they are left for something else.
    pub glasses_wanted: AtomicBool,
}

/// The app's Recentre, for the session to do as the HUD's Recentre does.
pub fn recentre_requested() -> &'static AtomicBool {
    static REQUESTED: AtomicBool = AtomicBool::new(false);
    &REQUESTED
}

/// The app asking for a picture of what the glasses show, saved as the HUD's screenshot is.
pub fn screenshot_requested() -> &'static AtomicBool {
    static REQUESTED: AtomicBool = AtomicBool::new(false);
    &REQUESTED
}

pub fn shared() -> &'static Arc<Shared> {
    static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();
    SHARED.get_or_init(|| {
        let shared = Shared::default();
        shared.glasses_wanted.store(true, Ordering::SeqCst);
        Arc::new(shared)
    })
}

// --- what `backend` asks of the platform it runs on ---

/// macOS is offered the glasses' two-eye picture at 60 Hz only: asked for the Deck's 72, the
/// glasses agree and the Mac goes on showing one eye.
pub const REFRESH_MHZ: i32 = 60_000;
/// How far ahead the head is predicted: this frame through the window server on its next
/// refresh, then scanout.
pub const PREDICT_AHEAD_S: f64 = 2.0 / 60.0;
/// What to do about there being no glasses, under the message that says so.
pub const PLUG_HINT: &str = "Plug the glasses into this Mac.";
/// Wi-Fi and Bluetooth are System Settings' own panes, brought here as windows.
pub const PANELS: spatiand_shell::DesktopPanels = spatiand_shell::DesktopPanels::ALL;

/// This Mac's applications, as the app listed them.
pub fn local_apps() -> Vec<spatiand_shell::AppEntry> {
    local::apps()
}

pub fn launch_local(app: &spatiand_shell::AppEntry) {
    local::launch(app);
}

pub fn return_to_desktop() {
    ffi::tell(ffi::Asked::ReturnToDesktop, "");
}

pub fn system_settings(panel: &str) {
    ffi::tell(ffi::Asked::SystemSettings, panel);
}

/// Computers the app says this Mac has paired with: (address, fingerprint).
pub static PAIRED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// The computers the app has paired with become the preferences' remote hosts, as the Deck's
/// are: pairing happens in the app's menu, and the room connects to what was paired there.
pub fn adopt_paired_hosts(prefs: &mut crate::prefs::Prefs) {
    let mut changed = false;
    for (address, fingerprint) in PAIRED.lock().unwrap().iter() {
        if prefs.remotes.iter().any(|r| &r.host == address) {
            continue;
        }
        log::info!("adopting {address} from the app");
        prefs.remotes.push(crate::prefs::RemoteHost {
            host: address.clone(),
            fingerprint: fingerprint.clone(),
            launch: Vec::new(),
        });
        changed = true;
    }
    if changed {
        prefs.save();
    }
}

/// When the glasses were last looked for, and whether they were there.
static LOOKED: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
const LOOK_EVERY: Duration = Duration::from_secs(1);

fn glasses_present() -> bool {
    let mut looked = LOOKED.lock().unwrap();
    match *looked {
        Some((when, present)) if when.elapsed() < LOOK_EVERY => present,
        _ => {
            let present = if std::env::var("SPATIAND_HMD").as_deref() == Ok("null") {
                true
            } else {
                spatiand_hmd::is_present()
            };
            *looked = Some((Instant::now(), present));
            present
        }
    }
}

/// Whether there are glasses to open that the session has not opened yet.
pub fn glasses_waiting(shared: &Shared, holding: bool) -> bool {
    !holding && shared.glasses_wanted.load(Ordering::SeqCst) && glasses_present()
}

/// Glasses plugged in and not yet held: opened, and asked for the Deck's 72 Hz.
pub fn take_glasses(
    shared: &Shared,
    held: &mut Option<Box<dyn spatiand_hmd::Hmd>>,
) -> Option<spatiand_hmd::Result<Box<dyn spatiand_hmd::Hmd>>> {
    if !glasses_waiting(shared, held.is_some()) {
        return None;
    }
    // Not asked again until the next look, whether this works or not: a pair that will not
    // open is not opened sixty times a second.
    *LOOKED.lock().unwrap() = Some((Instant::now(), false));
    if std::env::var("SPATIAND_HMD").as_deref() == Ok("null") {
        return Some(spatiand_hmd::open_any());
    }
    Some(spatiand_hmd::XrealGlasses::open_any().map(|mut glasses| {
        use spatiand_hmd::Hmd;
        log::info!("glasses opened: {}", glasses.info().name);
        glasses.prefer_refresh((REFRESH_MHZ / 1000) as u32);
        // Glasses left in a two-eye mode by whatever had them last do nothing when asked for
        // it again, and macOS, which only learns of the wide picture when they change, goes on
        // offering one eye. Back to one eye first, and a moment for the Mac to see it.
        if matches!(glasses.raw_display_mode(), Some(0x03 | 0x04 | 0x09)) {
            log::info!("the glasses were left in a two-eye mode; starting them from one eye");
            let _ = glasses.set_display_mode(spatiand_hmd::DisplayMode::Mono);
            std::thread::sleep(std::time::Duration::from_millis(3000));
        }
        Box::new(glasses) as Box<dyn spatiand_hmd::Hmd>
    }))
}

/// Say where what the app carries is, for the libraries that look in the environment: the
/// keyboard layouts libxkbcommon reads, and a directory for the compositor's socket. Before
/// anything else, since a compositor with no keymap has no keyboard.
pub fn prepare() {
    logging::init();
    // Where the compositor's socket goes. macOS has no runtime directory; the user's own
    // temporary one is as private.
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        let dir = std::env::temp_dir().join("spatiand-runtime");
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var("XDG_RUNTIME_DIR", &dir);
    }
    if std::env::var_os("XKB_CONFIG_ROOT").is_none() {
        let exe = std::env::current_exe().ok();
        let dir = exe.as_deref().and_then(|e| e.parent());
        let candidates = [
            dir.map(|d| d.join("../Resources/xkb")),
            Some(std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../mac/xkbcommon/out/xkb"))),
        ];
        if let Some(found) = candidates.iter().flatten().find(|p| p.join("rules/evdev").exists()) {
            std::env::set_var("XKB_CONFIG_ROOT", found);
        }
    }
}

/// How many pixels a point of this Mac's screen is, as the app said: a window's picture is
/// that many times its size.
pub static DISPLAY_SCALE: Mutex<f64> = Mutex::new(1.0);
/// Whether macOS has been told this app may use the microphone.
pub static MICROPHONE_ALLOWED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What the frame loop does for this Mac's own windows, once a frame.
///
/// **A Mac window is as big in the room as it is on the Mac, relative to the others.** The room
/// gives every window the same width -- right on the Deck, where every window is offered the
/// same 1280 pixels and takes them. A Mac's windows are the sizes their applications made them:
/// a calculator at the width of a browser is a wall of blurred buttons. So the first time one
/// is seen, its width in the room is set from its width in points, at the density the room's
/// own default window has (1280 points across 1.1 m).
///
/// **And every window open on the Mac is in the room's list, put away.** The app announces them
/// all; each arrives hidden, and is brought out from the list of windows, or by opening its
/// application in the launcher. Only the ones on show are captured.
pub fn size_new_windows(state: &mut crate::Spatiand, _windows: &mut [crate::scene::WindowQuad]) {
    use smithay::reexports::wayland_server::Resource;
    use smithay::wayland::seat::WaylandFocus;
    thread_local! {
        static SIZED: std::cell::RefCell<std::collections::HashSet<usize>> = Default::default();
    }
    let scale = *DISPLAY_SCALE.lock().unwrap();
    let prefix = format!("remote.{}.", local::HOST);
    let mut on_show = std::collections::HashSet::new();
    let all: Vec<smithay::desktop::Window> = state.space.elements().cloned().collect();
    for window in all {
        if !state.app_id_of(&window).is_some_and(|a| a.starts_with(&prefix)) {
            continue;
        }
        let Some(surface) = window.wl_surface() else { continue };
        let Some((mac_id, wanted)) = local::wanted(surface.id().protocol_id()) else { continue };
        if wanted.hide {
            state.hide_window(&window);
        }
        if wanted.show {
            state.show_window(&window);
            // In front of the wearer, as a window chosen from the list is.
            if let Some(mut placement) = state.layout.get(&window).filter(|_| !state.layout.is_pinned(&window)) {
                placement.yaw = state.spawn_yaw;
                placement.pitch = 0.0;
                state.layout.set(&window, placement);
            }
            state.focus_window(&window);
        }
        if !state.layout.is_hidden(&window) {
            on_show.insert(mac_id);
        }
        let Some(id) = state.layout.id_of(&window) else { continue };
        if SIZED.with(|s| s.borrow().contains(&id)) {
            continue;
        }
        let pixels = smithay::backend::renderer::utils::with_renderer_surface_state(&surface, |s| s.surface_size())
            .flatten()
            .map(|size| size.w.max(0) as u32)
            .unwrap_or(0);
        if pixels <= 16 {
            continue;
        }
        SIZED.with(|s| s.borrow_mut().insert(id));
        let Some(mut placement) = state.layout.get(&window) else { continue };
        let default = crate::window::Placement::default().width;
        placement.width = (default * pixels as f64 / scale / 1280.0).clamp(0.25, 2.4);
        state.layout.set(&window, placement);
    }
    local::on_show(on_show);
}

/// The notification the app wants shown, or nothing.
pub static NOTICE: Mutex<String> = Mutex::new(String::new());

/// The notification to show under the status bar: who it is from, then what it says.
pub fn notice() -> String {
    NOTICE.lock().map(|n| n.clone()).unwrap_or_default()
}

/// The card was pressed: the app presses the banner it came from.
pub fn notice_pressed() {
    ffi::tell(ffi::Asked::NoticePressed, "");
}
