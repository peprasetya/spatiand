//! A game controller, run through the same layouts the Deck and the Beam Pro use.
//!
//! The Swift side reads whatever pad is plugged in or paired -- a DualShock 4 over a cable or
//! Bluetooth, say -- and hands over one [`PadIn`] a frame. This is the same arrangement as
//! `spatiand/src/controls.rs`, less what needs a Linux box: the focused application's layout
//! (`spatiand_mapper`, the very same engine and files), run on it, and what comes out is the
//! virtual gamepad's report for a host, keys, mouse buttons and motion, a wheel and Spatiand's
//! own commands, as changes. A PlayStation pad's touchpad is the pointer, as it is on the
//! Beam Pro: by how far the thumb slides, and clicking while it is pressed.

use std::os::raw::c_char;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use spatiand_mapper::{
    AppKey, Button, Command, Engine, Frame, MouseButton, PadButton, Rates, Snapshot, Store,
};

/// How long the guide button is held for it to count as "held": the launcher rather than the menu.
const GUIDE_HOLD: Duration = Duration::from_millis(500);
/// A pad's touchpad is two units across; a unit of slide is this many of the room's mouse points,
/// which makes the whole pad three quarters of the view, as on the Beam Pro.
const POINTS_PER_UNIT: f32 = 320.0;

/// Bits of [`PadIn::buttons`].
pub const B_A: u32 = 1 << 0;
pub const B_B: u32 = 1 << 1;
pub const B_X: u32 = 1 << 2;
pub const B_Y: u32 = 1 << 3;
pub const B_UP: u32 = 1 << 4;
pub const B_DOWN: u32 = 1 << 5;
pub const B_LEFT: u32 = 1 << 6;
pub const B_RIGHT: u32 = 1 << 7;
pub const B_L1: u32 = 1 << 8;
pub const B_R1: u32 = 1 << 9;
pub const B_L2: u32 = 1 << 10;
pub const B_R2: u32 = 1 << 11;
pub const B_SELECT: u32 = 1 << 12;
pub const B_START: u32 = 1 << 13;
pub const B_GUIDE: u32 = 1 << 14;
pub const B_LSTICK: u32 = 1 << 15;
pub const B_RSTICK: u32 = 1 << 16;

/// What a pad reads, in the Deck's terms. Sticks and the touchpad are -1..1 with +y up.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PadIn {
    pub buttons: u32,
    pub lx: f32,
    pub ly: f32,
    pub rx: f32,
    pub ry: f32,
    pub lt: f32,
    pub rt: f32,
    pub touch_x: f32,
    pub touch_y: f32,
    pub touched: i32,
    pub clicked: i32,
    pub has_gyro: i32,
    /// Degrees a second about the pad's X, Y and Z.
    pub gyro: [f32; 3],
    /// A menu is over the world: the layout lets go of everything.
    pub suspended: i32,
}

/// Bits of [`PadOut::commands`], by `Command` order.
pub const C_HUD: u32 = 1;
pub const C_LAUNCHER: u32 = 2;
pub const C_KEYBOARD: u32 = 4;
pub const C_SCREENSHOT: u32 = 8;
pub const C_RECENTRE: u32 = 16;

/// Bits of [`PadOut::pointer`]: the buttons of the pointer, whatever pressed them.
pub const P_LEFT: u32 = 1;
pub const P_RIGHT: u32 = 2;
pub const P_MIDDLE: u32 = 4;

