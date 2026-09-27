//! The phone as the Deck's controller.
//!
//! [`PhoneController`] answers what the DRM backend asks of `spatiand_input::DeckController` --
//! `poll`, `state`, `pressed`, `just_pressed`, `pulse`, `rumble` -- from what the phone does:
//!
//! * its touch area is the left pad, and taps and holds are the clicks
//!   (`spatiand_input::phone`);
//! * turning the phone moves the right pad, as a thumb does: relative, and held at the pad's
//!   edge. Turn past the edge and nothing more happens; turn back and the pointer comes back at
//!   once. So the wearer holds the phone however is comfortable, and re-grips by turning past
//!   an edge, as a mouse is lifted and put down again. Pointing the phone *at* things in the
//!   room was tried first: comfortable only while the arm happened to be where it was at the
//!   last recentre;
//! * the orange key is STEAM, the button under the touch area is `⋯`, and the back button B.
//!
//! So everything above the controller -- menus, the pointer, window drags, the keyboard -- is
//! the Deck's own code, reading a Deck.

use std::sync::{Mutex, OnceLock};

use glam::{DQuat, DVec3};
use spatiand_input::phone::{Phase, PhoneTouch};
use spatiand_input::{Buttons, Control, ControllerState, Pad};
use spatiand_render::ray::PointerConfig;

/// What the app says about the phone, from its UI thread. Read by the compositor once a frame.
pub struct PhoneInput {
    pub touch: PhoneTouch,
    /// The phone's orientation, from Android's game rotation vector: +Z up, the yaw its own.
    pub rotation: Option<DQuat>,
    /// Buttons, as they went down and up, in order.
    pub buttons: Vec<(Control, bool)>,
    /// Keys from a keyboard, and text from Android's own.
    pub keys: Vec<Typed>,
    /// Point the laser where the head faces, from now on.
    pub reaim: bool,
    /// A mouse or a keyboard's trackpad: its travel since the last frame in counts, its
    /// buttons (Android's `BUTTON_*` bits) and its wheel, in notches.
    pub mouse_travel: (f32, f32),
    pub mouse_buttons: i32,
    pub wheel: (f32, f32),
    /// Milliseconds since the app started, for the gestures.
    pub started: std::time::Instant,
}

pub fn input() -> &'static Mutex<PhoneInput> {
    static INPUT: OnceLock<Mutex<PhoneInput>> = OnceLock::new();
    INPUT.get_or_init(|| {
        Mutex::new(PhoneInput {
            touch: PhoneTouch::new(),
            rotation: None,
            buttons: Vec::new(),
            keys: Vec::new(),
            reaim: false,
            mouse_travel: (0.0, 0.0),
            mouse_buttons: 0,
            wheel: (0.0, 0.0),
            started: std::time::Instant::now(),
        })
    })
}

pub fn now_ms() -> u64 {
    input().lock().unwrap().started.elapsed().as_millis() as u64
}

/// One touch event on the touch area, in its own 0..1 coordinates.
pub fn touch(phase: Phase, id: i32, x: f32, y: f32) {
    let mut input = input().lock().unwrap();
    let now = input.started.elapsed().as_millis() as u64;
    input.touch.touch(phase, id, x, y, now);
}

/// Something typed on the phone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Typed {
    /// A key going down or up, as an evdev code: from a keyboard, or Android's own keyboard's
    /// Enter and backspace.
    Key { code: u32, pressed: bool },
    /// A character Android's own keyboard committed. Typed as the key that makes it on a US
    /// layout, or into the shell as itself when the shell is asking for text.
    Char(char),
}

/// The direction the phone's top edge points, as (yaw, pitch) in its own frame.
fn phone_direction(rotation: DQuat) -> (f64, f64) {
    let d = rotation * DVec3::Y;
    (d.y.atan2(d.x), d.z.clamp(-1.0, 1.0).asin())
}

/// A mouse's travel, in pad units a count: a thousand counts across half the view.
const MOUSE_GAIN: f32 = 0.001;
/// Android's `MotionEvent.BUTTON_PRIMARY` and `BUTTON_SECONDARY`.
const BUTTON_PRIMARY: i32 = 1;
const BUTTON_SECONDARY: i32 = 2;

/// How long the phone may lie still before its pointer goes.
const HIDE_AFTER: std::time::Duration = std::time::Duration::from_secs(4);
/// How far the pointer must be turned, in pad units, to count as moved: about half a degree.
const WAKE: f32 = 0.025;

