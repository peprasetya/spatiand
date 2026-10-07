//! Controllers: a gamepad standing in for a pair of hands.
//!
//! Nothing here tracks a hand, and a game written for two motion controllers still needs
//! *something* to hold. So the pad that is already present -- Steam's virtual gamepad on the Deck,
//! `Spatiand Gamepad` on a host, which is what the wearer's real controls arrive as -- is presented
//! as a pair of controllers:
//!
//!   * each **hand rests in front of the head**, where a hand holding a controller would, and
//!     points where the head points -- so a laser from it lands where the wearer is looking, which
//!     is enough to use a menu with a trigger to click;
//!   * the **sticks, triggers, bumpers and face buttons** are the controllers' own, by the layout
//!     of a Touch controller, which is what most games bind first.
//!
//! The mapping is in plain functions of the pad's state, so it can be tested without a pad.
//!
//! Reading the pad is done straight from its evdev node, read-only and non-exclusive: the game's own
//! input system reads the same device, and neither takes it from the other.

use std::collections::HashSet;
use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use crate::pose::{Eye, Pose};

const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;

const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_Z: u16 = 0x02;
const ABS_RX: u16 = 0x03;
const ABS_RY: u16 = 0x04;
const ABS_RZ: u16 = 0x05;
const ABS_RUDDER: u16 = 0x07;
const ABS_WHEEL: u16 = 0x08;
const ABS_HAT0X: u16 = 0x10;
const ABS_HAT0Y: u16 = 0x11;

const BTN_SOUTH: u16 = 0x130;
const BTN_EAST: u16 = 0x131;
const BTN_NORTH: u16 = 0x133;
const BTN_WEST: u16 = 0x134;
const BTN_TL: u16 = 0x136;
const BTN_TR: u16 = 0x137;
const BTN_TL2: u16 = 0x138;
const BTN_TR2: u16 = 0x139;
const BTN_SELECT: u16 = 0x13a;
const BTN_START: u16 = 0x13b;
const BTN_THUMBL: u16 = 0x13d;
const BTN_THUMBR: u16 = 0x13e;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hand {
    Left,
    Right,
}

/// What the pad is doing, normalised.
#[derive(Debug, Clone, Default)]
pub struct PadState {
    /// Sticks, -1..1, with up positive: left x, left y, right x, right y.
    pub sticks: [f32; 4],
    /// Triggers, 0..1: left, right.
    pub triggers: [f32; 2],
    pub dpad: [f32; 2],
    pub down: HashSet<u16>,
    /// The wearer's pointer buttons, left and right: what Spatiand's own pointer clicks with. The
    /// left one pulls the right hand's trigger and the right one the left hand's, so a menu a
    /// game draws can be used with the pointer that is already in the wearer's hand.
    pub mouse: [bool; 2],
    /// Where the gyro aims the right hand, relative to the head: `[right, up]`, -1..1 for -90..90
    /// degrees. The pad's first two spare axes; zero when nothing is aiming it.
    pub hand: [f32; 2],
}

impl PadState {
    pub fn button(&self, code: u16) -> bool {
        self.down.contains(&code)
    }

    fn stick(&self, hand: Hand) -> [f32; 2] {
        match hand {
            Hand::Left => [self.sticks[0], self.sticks[1]],
            Hand::Right => [self.sticks[2], self.sticks[3]],
        }
    }

    fn trigger(&self, hand: Hand) -> f32 {
        let (analog, digital) = match hand {
            Hand::Left => (self.triggers[0], self.button(BTN_TL2) || self.mouse[1]),
            Hand::Right => (self.triggers[1], self.button(BTN_TR2) || self.mouse[0]),
        };
        if digital { 1.0f32.max(analog) } else { analog }
    }
}

/// One component's value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Bool(bool),
    Float(f32),
    Vec2([f32; 2]),
}

impl Value {
    pub fn as_bool(self) -> bool {
        match self {
            Value::Bool(b) => b,
            Value::Float(f) => f > 0.5,
            Value::Vec2(v) => v[0].abs().max(v[1].abs()) > 0.5,
        }
    }

    pub fn as_float(self) -> f32 {
        match self {
            Value::Bool(b) => b as u32 as f32,
            Value::Float(f) => f,
            Value::Vec2(v) => (v[0] * v[0] + v[1] * v[1]).sqrt().min(1.0),
        }
    }

    pub fn as_vec2(self) -> [f32; 2] {
        match self {
            Value::Vec2(v) => v,
            Value::Float(f) => [f, 0.0],
            Value::Bool(b) => [b as u32 as f32, 0.0],
        }
    }
}

