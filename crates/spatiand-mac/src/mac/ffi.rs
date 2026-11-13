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
    /// The notification on show was pressed.
    NoticePressed = 6,
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
    /// Film what the glasses show, from now.
    RecordStart = 20,
    /// And stop, and save it.
    RecordStop = 21,
    /// Say what sound devices there are, with `sp_audio_devices_begin` and the rest.
    AudioList = 22,
    /// Use sound device `id`: as the microphone if `a` is 1, else for sound to come out of.
    AudioChoose = 23,
    /// Say what is in the menus of the application window `id` belongs to, from the top,
    /// with `sp_menu_begin` and the rest.
    MenuOpen = 24,
    /// Row `a` of the menu last said was chosen: say the level it opens, or do it and say
    /// the menu is done with (`sp_menu_close`).
    MenuChoose = 25,
    /// Up a level: say that one, or that the menu is done with if this was the top.
    MenuBack = 26,
    /// 2D mode (`desk`): window `id` of the session's is to be shown on the Mac, `a` by `b`
    /// pixels; `text` is its title, a new line, and its application's id.
    DeskOpen = 30,
    /// Window `id`'s newest picture is the global `IOSurface` numbered `a`.
    DeskPicture = 31,
    /// Window `id`'s title is `text` now.
    DeskTitle = 32,
    /// Window `id` is `a` by `b` pixels now.
    DeskSize = 33,
    /// Window `id` has closed, or the glasses have it now.
    DeskClose = 34,
    /// The session's clipboard to the Mac's pasteboard (`pasteboard`): `text` is the 2D link's
    /// JSON for a host's clipboard message, `{"host": {"Clipboard": {...}}}`.
    Clipboard = 35,
    /// Say what the application with bundle id `text` shows in its focused window, through
    /// Accessibility, answering with `sp_described(id, json)`.
    Describe = 36,
    /// SpatiKey: type into a window of this Mac, as the characters themselves. `text` is
    /// `{"window": its number, "steps": [{"text": ...} | {"key": "tab" | "enter"}]}`; answered
    /// with `sp_typed(id, error)`, a null error for done.
    TypeText = 37,
    /// Force an application of this Mac to quit. `text` is its bundle id.
    ForceQuit = 38,
}

pub type Callback =
    extern "C" fn(user: *mut c_void, what: i32, id: u32, a: f64, b: f64, c: f64, text: *const c_char);

static CALLBACK: Mutex<Option<(Callback, usize)>> = Mutex::new(None);

fn call(what: Asked, id: u32, a: f64, b: f64, c: f64, text: &str) {
    let Some((callback, user)) = *CALLBACK.lock().unwrap() else { return };
    let text = CString::new(text.replace('\0', "")).unwrap_or_default();
    callback(user as *mut c_void, what as i32, id, a, b, c, text.as_ptr());
}

pub fn call_text(what: Asked, id: u32, a: f64, b: f64, c: f64, text: &str) {
    call(what, id, a, b, c, text);
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
    ALIVE.store(true, Ordering::SeqCst);
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
        ALIVE.store(false, Ordering::SeqCst);
    });
    if let Err(e) = started {
        log::error!("could not start the session: {e}");
        super::shared().running.store(false, Ordering::SeqCst);
        ALIVE.store(false, Ordering::SeqCst);
    }
}

/// Stop the session: the glasses go back to 2D on the way.
#[no_mangle]
pub extern "C" fn sp_end() {
    shared().running.store(false, Ordering::SeqCst);
}

