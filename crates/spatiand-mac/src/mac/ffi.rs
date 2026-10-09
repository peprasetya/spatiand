//! The C calls the app makes, and the one call made back to it. `spatiand_mac.h` is this file's
//! other half; keep the two in step.
//!
//! The app is Swift and owns everything that is AppKit's: the window on the glasses' screen,
//! the Mac's pointer and keyboard, screen capture, the menu. The compositor runs on a thread of
//! its own, started by [`sp_begin`], and the two meet only here.

use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use spatiand_input::Control;

use super::controller::{self, Typed};
use super::{local, pads, shared, NativeWindow};

/// What the compositor asks of the app, or tells it. The numbers are the header's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Asked {
    /// Leave the room: give the Mac its pointer and keyboard back.
    ReturnToDesktop = 1,
    /// Open a pane of System Settings and bring it into the room. `text` is `wifi` or `bluetooth`.
    SystemSettings = 2,
    /// Open an application and bring its windows into the room. `text` is what it was listed with.
    Launch = 3,
    /// A tick under the finger: `a` is 0 click, 1 tick, 2 alert.
    Haptic = 4,
    /// A screenshot or a video was saved at `text`.
    Saved = 5,
    /// The pointer is at (`a`, `b`) in window `id`'s own pixels.
    WindowMotion = 10,
    /// The pointer left window `id`.
    WindowLeave = 11,
    /// Button `a` (evdev: 0x110 left, 0x111 right, 0x112 middle) went down (`b` = 1) or up.
    WindowButton = 12,
    /// Scrolled by (`a` across, `b` down), in wheel-notch units of 15.
    WindowScroll = 13,
    /// Key `a` (a Mac virtual key code, −1 if it has none; `c` is the evdev code) went down
    /// (`b` = 1) or up.
    WindowKey = 14,
    /// The keyboard is window `id`'s now, or nobody's of this Mac's (`id` 0).
    WindowFocus = 15,
    /// Window `id` should be `a` by `b` pixels.
    WindowResize = 16,
    /// Window `id` should close.
    WindowClose = 17,
    /// Window `id` is on show in the room now: its picture is wanted.
    WindowShown = 18,
    /// Window `id` has been put away, and is only in the list: no picture is needed.
    WindowHidden = 19,
}

pub type Callback =
    extern "C" fn(user: *mut c_void, what: i32, id: u32, a: f64, b: f64, c: f64, text: *const c_char);

static CALLBACK: Mutex<Option<(Callback, usize)>> = Mutex::new(None);

fn call(what: Asked, id: u32, a: f64, b: f64, c: f64, text: &str) {
    let Some((callback, user)) = *CALLBACK.lock().unwrap() else { return };
    let text = CString::new(text.replace('\0', "")).unwrap_or_default();
    callback(user as *mut c_void, what as i32, id, a, b, c, text.as_ptr());
}

pub fn tell(what: Asked, text: &str) {
    call(what, 0, 0.0, 0.0, 0.0, text);
}

pub fn tell_number(what: Asked, a: f64) {
    call(what, 0, a, 0.0, 0.0, "");
}

pub fn tell_window(what: Asked, id: u32, a: f64, b: f64, c: f64) {
    call(what, id, a, b, c, "");
}

unsafe fn text(text: *const c_char) -> String {
    if text.is_null() {
        return String::new();
    }
    CStr::from_ptr(text).to_string_lossy().into_owned()
}

