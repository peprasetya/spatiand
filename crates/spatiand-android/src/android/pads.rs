//! Gamepads on Android: a DualShock 4 or anything else Android calls a gamepad.
//!
//! On the Deck, `spatiand_input::gamepad` reads pads from `/dev/input` itself; an Android app
//! may not. So the app hands over what Android's `InputDevice` events say -- buttons, sticks,
//! the touchpad through pointer capture, the gyro through the device's own sensors -- and this
//! keeps each pad's `GamepadState` in `spatiand_input::gamepad::external()`, where the Deck's
//! `Controls` finds it beside any it read itself. From there a pad is the Deck's: its home
//! button is STEAM, its D-pad and face buttons work the menus, its touchpad is the right
//! trackpad, and a remote game is played with it through the layout.

use std::sync::atomic::{AtomicU32, Ordering};

use spatiand_input::gamepad::{external, GamepadState, Touchpad};

fn with_pad(id: i32, f: impl FnOnce(&mut GamepadState)) {
    let Ok(mut pads) = external().lock() else { return };
    if let Some((_, state)) = pads.iter_mut().find(|(i, _)| *i == id) {
        f(state);
        return;
    }
    log::info!("controller {id} arrived");
    let mut state = GamepadState::default();
    f(&mut state);
    pads.push((id, state));
}

/// Buttons let go of before a frame could see them pressed: (pad, code). Released by
/// [`settle`], after the frame has read them, so a quick press is never lost between frames.
static UNSEEN: std::sync::Mutex<Vec<(i32, i32)>> = std::sync::Mutex::new(Vec::new());
/// Buttons pressed since the last frame, which a release must not overtake.
static FRESH: std::sync::Mutex<Vec<(i32, i32)>> = std::sync::Mutex::new(Vec::new());

/// A button, by Android's `KeyEvent` code. Returns whether it is a gamepad's.
pub fn button(id: i32, code: i32, down: bool) -> bool {
    if !down {
        if let Ok(fresh) = FRESH.lock() {
            if fresh.contains(&(id, code)) {
                if let Ok(mut unseen) = UNSEEN.lock() {
                    unseen.push((id, code));
                }
                return is_pad_button(code);
            }
        }
    } else if let Ok(mut fresh) = FRESH.lock() {
        fresh.push((id, code));
    }
    set_button(id, code, down)
}

fn is_pad_button(code: i32) -> bool {
    matches!(code, 96 | 97 | 99 | 100 | 102..=110 | 19..=22)
}

/// Once a frame, after the pads were read: presses have been seen, and the releases that
/// waited for that are applied.
pub fn settle() {
    if let Ok(mut fresh) = FRESH.lock() {
        fresh.clear();
    }
    let released = UNSEEN.lock().map(|mut u| std::mem::take(&mut *u)).unwrap_or_default();
    for (id, code) in released {
        set_button(id, code, false);
    }
}

fn set_button(id: i32, code: i32, down: bool) -> bool {
    let mut taken = true;
    with_pad(id, |p| match code {
        96 => p.a = down,              // BUTTON_A: cross
        97 => p.b = down,              // BUTTON_B: circle
        99 => p.x = down,              // BUTTON_X: square
        100 => p.y = down,             // BUTTON_Y: triangle
        102 => p.l1 = down,
        103 => p.r1 = down,
        104 => p.l2 = down,
        105 => p.r2 = down,
        106 => p.left_stick_click = down,
        107 => p.right_stick_click = down,
        108 => p.start = down,         // Options
        109 => p.select = down,        // Share
        110 => p.guide = down,         // PS: STEAM
        19 => p.up = down,
        20 => p.down = down,
        21 => p.left = down,
        22 => p.right = down,
        _ => taken = false,
    });
    taken
}

/// Sticks, triggers and the hat, as Android scales them: sticks −1..1 with +y down, triggers
/// 0..1, the hat −1, 0 or 1.
#[allow(clippy::too_many_arguments)]
pub fn axes(id: i32, lx: f32, ly: f32, rx: f32, ry: f32, lt: f32, rt: f32, hat_x: f32, hat_y: f32) {
    with_pad(id, |p| {
        p.left_stick = (lx, -ly);
        p.right_stick = (rx, -ry);
        p.left_trigger = lt;
        p.right_trigger = rt;
        // The DualShock's D-pad is a hat; Android also sends it as keys, which set the same.
        p.left = hat_x < -0.5;
        p.right = hat_x > 0.5;
        p.up = hat_y < -0.5;
        p.down = hat_y > 0.5;
    });
}

/// The pad's touchpad, 0..1 from its top-left, captured from Android's pointer.
pub fn touch(id: i32, x: f32, y: f32, touched: bool, clicked: bool) {
    with_pad(id, |p| {
        p.touchpad = Some(Touchpad {
            x: x * 2.0 - 1.0,
            y: 1.0 - y * 2.0,
            touched,
            clicked,
        });
    });
}

/// The pad's gyro, from its own sensor, radians a second about its three axes in the order
/// the driver gives them -- the order `spatiand_input::gamepad` reads them in on Linux.
pub fn gyro(id: i32, x: f32, y: f32, z: f32) {
    let to_degrees = 180.0 / std::f32::consts::PI;
    with_pad(id, |p| p.gyro = Some([x * to_degrees, y * to_degrees, z * to_degrees]));
}

pub fn gone(id: i32) {
    if let Ok(mut pads) = external().lock() {
        let before = pads.len();
        pads.retain(|(i, _)| *i != id);
        if pads.len() != before {
            log::info!("controller {id} left");
        }
    }
}

/// What a game asked the motors to do, strong and weak, for the app to play on the pad.
static RUMBLE: AtomicU32 = AtomicU32::new(0);
static RUMBLE_NEW: AtomicU32 = AtomicU32::new(0);

pub fn set_rumble(strong: u16, weak: u16) {
    RUMBLE.store(((strong as u32) << 16) | weak as u32, Ordering::Relaxed);
    RUMBLE_NEW.store(1, Ordering::Relaxed);
}

/// The rumble asked for since the last look, packed strong << 16 | weak, or −1 for none.
pub fn take_rumble() -> i64 {
    if RUMBLE_NEW.swap(0, Ordering::Relaxed) == 0 {
        return -1;
    }
    RUMBLE.load(Ordering::Relaxed) as i64
}