pub struct PhoneController {
    state: ControllerState,
    last_buttons: Buttons,
    held: Buttons,
    pressed: Vec<Control>,
    released: Vec<Control>,
    /// Where the right pad is, −1..1 each way, moved by the phone turning.
    pad: (f32, f32),
    /// The phone's direction when last read, to move the pad by the difference.
    last: Option<(f64, f64)>,
    recentre: bool,
    /// Where the pad was when the phone last counted as moved, and when that was.
    rest: ((f32, f32), std::time::Instant),
    pinch: f32,
    wheel: (f32, f32),
    keys: Vec<Typed>,
    config: PointerConfig,
}

impl PhoneController {
    pub fn new() -> PhoneController {
        PhoneController {
            state: ControllerState::default(),
            last_buttons: Buttons::default(),
            held: Buttons::default(),
            pressed: Vec::new(),
            released: Vec::new(),
            pad: (0.0, 0.0),
            last: None,
            recentre: false,
            rest: ((0.0, 0.0), std::time::Instant::now()),
            pinch: 1.0,
            wheel: (0.0, 0.0),
            keys: Vec::new(),
            config: PointerConfig::default(),
        }
    }

    /// Read the phone, for a head facing `head`.
    pub fn poll(&mut self, _head: DQuat) {
        self.pressed.clear();
        self.released.clear();
        let (frame, rotation, buttons, keys, reaim, travel, mouse_buttons) = {
            let mut input = input().lock().unwrap();
            let now = input.started.elapsed().as_millis() as u64;
            let frame = input.touch.frame(now);
            let reaim = std::mem::take(&mut input.reaim);
            let travel = std::mem::take(&mut input.mouse_travel);
            let wheel = std::mem::take(&mut input.wheel);
            self.wheel.0 += wheel.0;
            self.wheel.1 += wheel.1;
            let mouse_buttons = input.mouse_buttons;
            (
                frame,
                input.rotation,
                std::mem::take(&mut input.buttons),
                std::mem::take(&mut input.keys),
                reaim,
                travel,
                mouse_buttons,
            )
        };
        self.pinch *= frame.pinch;
        self.keys.extend(keys);

        // Buttons pressed and released between two frames are still pressed this frame.
        let bit = |control: Control| control.bit().map(|b| 1u64 << b).unwrap_or(0);
        let mut held = self.held.raw();
        for (control, down) in buttons {
            if down {
                self.pressed.push(control);
                held |= bit(control);
            } else {
                held &= !bit(control);
            }
        }
        self.held = Buttons::from_raw(held);

        // The right pad, moved by how far the phone turned since the last frame, as far as
        // the pad's edge and no further.
        let right_pad = match rotation {
            Some(rotation) => {
                let (yaw, pitch) = phone_direction(rotation);
                if reaim || std::mem::take(&mut self.recentre) {
                    self.pad = (0.0, 0.0);
                }
                if let Some((last_yaw, last_pitch)) = self.last {
                    let mut turned = yaw - last_yaw;
                    if turned > std::f64::consts::PI {
                        turned -= std::f64::consts::TAU;
                    } else if turned < -std::f64::consts::PI {
                        turned += std::f64::consts::TAU;
                    }
                    let raised = pitch - last_pitch;
                    // The same scale `ray_from_pad` reads with: the pad's edge is the edge of
                    // the view, so the pointer turns exactly as far as the phone does.
                    self.pad.0 = (self.pad.0 - (turned / self.config.half_fov_x_deg.to_radians()) as f32).clamp(-1.0, 1.0);
                    self.pad.1 = (self.pad.1 + (raised / self.config.half_fov_y_deg.to_radians()) as f32).clamp(-1.0, 1.0);
                }
                self.last = Some((yaw, pitch));
                if travel != (0.0, 0.0) {
                    self.pad.0 = (self.pad.0 + travel.0 * MOUSE_GAIN).clamp(-1.0, 1.0);
                    self.pad.1 = (self.pad.1 - travel.1 * MOUSE_GAIN).clamp(-1.0, 1.0);
                    self.rest = (self.pad, std::time::Instant::now());
                }
                // **The pointer goes after a few seconds still**, as a mouse's does, and comes
                // back at the first real turn or touch. A thumb leaving the Deck's pad is the
                // same thing: nothing touched, no laser. A phone put down on the desk drifts by
                // hundredths of a degree, so only a turn past `WAKE` counts.
                let (anchor, _) = self.rest;
                let turned = (self.pad.0 - anchor.0).abs().max((self.pad.1 - anchor.1).abs());
                if turned > WAKE || frame.left_pad.touched || frame.right_click || frame.left_click || reaim || mouse_buttons != 0 {
                    self.rest = (self.pad, std::time::Instant::now());
                }
                let shown = self.rest.1.elapsed() < HIDE_AFTER;
                Pad {
                    x: self.pad.0,
                    y: self.pad.1,
                    touched: shown,
                    clicked: frame.right_click,
                    pressure: 0,
                }
            }
            None => Pad {
                clicked: frame.right_click,
                ..Pad::default()
            },
        };
        let mut right_pad = right_pad;
        right_pad.clicked |= mouse_buttons & BUTTON_PRIMARY != 0;
        let mut left_pad = frame.left_pad;
        left_pad.clicked = frame.left_click || mouse_buttons & BUTTON_SECONDARY != 0;

        let mut raw = self.held.raw();
        for (on, control) in [
            (right_pad.touched, Control::RPadTouch),
            (right_pad.clicked, Control::RPadClick),
            (left_pad.touched, Control::LPadTouch),
            (left_pad.clicked, Control::LPadClick),
        ] {
            if on {
                raw |= bit(control);
            }
        }
        let buttons = Buttons::from_raw(raw);
        for control in buttons.pressed_since(self.last_buttons) {
            if !self.pressed.contains(&control) {
                self.pressed.push(control);
            }
        }
        self.released.extend(buttons.released_since(self.last_buttons));
        self.last_buttons = buttons;
        self.state = ControllerState {
            sequence: self.state.sequence.wrapping_add(1),
            buttons,
            left_pad,
            right_pad,
            ..ControllerState::default()
        };
    }