/// Start the session, once. Returns at once; the session runs on its own thread. `callback` is
/// called from the compositor's threads with whatever it asks of the app.
#[no_mangle]
pub extern "C" fn sp_begin(callback: Option<Callback>, user: *mut c_void) {
    super::prepare();
    *CALLBACK.lock().unwrap() = callback.map(|c| (c, user as usize));
    let shared = shared().clone();
    if shared.running.swap(true, Ordering::SeqCst) {
        return;
    }
    let started = std::thread::Builder::new().name("spatiand".into()).spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            use smithay::reexports::calloop::EventLoop;
            use smithay::reexports::wayland_server::Display;
            let mut event_loop: EventLoop<crate::Runtime> = match EventLoop::try_new() {
                Ok(l) => l,
                Err(e) => return log::error!("no event loop: {e}"),
            };
            let mut display: Display<crate::Spatiand> = match Display::new() {
                Ok(d) => d,
                Err(e) => return log::error!("no wayland display: {e}"),
            };
            let mut display_handle = display.handle();
            let state = crate::Spatiand::new(&mut display, &event_loop.handle());
            if let Err(e) = local::attach(&mut display_handle) {
                log::error!("{e}");
            }
            let mut runtime = crate::Runtime { state, display_handle };
            log::info!("session started");
            if let Err(e) = super::backend::run(&mut event_loop, &mut display, &mut runtime, &shared) {
                log::error!("the session stopped: {e}");
            }
        }));
        if result.is_err() {
            log::error!("the session died; see the panic above");
        }
        shared.running.store(false, Ordering::SeqCst);
    });
    if let Err(e) = started {
        log::error!("could not start the session: {e}");
        super::shared().running.store(false, Ordering::SeqCst);
    }
}

/// Stop the session: the glasses go back to 2D on the way.
#[no_mangle]
pub extern "C" fn sp_end() {
    shared().running.store(false, Ordering::SeqCst);
}

#[no_mangle]
pub extern "C" fn sp_running() -> bool {
    shared().running.load(Ordering::SeqCst)
}

/// The layer on the glasses' screen to draw into, its size in pixels, and which display it is on
/// (a `CGDirectDisplayID`; 0 for the Mac's own), whose refresh is the frame clock. A null layer
/// draws into nothing, at that size: the tests'.
///
/// # Safety
/// `layer` must be a live `CALayer` or null.
#[no_mangle]
pub unsafe extern "C" fn sp_glasses(layer: *mut c_void, width: i32, height: i32, display: u32) {
    if !layer.is_null() {
        super::egl::follow_display(display);
    }
    shared().glasses.set(Some(NativeWindow::new(layer, (width, height))));
}

/// The glasses' screen has gone. Returns once nothing is drawing into its layer.
#[no_mangle]
pub extern "C" fn sp_glasses_gone() {
    let slot = &shared().glasses;
    let generation = slot.set(None);
    slot.wait_released(generation);
}

/// Whether the glasses' sensors are to be held at all. Off, they are let go of and left alone.
#[no_mangle]
pub extern "C" fn sp_hold_glasses(hold: bool) {
    shared().glasses_wanted.store(hold, Ordering::SeqCst);
    if !hold {
        shared().usb_gone.store(true, Ordering::SeqCst);
    }
}

/// The Mac's pointer: its travel in points (+y down), the buttons down (1 primary, 2 secondary,
/// 4 middle), and scrolling in wheel notches (+ right, + up).
#[no_mangle]
pub extern "C" fn sp_pointer(dx: f32, dy: f32, buttons: i32, wheel_right: f32, wheel_up: f32) {
    let mut input = controller::input().lock().unwrap();
    input.travel.0 += dx;
    input.travel.1 += dy;
    input.buttons = buttons;
    input.wheel.0 += wheel_right;
    input.wheel.1 += wheel_up;
}

/// Whether the room has the Mac's pointer. Without it there is no laser in the room.
#[no_mangle]
pub extern "C" fn sp_pointer_held(held: bool) {
    let mut input = controller::input().lock().unwrap();
    input.held = held;
    if !held {
        input.buttons = 0;
    }
}

#[no_mangle]
pub extern "C" fn sp_pointer_speed(speed: f32) {
    controller::set_speed(speed);
}

/// Two fingers spreading (above 1) or closing, since the last call.
#[no_mangle]
pub extern "C" fn sp_pinch(scale: f32) {
    if scale.is_finite() && scale > 0.0 {
        controller::input().lock().unwrap().pinch *= scale;
    }
}