/// The interaction profiles this can stand in for, best first. A game binds the ones it knows and
/// the first of these it has bound is the one it is told it has.
pub const PROFILES: [&str; 5] = [
    "/interaction_profiles/oculus/touch_controller",
    "/interaction_profiles/valve/index_controller",
    "/interaction_profiles/microsoft/motion_controller",
    "/interaction_profiles/htc/vive_controller",
    "/interaction_profiles/khr/simple_controller",
];

/// Which hand a binding path is for, and the rest of it: `/user/hand/left/input/trigger/value`
/// is `(Left, "trigger/value")`.
pub fn split(path: &str) -> Option<(Hand, &str)> {
    let rest = path.strip_prefix("/user/hand/")?;
    let (hand, rest) = rest.split_once('/')?;
    let hand = match hand {
        "left" => Hand::Left,
        "right" => Hand::Right,
        _ => return None,
    };
    let rest = rest.strip_prefix("input/").or_else(|| rest.strip_prefix("output/"))?;
    Some((hand, rest))
}

/// What a binding path reads from the pad, or `None` if this pad has nothing for it.
///
/// A layout like a Touch controller's: the trigger is a trigger, the grip a bumper, the thumbstick
/// a stick; `a`/`b` are the right hand's face buttons and `x`/`y` the left's.
pub fn read(path: &str, pad: &PadState) -> Option<Value> {
    let (hand, name) = split(path)?;
    let (face_low, face_high) = match hand {
        Hand::Right => (BTN_SOUTH, BTN_EAST),
        Hand::Left => (BTN_NORTH, BTN_WEST),
    };
    let bumper = match hand {
        Hand::Left => BTN_TL,
        Hand::Right => BTN_TR,
    };
    let click = match hand {
        Hand::Left => BTN_THUMBL,
        Hand::Right => BTN_THUMBR,
    };
    let stick = pad.stick(hand);
    Some(match name {
        "trigger/value" => Value::Float(pad.trigger(hand)),
        "trigger/click" | "select/click" | "trigger" => Value::Bool(pad.trigger(hand) > 0.6),
        "trigger/touch" => Value::Bool(pad.trigger(hand) > 0.05),
        "squeeze/value" => Value::Float(pad.button(bumper) as u32 as f32),
        "squeeze/click" | "grip/click" | "squeeze" => Value::Bool(pad.button(bumper)),
        "thumbstick" | "trackpad" => Value::Vec2(stick),
        "thumbstick/x" | "trackpad/x" => Value::Float(stick[0]),
        "thumbstick/y" | "trackpad/y" => Value::Float(stick[1]),
        "thumbstick/click" | "trackpad/click" => Value::Bool(pad.button(click)),
        "thumbstick/touch" | "trackpad/touch" => Value::Bool(stick[0].abs().max(stick[1].abs()) > 0.1),
        // Touch's right-hand face buttons are `a` and `b`, its left's `x` and `y`; Index calls
        // them `a`/`b` on both. Taking each by hand covers both.
        "a/click" | "x/click" => Value::Bool(pad.button(face_low)),
        "b/click" | "y/click" => Value::Bool(pad.button(face_high)),
        "a/touch" | "x/touch" => Value::Bool(pad.button(face_low)),
        "b/touch" | "y/touch" => Value::Bool(pad.button(face_high)),
        "menu/click" | "menu" => Value::Bool(match hand {
            Hand::Left => pad.button(BTN_START),
            Hand::Right => pad.button(BTN_SELECT),
        }),
        "system/click" => Value::Bool(pad.button(BTN_SELECT)),
        _ => return None,
    })
}

/// Whether a binding path is for a hand's pose, and which of the two.
pub fn pose_kind(name: &str) -> Option<&'static str> {
    match name {
        "grip/pose" => Some("grip"),
        "aim/pose" => Some("aim"),
        "palm_ext/pose" => Some("grip"),
        _ => None,
    }
}

/// Where a hand is: a little to its side, down and forward of the head, turned the way the head is.
/// A laser from it, then, goes where the wearer is looking.
pub fn hand_pose(hand: Hand, head: &Pose) -> Pose {
    let side = match hand {
        Hand::Left => -0.22,
        Hand::Right => 0.22,
    };
    let offset = Pose {
        orientation: glam::Quat::IDENTITY,
        position: glam::Vec3::new(side, -0.28, -0.35),
    };
    head.then(&offset)
}

