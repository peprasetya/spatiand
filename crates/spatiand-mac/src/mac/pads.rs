//! Game controllers on the Mac: whatever `GameController` calls one.
//!
//! On the Deck, `spatiand_input::gamepad` reads pads from `/dev/input` itself; a Mac has no such
//! thing. So the app hands over what macOS says -- buttons, sticks, a DualShock's touchpad and
//! gyro -- and this keeps each pad's `GamepadState` in `spatiand_input::gamepad::external()`,
//! where the Deck's `Controls` finds it. From there a pad is the Deck's: its home button is
//! STEAM, its D-pad and face buttons work the menus, its touchpad is the right trackpad, and a
//! remote game is played with it through the layout. The same shape as Android's `pads.rs`.

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

/// A pad's buttons, in the order of the bits the app sends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum Button {
    A = 0,
    B,
    X,
    Y,
    L1,
    R1,
    L2,
    R2,
    Select,
    Start,
    Guide,
    LeftStick,
    RightStick,
    Up,
    Down,
    Left,
    Right,
}

const BUTTONS: [Button; 17] = [
    Button::A,
    Button::B,
    Button::X,
    Button::Y,
    Button::L1,
    Button::R1,
    Button::L2,
    Button::R2,
    Button::Select,
    Button::Start,
    Button::Guide,
    Button::LeftStick,
    Button::RightStick,
    Button::Up,
    Button::Down,
    Button::Left,
    Button::Right,
];

/// Buttons let go of before a frame could see them pressed: (pad, button). Released by
/// [`settle`], after the frame has read them, so a quick press is never lost between frames.
static UNSEEN: std::sync::Mutex<Vec<(i32, i32)>> = std::sync::Mutex::new(Vec::new());
/// Buttons pressed since the last frame, which a release must not overtake.
static FRESH: std::sync::Mutex<Vec<(i32, i32)>> = std::sync::Mutex::new(Vec::new());

/// A button going down or up.
pub fn button(id: i32, code: i32, down: bool) {
    if !down {
        if let Ok(fresh) = FRESH.lock() {
            if fresh.contains(&(id, code)) {
                if let Ok(mut unseen) = UNSEEN.lock() {
                    unseen.push((id, code));
                }
                return;
            }
        }
    } else if let Ok(mut fresh) = FRESH.lock() {
        fresh.push((id, code));
    }
    set_button(id, code, down);
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

fn set_button(id: i32, code: i32, down: bool) {
    let Some(button) = BUTTONS.get(code.max(0) as usize).copied() else { return };
    with_pad(id, |p| match button {
        Button::A => p.a = down,
        Button::B => p.b = down,
        Button::X => p.x = down,
        Button::Y => p.y = down,
        Button::L1 => p.l1 = down,
        Button::R1 => p.r1 = down,
        Button::L2 => p.l2 = down,
        Button::R2 => p.r2 = down,
        Button::Select => p.select = down,
        Button::Start => p.start = down,
        Button::Guide => p.guide = down,
        Button::LeftStick => p.left_stick_click = down,
        Button::RightStick => p.right_stick_click = down,
        Button::Up => p.up = down,
        Button::Down => p.down = down,
        Button::Left => p.left = down,
        Button::Right => p.right = down,
    });
}

/// Sticks −1..1 with +y up, triggers 0..1: as `GameController` scales them.
pub fn axes(id: i32, lx: f32, ly: f32, rx: f32, ry: f32, lt: f32, rt: f32) {
    with_pad(id, |p| {
        p.left_stick = (lx, ly);
        p.right_stick = (rx, ry);
        p.left_trigger = lt;
        p.right_trigger = rt;
    });
}

/// The pad's touchpad, −1..1 with +y up.
pub fn touch(id: i32, x: f32, y: f32, touched: bool, clicked: bool) {
    with_pad(id, |p| p.touchpad = Some(Touchpad { x, y, touched, clicked }));
}

/// The pad's gyro, degrees a second about its X (pitch), Y (yaw) and Z (roll) axes.
pub fn gyro(id: i32, x: f32, y: f32, z: f32) {
    with_pad(id, |p| p.gyro = Some([x, y, z]));
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