/// A key of the Mac's keyboard, by its virtual key code.
#[no_mangle]
pub extern "C" fn sp_key(mac_code: u16, pressed: bool) {
    if let Some(code) = super::keycodes::evdev_of(mac_code) {
        controller::input().lock().unwrap().keys.push(Typed::Key { code, pressed });
    }
}

/// A key by its evdev code, for what is not a key of the Mac's keyboard.
#[no_mangle]
pub extern "C" fn sp_key_evdev(code: u32, pressed: bool) {
    controller::input().lock().unwrap().keys.push(Typed::Key { code, pressed });
}

/// One of the Deck's buttons, which the app gives its own shortcuts: 0 STEAM (the settings),
/// 1 `⋯` (the launcher), 2 A, 3 B, 4 X, 5 Y, 6 up, 7 down, 8 left, 9 right, 10 menu, 11 view.
#[no_mangle]
pub extern "C" fn sp_control(control: i32, pressed: bool) {
    let control = match control {
        0 => Control::Steam,
        1 => Control::Quick,
        2 => Control::A,
        3 => Control::B,
        4 => Control::X,
        5 => Control::Y,
        6 => Control::Up,
        7 => Control::Down,
        8 => Control::Left,
        9 => Control::Right,
        10 => Control::Menu,
        11 => Control::View,
        _ => return,
    };
    controller::input().lock().unwrap().controls.push((control, pressed));
}

/// Make where the wearer is looking the new forward, as the settings' Recentre does.
#[no_mangle]
pub extern "C" fn sp_recentre() {
    super::recentre_requested().store(true, Ordering::SeqCst);
}

/// Save a picture of what the glasses show, as the settings' screenshot does.
#[no_mangle]
pub extern "C" fn sp_screenshot() {
    super::screenshot_requested().store(true, Ordering::SeqCst);
}

// --- game controllers ---

/// A pad's button: see `pads::Button` for the numbers.
#[no_mangle]
pub extern "C" fn sp_pad_button(pad: i32, button: i32, down: bool) {
    pads::button(pad, button, down);
}

#[no_mangle]
pub extern "C" fn sp_pad_axes(pad: i32, lx: f32, ly: f32, rx: f32, ry: f32, lt: f32, rt: f32) {
    pads::axes(pad, lx, ly, rx, ry, lt, rt);
}

#[no_mangle]
pub extern "C" fn sp_pad_touch(pad: i32, x: f32, y: f32, touched: bool, clicked: bool) {
    pads::touch(pad, x, y, touched, clicked);
}

#[no_mangle]
pub extern "C" fn sp_pad_gyro(pad: i32, x: f32, y: f32, z: f32) {
    pads::gyro(pad, x, y, z);
}

#[no_mangle]
pub extern "C" fn sp_pad_gone(pad: i32) {
    pads::gone(pad);
}

/// The rumble a game asked for since the last call, strong << 16 | weak, or −1 for none.
#[no_mangle]
pub extern "C" fn sp_pad_rumble() -> i64 {
    pads::take_rumble()
}

// --- this Mac's applications and their windows ---

/// Forget the launcher's list, before giving it again with [`sp_app`].
#[no_mangle]
pub extern "C" fn sp_apps_begin() {
    APPS.lock().unwrap().clear();
}

static APPS: Mutex<Vec<spatiand_shell::AppEntry>> = Mutex::new(Vec::new());

/// One application: its name, what to hand back to open it, and a PNG of its icon (or null).
///
/// # Safety
/// The strings must be valid C strings or null.
#[no_mangle]
pub unsafe extern "C" fn sp_app(name: *const c_char, open_with: *const c_char, icon: *const c_char) {
    let icon = text(icon);
    APPS.lock().unwrap().push(spatiand_shell::AppEntry {
        name: text(name),
        exec: text(open_with),
        icon: (!icon.is_empty()).then_some(icon),
        categories: Vec::new(),
    });
}