/// Whether the session's thread is still there: from its start until it has left its loop and
/// put the glasses back to one picture. Not the flag `sp_end` clears -- the Mac app waited on
/// that one after clearing it, so never waited, and quit with the glasses left in two eyes.
static ALIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[no_mangle]
pub extern "C" fn sp_running() -> bool {
    ALIVE.load(Ordering::SeqCst)
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

/// Where the room's pointer is, across and up, each from -1 to 1 over the view. For tracing.
///
/// # Safety
/// `x` and `y` must be valid to write.
#[no_mangle]
pub unsafe extern "C" fn sp_pointer_where(x: *mut f32, y: *mut f32) {
    let pad = *controller::PAD_NOW.lock().unwrap();
    *x = pad.0;
    *y = pad.1;
}

#[no_mangle]
pub extern "C" fn sp_pointer_speed(speed: f32) {
    controller::set_speed(speed);
}

/// The trackpad's press, as it is now: the fingers on the pad as it began (0 for a mouse's
/// click, which is no press), the stage macOS says (0 none -- the press has ended, 1 clicked, 2
/// the deep click) and its pressure (0..1). `two_finger_deep` is the pressure at which two
/// fingers make a deep click, 0 for never. Returns whether the pad should tap under the fingers
/// now: as a two-finger deep click begins, which macOS does not confirm itself.
///
/// What it means is decided in the core (`spatiand_input::PressReader`), the same for any pad;
/// the room reads it as `ControllerState::pad_press`, and a layout binds it.
#[no_mangle]
pub extern "C" fn sp_pad_press(fingers: i32, stage: i32, pressure: f32, two_finger_deep: f32) -> bool {
    let mut input = controller::input().lock().unwrap();
    let two = (two_finger_deep > 0.0).then_some(two_finger_deep);
    let (press, tap) = input.press_reader.update(
        fingers.clamp(0, 255) as u8,
        stage.clamp(0, 2) as u8,
        if pressure.is_finite() { pressure } else { 0.0 },
        two,
    );
    input.presses_known = true;
    if input.press_now != press {
        input.press_now = press;
        input.presses.push(press);
    }
    tap
}

/// Two fingers spreading (above 1) or closing, since the last call.
#[no_mangle]
pub extern "C" fn sp_pinch(scale: f32) {
    if scale.is_finite() && scale > 0.0 {
        controller::input().lock().unwrap().pinch *= scale;
    }
}

pub static SCROLL_END: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static OVER_CLIENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The fingers have lifted from a scroll made with `sp_pointer`'s wheel: the client under the
/// pointer is told the gesture is over (`wl_pointer.axis_stop`, finger source), as the Deck's
/// pad tells it. A toolkit carries the page on from there by itself, and a browser takes a
/// sideways swipe that scrolled nothing for back or forward only once it has ended.
#[no_mangle]
pub extern "C" fn sp_scroll_end() {
    SCROLL_END.store(true, Ordering::SeqCst);
}

/// Whether the room's pointer is on a window of a client's (which carries a scroll on by
/// itself after `sp_scroll_end`), and not on the room's own things, which do not.
#[no_mangle]
pub extern "C" fn sp_scroll_over_client() -> bool {
    OVER_CLIENT.load(Ordering::Relaxed)
}

/// **Command is the applications' Control** (the owner, 2026-11-12: "I use to press cmd and
/// click to open link in new tab", "in mac we used to use cmd-c and v, also x for cut"). The
/// session's applications are Linux's, whose shortcuts are Control's; a Mac's hand goes to
/// Command. So while the keys are a client's -- one of Spatiand's own applications, SpatiMesh,
/// a host's window -- either Command key is told as a Control key (the right one, so the real
/// left Control is its own key still, and Control itself stays Control: a terminal's Ctrl-C).
/// While the keys are a window of this Mac's own, Command is Command: that window is a Mac
/// application's. The keys changing hands with Command held changes what is held with them.
struct Command {
    /// Each Command key (left, right) while it is down: whether it was told as Control.
    held: [Option<bool>; 2],
    /// Whether the keys are a window of this Mac's own.
    on_mac: bool,
}

static COMMAND: std::sync::Mutex<Command> = std::sync::Mutex::new(Command { held: [None, None], on_mac: false });

const SUPER: [u32; 2] = [125, 126];
const RIGHT_CONTROL: u32 = 97;

impl Command {
    fn as_control(&self) -> usize {
        self.held.iter().filter(|h| **h == Some(true)).count()
    }

    /// A key's press or release, as what the session is to be told (nothing, or keys).
    fn key(&mut self, code: u32, pressed: bool) -> Vec<(u32, bool)> {
        let Some(which) = SUPER.iter().position(|c| *c == code) else { return vec![(code, pressed)] };
        if pressed {
            if self.held[which].is_some() {
                return Vec::new();
            }
            let control = !self.on_mac;
            let first = self.as_control() == 0;
            self.held[which] = Some(control);
            match (control, first) {
                (false, _) => vec![(code, true)],
                (true, true) => vec![(RIGHT_CONTROL, true)],
                (true, false) => Vec::new(),
            }
        } else {
            match self.held[which].take() {
                None => Vec::new(),
                Some(false) => vec![(code, false)],
                Some(true) if self.as_control() == 0 => vec![(RIGHT_CONTROL, false)],
                Some(true) => Vec::new(),
            }
        }
    }

    /// The keys are a window of this Mac's own now, or no longer: what is held is told anew.
    fn keys_on_mac(&mut self, on_mac: bool) -> Vec<(u32, bool)> {
        if self.on_mac == on_mac {
            return Vec::new();
        }
        self.on_mac = on_mac;
        let mut out = Vec::new();
        let was_control = self.as_control() > 0;
        for which in 0..2 {
            if let Some(control) = self.held[which] {
                if control == on_mac {
                    self.held[which] = Some(!on_mac);
                    if on_mac {
                        out.push((SUPER[which], true));
                    } else {
                        out.push((SUPER[which], false));
                    }
                }
            }
        }
        let is_control = self.as_control() > 0;
        if was_control && !is_control {
            out.insert(0, (RIGHT_CONTROL, false));
        } else if !was_control && is_control {
            out.push((RIGHT_CONTROL, true));
        }
        out
    }
}

/// Said once a frame by the loop: whether the keys are a window of this Mac's own.
pub fn keys_on_mac(on_mac: bool) {
    let keys = COMMAND.lock().unwrap().keys_on_mac(on_mac);
    if !keys.is_empty() {
        let mut input = controller::input().lock().unwrap();
        input.keys.extend(keys.into_iter().map(|(code, pressed)| Typed::Key { code, pressed }));
    }
}

/// A key of the Mac's keyboard, by its virtual key code.
#[no_mangle]
pub extern "C" fn sp_key(mac_code: u16, pressed: bool) {
    if let Some(code) = super::keycodes::evdev_of(mac_code) {
        let keys = COMMAND.lock().unwrap().key(code, pressed);
        let mut input = controller::input().lock().unwrap();
        input.keys.extend(keys.into_iter().map(|(code, pressed)| Typed::Key { code, pressed }));
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;

    fn fresh() -> Command {
        Command { held: [None, None], on_mac: false }
    }

    #[test]
    fn command_is_control_for_a_client_and_control_stays_control() {
        let mut c = fresh();
        assert_eq!(c.key(125, true), vec![(97, true)]);
        assert_eq!(c.key(46, true), vec![(46, true)]);
        assert_eq!(c.key(46, false), vec![(46, false)]);
        assert_eq!(c.key(125, false), vec![(97, false)]);
        assert_eq!(c.key(29, true), vec![(29, true)]);
        assert_eq!(c.key(29, false), vec![(29, false)]);
    }

    #[test]
    fn both_commands_are_one_control_let_go_with_the_last() {
        let mut c = fresh();
        assert_eq!(c.key(125, true), vec![(97, true)]);
        assert_eq!(c.key(126, true), vec![]);
        assert_eq!(c.key(125, false), vec![]);
        assert_eq!(c.key(126, false), vec![(97, false)]);
    }

    #[test]
    fn command_is_command_for_a_window_of_the_macs() {
        let mut c = fresh();
        assert_eq!(c.keys_on_mac(true), vec![]);
        assert_eq!(c.key(125, true), vec![(125, true)]);
        assert_eq!(c.key(125, false), vec![(125, false)]);
    }

    #[test]
    fn the_keys_changing_hands_change_what_is_held() {
        let mut c = fresh();
        assert_eq!(c.key(125, true), vec![(97, true)]);
        assert_eq!(c.keys_on_mac(true), vec![(97, false), (125, true)]);
        assert_eq!(c.keys_on_mac(false), vec![(125, false), (97, true)]);
        assert_eq!(c.key(125, false), vec![(97, false)]);
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

/// Close the window that has the keyboard, as the cross on its bar does.
#[no_mangle]
pub extern "C" fn sp_close_window() {
    super::close_requested().store(true, Ordering::SeqCst);
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

/// Whether the microphone has been allowed: until it has, it is not opened for a host.
#[no_mangle]
pub extern "C" fn sp_microphone_allowed(allowed: bool) {
    super::MICROPHONE_ALLOWED.store(allowed, std::sync::atomic::Ordering::Relaxed);
}

/// Dictation with no glasses on: its thread, the first time it is asked for, and what it has said.
fn desktop_dictation() -> &'static std::sync::mpsc::Sender<crate::dictate::Asked> {
    use std::sync::OnceLock;
    static ASK: OnceLock<std::sync::mpsc::Sender<crate::dictate::Asked>> = OnceLock::new();
    ASK.get_or_init(|| {
        let (ask, asked) = std::sync::mpsc::channel();
        std::thread::spawn(move || crate::dictate::on_a_desktop(asked, |said| DICTATION_SAID.lock().unwrap().push_back(said)));
        ask
    })
}

static DICTATION_SAID: Mutex<std::collections::VecDeque<crate::dictate::Said>> = Mutex::new(std::collections::VecDeque::new());

/// Be listening or not, for the front window of the Mac.
#[no_mangle]
pub extern "C" fn sp_dictation_listen(on: bool) {
    let _ = desktop_dictation().send(crate::dictate::Asked::Listen(on));
}

/// What is in front: the application's process, and MacDescribe's account of it.
#[no_mangle]
pub unsafe extern "C" fn sp_dictation_focus(pid: i32, description: *const c_char) {
    let focus = spatiand_voice::Description::from_json(&text(description)).unwrap_or_default();
    let _ = desktop_dictation().send(crate::dictate::Asked::Focus(focus, pid as u64));
}

/// The next thing dictation has said, or NULL: for `sp_dictation_free`. `kind` as in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_dictation_next(kind: *mut i32) -> *mut c_char {
    use crate::dictate::Said;
    let Some(said) = DICTATION_SAID.lock().unwrap().pop_front() else { return std::ptr::null_mut() };
    let (number, words) = match said {
        Said::Text(words) => (1, words),
        Said::Review(words) => (2, words),
        Said::Trouble(words) => (3, words),
        Said::Refused(words) => (4, words),
    };
    *kind = number;
    CString::new(words.replace('\0', "")).unwrap_or_default().into_raw()
}

#[no_mangle]
pub unsafe extern "C" fn sp_dictation_free(words: *mut c_char) {
    if !words.is_null() {
        drop(CString::from_raw(words));
    }
}

thread_local! {
    static AUDIO_BEING_SAID: std::cell::RefCell<Vec<super::AudioDevice>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A level of an application's menu, said from one thread: begin with what it is called,
/// each row, end.
///
/// # Safety
/// `title` must be a valid C string or null.
#[no_mangle]
pub unsafe extern "C" fn sp_menu_begin(title: *const c_char) {
    let title = text(title);
    MENU_BEING_SAID.with(|menu| *menu.borrow_mut() = (title, Vec::new()));
}

/// # Safety
/// `label` and `detail` must be valid C strings or null.
#[no_mangle]
pub unsafe extern "C" fn sp_menu_row(label: *const c_char, detail: *const c_char) {
    let row = (text(label), text(detail));
    MENU_BEING_SAID.with(|menu| menu.borrow_mut().1.push(row));
}

#[no_mangle]
pub extern "C" fn sp_menu_end() {
    let (title, rows) = MENU_BEING_SAID.with(|menu| std::mem::take(&mut *menu.borrow_mut()));
    *super::APP_MENU.lock().unwrap() = Some(super::AppMenuSaid::Level { title, rows });
}

/// What an application of this Mac shows, in answer to [`Asked::Describe`] number `id`.
///
/// # Safety
/// `json` must be a valid C string, or null.
#[no_mangle]
pub unsafe extern "C" fn sp_described(id: u32, json: *const c_char) {
    let json = if json.is_null() { String::new() } else { CStr::from_ptr(json).to_string_lossy().into_owned() };
    super::described(id, json);
}

/// SpatiKey's typing into an application of this Mac, done ([`Asked::TypeText`] number `id`):
/// `error` null, or why not.
///
/// # Safety
/// `error` must be a valid C string, or null.
#[no_mangle]
pub unsafe extern "C" fn sp_typed(id: u32, error: *const c_char) {
    let result = if error.is_null() { Ok(()) } else { Err(CStr::from_ptr(error).to_string_lossy().into_owned()) };
    super::typed(id, result);
}

/// The application's menu is done with: something was chosen, or it was backed out of.
#[no_mangle]
pub extern "C" fn sp_menu_close() {
    *super::APP_MENU.lock().unwrap() = Some(super::AppMenuSaid::Done);
}

thread_local! {
    static MENU_BEING_SAID: std::cell::RefCell<(String, Vec<(String, String)>)> = Default::default();
}

/// The sound devices, said from one thread: begin, each device, end.
#[no_mangle]
pub extern "C" fn sp_audio_devices_begin() {
    AUDIO_BEING_SAID.with(|list| list.borrow_mut().clear());
}

/// # Safety
/// `name` must be a valid C string or null.
#[no_mangle]
pub unsafe extern "C" fn sp_audio_device(id: u32, name: *const c_char, input: bool, current: bool) {
    let name = text(name);
    AUDIO_BEING_SAID.with(|list| list.borrow_mut().push((id, name, input, current)));
}

#[no_mangle]
pub extern "C" fn sp_audio_devices_end() {
    let said = AUDIO_BEING_SAID.with(|list| std::mem::take(&mut *list.borrow_mut()));
    *super::AUDIO_DEVICES.lock().unwrap() = (said, true);
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

/// Put a window on show away into the room's list; SP_WINDOW_HIDDEN follows.
#[no_mangle]
pub extern "C" fn sp_window_hide(id: u32) {
    local::put_away(id);
}

/// Bring a window put away with sp_window_hide back on show where it was; SP_WINDOW_SHOWN follows.
#[no_mangle]
pub extern "C" fn sp_window_restore(id: u32) {
    local::restore(id);
}

/// A window's newest picture, as an `IOSurfaceRef`, which is held for as long as it is shown.
///
/// # Safety
/// `surface` must be a live `IOSurfaceRef`.
#[no_mangle]
pub unsafe extern "C" fn sp_window_picture(id: u32, surface: *const c_void, x: u32, y: u32, width: u32, height: u32) {
    if !surface.is_null() {
        local::picture(id, surface, (x, y, width, height));
    }
}

/// Whether a window's picture has parts that are nothing, to be seen through: it does while a
/// menu or a list hangs beside the window and the picture is of both.
#[no_mangle]
pub extern "C" fn sp_window_clear(id: u32, on: bool) {
    local::clear(id, on);
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

/// A notification to show under the clock: who it is from on its first line and what it says on
/// the second, or nothing to take it away.
///
/// # Safety
/// `text` must be a valid C string or null.
#[no_mangle]
pub unsafe extern "C" fn sp_notice(notice: *const c_char) {
    *super::NOTICE.lock().unwrap() = text(notice);
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
    spatiand_audio::server::feed_local(slot, samples, channels as usize);
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
