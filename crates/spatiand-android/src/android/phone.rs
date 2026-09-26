//! The phone as the Deck's controller.
//!
//! [`PhoneController`] answers what the DRM backend asks of `spatiand_input::DeckController` --
//! `poll`, `state`, `pressed`, `just_pressed`, `pulse`, `rumble` -- from what the phone does:
//!
//! * its touch area is the left pad, and taps and holds are the clicks
//!   (`spatiand_input::phone`);
//! * where the phone points is the right pad: turned into the pad position that aims the Deck's
//!   ray exactly along the phone, so the laser goes where the phone does in the room and not
//!   where the head happens to face (`spatiand_render::ray::pad_for_direction`);
//! * the orange key is STEAM, the button under the touch area is `⋯`, and the back button B.
//!
//! So everything above the controller -- menus, the pointer, window drags, the keyboard -- is
//! the Deck's own code, reading a Deck.

use std::sync::{Mutex, OnceLock};

use glam::{DQuat, DVec3};
use spatiand_input::phone::{Phase, PhoneTouch};
use spatiand_input::{Buttons, Control, ControllerState, Pad};
use spatiand_render::ray::{pad_for_direction, PointerConfig};

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

/// Where the phone points, and where that was when the laser was last aimed.
#[derive(Debug, Clone, Copy)]
struct Aim {
    phone_yaw: f64,
    phone_pitch: f64,
    world_yaw: f64,
    world_pitch: f64,
}

/// The direction the phone's top edge points, as (yaw, pitch) in its own frame.
fn phone_direction(rotation: DQuat) -> (f64, f64) {
    let d = rotation * DVec3::Y;
    (d.y.atan2(d.x), d.z.clamp(-1.0, 1.0).asin())
}

pub struct PhoneController {
    state: ControllerState,
    last_buttons: Buttons,
    held: Buttons,
    pressed: Vec<Control>,
    released: Vec<Control>,
    aim: Option<Aim>,
    pinch: f32,
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
            aim: None,
            pinch: 1.0,
            keys: Vec::new(),
            config: PointerConfig::default(),
        }
    }

    /// Read the phone, for a head facing `head`.
    pub fn poll(&mut self, head: DQuat) {
        self.pressed.clear();
        self.released.clear();
        let (frame, rotation, buttons, keys, reaim) = {
            let mut input = input().lock().unwrap();
            let now = input.started.elapsed().as_millis() as u64;
            let frame = input.touch.frame(now);
            let reaim = std::mem::take(&mut input.reaim);
            (
                frame,
                input.rotation,
                std::mem::take(&mut input.buttons),
                std::mem::take(&mut input.keys),
                reaim,
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

        // The right pad: where the phone points, as the pad position that aims there.
        let right_pad = match rotation {
            Some(rotation) => {
                let (phone_yaw, phone_pitch) = phone_direction(rotation);
                if reaim || self.aim.is_none() {
                    let forward = head * DVec3::X;
                    self.aim = Some(Aim {
                        phone_yaw,
                        phone_pitch,
                        world_yaw: forward.y.atan2(forward.x),
                        world_pitch: forward.z.clamp(-1.0, 1.0).asin(),
                    });
                    log::info!("the phone aims where the head faces");
                }
                let aim = self.aim.unwrap();
                let yaw = aim.world_yaw + (phone_yaw - aim.phone_yaw);
                let pitch = (aim.world_pitch + (phone_pitch - aim.phone_pitch))
                    .clamp(-std::f64::consts::FRAC_PI_2 + 0.01, std::f64::consts::FRAC_PI_2 - 0.01);
                let direction = DVec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin());
                let (x, y) = pad_for_direction(direction, head, &self.config);
                Pad {
                    x,
                    y,
                    touched: true,
                    clicked: frame.right_click,
                    pressure: 0,
                }
            }
            None => Pad {
                clicked: frame.right_click,
                ..Pad::default()
            },
        };
        let mut left_pad = frame.left_pad;
        left_pad.clicked = frame.left_click;

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

    /// The phone has no pads to buzz; a click is felt as the tap it was.
    pub fn pulse(&self, _pad: spatiand_input::HapticPad, _feel: spatiand_input::Feel) {}

    pub fn rumble(&self, _strong: u16, _weak: u16) {}

    /// How much two fingers have spread since this was last asked; 1 for not at all.
    pub fn take_pinch(&mut self) -> f32 {
        std::mem::replace(&mut self.pinch, 1.0)
    }

    /// Keys typed since this was last asked.
    pub fn take_keys(&mut self) -> Vec<Typed> {
        std::mem::take(&mut self.keys)
    }

    /// Aim the laser where the head faces from the next frame, as recentring does for the room.
    pub fn reaim(&mut self) {
        self.aim = None;
    }
}