pub const MAX_CHANGES: usize = 32;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PadOut {
    /// The virtual gamepad for a host: bit `i` is `spatiand_pad::BUTTON_CODES[i]`.
    pub pad_buttons: u16,
    /// Bits 0..3: up, down, left, right.
    pub dpad: u8,
    pub left: [f32; 2],
    pub right: [f32; 2],
    pub triggers: [f32; 2],
    /// Keys that changed this frame, by evdev code.
    pub key_count: i32,
    pub key_code: [u32; MAX_CHANGES],
    pub key_down: [u8; MAX_CHANGES],
    /// The pointer's buttons held now, a bit each: see `P_LEFT`.
    pub pointer: u32,
    /// Points of relative mouse motion, from a layout and from a touchpad's slide, +y down.
    pub motion_x: f32,
    pub motion_y: f32,
    /// Wheel notches, +y up.
    pub wheel_x: i32,
    pub wheel_y: i32,
    pub commands: u32,
    /// The guide button was pressed and let go, or held.
    pub guide: i32,
    pub guide_held: i32,
    /// D-pad, A, B and Y pressed this frame, for working a menu: bits 0..3 up, down, left, right, 4 A, 5 B, 6 Y.
    pub menu_presses: u32,
}

impl Default for PadOut {
    fn default() -> Self {
        // SAFETY: plain numbers; all-zero is the neutral report.
        unsafe { std::mem::zeroed() }
    }
}

pub struct Pads {
    store: Store,
    engine: Engine,
    app: AppKey,
    keys: Vec<u16>,
    last_step: Option<Instant>,
    suspended: bool,
    guide_down: Option<(Instant, bool)>,
    was: u32,
    touch_last: Option<(f32, f32)>,
}

impl Pads {
    pub fn new(dir: PathBuf) -> Pads {
        let app = AppKey::App(String::new());
        Pads {
            store: Store::new(dir),
            engine: Engine::new(app.default_layout()),
            app,
            keys: Vec::new(),
            last_step: None,
            suspended: false,
            guide_down: None,
            was: 0,
            touch_last: None,
        }
    }

    /// The application in front changed: its own layout if it has one, the desktop's if not.
    pub fn focus(&mut self, app: &str) {
        let key = AppKey::App(app.to_string());
        if key == self.app {
            return;
        }
        let (layout, own) = self.store.layout_for(&key);
        log::info!("controls for {app}: {} ({})", layout.name, if own { "its own layout" } else { "the default" });
        self.engine.set_layout(layout);
        self.app = key;
    }

    pub fn layout_name(&self) -> String {
        self.engine.layout().name.clone()
    }

