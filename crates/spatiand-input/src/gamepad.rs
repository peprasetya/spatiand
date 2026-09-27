//! Other controllers — a Bluetooth or USB pad — read through evdev and merged with the Deck.
//!
//! The kernel's own drivers already decode these (xpad, hid-playstation, hid-nintendo,
//! xpadneo), so unlike the Deck there is nothing to take over: open the event node and read.
//! Each is grabbed while open, so its presses reach only the layout — the game is shown the
//! virtual pad instead, and a pad that also reached it directly would be a second controller.
//!
//! Found by rescanning `/proc/bus/input/devices` every couple of seconds, which is how a pad
//! paired mid-session joins without anything having to listen for hotplug.
//!
//! A PlayStation pad is three devices to the kernel, not one: the pad, its "Touchpad" and its
//! "Motion Sensors". They are read together here, as parts of the pad they came with -- told
//! apart from another pad's parts by the Bluetooth address the kernel gives all three -- so the
//! touchpad can be the pointer and the gyro can reach the layout.

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

const RESCAN: Duration = Duration::from_secs(2);

/// What one pad currently reads, in the Deck's terms.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GamepadState {
    pub a: bool,
    pub b: bool,
    pub x: bool,
    pub y: bool,
    pub l1: bool,
    pub r1: bool,
    pub l2: bool,
    pub r2: bool,
    pub select: bool,
    pub start: bool,
    /// The Xbox, PlayStation or Home button: what STEAM is on the Deck.
    pub guide: bool,
    pub left_stick_click: bool,
    pub right_stick_click: bool,
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    /// −1..1, +y up.
    pub left_stick: (f32, f32),
    pub right_stick: (f32, f32),
    /// 0..1.
    pub left_trigger: f32,
    pub right_trigger: f32,
    /// A touchpad built into the pad, if it has one: what the Deck's right trackpad is.
    pub touchpad: Option<Touchpad>,
    /// Rotation rates from the pad's own gyro, degrees per second about its X (pitch), Y (yaw)
    /// and Z (roll) axes, in SDL's convention for these pads. `None` without one.
    pub gyro: Option<[f32; 3]>,
}

/// A pad's touchpad, in the Deck's trackpad terms.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Touchpad {
    /// −1..1, +x right and +y up, like `crate::Pad`.
    pub x: f32,
    pub y: f32,
    pub touched: bool,
    /// The whole pad is a button.
    pub clicked: bool,
}

/// A joystick in the kernel's device list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoystickNode {
    pub name: String,
    pub vendor: u16,
    pub product: u16,
    /// `eventN`.
    pub event: String,
    /// The pad's "Touchpad" device, `eventN`, if it has one.
    pub touchpad: Option<String>,
    /// The pad's "Motion Sensors" device, `eventN`, if it has one.
    pub motion: Option<String>,
}

/// One block of `/proc/bus/input/devices`, as much of it as matters here.
#[derive(Debug, Default)]
struct Block {
    name: String,
    vendor: u16,
    product: u16,
    /// The Bluetooth address for a wireless pad, shared by all its devices. Often empty on USB.
    uniq: String,
    event: Option<String>,
    joystick: bool,
    /// `None` when the block does not say, which only a test fixture does not.
    pad_buttons: Option<bool>,
}

impl Block {
    /// Whether `self` is a part -- named `suffix` -- of the pad `pad`.
    fn part_of(&self, pad: &Block, suffix: &str) -> bool {
        (self.vendor, self.product) == (pad.vendor, pad.product)
            && self.name.ends_with(suffix)
            && if pad.uniq.is_empty() {
                // No address to go by: then only by name, which two identical pads share, so
                // the caller refuses to guess when there is more than one.
                self.name == format!("{}{suffix}", pad.name)
            } else {
                self.uniq == pad.uniq
            }
    }
}

