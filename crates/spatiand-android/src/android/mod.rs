//! What is Android's: where the picture goes, where the input comes from, and the JNI calls
//! that hand both over from the app.

pub mod apps;
pub mod backend;
pub mod bluetooth;
pub mod clipboard;
pub mod egl;
pub mod explicit_sync;
mod jni;
pub mod keys;
mod logging;
pub mod pads;
pub mod notices;
pub mod panel;
pub mod phone;
pub mod mkv;
pub mod record;
pub mod record_codec;
pub mod remote_video;
pub mod sound;

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
    /// Whether the glasses' display is the stand-in tools/beam-stand-in makes: a display with no
    /// glasses behind it, for testing the room on the phone alone.
    pub stand_in: AtomicBool,
    /// The glasses lent to one of the phone's apps, flat (`apps`, `MainActivity.showFlat`): one
    /// eye's picture, so mono, and the room drawing nothing until they are given back.
    pub flat: AtomicBool,
}

/// The phone's Recenter button, for the session to do as the HUD's Recentre does.
pub fn recentre_requested() -> &'static AtomicBool {
    static REQUESTED: AtomicBool = AtomicBool::new(false);
    &REQUESTED
}

/// The app asking for the window that has the keyboard to be closed, as its bar's cross does:
/// a keyboard's way to it, for a wearer with no controller to hand.
pub fn close_requested() -> &'static AtomicBool {
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
/// Bluetooth is the room's own page, with Android's Bluetooth behind it ([`bluetooth`]); Wi-Fi is
/// Android's own Wi-Fi page as a window in front of the wearer, as the Deck's is its desktop's
/// ([`system_settings`]).
pub const PANELS: spatiand_shell::DesktopPanels = spatiand_shell::DesktopPanels::ALL;

/// The room's Bluetooth page's devices: Android's.
pub fn bluetooth_devices() -> crate::bluetooth::Devices {
    crate::bluetooth::Devices::start_with(bluetooth::Phone, bluetooth::add_device)
}

/// The phone's own apps, each opened in a window of its own (`apps`).
pub fn local_apps() -> Vec<spatiand_shell::AppEntry> {
    apps::apps()
}

pub fn launch_local(app: &spatiand_shell::AppEntry) {
    apps::launch(app);
}

/// The phone's apps arrive after the session has started, and again on every install.
pub fn local_apps_changed() -> Option<Vec<spatiand_shell::AppEntry>> {
    apps::changed()
}

/// "Leave Spatiand", as the Deck returns to its desktop and the Mac to its own: the glasses let go
/// of -- back to 2D -- and then the app closes and its process ends (`MainActivity.leave`), taking
/// every window with it: the phone's apps' displays (the helper lets go of them when the app's
/// process ends) and Spatiand's own apps (its children). The next start is a new session from
/// nothing, as the Deck's next login is.
pub fn return_to_desktop() {
    log::info!("leaving Spatiand: the glasses let go of, then the app closed");
    shared().usb_gone.store(true, Ordering::SeqCst);
    jni::with_shell(|env, class| unsafe {
        let Some(method) = jni::shell_method(env, class, c"leave", c"()V") else { return };
        ((**env).v1_1.CallStaticVoidMethodA)(env, class, method, std::ptr::null());
    });
}

/// One of Android's own settings pages as a window in the room, as the Deck opens its desktop's:
/// "wifi" and "bluetooth".
pub fn system_settings(panel: &str) {
    if !settings_window(panel) {
        log::warn!("no {panel} settings to open here");
    }
}

/// Android's settings page for `panel`, opened in the room as one of the phone's apps is. False
/// when Android has none.
pub fn settings_window(panel: &str) -> bool {
    let (action, name) = match panel {
        "wifi" => ("android.settings.WIFI_SETTINGS", "Wi-Fi"),
        "bluetooth" => ("android.settings.BLUETOOTH_SETTINGS", "Bluetooth"),
        _ => return false,
    };
    // The activity that answers the action, as the launcher names one: "package/class".
    let Some(component) = bluetooth::java_text(c"settingsComponent", Some(action)) else { return false };
    log::info!("{name}: Android's own page, {component}");
    apps::launch(&spatiand_shell::AppEntry { name: name.into(), exec: component, icon: None, categories: Vec::new() });
    true
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
    // Whichever headset the device table knows this is, by its driver.
    Some(spatiand_hmd::open_usb(fd).map(|mut glasses| {
        // The Beam Pro composites every display on the phone's 60 Hz clock; the glasses' own
        // 72 would show one frame in five twice.
        glasses.prefer_refresh((REFRESH_MHZ / 1000) as u32);
        glasses
    }))
}

/// Whether there are glasses to open that the session has not opened yet.
pub fn glasses_waiting(shared: &Shared, _holding: bool) -> bool {
    shared.usb.lock().unwrap().is_some()
}

/// A window on the Beam Pro is a host's or one of the phone's own apps', and is the size the room
/// gives any window; a phone app's opened again from the launcher is brought out.
pub fn size_new_windows(state: &mut crate::Spatiand, _windows: &mut [crate::scene::WindowQuad]) {
    apps::size_new_windows(state);
}

/// A trackpad's scroll ended by its fingers lifting: a Mac's. The Beam Pro has no trackpad.
pub fn scroll_ended() -> bool {
    false
}

/// The Mac's 2D mode, which shows the session's windows on its desktop. The Beam Pro has no
/// desktop of its own to show them on.
pub fn desk_tick(_state: &mut crate::Spatiand) -> bool {
    false
}

pub fn desk_on() -> bool {
    false
}

/// The phone's clipboard, a peer of the session's as a host is ([`clipboard`]).
pub fn local_clipboard() -> Option<crate::remote::LocalClipboard> {
    Some(clipboard::peer())
}

/// A notification to show under the status bar: who it is from, then what it says. The room's
/// word about the phone's apps first (why one could not open), then the phone's own
/// notifications ([`notices`]).
pub fn notice() -> String {
    let said = apps::notice();
    if said.is_empty() {
        notices::text()
    } else {
        said
    }
}

pub fn notice_pressed() {
    if !apps::notice_pressed() {
        notices::pressed();
    }
}

// --- the sound devices' list in the settings, as the Mac's ---

/// The settings' list has been opened: the app says what there is.
pub fn audio_devices_wanted() {
    sound::wanted();
}

/// The device on this row of the list is to be used.
pub fn audio_device_chosen(index: usize) {
    sound::chosen(index);
}

/// The list as the settings show it, if it has changed since this was last asked.
pub fn audio_devices_changed() -> Option<Vec<(String, bool, bool)>> {
    sound::changed()
}