    pub fn step(&mut self, input: &PadIn) -> PadOut {
        let snapshot = snapshot_of(input);
        let now = Instant::now();
        let dt = self.last_step.map(|t| now.duration_since(t).as_secs_f64()).unwrap_or(0.0);
        self.last_step = Some(now);
        let suspended = input.suspended != 0;
        let frame = if suspended {
            if !self.suspended {
                self.engine.reset();
            }
            Frame::default()
        } else {
            self.engine.step(&snapshot, dt)
        };
        self.suspended = suspended;

        let mut out = PadOut::default();
        let (buttons, dpad) = report_buttons(&frame);
        out.pad_buttons = buttons;
        out.dpad = dpad;
        out.left = [frame.pad.left.0, frame.pad.left.1];
        out.right = [frame.pad.right.0, frame.pad.right.1];
        out.triggers = [frame.pad.left_trigger, frame.pad.right_trigger];

        // Keys, as changes.
        let mut n = 0;
        for code in &self.keys {
            if !frame.keys.contains(code) && n < MAX_CHANGES {
                out.key_code[n] = *code as u32;
                out.key_down[n] = 0;
                n += 1;
            }
        }
        for code in &frame.keys {
            if !self.keys.contains(code) && n < MAX_CHANGES {
                out.key_code[n] = *code as u32;
                out.key_down[n] = 1;
                n += 1;
            }
        }
        out.key_count = n as i32;
        self.keys = frame.keys.clone();

        // The pointer's buttons. A layout's mouse buttons, a trigger the desktop layout makes a
        // click (the right trigger is the left button, the left trigger the right one), the
        // touchpad's own click, and A, B and X while a thumb is on the touchpad, where the layout
        // has not claimed them -- in a game A is jump, and must not also click.
        let mut pointer = 0u32;
        for b in &frame.mouse_buttons {
            pointer |= match b {
                MouseButton::Left => P_LEFT,
                MouseButton::Right => P_RIGHT,
                MouseButton::Middle => P_MIDDLE,
                _ => 0,
            };
        }
        if frame.pointer_clicks[1] && (input.rt > 0.5 || input.buttons & B_R2 != 0) {
            pointer |= P_LEFT;
        }
        if frame.pointer_clicks[0] && (input.lt > 0.5 || input.buttons & B_L2 != 0) {
            pointer |= P_RIGHT;
        }
        let touched = input.touched != 0;
        if input.clicked != 0 {
            pointer |= P_LEFT;
        }
        if touched && !suspended {
            for (bit, button, p) in [(B_A, Button::A, P_LEFT), (B_B, Button::B, P_RIGHT), (B_X, Button::X, P_MIDDLE)] {
                if input.buttons & bit != 0 && !self.engine.binds(button) {
                    pointer |= p;
                }
            }
        }
        out.pointer = pointer;

        // Motion: a layout's mouse, and the touchpad's slide.
        out.motion_x = frame.mouse_motion.0 as f32;
        out.motion_y = frame.mouse_motion.1 as f32;
        if touched {
            if let Some((x, y)) = self.touch_last {
                out.motion_x += (input.touch_x - x) * POINTS_PER_UNIT;
                out.motion_y -= (input.touch_y - y) * POINTS_PER_UNIT;
            }
            self.touch_last = Some((input.touch_x, input.touch_y));
        } else {
            self.touch_last = None;
        }
        out.wheel_x = frame.wheel.0;
        out.wheel_y = frame.wheel.1;
        for c in &frame.commands {
            out.commands |= match c {
                Command::Hud => C_HUD,
                Command::Launcher => C_LAUNCHER,
                Command::Keyboard => C_KEYBOARD,
                Command::Screenshot => C_SCREENSHOT,
                Command::Recentre => C_RECENTRE,
            };
        }

        // The guide button is the session's: pressed it is the menu, held it is the launcher.
        match (input.buttons & B_GUIDE != 0, self.guide_down) {
            (true, None) => self.guide_down = Some((now, false)),
            (true, Some((since, false))) if now.duration_since(since) >= GUIDE_HOLD => {
                out.guide_held = 1;
                self.guide_down = Some((since, true));
            }
            (false, Some((_, held))) => {
                out.guide = (!held) as i32;
                self.guide_down = None;
            }
            _ => {}
        }
        let fresh = input.buttons & !self.was;
        for (i, bit) in [B_UP, B_DOWN, B_LEFT, B_RIGHT, B_A, B_B, B_Y].iter().enumerate() {
            if fresh & bit != 0 {
                out.menu_presses |= 1 << i;
            }
        }
        self.was = input.buttons;
        out
    }
}

/// The report's buttons in `spatiand_pad::BUTTON_CODES` order, and the D-pad apart.
fn report_buttons(frame: &Frame) -> (u16, u8) {
    const ORDER: [PadButton; 11] = [
        PadButton::A,
        PadButton::B,
        PadButton::X,
        PadButton::Y,
        PadButton::LeftBumper,
        PadButton::RightBumper,
        PadButton::LeftStick,
        PadButton::RightStick,
        PadButton::Start,
        PadButton::Back,
        PadButton::Guide,
    ];
    let p = &frame.pad;
    let mut buttons = 0u16;
    for (i, b) in ORDER.iter().enumerate() {
        if p.is_down(*b) {
            buttons |= 1 << i;
        }
    }
    let mut dpad = 0u8;
    for (i, b) in [PadButton::DpadUp, PadButton::DpadDown, PadButton::DpadLeft, PadButton::DpadRight].iter().enumerate() {
        if p.is_down(*b) {
            dpad |= 1 << i;
        }
    }
    (buttons, dpad)
}