/// Every joystick worth merging, from the text of `/proc/bus/input/devices`.
///
/// Leaves out Valve's own devices — the Deck is read over hidraw, and `28de:11ff` is Steam's
/// virtual pad — the virtual pad Spatiand itself creates, and the separate motion-sensor
/// devices some drivers add beside a pad, which carry no buttons.
pub fn parse_joysticks(text: &str) -> Vec<JoystickNode> {
    let blocks: Vec<Block> = text.split("\n\n").map(parse_block).collect();
    let mut out = Vec::new();
    for block in &blocks {
        let ours = block.name == crate::virtual_pad::NAME;
        let valve = block.vendor == 0x28DE;
        let sensors = block.name.contains("Motion Sensors") || block.name.contains("IMU");
        // A joystick node is not a joystick. The kernel gives one to anything with an odd
        // axis: a Bluetooth keyboard with a built-in trackpad reported `js0` for a single
        // ABS_MISC, was grabbed here as a controller, and from then on typed nothing and moved
        // no pointer. A pad has pad buttons; a keyboard does not.
        let buttons = block.pad_buttons.unwrap_or(true);
        let Some(event) = block.event.clone() else { continue };
        if !(block.joystick && buttons && !ours && !valve && !sensors) {
            continue;
        }
        let part = |suffix: &str| {
            let found: Vec<&Block> = blocks.iter().filter(|b| b.part_of(block, suffix)).collect();
            // Two pads with no address and the same name: which touchpad is whose is a guess,
            // and a wrong guess is one person's thumb moving another's pointer.
            match found.as_slice() {
                [only] => only.event.clone(),
                _ => None,
            }
        };
        out.push(JoystickNode {
            name: block.name.clone(),
            vendor: block.vendor,
            product: block.product,
            event,
            touchpad: part(" Touchpad"),
            motion: part(" Motion Sensors"),
        });
    }
    out
}

