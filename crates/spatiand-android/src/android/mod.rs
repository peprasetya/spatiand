//! What is Android's: where the picture goes, where the input comes from, and the JNI calls
//! that hand both over from the app.

pub mod backend;
pub mod egl;
mod jni;
pub mod keys;
mod logging;
pub mod pads;
pub mod panel;
pub mod phone;
pub mod mkv;
pub mod record;
pub mod remote_video;

use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// An `ANativeWindow` the app handed over, released when dropped.
pub struct NativeWindow(pub *mut ndk_sys::ANativeWindow);

// Only ever drawn into from the compositor's thread; handed there through `Shared`.
unsafe impl Send for NativeWindow {}

impl NativeWindow {
    pub fn size(&self) -> (i32, i32) {
        unsafe {
            (
                ndk_sys::ANativeWindow_getWidth(self.0),
                ndk_sys::ANativeWindow_getHeight(self.0),
            )
        }
    }
}

impl Drop for NativeWindow {
    fn drop(&mut self) {
        unsafe { ndk_sys::ANativeWindow_release(self.0) };
    }
}

/// A surface handed over by the app, and which one it is, so the compositor can notice a new
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
    /// `generation`. What `SurfaceHolder.Callback.surfaceDestroyed` requires before it returns.
    pub fn wait_released(&self, generation: u64) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while self.seen.load(Ordering::SeqCst) < generation && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}

/// What the app and the compositor's thread share.
#[derive(Default)]
pub struct Shared {
    /// The glasses' display.
    pub glasses: Slot,
    /// The phone's touch area, where the monitors are drawn under the thumb.
    pub phone: Slot,
    /// The glasses' usbfs descriptor, when a new one has been handed over.
    pub usb: Mutex<Option<OwnedFd>>,
    /// Set when the glasses have been let go of, so an open handle is closed.
    pub usb_gone: AtomicBool,
    /// A line for the phone's screen.
    pub status: Mutex<String>,
    pub running: AtomicBool,
}

/// The phone's Recenter button, for the session to do as the HUD's Recentre does.
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
    SHARED.get_or_init(|| Arc::new(Shared::default()))
}

// --- what `backend` asks of the platform it runs on. The Mac has the same names; see the top of
// `backend.rs`. ---

/// The controller the session reads: on Android, the phone.
pub mod controller {
    pub use super::phone::{PhoneController as Controller, Typed};
}

/// The Beam Pro composites every display on the phone screen's 60 Hz clock.
pub const REFRESH_MHZ: i32 = 60_000;
/// How far ahead the head is predicted: two frames -- this one through Android's compositor on
/// its next tick, then scanout -- the Beam Pro's latency, measured as smoothest.
pub const PREDICT_AHEAD_S: f64 = 2.0 / 60.0;
/// What to do about there being no glasses, under the message that says so.
pub const PLUG_HINT: &str = "Plug the glasses into the Beam Pro.";
/// Android's own Wi-Fi and Bluetooth settings are not windows that can be brought here.
pub const PANELS: spatiand_shell::DesktopPanels = spatiand_shell::DesktopPanels {
    network: false,
    bluetooth: false,
};

/// The launcher has only remote applications here: Android's own are not windows yet.
pub fn local_apps() -> Vec<spatiand_shell::AppEntry> {
    Vec::new()
}

pub fn launch_local(app: &spatiand_shell::AppEntry) {
    log::info!("{} is a local application; none run here", app.name);
}

pub fn return_to_desktop() {
    log::info!("there is no desktop to return to here");
}

pub fn system_settings(panel: &str) {
    log::info!("no {panel} settings here");
}

pub fn adopt_paired_hosts(prefs: &mut crate::prefs::Prefs) {
    jni::adopt_paired_hosts(prefs);
}

/// New glasses on the USB side, if the app has handed any over: opened, in place of any held,
/// and told the refresh rate the display they are on is drawn at.
pub fn take_glasses(
    shared: &Shared,
    held: &mut Option<Box<dyn spatiand_hmd::Hmd>>,
) -> Option<spatiand_hmd::Result<Box<dyn spatiand_hmd::Hmd>>> {
    let fd = shared.usb.lock().unwrap().take()?;
    // Let go of first: two handles on one pair of glasses silence each other.
    *held = None;
    Some(spatiand_hmd::XrealGlasses::open_usb(fd).map(|mut glasses| {
        // The Beam Pro composites every display on the phone's 60 Hz clock; the glasses' own
        // 72 would show one frame in five twice.
        glasses.prefer_refresh((REFRESH_MHZ / 1000) as u32);
        Box::new(glasses) as Box<dyn spatiand_hmd::Hmd>
    }))
}

/// Whether there are glasses to open that the session has not opened yet.
pub fn glasses_waiting(shared: &Shared, _holding: bool) -> bool {
    shared.usb.lock().unwrap().is_some()
}

/// Every window on the Beam Pro is a host's, and is the size the room gives any window.
pub fn size_new_windows(_state: &mut crate::Spatiand, _windows: &mut [crate::scene::WindowQuad]) {}

/// A notification to show under the status bar: who it is from, then what it says. Android's
/// are not read yet.
pub fn notice() -> String {
    String::new()
}

pub fn notice_pressed() {}