fn snapshot_of(i: &PadIn) -> Snapshot {
    let mut s = Snapshot::default();
    for (bit, button) in [
        (B_A, Button::A),
        (B_B, Button::B),
        (B_X, Button::X),
        (B_Y, Button::Y),
        (B_UP, Button::DpadUp),
        (B_DOWN, Button::DpadDown),
        (B_LEFT, Button::DpadLeft),
        (B_RIGHT, Button::DpadRight),
        (B_L1, Button::L1),
        (B_R1, Button::R1),
        (B_L2, Button::L2),
        (B_R2, Button::R2),
        (B_START, Button::Menu),
        (B_SELECT, Button::View),
        (B_LSTICK, Button::LStick),
        (B_RSTICK, Button::RStick),
    ] {
        s.buttons.set(button, i.buttons & bit != 0);
    }
    s.left_stick = (i.lx, i.ly);
    s.right_stick = (i.rx, i.ry);
    s.left_trigger = i.lt.clamp(0.0, 1.0);
    s.right_trigger = i.rt.clamp(0.0, 1.0);
    if i.has_gyro != 0 {
        // About X is pitch, Y yaw and Z roll, as SDL reads these pads; every gyro mode has invert switches.
        s.gyro = Some(Rates { pitch: i.gyro[0], yaw: i.gyro[1], roll: i.gyro[2] });
    }
    s
}

// MARK: the C interface

pub struct PadsHandle(Mutex<Pads>);

/// Layouts are read from this folder (the Deck's `~/.config/spatiand/layouts`, copied over, works).
#[no_mangle]
pub extern "C" fn sp_pads_new(dir: *const c_char) -> *mut PadsHandle {
    let dir = if dir.is_null() {
        PathBuf::from(".")
    } else {
        // SAFETY: a NUL-terminated string, by the contract.
        PathBuf::from(unsafe { std::ffi::CStr::from_ptr(dir) }.to_string_lossy().into_owned())
    };
    Box::into_raw(Box::new(PadsHandle(Mutex::new(Pads::new(dir)))))
}

#[no_mangle]
pub extern "C" fn sp_pads_free(pads: *mut PadsHandle) {
    if !pads.is_null() {
        // SAFETY: from `sp_pads_new`, freed once.
        drop(unsafe { Box::from_raw(pads) });
    }
}

/// The application in front is this one (its catalogue id); empty for the desktop.
#[no_mangle]
pub extern "C" fn sp_pads_focus(pads: *mut PadsHandle, app: *const c_char) {
    // SAFETY: from `sp_pads_new`, or null; a NUL-terminated string.
    if let (Some(p), false) = (unsafe { pads.as_ref() }, app.is_null()) {
        let app = unsafe { std::ffi::CStr::from_ptr(app) }.to_string_lossy().into_owned();
        p.0.lock().unwrap().focus(&app);
    }
}

/// Run a frame.
#[no_mangle]
pub extern "C" fn sp_pads_step(pads: *mut PadsHandle, input: *const PadIn, out: *mut PadOut) {
    // SAFETY: as above, and readable/writable structs.
    if let (Some(p), Some(input), Some(out)) = (unsafe { pads.as_ref() }, unsafe { input.as_ref() }, unsafe { out.as_mut() }) {
        *out = p.0.lock().unwrap().step(input);
    }
}