/// A ray from an eye through a point of the picture's left half, in the world the poses are in:
/// where the wearer's pointer is pointing. `at` is in picture pixels, as the compositor sends it;
/// the picture is `picture` pixels, both eyes side by side.
///
/// The inverse of how the compositor turns the pointer's ray into a point on the picture (the
/// `Room` in its pointer module), so the pixel it says is the pixel this ray goes through.
pub fn pointer_ray(eye: &Eye, picture: (u32, u32), at: (f64, f64)) -> (glam::Vec3, glam::Vec3) {
    let u = (at.0 / (picture.0 as f64 / 2.0)).clamp(0.0, 1.0) as f32;
    let v = (at.1 / picture.1.max(1) as f64).clamp(0.0, 1.0) as f32;
    let [left, right, up, down] = eye.fov.map(|a| a.tan());
    let direction = glam::Vec3::new(left + u * (right - left), up - v * (up - down), -1.0).normalize();
    (eye.pose.position, eye.pose.orientation * direction)
}

/// How far along a ray the nearest of these rectangles is: each a pose (it faces +Z) and a width
/// and height. A laser from a hand ends where the pointer is, which is on whatever the game has
/// put in front of the wearer -- its menu, a quad layer -- and the runtime knows where those are.
pub fn nearest_quad(origin: glam::Vec3, direction: glam::Vec3, quads: &[(Pose, [f32; 2])]) -> Option<f32> {
    quads
        .iter()
        .filter_map(|(pose, size)| {
            let normal = pose.orientation * glam::Vec3::Z;
            let facing = direction.dot(normal);
            if facing.abs() < 1e-4 {
                return None;
            }
            let t = (pose.position - origin).dot(normal) / facing;
            if t < 0.05 {
                return None;
            }
            let local = pose.orientation.inverse() * (origin + direction * t - pose.position);
            (local.x.abs() <= size[0] / 2.0 && local.y.abs() <= size[1] / 2.0).then_some(t)
        })
        .min_by(|a, b| a.total_cmp(b))
}

/// A quad layer as the game placed it and as it is shown.
///
/// A glasses' field is a fraction of a headset's, and a game lays its panels out for the headset: the
/// tutorial of X8 is eight metres square at five, which is four times as tall as the glasses see.
/// Such a panel is shown smaller and in front of the wearer so that all of it is in view and its
/// buttons can be reached; `actual` is where the game thinks it is, which is where its hands must
/// point for a button on it to be hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuadPlace {
    pub shown: Pose,
    pub shown_size: [f32; 2],
    pub actual: Pose,
    pub actual_size: [f32; 2],
}

/// How much of the field of view a fitted panel is let take, as the angle from the middle.
const FIT_FRACTION: f32 = 0.85;

/// Where to show a quad the game has placed at `placed`: where it is when it is no bigger than the
/// field, and otherwise smaller to fit it and in front of the head. `fov` is the eye's, as `XrFovf`.
pub fn fit_quad(head: &Pose, fov: [f32; 4], placed: &Pose, size: [f32; 2]) -> QuadPlace {
    let unchanged = QuadPlace { shown: *placed, shown_size: size, actual: *placed, actual_size: size };
    let local = head.inverse().then(placed).position;
    let depth = local.length();
    if local.z > -0.1 || depth < 0.2 || size[0] <= 0.0 || size[1] <= 0.0 {
        return unchanged;
    }
    let (half_x, half_y) = (fov[1].min(-fov[0]).max(0.05), fov[2].min(-fov[3]).max(0.05));
    let (wide, tall) = (((size[0] / 2.0) / depth).atan(), ((size[1] / 2.0) / depth).atan());
    if wide <= half_x * FIT_FRACTION && tall <= half_y * FIT_FRACTION {
        return unchanged;
    }
    let scale = (((half_x * FIT_FRACTION).tan() * depth) / (size[0] / 2.0))
        .min(((half_y * FIT_FRACTION).tan() * depth) / (size[1] / 2.0))
        .clamp(0.05, 1.0);
    QuadPlace {
        shown: Pose {
            orientation: head.orientation,
            position: head.position + head.orientation * glam::Vec3::new(0.0, 0.0, -depth),
        },
        shown_size: [size[0] * scale, size[1] * scale],
        actual: *placed,
        actual_size: size,
    }
}