fn parse_block(block: &str) -> Block {
    let mut b = Block::default();
    for line in block.lines() {
        if let Some(rest) = line.strip_prefix("N: Name=") {
            b.name = rest.trim_matches('"').to_string();
        } else if let Some(rest) = line.strip_prefix("U: Uniq=") {
            b.uniq = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("I: ") {
            for field in rest.split_whitespace() {
                if let Some(v) = field.strip_prefix("Vendor=") {
                    b.vendor = u16::from_str_radix(v, 16).unwrap_or(0);
                } else if let Some(p) = field.strip_prefix("Product=") {
                    b.product = u16::from_str_radix(p, 16).unwrap_or(0);
                }
            }
        } else if let Some(rest) = line.strip_prefix("B: KEY=") {
            b.pad_buttons = Some(has_pad_buttons(rest));
        } else if let Some(rest) = line.strip_prefix("H: Handlers=") {
            for handler in rest.split_whitespace() {
                if handler.starts_with("js") {
                    b.joystick = true;
                } else if handler.starts_with("event") {
                    b.event = Some(handler.to_string());
                }
            }
        }
    }
    b
}

/// Whether a `B: KEY=` bitmap has any joystick or gamepad button: `BTN_TRIGGER` (0x120) to
/// `BTN_THUMBR` (0x13E). Keyboards and trackpads use the codes either side of that range.
fn has_pad_buttons(bitmap: &str) -> bool {
    // Words of a `long`, highest first; the last word holds bits 0..64.
    let words: Vec<u64> = bitmap
        .split_whitespace()
        .rev()
        .map(|w| u64::from_str_radix(w, 16).unwrap_or(0))
        .collect();
    (0x120u16..=0x13E).any(|bit| {
        let (word, offset) = (bit as usize / 64, bit as usize % 64);
        words.get(word).is_some_and(|w| w & (1 << offset) != 0)
    })
}

const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;

const BTN_LEFT: u16 = 0x110;
const BTN_TOUCH: u16 = 0x14A;

const BTN_SOUTH: u16 = 0x130;
const BTN_EAST: u16 = 0x131;
const BTN_NORTH: u16 = 0x133;
const BTN_WEST: u16 = 0x134;
const BTN_TL: u16 = 0x136;
const BTN_TR: u16 = 0x137;
const BTN_TL2: u16 = 0x138;
const BTN_TR2: u16 = 0x139;
const BTN_SELECT: u16 = 0x13A;
const BTN_START: u16 = 0x13B;
const BTN_MODE: u16 = 0x13C;
const BTN_THUMBL: u16 = 0x13D;
const BTN_THUMBR: u16 = 0x13E;
const BTN_DPAD_UP: u16 = 0x220;
const BTN_DPAD_DOWN: u16 = 0x221;
const BTN_DPAD_LEFT: u16 = 0x222;
const BTN_DPAD_RIGHT: u16 = 0x223;

const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_Z: u16 = 0x02;
const ABS_RX: u16 = 0x03;
const ABS_RY: u16 = 0x04;
const ABS_RZ: u16 = 0x05;
const ABS_GAS: u16 = 0x09;
const ABS_BRAKE: u16 = 0x0A;
const ABS_HAT0X: u16 = 0x10;
const ABS_HAT0Y: u16 = 0x11;

/// `EVIOCGRAB`.
const EVIOCGRAB: u64 = 0x4004_4590;

fn eviocgabs(axis: u16) -> u64 {
    (2 << 30) | (24 << 16) | ((b'E' as u64) << 8) | (0x40 + axis as u64)
}

/// An input node opened for reading, grabbed so nothing else sees it.
fn open_grabbed(event: &str, name: &str) -> io::Result<OwnedFd> {
    let path = std::ffi::CString::new(format!("/dev/input/{event}")).unwrap();
    let raw = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    if unsafe { libc::ioctl(fd.as_raw_fd(), EVIOCGRAB as _, 1 as libc::c_ulong) } < 0 {
        log::warn!(
            "{name}: could not take it for ourselves ({}); something else may also see it",
            io::Error::last_os_error()
        );
    }
    Ok(fd)
}

/// `(minimum, maximum, resolution)` of one absolute axis, if the device has it.
fn abs_info(fd: &OwnedFd, axis: u16) -> Option<(i32, i32, i32)> {
    let mut info = [0i32; 6];
    let rc = unsafe { libc::ioctl(fd.as_raw_fd(), eviocgabs(axis) as _, info.as_mut_ptr()) };
    (rc >= 0 && info[2] > info[1]).then_some((info[1], info[2], info[5]))
}

/// Every pending event on a node, as `(type, code, value)`. `None` once it has gone.
fn drain(fd: &OwnedFd) -> Option<Vec<(u16, u16, i32)>> {
    let mut buf = [0u8; 24 * 32];
    let mut events = Vec::new();
    loop {
        let n = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            let e = io::Error::last_os_error();
            return (e.kind() == io::ErrorKind::WouldBlock).then_some(events);
        }
        if n == 0 {
            return None;
        }
        for chunk in buf[..n as usize].chunks_exact(24) {
            let kind = u16::from_le_bytes([chunk[16], chunk[17]]);
            let code = u16::from_le_bytes([chunk[18], chunk[19]]);
            let value = i32::from_le_bytes([chunk[20], chunk[21], chunk[22], chunk[23]]);
            events.push((kind, code, value));
        }
    }
}

/// The touchpad part of a pad.
struct TouchpadPart {
    fd: OwnedFd,
    x: (i32, i32),
    y: (i32, i32),
}

/// The gyro part of a pad: counts per degree per second on each axis.
struct MotionPart {
    fd: OwnedFd,
    per_degree: [f32; 3],
}

