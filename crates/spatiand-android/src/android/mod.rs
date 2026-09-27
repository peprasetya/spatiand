//! What is Android's: where the picture goes, where the input comes from, and the JNI calls
//! that hand both over from the app.

pub mod backend;
pub mod egl;
mod jni;
mod keys;
mod logging;
pub mod pads;
pub mod panel;
pub mod phone;
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

pub fn shared() -> &'static Arc<Shared> {
    static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();
    SHARED.get_or_init(|| Arc::new(Shared::default()))
}