/// The list is complete.
#[no_mangle]
pub extern "C" fn sp_apps_end() {
    local::set_apps(std::mem::take(&mut *APPS.lock().unwrap()));
}

/// A computer this Mac has paired with, which the room is to connect to as the Deck does to the
/// hosts in its preferences.
///
/// # Safety
/// The strings must be valid C strings or null.
#[no_mangle]
pub unsafe extern "C" fn sp_paired_host(address: *const c_char, fingerprint: *const c_char) {
    super::PAIRED.lock().unwrap().push((text(address), text(fingerprint)));
}

/// How many pixels a point of this Mac's screen is: 2 on a Retina display.
#[no_mangle]
pub extern "C" fn sp_display_scale(scale: f64) {
    if scale.is_finite() && scale > 0.0 {
        *super::DISPLAY_SCALE.lock().unwrap() = scale;
    }
}

/// A window of this Mac's, for the room: `id` is the app's own and never 0, and the size is its
/// picture's in pixels. `hidden` lists it without showing it -- it is in the room's list of
/// windows, to be brought out from there -- and the app hears `WindowShown` when it is.
///
/// # Safety
/// The strings must be valid C strings or null.
#[no_mangle]
pub unsafe extern "C" fn sp_window_open(
    id: u32,
    bundle: *const c_char,
    title: *const c_char,
    width: u32,
    height: u32,
    hidden: bool,
) {
    local::open(id, &text(bundle), &text(title), (width, height), hidden);
}

/// Bring a listed window out, in front of the wearer.
#[no_mangle]
pub extern "C" fn sp_window_show(id: u32) {
    local::show(id);
}

/// A window's newest picture, as an `IOSurfaceRef`, which is held for as long as it is shown.
///
/// # Safety
/// `surface` must be a live `IOSurfaceRef`.
#[no_mangle]
pub unsafe extern "C" fn sp_window_picture(id: u32, surface: *const c_void) {
    if !surface.is_null() {
        local::picture(id, surface);
    }
}

/// # Safety
/// `title` must be a valid C string or null.
#[no_mangle]
pub unsafe extern "C" fn sp_window_title(id: u32, title: *const c_char) {
    local::retitle(id, &text(title));
}

#[no_mangle]
pub extern "C" fn sp_window_close(id: u32) {
    local::close(id);
}

// --- sound ---

/// Play to this Core Audio device from now on; 0 for the system's default output.
#[no_mangle]
pub extern "C" fn sp_audio_output(device: i32) {
    spatiand_audio::server::set_output_device(device);
}

/// Sound one of this Mac's applications made, tapped by the app: interleaved 16-bit at 48 kHz.
/// It is placed at that application's window, if it has one in the room.
///
/// # Safety
/// `bundle` must be a valid C string and `pcm` must hold `frames * channels` samples.
#[no_mangle]
pub unsafe extern "C" fn sp_app_sound(bundle: *const c_char, pcm: *const i16, frames: u32, channels: u32) {
    if pcm.is_null() || frames == 0 || channels == 0 {
        return;
    }
    let samples = std::slice::from_raw_parts(pcm, (frames * channels) as usize);
    let slot = crate::remote::sound::slot_for(&local::app_id(&text(bundle)));
    spatiand_audio::server::feed(slot, samples, channels as usize);
}

/// A line saying how the session is, for the app's menu. Returns its length; at most `size − 1`
/// bytes and a terminator are written.
///
/// # Safety
/// `into` must point at `size` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn sp_status(into: *mut c_char, size: usize) -> usize {
    let status = shared().status.lock().map(|s| s.clone()).unwrap_or_default();
    if into.is_null() || size == 0 {
        return status.len();
    }
    let n = status.len().min(size - 1);
    std::ptr::copy_nonoverlapping(status.as_ptr(), into as *mut u8, n);
    *into.add(n) = 0;
    n
}