struct Gamepad {
    node: JoystickNode,
    fd: OwnedFd,
    touch: Option<TouchpadPart>,
    motion: Option<MotionPart>,
    ranges: HashMap<u16, (i32, i32)>,
    /// Which axes the right stick and the triggers are on. Drivers differ: xpad and
    /// hid-playstation put the right stick on RX/RY and the triggers on Z/RZ, while plain HID
    /// pads put the right stick on Z/RZ and the triggers on GAS/BRAKE.
    right_stick: (u16, u16),
    triggers: (u16, u16),
    /// xpad reports the Xbox X and Y under the codes the kernel's layout document assigns to
    /// the top and left buttons the other way round, so Microsoft's pads are read by label.
    by_label: bool,
    state: GamepadState,
}

impl Gamepad {
    fn open(node: JoystickNode) -> io::Result<Self> {
        let fd = open_grabbed(&node.event, &node.name)?;
        // The parts are extras: a pad whose touchpad will not open is still a pad.
        let touch = node.touchpad.as_deref().and_then(|event| {
            let fd = open_grabbed(event, &format!("{} touchpad", node.name)).ok()?;
            let (x0, x1, _) = abs_info(&fd, ABS_X)?;
            let (y0, y1, _) = abs_info(&fd, ABS_Y)?;
            Some(TouchpadPart {
                fd,
                x: (x0, x1),
                y: (y0, y1),
            })
        });
        let motion = node.motion.as_deref().and_then(|event| {
            let fd = open_grabbed(event, &format!("{} motion sensors", node.name)).ok()?;
            // The driver states the gyro's scale as each axis's resolution: counts per degree
            // per second. hid-playstation gives 1024.
            let per = |axis| abs_info(&fd, axis).map(|(_, _, r)| r.max(1) as f32);
            let per_degree = [per(ABS_RX)?, per(ABS_RY)?, per(ABS_RZ)?];
            Some(MotionPart { fd, per_degree })
        });
        let mut ranges = HashMap::new();
        for axis in [ABS_X, ABS_Y, ABS_Z, ABS_RX, ABS_RY, ABS_RZ, ABS_GAS, ABS_BRAKE, ABS_HAT0X, ABS_HAT0Y] {
            let mut info = [0i32; 6];
            let rc = unsafe { libc::ioctl(fd.as_raw_fd(), eviocgabs(axis) as _, info.as_mut_ptr()) };
            if rc >= 0 && info[2] > info[1] {
                ranges.insert(axis, (info[1], info[2]));
            }
        }
        let has = |a| ranges.contains_key(&a);
        let (right_stick, triggers) = if has(ABS_RX) {
            ((ABS_RX, ABS_RY), (ABS_Z, ABS_RZ))
        } else {
            ((ABS_Z, ABS_RZ), (ABS_BRAKE, ABS_GAS))
        };
        log::info!(
            "controller joined: {} ({:04x}:{:04x}) on {}{}{}",
            node.name,
            node.vendor,
            node.product,
            node.event,
            if touch.is_some() { ", with its touchpad" } else { "" },
            if motion.is_some() { ", with its gyro" } else { "" },
        );
        let state = GamepadState {
            touchpad: touch.as_ref().map(|_| Touchpad::default()),
            gyro: motion.as_ref().map(|_| [0.0; 3]),
            ..GamepadState::default()
        };
        Ok(Self {
            by_label: node.vendor == 0x045E,
            node,
            fd,
            touch,
            motion,
            ranges,
            right_stick,
            triggers,
            state,
        })
    }

    fn normalised(&self, axis: u16, value: i32) -> f32 {
        let (min, max) = self.ranges.get(&axis).copied().unwrap_or((-32768, 32767));
        let span = (max - min).max(1) as f32;
        (value - min) as f32 / span
    }