/// What a ray meets of the quads as they are shown: how far along it, and the place on the game's
/// own quad that this is -- which is the place the game's hand has to point at.
pub fn pointer_target(origin: glam::Vec3, direction: glam::Vec3, quads: &[QuadPlace]) -> Option<(f32, glam::Vec3)> {
    quads
        .iter()
        .filter_map(|q| {
            let normal = q.shown.orientation * glam::Vec3::Z;
            let facing = direction.dot(normal);
            if facing.abs() < 1e-4 {
                return None;
            }
            let t = (q.shown.position - origin).dot(normal) / facing;
            if t < 0.05 {
                return None;
            }
            let local = q.shown.orientation.inverse() * (origin + direction * t - q.shown.position);
            if local.x.abs() > q.shown_size[0] / 2.0 || local.y.abs() > q.shown_size[1] / 2.0 {
                return None;
            }
            // The same place on the game's quad, which may be larger.
            let there = glam::Vec3::new(
                local.x * q.actual_size[0] / q.shown_size[0],
                local.y * q.actual_size[1] / q.shown_size[1],
                0.0,
            );
            Some((t, q.actual.position + q.actual.orientation * there))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
}

/// A hand resting where [`hand_pose`] puts it, turned by `hand` (right and up, -1..1 for -90..90
/// degrees) from where the head points: what a gyro does. `None` while nothing aims it.
pub fn aim_hand_by(hand: Hand, head: &Pose, aim: [f32; 2]) -> Option<Pose> {
    if aim[0].abs() < GYRO_AIM_REST && aim[1].abs() < GYRO_AIM_REST {
        return None;
    }
    let rest = hand_pose(hand, head);
    let (yaw, pitch) = (aim[0] * std::f32::consts::FRAC_PI_2, aim[1] * std::f32::consts::FRAC_PI_2);
    // Right is a turn the other way about +Y in OpenXR's frame, up a turn about +X.
    let turn = glam::Quat::from_rotation_y(-yaw) * glam::Quat::from_rotation_x(pitch);
    // An arm swung, not a wrist turned: the hand is where the arm puts it, so a hand pointed well
    // down is down at the wearer's waist, which is where a game looks for a holster or a pocket.
    // At rest ahead it is exactly where it was.
    let swung = turn * glam::Vec3::NEG_Z - glam::Vec3::NEG_Z;
    Some(Pose {
        orientation: (head.orientation * turn).normalize(),
        position: rest.position + head.orientation * (swung * ARM_REACH),
    })
}

/// How long the arm is that swings a gyro-aimed hand, in metres.
const ARM_REACH: f32 = 0.5;

/// Below this the gyro is not aiming the hand: the axes are at rest, which is not exactly zero.
const GYRO_AIM_REST: f32 = 0.004;

/// A hand resting where [`hand_pose`] puts it, turned to point at a place: the laser it casts
/// ends there. The turn is made in the head's own frame so the hand keeps the head's roll.
pub fn aim_hand_at(hand: Hand, head: &Pose, target: glam::Vec3) -> Pose {
    let rest = hand_pose(hand, head);
    let inverse = head.inverse();
    let local_target = inverse.then(&Pose { orientation: glam::Quat::IDENTITY, position: target }).position;
    let local_hand = inverse.then(&rest).position;
    let toward = (local_target - local_hand).normalize_or_zero();
    if toward == glam::Vec3::ZERO {
        return rest;
    }
    Pose {
        orientation: (head.orientation * glam::Quat::from_rotation_arc(glam::Vec3::NEG_Z, toward)).normalize(),
        position: rest.position,
    }
}

// ---- reading the pad ------------------------------------------------------------------------------

struct Axis {
    code: u16,
    min: i32,
    max: i32,
}

/// An opened pad.
pub struct Pad {
    fd: OwnedFd,
    axes: Vec<Axis>,
    pub name: String,
}

/// `EVIOCGABS(abs)`: `_IOR('E', 0x40 + abs, struct input_absinfo)`.
fn eviocgabs(abs: u16) -> libc::c_ulong {
    ((2u64 << 30) | (24u64 << 16) | (0x45u64 << 8) | (0x40 + abs as u64)) as libc::c_ulong
}

/// `EVIOCGNAME(len)`.
fn eviocgname(len: usize) -> libc::c_ulong {
    ((2u64 << 30) | ((len as u64) << 16) | (0x45u64 << 8) | 0x06) as libc::c_ulong
}

impl Pad {
    /// The first pad that looks like the one this machine's games are given.
    pub fn open() -> Option<Pad> {
        let mut candidates = Vec::new();
        for entry in std::fs::read_dir("/dev/input").ok()?.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("event") {
                candidates.push(entry.path());
            }
        }
        candidates.sort();
        let mut best: Option<(u32, Pad)> = None;
        for path in candidates {
            let Ok(c) = CString::new(path.to_string_lossy().as_bytes()) else { continue };
            // SAFETY: opening a path read-only and non-blocking.
            let raw = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC) };
            if raw < 0 {
                continue;
            }
            // SAFETY: a fresh descriptor, ours.
            let fd = unsafe { OwnedFd::from_raw_fd(raw) };
            let mut name = [0u8; 128];
            // SAFETY: a name buffer of the length stated.
            let n = unsafe { libc::ioctl(fd.as_raw_fd(), eviocgname(name.len()), name.as_mut_ptr()) };
            if n <= 0 {
                continue;
            }
            let name = String::from_utf8_lossy(&name[..(n as usize).saturating_sub(1)]).into_owned();
            let rank = rank(&name);
            if rank == 0 {
                continue;
            }
            let mut axes = Vec::new();
            for code in [ABS_X, ABS_Y, ABS_Z, ABS_RX, ABS_RY, ABS_RZ, ABS_RUDDER, ABS_WHEEL] {
                // `struct input_absinfo`: value, minimum, maximum, fuzz, flat, resolution.
                let mut info = [0i32; 6];
                // SAFETY: an absinfo struct is six 32-bit values.
                if unsafe { libc::ioctl(fd.as_raw_fd(), eviocgabs(code), info.as_mut_ptr()) } == 0 && info[2] != info[1] {
                    axes.push(Axis { code, min: info[1], max: info[2] });
                }
            }
            if axes.len() < 2 {
                continue;
            }
            if best.as_ref().is_none_or(|(r, _)| rank > *r) {
                best = Some((rank, Pad { fd, axes, name }));
            }
        }
        best.map(|(_, pad)| pad)
    }

    /// Fold whatever the pad has said since last time into `state`.
    pub fn poll(&mut self, state: &mut PadState) {
        let mut buffer = [0u8; 24 * 64];
        loop {
            // SAFETY: reading into a buffer we own from a descriptor we own.
            let n = unsafe { libc::read(self.fd.as_raw_fd(), buffer.as_mut_ptr() as *mut libc::c_void, buffer.len()) };
            if n <= 0 {
                break;
            }
            for event in buffer[..n as usize].chunks_exact(24) {
                let kind = u16::from_le_bytes([event[16], event[17]]);
                let code = u16::from_le_bytes([event[18], event[19]]);
                let value = i32::from_le_bytes([event[20], event[21], event[22], event[23]]);
                match kind {
                    EV_KEY => {
                        if value != 0 {
                            state.down.insert(code);
                        } else {
                            state.down.remove(&code);
                        }
                    }
                    EV_ABS => self.axis(state, code, value),
                    _ => {}
                }
            }
        }
    }

    fn axis(&self, state: &mut PadState, code: u16, value: i32) {
        if code == ABS_HAT0X {
            state.dpad[0] = value as f32;
            return;
        }
        if code == ABS_HAT0Y {
            state.dpad[1] = -(value as f32);
            return;
        }
        let Some(axis) = self.axes.iter().find(|a| a.code == code) else { return };
        let (unit, signed) = normalised(value, axis.min, axis.max);
        // A little dead zone: a stick at rest never reads exactly zero.
        let stick = if signed.abs() < 0.08 { 0.0 } else { signed };
        match code {
            ABS_X => state.sticks[0] = stick,
            ABS_Y => state.sticks[1] = -stick,
            ABS_RX => state.sticks[2] = stick,
            ABS_RY => state.sticks[3] = -stick,
            ABS_Z => state.triggers[0] = unit.clamp(0.0, 1.0),
            ABS_RZ => state.triggers[1] = unit.clamp(0.0, 1.0),
            // No dead zone: a gyro's aim is small movements from a rest that is exactly centre.
            ABS_RUDDER => state.hand[0] = signed,
            ABS_WHEEL => state.hand[1] = signed,
            _ => {}
        }
    }
}

