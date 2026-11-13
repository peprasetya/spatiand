//! The Mac's mouse, trackpad and keyboard as the Deck's controller.
//!
//! [`Controller`] answers what the session asks of a controller -- `poll`, `state`, `pressed`,
//! `just_pressed`, `pulse` -- from what a Mac has, as `spatiand-android`'s `PhoneController`
//! does from a phone:
//!
//! * the mouse or trackpad moves the right pad, as a thumb does: relative, and held at the
//!   pad's edge, which is the edge of the view. Its button is the pad's click, the secondary
//!   button the left pad's (the right mouse button in the room, as on the Deck);
//! * scrolling is the wheel, and two fingers spreading are a pinch;
//! * the keyboard is a keyboard: its keys go to whatever has focus;
//! * the app's own shortcuts are STEAM and `⋯`.
//!
//! So everything above the controller -- menus, the pointer, window drags, the keyboard -- is
//! the Deck's own code, reading a Deck.

use std::sync::{Mutex, OnceLock};

use glam::DQuat;
use spatiand_input::{Buttons, Control, ControllerState, Pad, PadPress, PressReader};

/// What the app says the Mac's devices did. Read by the compositor once a frame.
#[derive(Default)]
pub struct MacInput {
    /// The pointer's travel since the last frame, in points, +y down.
    pub travel: (f32, f32),
    /// Buttons down: 1 primary, 2 secondary, 4 middle.
    pub buttons: i32,
    /// Scrolling since the last frame, in wheel notches: (right, up).
    pub wheel: (f32, f32),
    /// How much two fingers have spread since the last frame; 1 for not at all.
    pub pinch: f32,
    /// The app's buttons, as they went down and up, in order.
    pub controls: Vec<(Control, bool)>,
    /// Keys from the keyboard.
    pub keys: Vec<Typed>,
    /// Put the pointer where the head faces.
    pub reaim: bool,
    /// Whether the room has the Mac's pointer at all. Without it there is no laser.
    pub held: bool,
    /// The trackpad's press as it changed, in order, since the last frame (`sp_pad_press`).
    pub presses: Vec<Option<PadPress>>,
    /// The press as it last was, and what reads it from what macOS says.
    pub press_now: Option<PadPress>,
    pub press_reader: PressReader,
    /// A trackpad here has said its presses.
    pub presses_known: bool,
}

pub fn input() -> &'static Mutex<MacInput> {
    static INPUT: OnceLock<Mutex<MacInput>> = OnceLock::new();
    INPUT.get_or_init(|| Mutex::new(MacInput { pinch: 1.0, held: true, ..MacInput::default() }))
}

/// Something typed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Typed {
    /// A key going down or up, as an evdev code.
    Key { code: u32, pressed: bool },
    /// A character with no key behind it. Typed as the key that makes it on a US layout, or into
    /// the shell as itself when the shell is asking for text.
    Char(char),
}

/// The press this frame shows, and the press as it is now, from the press last frame and how it
/// changed since. **A press that came and went between two frames is still shown for one**, as
/// the buttons are (a click is a press this frame and a release the next): what it reached --
/// its fingers, and deep if it went deep at any moment -- is this frame's, and what it is now is
/// the next frame's.
fn this_frame(before: Option<PadPress>, changes: &[Option<PadPress>]) -> (Option<PadPress>, Option<PadPress>) {
    let latest = changes.last().copied().unwrap_or(before);
    let mut shown = before;
    for change in changes {
        shown = match (shown, *change) {
            (Some(s), Some(c)) if s.fingers == c.fingers => Some(PadPress { fingers: c.fingers, deep: s.deep || c.deep }),
            (_, Some(c)) => Some(c),
            (s, None) => s,
        };
    }
    // Nothing new reached: the press as it is now.
    if changes.is_empty() || shown == before && latest.is_none() {
        return (latest, latest);
    }
    (shown, latest)
}

/// The pointer's travel in pad units a point. The pad is two across and so is the view: at
/// this, the width of a trackpad is most of the view. The app's setting scales it.
const MOUSE_GAIN: f32 = 0.0022;

/// How fast the pointer goes for the same travel: the app's setting, 1 as shipped.
static SPEED: Mutex<f32> = Mutex::new(1.0);

pub fn set_speed(speed: f32) {
    *SPEED.lock().unwrap() = speed.clamp(0.2, 5.0);
}

/// How long the pointer may be still before it goes.
/// Where the pointer is on its pad as of the last frame, for tests that trace it.
pub static PAD_NOW: std::sync::Mutex<(f32, f32)> = std::sync::Mutex::new((0.0, 0.0));

const HIDE_AFTER: std::time::Duration = std::time::Duration::from_secs(4);

pub struct Controller {
    state: ControllerState,
    last_buttons: Buttons,
    held: Buttons,
    pressed: Vec<Control>,
    released: Vec<Control>,
    /// Where the right pad is, −1..1 each way.
    pad: (f32, f32),
    recentre: bool,
    /// When the pointer last moved or was pressed.
    stirred: std::time::Instant,
    pinch: f32,
    wheel: (f32, f32),
    keys: Vec<Typed>,
    /// The trackpad's press as of the last frame.
    press: Option<PadPress>,
}

impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}

impl Controller {
    pub fn new() -> Controller {
        Controller {
            state: ControllerState::default(),
            last_buttons: Buttons::default(),
            held: Buttons::default(),
            pressed: Vec::new(),
            released: Vec::new(),
            pad: (0.0, 0.0),
            recentre: false,
            stirred: std::time::Instant::now(),
            pinch: 1.0,
            wheel: (0.0, 0.0),
            keys: Vec::new(),
            press: None,
        }
    }