    /// Drain pending events. `false` once the device has gone.
    fn read(&mut self) -> bool {
        let Some(events) = drain(&self.fd) else {
            return false;
        };
        for (kind, code, value) in events {
            match kind {
                EV_KEY => self.key(code, value != 0),
                EV_ABS => self.axis(code, value),
                _ => {}
            }
        }
        // A part that goes quiet for good is dropped on its own; the pad goes on without it.
        if let Some(touch) = &self.touch {
            match drain(&touch.fd) {
                Some(events) => {
                    let pad = self.state.touchpad.get_or_insert_with(Touchpad::default);
                    let scale = |v: i32, (lo, hi): (i32, i32)| {
                        ((v - lo) as f32 / (hi - lo).max(1) as f32) * 2.0 - 1.0
                    };
                    for (kind, code, value) in events {
                        match (kind, code) {
                            (EV_KEY, BTN_TOUCH) => pad.touched = value != 0,
                            (EV_KEY, BTN_LEFT) => pad.clicked = value != 0,
                            (EV_ABS, ABS_X) => pad.x = scale(value, touch.x),
                            // Rows run down the pad, and +y is up.
                            (EV_ABS, ABS_Y) => pad.y = -scale(value, touch.y),
                            _ => {}
                        }
                    }
                }
                None => {
                    self.touch = None;
                    self.state.touchpad = None;
                }
            }
        }
        if let Some(motion) = &self.motion {
            match drain(&motion.fd) {
                Some(events) => {
                    let gyro = self.state.gyro.get_or_insert([0.0; 3]);
                    for (kind, code, value) in events {
                        let axis = match (kind, code) {
                            (EV_ABS, ABS_RX) => 0,
                            (EV_ABS, ABS_RY) => 1,
                            (EV_ABS, ABS_RZ) => 2,
                            _ => continue,
                        };
                        gyro[axis] = value as f32 / motion.per_degree[axis];
                    }
                }
                None => {
                    self.motion = None;
                    self.state.gyro = None;
                }
            }
        }
        true
    }

    fn key(&mut self, code: u16, down: bool) {
        let s = &mut self.state;
        match code {
            BTN_SOUTH => s.a = down,
            BTN_EAST => s.b = down,
            BTN_NORTH if self.by_label => s.x = down,
            BTN_WEST if self.by_label => s.y = down,
            BTN_NORTH => s.y = down,
            BTN_WEST => s.x = down,
            BTN_TL => s.l1 = down,
            BTN_TR => s.r1 = down,
            BTN_TL2 => s.l2 = down,
            BTN_TR2 => s.r2 = down,
            BTN_SELECT => s.select = down,
            BTN_START => s.start = down,
            BTN_MODE => s.guide = down,
            BTN_THUMBL => s.left_stick_click = down,
            BTN_THUMBR => s.right_stick_click = down,
            BTN_DPAD_UP => s.up = down,
            BTN_DPAD_DOWN => s.down = down,
            BTN_DPAD_LEFT => s.left = down,
            BTN_DPAD_RIGHT => s.right = down,
            _ => {}
        }
    }

    fn axis(&mut self, code: u16, value: i32) {
        let centred = |this: &Self| this.normalised(code, value) * 2.0 - 1.0;
        if code == ABS_X {
            self.state.left_stick.0 = centred(self);
        } else if code == ABS_Y {
            self.state.left_stick.1 = -centred(self);
        } else if code == self.right_stick.0 {
            self.state.right_stick.0 = centred(self);
        } else if code == self.right_stick.1 {
            self.state.right_stick.1 = -centred(self);
        } else if code == self.triggers.0 {
            self.state.left_trigger = self.normalised(code, value);
            self.state.l2 |= self.state.left_trigger > 0.95;
            if self.state.left_trigger < 0.9 {
                self.state.l2 = false;
            }
        } else if code == self.triggers.1 {
            self.state.right_trigger = self.normalised(code, value);
            self.state.r2 |= self.state.right_trigger > 0.95;
            if self.state.right_trigger < 0.9 {
                self.state.r2 = false;
            }
        } else if code == ABS_HAT0X {
            self.state.left = value < 0;
            self.state.right = value > 0;
        } else if code == ABS_HAT0Y {
            self.state.up = value < 0;
            self.state.down = value > 0;
        }
    }
}