/// An axis reading as 0..1 across its range and as -1..1 about its middle.
fn normalised(value: i32, min: i32, max: i32) -> (f32, f32) {
    let span = (max - min) as f32;
    let unit = ((value - min) as f32 / span).clamp(0.0, 1.0);
    (unit, (unit * 2.0 - 1.0).clamp(-1.0, 1.0))
}

/// How much a device's name looks like the pad games are given. Zero is "not one".
fn rank(name: &str) -> u32 {
    let lower = name.to_lowercase();
    if lower.contains("spatiand gamepad") {
        3
    } else if lower.contains("steam virtual gamepad") {
        2
    } else if lower.contains("x-box") || lower.contains("xbox") || lower.contains("gamepad") {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_axis_is_read_across_its_own_range() {
        // A stick as the Deck's virtual pad has it: signed 16 bits, rest in the middle.
        assert_eq!(normalised(0, -32768, 32767).1.abs() < 0.001, true);
        assert!((normalised(32767, -32768, 32767).1 - 1.0).abs() < 1e-6);
        assert!((normalised(-32768, -32768, 32767).1 + 1.0).abs() < 1e-6);
        // A trigger: 0 to 255, rest at the bottom.
        assert_eq!(normalised(0, 0, 255).0, 0.0);
        assert_eq!(normalised(255, 0, 255).0, 1.0);
    }

    fn pad() -> PadState {
        PadState::default()
    }

    #[test]
    fn a_binding_path_is_split_into_its_hand_and_its_component() {
        assert_eq!(split("/user/hand/left/input/trigger/value"), Some((Hand::Left, "trigger/value")));
        assert_eq!(split("/user/hand/right/input/aim/pose"), Some((Hand::Right, "aim/pose")));
        assert_eq!(split("/user/head/input/volume_up/click"), None);
        assert_eq!(split("/user/hand/left"), None);
    }

    #[test]
    fn the_triggers_and_sticks_are_each_hands_own() {
        let mut p = pad();
        p.triggers = [0.0, 0.8];
        p.sticks = [-1.0, 0.5, 0.25, 0.0];
        assert_eq!(read("/user/hand/right/input/trigger/value", &p), Some(Value::Float(0.8)));
        assert_eq!(read("/user/hand/left/input/trigger/value", &p), Some(Value::Float(0.0)));
        assert_eq!(read("/user/hand/left/input/thumbstick", &p), Some(Value::Vec2([-1.0, 0.5])));
        assert_eq!(read("/user/hand/right/input/thumbstick/x", &p), Some(Value::Float(0.25)));
    }

    #[test]
    fn a_trigger_clicks_past_most_of_its_travel_and_not_before() {
        let mut p = pad();
        p.triggers = [0.5, 0.7];
        assert_eq!(read("/user/hand/left/input/select/click", &p), Some(Value::Bool(false)));
        assert_eq!(read("/user/hand/right/input/select/click", &p), Some(Value::Bool(true)));
        p.triggers = [0.0, 0.0];
        p.down.insert(BTN_TR2);
        assert_eq!(read("/user/hand/right/input/trigger/value", &p), Some(Value::Float(1.0)), "a digital trigger is all the way");
    }

    #[test]
    fn the_face_buttons_are_the_right_hands_a_and_b_and_the_lefts_x_and_y() {
        let mut p = pad();
        p.down.insert(BTN_SOUTH);
        assert_eq!(read("/user/hand/right/input/a/click", &p), Some(Value::Bool(true)));
        assert_eq!(read("/user/hand/left/input/x/click", &p), Some(Value::Bool(false)));
        p.down.insert(BTN_WEST);
        assert_eq!(read("/user/hand/left/input/y/click", &p), Some(Value::Bool(true)));
        p.down.insert(BTN_START);
        assert_eq!(read("/user/hand/left/input/menu/click", &p), Some(Value::Bool(true)));
    }

    #[test]
    fn a_component_this_pad_does_not_have_reads_as_nothing() {
        assert_eq!(read("/user/hand/left/input/thumbrest/touch", &pad()), None);
    }

    #[test]
    fn a_value_reads_as_the_type_an_action_asks_for() {
        assert!(Value::Float(0.9).as_bool());
        assert!(!Value::Float(0.2).as_bool());
        assert_eq!(Value::Bool(true).as_float(), 1.0);
        assert_eq!(Value::Vec2([0.0, -1.0]).as_float(), 1.0);
        assert_eq!(Value::Float(0.5).as_vec2(), [0.5, 0.0]);
    }

    #[test]
    fn a_hand_rests_in_front_of_and_beside_the_head_and_points_where_it_does() {
        let head = Pose { orientation: glam::Quat::from_rotation_y(0.7), position: glam::Vec3::new(0.0, 1.6, 0.0) };
        let left = hand_pose(Hand::Left, &head);
        let right = hand_pose(Hand::Right, &head);
        assert!(left.orientation.dot(head.orientation).abs() > 0.999);
        // Forward in the head's own frame is -Z; the hands are ahead of it...
        let local = |hand: &Pose| head.inverse().then(hand).position;
        assert!(local(&left).z < 0.0 && local(&right).z < 0.0);
        // ...and either side, and below.
        assert!(local(&left).x < 0.0 && local(&right).x > 0.0);
        assert!(local(&right).y < 0.0);
    }

    fn eye() -> Eye {
        Eye { pose: Pose { orientation: glam::Quat::from_rotation_y(0.4), position: glam::Vec3::new(0.0, 1.6, 0.0) }, fov: [-0.7, 0.7, 0.6, -0.6] }
    }

    #[test]
    fn the_middle_of_the_picture_is_straight_ahead_of_the_eye() {
        let eye = eye();
        let (origin, direction) = pointer_ray(&eye, (3840, 1080), (960.0, 540.0));
        assert_eq!(origin, eye.pose.position);
        let ahead = eye.pose.orientation * glam::Vec3::NEG_Z;
        assert!(direction.dot(ahead) > 0.999);
    }

    #[test]
    fn the_edges_of_the_picture_are_the_edges_of_the_field_of_view() {
        let eye = Eye { pose: Pose::IDENTITY, fov: [-0.7, 0.7, 0.6, -0.6] };
        let (_, right) = pointer_ray(&eye, (3840, 1080), (1920.0, 540.0));
        assert!((right.x / -right.z - 0.7f32.tan()).abs() < 1e-4);
        let (_, top) = pointer_ray(&eye, (3840, 1080), (960.0, 0.0));
        assert!((top.y / -top.z - 0.6f32.tan()).abs() < 1e-4);
        // Above the picture is no further than its edge.
        let (_, above) = pointer_ray(&eye, (3840, 1080), (960.0, -40.0));
        assert!((above.y - top.y).abs() < 1e-6);
    }

    #[test]
    fn a_ray_ends_on_the_nearest_rectangle_it_crosses() {
        let ahead = (Pose { orientation: glam::Quat::IDENTITY, position: glam::Vec3::new(0.0, 0.0, -4.0) }, [2.0, 2.0]);
        let nearer = (Pose { orientation: glam::Quat::IDENTITY, position: glam::Vec3::new(0.0, 0.0, -2.0) }, [0.5, 0.5]);
        let aside = (Pose { orientation: glam::Quat::IDENTITY, position: glam::Vec3::new(5.0, 0.0, -1.0) }, [1.0, 1.0]);
        let quads = [ahead, nearer, aside];
        assert_eq!(nearest_quad(glam::Vec3::ZERO, glam::Vec3::NEG_Z, &quads), Some(2.0));
        // Off the small one, still on the big one.
        let direction = glam::Vec3::new(0.2, 0.0, -1.0).normalize();
        let t = nearest_quad(glam::Vec3::ZERO, direction, &quads).unwrap();
        assert!((direction * t).z + 4.0 < 1e-4);
        // Past the edge of everything, nothing; and a rectangle behind is not hit.
        assert_eq!(nearest_quad(glam::Vec3::ZERO, glam::Vec3::new(3.0, 0.0, -1.0).normalize(), &quads), None);
        assert_eq!(nearest_quad(glam::Vec3::ZERO, glam::Vec3::Z, &quads), None);
    }

    #[test]
    fn a_hand_aimed_at_a_place_casts_a_laser_that_ends_there() {
        let head = Pose { orientation: glam::Quat::from_rotation_y(0.7), position: glam::Vec3::new(0.0, 1.6, 0.0) };
        let target = head.position + head.orientation * glam::Vec3::new(0.3, 0.2, -3.0);
        for hand in [Hand::Left, Hand::Right] {
            let aimed = aim_hand_at(hand, &head, target);
            assert_eq!(aimed.position, hand_pose(hand, &head).position, "it does not move, only turn");
            let along = aimed.orientation * glam::Vec3::NEG_Z;
            let to_target = (target - aimed.position).normalize();
            assert!(along.dot(to_target) > 0.9999, "{hand:?}: {along} against {to_target}");
        }
    }

    #[test]
    fn a_gyro_turns_the_hand_from_where_the_head_points() {
        let head = Pose { orientation: glam::Quat::from_rotation_y(0.5), position: glam::Vec3::new(0.0, 1.6, 0.0) };
        assert!(aim_hand_by(Hand::Right, &head, [0.0, 0.0]).is_none(), "at rest it is not aiming");
        let along = |p: &Pose| p.orientation * glam::Vec3::NEG_Z;
        let ahead = head.orientation * glam::Vec3::NEG_Z;
        // A quarter of the way to ninety degrees to the right: 22.5 degrees clockwise seen from above.
        let right = aim_hand_by(Hand::Right, &head, [0.25, 0.0]).unwrap();
        let turned = along(&right);
        assert!((turned.dot(ahead) - (22.5f32).to_radians().cos()).abs() < 1e-3);
        assert!(turned.cross(ahead).y > 0.0, "turned clockwise seen from above is right, the way a head turns right");
        let up = aim_hand_by(Hand::Right, &head, [0.0, 0.5]).unwrap();
        assert!(along(&up).y > 0.6, "up is up: {}", along(&up));
        // Pointed up it is raised; pointed right down it is at the waist, below the head by most of an arm.
        let rest = hand_pose(Hand::Right, &head).position;
        assert!(up.position.y > rest.y, "raised: {} vs {}", up.position, rest);
        let down = aim_hand_by(Hand::Right, &head, [0.0, -0.83]).unwrap();
        assert!(down.position.y < rest.y - 0.4, "lowered: {} vs {}", down.position, rest);
    }

    #[test]
    fn a_panel_bigger_than_the_field_is_shown_smaller_and_in_front() {
        let head = Pose { orientation: glam::Quat::from_rotation_y(0.3), position: glam::Vec3::new(0.0, 1.6, 0.0) };
        let fov = [-0.35f32, 0.35, 0.2, -0.2];
        // X8's tutorial: eight metres square, five away, below and to one side.
        let placed = head.then(&Pose { orientation: glam::Quat::IDENTITY, position: glam::Vec3::new(1.0, -0.6, -4.9) });
        let q = fit_quad(&head, fov, &placed, [8.0, 8.0]);
        assert!(q.shown_size[0] < 8.0 * 0.4, "{:?}", q.shown_size);
        assert_eq!(q.actual, placed);
        // It is in front of the head, as far away as it was.
        let local = head.inverse().then(&q.shown).position;
        assert!(local.x.abs() < 1e-4 && local.y.abs() < 1e-4 && local.z < -4.0, "{local}");
        // And all of it is inside the field, with room to spare.
        let half = (q.shown_size[1] / 2.0 / -local.z).atan();
        assert!(half < fov[2], "{half} vs {}", fov[2]);
        // A panel that fits is left where the game put it.
        let small = fit_quad(&head, fov, &placed, [0.5, 0.3]);
        assert_eq!((small.shown, small.shown_size), (placed, [0.5, 0.3]));
    }

    #[test]
    fn the_pointer_finds_the_same_place_on_the_games_panel() {
        let head = Pose::IDENTITY;
        let fov = [-0.35f32, 0.35, 0.2, -0.2];
        // Eight by eight, five away, straight ahead: the fitted one is smaller, the same distance.
        let placed = Pose { orientation: glam::Quat::IDENTITY, position: glam::Vec3::new(0.0, 0.0, -5.0) };
        let q = fit_quad(&head, fov, &placed, [8.0, 8.0]);
        // Through the right-hand edge of the shown panel (at its middle height) is the right-hand edge
        // of the game's.
        let edge = q.shown_size[0] / 2.0 - 0.01;
        let direction = glam::Vec3::new(edge, 0.0, -5.0).normalize();
        let (_, at) = pointer_target(glam::Vec3::ZERO, direction, &[q]).unwrap();
        assert!((at.x - 3.99).abs() < 0.05 && at.y.abs() < 1e-3 && (at.z + 5.0).abs() < 1e-3, "{at}");
        // Past the shown panel is not on the game's, even though the game's is larger.
        let outside = glam::Vec3::new(q.shown_size[0], 0.0, -5.0).normalize();
        assert!(pointer_target(glam::Vec3::ZERO, outside, &[q]).is_none());
    }

    #[test]
    fn the_pointers_buttons_pull_the_triggers() {
        let mut p = pad();
        p.mouse = [true, false];
        assert_eq!(read("/user/hand/right/input/trigger/value", &p), Some(Value::Float(1.0)));
        assert_eq!(read("/user/hand/right/input/select/click", &p), Some(Value::Bool(true)));
        assert_eq!(read("/user/hand/left/input/trigger/value", &p), Some(Value::Float(0.0)));
        p.mouse = [false, true];
        assert_eq!(read("/user/hand/left/input/trigger/value", &p), Some(Value::Float(1.0)));
    }

    #[test]
    fn the_virtual_pads_this_machine_hands_games_are_the_ones_chosen() {
        assert!(rank("Spatiand Gamepad") > rank("Steam Virtual Gamepad"));
        assert!(rank("Steam Virtual Gamepad") > rank("Microsoft X-Box 360 pad"));
        assert_eq!(rank("AT Translated Set 2 keyboard"), 0);
    }
}