    pub fn state(&self) -> &ControllerState {
        &self.state
    }

    pub fn pressed(&self) -> &[Control] {
        &self.pressed
    }

    pub fn released(&self) -> &[Control] {
        &self.released
    }

    pub fn just_pressed(&self, control: Control) -> bool {
        self.pressed.contains(&control)
    }

    /// Buzz the phone, as the Deck buzzes the pad under the thumb: the phone is both pads.
    pub fn pulse(&self, _pad: spatiand_input::HapticPad, feel: spatiand_input::Feel) {
        buzz(feel);
    }

    pub fn rumble(&self, _strong: u16, _weak: u16) {}

    /// How much two fingers have spread since this was last asked; 1 for not at all.
    pub fn take_pinch(&mut self) -> f32 {
        std::mem::replace(&mut self.pinch, 1.0)
    }

    /// Move the pointer by this much, in pad units, as another pad's touchpad does: the same
    /// pointer, held at the same edges. Returns where it is now.
    pub fn nudge(&mut self, dx: f32, dy: f32) -> (f32, f32) {
        self.pad.0 = (self.pad.0 + dx).clamp(-1.0, 1.0);
        self.pad.1 = (self.pad.1 + dy).clamp(-1.0, 1.0);
        self.rest = (self.pad, std::time::Instant::now());
        self.pad
    }

    /// A mouse wheel's turning since this was last asked, in notches: (right, up).
    pub fn take_wheel(&mut self) -> (f32, f32) {
        std::mem::take(&mut self.wheel)
    }

    /// Keys typed since this was last asked.
    pub fn take_keys(&mut self) -> Vec<Typed> {
        std::mem::take(&mut self.keys)
    }

    /// Put the pointer in the middle of the view, as recentring does for the room.
    pub fn reaim(&mut self) {
        self.recentre = true;
    }
}

/// A buzz asked for and not yet played: 0 click, 1 tick, 2 alert.
static BUZZ: (Mutex<Option<i32>>, std::sync::Condvar) = (Mutex::new(None), std::sync::Condvar::new());

/// Ask the app to buzz the phone. The newest ask wins: two in one frame are felt as one.
pub fn buzz(feel: spatiand_input::Feel) {
    let code = match feel {
        spatiand_input::Feel::Click => 0,
        spatiand_input::Feel::Tick => 1,
        spatiand_input::Feel::Alert => 2,
    };
    if let Ok(mut pending) = BUZZ.0.lock() {
        *pending = Some(code);
        BUZZ.1.notify_one();
    }
}

/// The next buzz to play, waiting up to `timeout` for one; −1 for none.
///
/// The app's own thread sits here, so a key's buzz is played the moment it is pressed rather
/// than at the next look of a timer.
pub fn next_buzz(timeout: std::time::Duration) -> i32 {
    let Ok(pending) = BUZZ.0.lock() else { return -1 };
    let Ok((mut pending, _)) = BUZZ.1.wait_timeout_while(pending, timeout, |p| p.is_none()) else {
        return -1;
    };
    pending.take().unwrap_or(-1)
}

/// A mouse or a trackpad, captured from Android's pointer: travel in counts (+y down),
/// Android's button bits, and the wheel in notches (+ up, + right).
pub fn mouse(dx: f32, dy: f32, buttons: i32, wheel_up: f32, wheel_right: f32) {
    let mut input = input().lock().unwrap();
    input.mouse_travel.0 += dx;
    input.mouse_travel.1 += dy;
    input.mouse_buttons = buttons;
    input.wheel.0 += wheel_right;
    input.wheel.1 += wheel_up;
}