/// Every extra controller that is plugged in or paired.
/// Pads read by something else and handed in whole, by an id of its own: on Android, where
/// an app cannot open `/dev/input`, the app's `InputDevice` events. [`Gamepads::poll`] returns
/// them beside the ones it reads itself, so everything after it treats them alike.
pub fn external() -> &'static std::sync::Mutex<Vec<(i32, GamepadState)>> {
    static EXTERNAL: std::sync::OnceLock<std::sync::Mutex<Vec<(i32, GamepadState)>>> = std::sync::OnceLock::new();
    EXTERNAL.get_or_init(Default::default)
}

pub struct Gamepads {
    pads: Vec<Gamepad>,
    scanned: Option<Instant>,
    /// Nodes that would not open, so a pad this user cannot read is reported once, not every
    /// two seconds.
    refused: Vec<String>,
}

impl Default for Gamepads {
    fn default() -> Self {
        Self::new()
    }
}

impl Gamepads {
    pub fn new() -> Self {
        Self {
            pads: Vec::new(),
            scanned: None,
            refused: Vec::new(),
        }
    }

    /// Read every pad, looking for new ones every couple of seconds.
    pub fn poll(&mut self) -> Vec<GamepadState> {
        if self.scanned.is_none_or(|t| t.elapsed() >= RESCAN) {
            self.scanned = Some(Instant::now());
            self.rescan();
        }
        self.pads.retain_mut(|pad| {
            let alive = pad.read();
            if !alive {
                log::info!("controller left: {}", pad.node.name);
            }
            alive
        });
        let mut states: Vec<GamepadState> = self.pads.iter().map(|p| p.state).collect();
        if let Ok(external) = external().lock() {
            states.extend(external.iter().map(|(_, state)| *state));
        }
        states
    }

    pub fn names(&self) -> Vec<String> {
        self.pads.iter().map(|p| p.node.name.clone()).collect()
    }