    /// Read the Mac's devices, for a head facing `head`.
    pub fn poll(&mut self, _head: DQuat) {
        self.pressed.clear();
        self.released.clear();
        let (travel, mouse_buttons, controls, keys, reaim, pointer_held, presses, presses_known) = {
            let mut input = input().lock().unwrap();
            let wheel = std::mem::take(&mut input.wheel);
            self.wheel.0 += wheel.0;
            self.wheel.1 += wheel.1;
            self.pinch *= std::mem::replace(&mut input.pinch, 1.0);
            (
                std::mem::take(&mut input.travel),
                input.buttons,
                std::mem::take(&mut input.controls),
                std::mem::take(&mut input.keys),
                std::mem::take(&mut input.reaim),
                input.held,
                std::mem::take(&mut input.presses),
                input.presses_known,
            )
        };
        let (press, latest) = this_frame(self.press, &presses);
        self.press = latest;
        self.keys.extend(keys);

        // Buttons pressed and released between two frames are still pressed this frame.
        let bit = |control: Control| control.bit().map(|b| 1u64 << b).unwrap_or(0);
        let mut held = self.held.raw();
        for (control, down) in controls {
            if down {
                self.pressed.push(control);
                held |= bit(control);
            } else {
                held &= !bit(control);
            }
        }
        self.held = Buttons::from_raw(held);

        if reaim || std::mem::take(&mut self.recentre) {
            self.pad = (0.0, 0.0);
            self.stirred = std::time::Instant::now();
        }
        if travel != (0.0, 0.0) {
            let gain = MOUSE_GAIN * *SPEED.lock().unwrap();
            self.pad.0 = (self.pad.0 + travel.0 * gain).clamp(-1.0, 1.0);
            self.pad.1 = (self.pad.1 - travel.1 * gain).clamp(-1.0, 1.0);
            self.stirred = std::time::Instant::now();
        }
        if mouse_buttons != 0 {
            self.stirred = std::time::Instant::now();
        }
        // **The pointer goes after a few seconds still**, as the Mac's own does while typing,
        // and comes back at the first movement. A thumb leaving the Deck's pad is the same
        // thing: nothing touched, no laser.
        *PAD_NOW.lock().unwrap() = self.pad;
        let shown = pointer_held && self.stirred.elapsed() < HIDE_AFTER;
        let right_pad = Pad {
            x: self.pad.0,
            y: self.pad.1,
            touched: shown,
            clicked: pointer_held && mouse_buttons & 1 != 0,
            pressure: 0,
        };
        let left_pad = Pad {
            clicked: pointer_held && mouse_buttons & 2 != 0,
            ..Pad::default()
        };

        let mut raw = self.held.raw();
        for (on, control) in [
            (right_pad.touched, Control::RPadTouch),
            (right_pad.clicked, Control::RPadClick),
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
            pad_press: if pointer_held { press } else { None },
            reports_pad_presses: presses_known,
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

    /// A tick under the finger, where the trackpad can give one.
    pub fn pulse(&self, _pad: spatiand_input::HapticPad, feel: spatiand_input::Feel) {
        let code = match feel {
            spatiand_input::Feel::Click => 0,
            spatiand_input::Feel::Tick => 1,
            spatiand_input::Feel::Alert => 2,
        };
        super::ffi::tell_number(super::ffi::Asked::Haptic, code as f64);
    }

    pub fn rumble(&self, _strong: u16, _weak: u16) {}

    /// How much two fingers have spread since this was last asked; 1 for not at all.
    pub fn take_pinch(&mut self) -> f32 {
        std::mem::replace(&mut self.pinch, 1.0)
    }

    /// Move the pointer by this much, in pad units, as a pad's touchpad does: the same pointer,
    /// held at the same edges. Returns where it is now.
    pub fn nudge(&mut self, dx: f32, dy: f32) -> (f32, f32) {
        self.pad.0 = (self.pad.0 + dx).clamp(-1.0, 1.0);
        self.pad.1 = (self.pad.1 + dy).clamp(-1.0, 1.0);
        self.stirred = std::time::Instant::now();
        self.pad
    }

    /// Scrolling since this was last asked, in notches: (right, up).
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

#[cfg(test)]
mod tests {
    use super::*;

    fn p(fingers: u8, deep: bool) -> Option<PadPress> {
        Some(PadPress { fingers, deep })
    }

    #[test]
    fn a_press_between_two_frames_is_shown_for_one() {
        // Down and up before the frame: shown down, and up at the next.
        assert_eq!(this_frame(None, &[p(1, false), None]), (p(1, false), None));
        assert_eq!(this_frame(None, &[]), (None, None));
        // A deep click in and out within a frame: deep this frame, not the next.
        assert_eq!(this_frame(p(1, false), &[p(1, true), p(1, false)]), (p(1, true), p(1, false)));
        assert_eq!(this_frame(p(1, false), &[p(1, true), None]), (p(1, true), None));
        // An ordinary release is at once.
        assert_eq!(this_frame(p(2, false), &[None]), (None, None));
        assert_eq!(this_frame(p(2, true), &[p(2, false), None]), (None, None));
        // Nothing changed: as it was.
        assert_eq!(this_frame(p(3, false), &[]), (p(3, false), p(3, false)));
    }
}