/// The name of the layout in force, for the menu. Free it with `sp_free_string`.
#[no_mangle]
pub extern "C" fn sp_pads_layout_name(pads: *mut PadsHandle) -> *mut c_char {
    // SAFETY: from `sp_pads_new`, or null.
    let name = unsafe { pads.as_ref() }.map(|p| p.0.lock().unwrap().layout_name()).unwrap_or_default();
    std::ffi::CString::new(name).unwrap_or_default().into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pads() -> Pads {
        Pads::new(std::env::temp_dir().join("spatiand-no-layouts-here"))
    }

    #[test]
    fn on_the_desktop_the_dpad_types_arrows_and_a_types_enter() {
        let mut p = pads();
        let held = p.step(&PadIn { buttons: B_UP | B_A, ..Default::default() });
        let codes: Vec<u32> = (0..held.key_count as usize).map(|i| held.key_code[i]).collect();
        assert!(codes.contains(&103) && codes.contains(&28), "{codes:?}");
        let rest = p.step(&PadIn::default());
        assert_eq!(rest.key_count, 2);
        assert!(rest.key_down[..2].iter().all(|d| *d == 0));
    }

    #[test]
    fn the_right_trigger_clicks_the_pointer_on_the_desktop_and_the_left_is_a_right_click() {
        let mut p = pads();
        assert_eq!(p.step(&PadIn { rt: 1.0, buttons: B_R2, ..Default::default() }).pointer, P_LEFT);
        assert_eq!(p.step(&PadIn { lt: 1.0, buttons: B_L2, ..Default::default() }).pointer, P_RIGHT);
        assert_eq!(p.step(&PadIn::default()).pointer, 0);
    }

    #[test]
    fn a_touchpad_slides_the_pointer_and_its_press_clicks() {
        let mut p = pads();
        let first = p.step(&PadIn { touched: 1, ..Default::default() });
        assert_eq!((first.motion_x, first.motion_y), (0.0, 0.0));
        let slid = p.step(&PadIn { touched: 1, touch_x: 0.25, touch_y: 0.1, clicked: 1, ..Default::default() });
        assert!((slid.motion_x - 80.0).abs() < 1e-3 && (slid.motion_y + 32.0).abs() < 1e-3, "{} {}", slid.motion_x, slid.motion_y);
        assert_eq!(slid.pointer & P_LEFT, P_LEFT);
        // Lifted, then touched again: no jump from where the thumb was before.
        p.step(&PadIn::default());
        let again = p.step(&PadIn { touched: 1, touch_x: -0.8, touch_y: 0.8, ..Default::default() });
        assert_eq!((again.motion_x, again.motion_y), (0.0, 0.0));
    }

    #[test]
    fn a_thumb_on_the_touchpad_makes_a_a_click_only_where_the_layout_leaves_it_alone() {
        let a = PadIn { buttons: B_A, touched: 1, ..Default::default() };
        // The desktop binds A to Enter, so it is not also a click.
        assert_eq!(pads().step(&a).pointer & P_LEFT, 0);
        // A layout with no meaning for A leaves it to the pointer.
        let mut free = pads();
        let mut layout = spatiand_mapper::templates::desktop();
        for set in &mut layout.sets {
            set.controls.buttons.remove(&Button::A);
        }
        free.engine.set_layout(layout);
        assert_eq!(free.step(&a).pointer & P_LEFT, P_LEFT);
    }

    #[test]
    fn a_gamepad_layout_reports_the_pad_for_the_host() {
        let mut p = pads();
        p.engine.set_layout(spatiand_mapper::templates::gamepad());
        let out = p.step(&PadIn { buttons: B_X | B_LEFT | B_L1, lx: 0.5, ly: -0.25, ..Default::default() });
        assert_eq!(out.pad_buttons, (1 << 2) | (1 << 4));
        assert_eq!(out.dpad, 4);
        assert!(out.left[0] > 0.2);
    }

    #[test]
    fn the_guide_button_is_a_press_or_a_hold() {
        let mut p = pads();
        p.step(&PadIn { buttons: B_GUIDE, ..Default::default() });
        assert_eq!(p.step(&PadIn::default()).guide, 1);
        p.step(&PadIn { buttons: B_GUIDE, ..Default::default() });
        std::thread::sleep(GUIDE_HOLD + Duration::from_millis(30));
        assert_eq!(p.step(&PadIn { buttons: B_GUIDE, ..Default::default() }).guide_held, 1);
        assert_eq!(p.step(&PadIn::default()).guide, 0);
    }
}