    fn rescan(&mut self) {
        let Ok(text) = std::fs::read_to_string("/proc/bus/input/devices") else {
            return;
        };
        for node in parse_joysticks(&text) {
            if self.pads.iter().any(|p| p.node.event == node.event)
                || self.refused.contains(&node.event)
            {
                continue;
            }
            let event = node.event.clone();
            let name = node.name.clone();
            match Gamepad::open(node) {
                Ok(pad) => self.pads.push(pad),
                Err(e) => {
                    log::warn!("found {name} on {event} but could not open it: {e}");
                    self.refused.push(event);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICES: &str = r#"I: Bus=0003 Vendor=28de Product=1205 Version=0110
N: Name="Steam Deck"
H: Handlers=event10 js0

I: Bus=0003 Vendor=28de Product=11ff Version=0001
N: Name="Steam Virtual Gamepad"
H: Handlers=event18 js2

I: Bus=0003 Vendor=045e Product=028e Version=0114
N: Name="Microsoft X-Box 360 pad"
H: Handlers=event19 js6

I: Bus=0005 Vendor=054c Product=0ce6 Version=8100
N: Name="DualSense Wireless Controller"
H: Handlers=event20 js3

I: Bus=0005 Vendor=054c Product=0ce6 Version=8100
N: Name="DualSense Wireless Controller Motion Sensors"
H: Handlers=event21 js4

I: Bus=0005 Vendor=045e Product=0b13 Version=0515
N: Name="Xbox Wireless Controller"
H: Handlers=kbd event22 js5

I: Bus=0003 Vendor=046d Product=c52b Version=0111
N: Name="Logitech USB Receiver"
H: Handlers=sysfs kbd event5

I: Bus=0005 Vendor=04e8 Product=7021 Version=0001
N: Name="BT5.0 Keyboard"
H: Handlers=sysrq kbd leds event11 mouse3 js0 
B: KEY=101f 0 3f00033fff 0 0 483ffff17aff32d bfd5444600000000 ff0001 130ff38b17d007 ffff7bfad9415fff ffbeffdfffefffff fffffffffffffffe

I: Bus=0005 Vendor=054c Product=09cc Version=8100
N: Name="Wireless Controller"
U: Uniq=f4:4e:fd:0c:d0:1a
H: Handlers=event23 js7
B: KEY=7fdb000000000000 0 0 0 0

I: Bus=0005 Vendor=054c Product=09cc Version=8100
N: Name="Wireless Controller Motion Sensors"
U: Uniq=f4:4e:fd:0c:d0:1a
H: Handlers=event24

I: Bus=0005 Vendor=054c Product=09cc Version=8100
N: Name="Wireless Controller Touchpad"
U: Uniq=f4:4e:fd:0c:d0:1a
H: Handlers=mouse4 event25"#;

    #[test]
    fn only_real_extra_pads_are_merged() {
        // Our own pad and the Deck's own controller are left out; everything a person actually
        // plugged in is merged -- including a real wired Xbox 360 pad, which is the identity
        // this virtual one used to wear and could not have been told apart from.
        let found = parse_joysticks(DEVICES);
        let names: Vec<&str> = found.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Microsoft X-Box 360 pad",
                "DualSense Wireless Controller",
                "Xbox Wireless Controller",
                // A DualShock 4 as the kernel lists it, with its button bitmap.
                "Wireless Controller"
            ]
        );
        assert_eq!(found[1].event, "event20");
        assert_eq!((found[2].vendor, found[2].product), (0x045E, 0x0B13));
    }

    #[test]
    fn a_playstation_pad_brings_its_touchpad_and_gyro() {
        // The names and address are the DS4-compatible pad this was written for, as a Beam Pro
        // listed it. Its touchpad has no `js` handler and is not a pad by itself.
        let found = parse_joysticks(DEVICES);
        let ds4 = found.iter().find(|n| n.product == 0x09CC).unwrap();
        assert_eq!(ds4.event, "event23");
        assert_eq!(ds4.touchpad.as_deref(), Some("event25"));
        assert_eq!(ds4.motion.as_deref(), Some("event24"));
        // The DualSense in the same list gives no address, so its motion sensors are matched
        // by name, there being just the one.
        let dualsense = found.iter().find(|n| n.product == 0x0CE6).unwrap();
        assert_eq!(dualsense.motion.as_deref(), Some("event21"));
        assert_eq!(dualsense.touchpad, None);
    }

    #[test]
    fn another_pads_parts_are_never_taken() {
        // Two identical pads, told apart only by address: each gets its own touchpad.
        let two = "I: Vendor=054c Product=09cc\nN: Name=\"Wireless Controller\"\nU: Uniq=aa\nH: Handlers=event1 js0\n\n\
                   I: Vendor=054c Product=09cc\nN: Name=\"Wireless Controller\"\nU: Uniq=bb\nH: Handlers=event2 js1\n\n\
                   I: Vendor=054c Product=09cc\nN: Name=\"Wireless Controller Touchpad\"\nU: Uniq=bb\nH: Handlers=event3\n\n\
                   I: Vendor=054c Product=09cc\nN: Name=\"Wireless Controller Touchpad\"\nU: Uniq=aa\nH: Handlers=event4";
        let found = parse_joysticks(two);
        assert_eq!(found[0].touchpad.as_deref(), Some("event4"));
        assert_eq!(found[1].touchpad.as_deref(), Some("event3"));
        // Without addresses the same two cannot be told apart, and neither is guessed at.
        let blind = two.replace("U: Uniq=aa", "U: Uniq=").replace("U: Uniq=bb", "U: Uniq=");
        assert!(parse_joysticks(&blind).iter().all(|n| n.touchpad.is_none()));
    }

    #[test]
    fn the_absinfo_ioctl_reads_the_right_axis() {
        assert_eq!(eviocgabs(ABS_X), 0x8018_4540);
        assert_eq!(eviocgabs(ABS_HAT0Y), 0x8018_4551);
    }
}
