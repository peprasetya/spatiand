//! What the mapper reads: every physical control, merged into one controller-shaped frame.

use serde::{Deserialize, Serialize};

/// A physical button a layout can bind.
///
/// The Deck's own controls, named as Steam names them, the glasses' two temple buttons, and a
/// pointing pad's presses by fingers and depth. A Bluetooth pad's buttons arrive as the Deck
/// button in the same place — its A is [`Button::A`] — which is what lets one layout cover both
/// hands on either device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    A,
    B,
    X,
    Y,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    L1,
    R1,
    /// The click at the end of the trigger's travel. The analogue pull is [`Group::LeftTrigger`].
    ///
    /// [`Group::LeftTrigger`]: crate::layout::Group::LeftTrigger
    L2,
    R2,
    L4,
    R4,
    L5,
    R5,
    Menu,
    View,
    LStick,
    RStick,
    /// A thumb resting on a trackpad. The pads' pointing is never bound — it is Spatiand's
    /// pointer in every layout — but *touching* one is the natural switch for gyro aiming.
    LPadTouch,
    RPadTouch,
    /// A thumb resting on a stick, without clicking it. The Deck senses this the way it
    /// senses a thumb on a pad, and it is what "the gyro wakes when you reach for the stick"
    /// is bound to.
    LStickTouch,
    RStickTouch,
    GlassesUp,
    GlassesDown,
    /// A pointing pad clicked with one finger … five: a Mac's trackpad. Unbound, each is the
    /// click it always was (the pointer's left or right button, the three-finger drag); bound in
    /// the application's layout, the binding has it instead. See `docs/input-mapper.md`.
    PadClick1,
    PadClick2,
    PadClick3,
    PadClick4,
    PadClick5,
    /// The same pressed on through the click: a Force Touch trackpad's deeper click. Always
    /// after its click has gone down, which stays down beneath it.
    PadDeepClick1,
    PadDeepClick2,
    PadDeepClick3,
    PadDeepClick4,
    PadDeepClick5,
}

/// Which device a button is on: how the layout editor groups them, and what decides whether it
/// is offered at all (a trackpad's presses only where one reports them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    /// The Deck, or a gamepad: they share one set of buttons.
    Controller,
    Glasses,
    Trackpad,
}

impl Button {
    pub const ALL: [Button; 36] = [
        Button::A,
        Button::B,
        Button::X,
        Button::Y,
        Button::DpadUp,
        Button::DpadDown,
        Button::DpadLeft,
        Button::DpadRight,
        Button::L1,
        Button::R1,
        Button::L2,
        Button::R2,
        Button::L4,
        Button::R4,
        Button::L5,
        Button::R5,
        Button::Menu,
        Button::View,
        Button::LStick,
        Button::RStick,
        Button::LPadTouch,
        Button::RPadTouch,
        Button::LStickTouch,
        Button::RStickTouch,
        Button::GlassesUp,
        Button::GlassesDown,
        Button::PadClick1,
        Button::PadClick2,
        Button::PadClick3,
        Button::PadClick4,
        Button::PadClick5,
        Button::PadDeepClick1,
        Button::PadDeepClick2,
        Button::PadDeepClick3,
        Button::PadDeepClick4,
        Button::PadDeepClick5,
    ];

    /// The press of a pointing pad with this many fingers, deep or not.
    pub fn pad_press(fingers: u8, deep: bool) -> Option<Button> {
        let clicks = [Button::PadClick1, Button::PadClick2, Button::PadClick3, Button::PadClick4, Button::PadClick5];
        let deeps = [
            Button::PadDeepClick1,
            Button::PadDeepClick2,
            Button::PadDeepClick3,
            Button::PadDeepClick4,
            Button::PadDeepClick5,
        ];
        let index = (fingers as usize).checked_sub(1)?;
        if deep { deeps.get(index).copied() } else { clicks.get(index).copied() }
    }

    /// For a deep press, the ordinary press of the same fingers beneath it.
    pub fn shallow(self) -> Option<Button> {
        match self {
            Button::PadDeepClick1 => Some(Button::PadClick1),
            Button::PadDeepClick2 => Some(Button::PadClick2),
            Button::PadDeepClick3 => Some(Button::PadClick3),
            Button::PadDeepClick4 => Some(Button::PadClick4),
            Button::PadDeepClick5 => Some(Button::PadClick5),
            _ => None,
        }
    }

    pub fn device(self) -> Device {
        match self {
            Button::GlassesUp | Button::GlassesDown => Device::Glasses,
            Button::PadClick1
            | Button::PadClick2
            | Button::PadClick3
            | Button::PadClick4
            | Button::PadClick5
            | Button::PadDeepClick1
            | Button::PadDeepClick2
            | Button::PadDeepClick3
            | Button::PadDeepClick4
            | Button::PadDeepClick5 => Device::Trackpad,
            _ => Device::Controller,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Button::A => "A",
            Button::B => "B",
            Button::X => "X",
            Button::Y => "Y",
            Button::DpadUp => "D-pad up",
            Button::DpadDown => "D-pad down",
            Button::DpadLeft => "D-pad left",
            Button::DpadRight => "D-pad right",
            Button::L1 => "L1 bumper",
            Button::R1 => "R1 bumper",
            Button::L2 => "L2 full pull",
            Button::R2 => "R2 full pull",
            Button::L4 => "L4 back button",
            Button::R4 => "R4 back button",
            Button::L5 => "L5 back button",
            Button::R5 => "R5 back button",
            Button::Menu => "Menu",
            Button::View => "View",
            Button::LStick => "Left stick click",
            Button::RStick => "Right stick click",
            Button::LPadTouch => "Left trackpad touch",
            Button::RPadTouch => "Right trackpad touch",
            Button::LStickTouch => "Left stick touch",
            Button::RStickTouch => "Right stick touch",
            Button::GlassesUp => "Glasses button up",
            Button::GlassesDown => "Glasses button down",
            Button::PadClick1 => "Trackpad: click, 1 finger",
            Button::PadClick2 => "Trackpad: click, 2 fingers",
            Button::PadClick3 => "Trackpad: click, 3 fingers",
            Button::PadClick4 => "Trackpad: click, 4 fingers",
            Button::PadClick5 => "Trackpad: click, 5 fingers",
            Button::PadDeepClick1 => "Trackpad: deep click, 1 finger",
            Button::PadDeepClick2 => "Trackpad: deep click, 2 fingers",
            Button::PadDeepClick3 => "Trackpad: deep click, 3 fingers",
            Button::PadDeepClick4 => "Trackpad: deep click, 4 fingers",
            Button::PadDeepClick5 => "Trackpad: deep click, 5 fingers",
        }
    }

    fn bit(self) -> u64 {
        1 << (self as u64)
    }
}

/// A set of buttons that are down.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Buttons(u64);

impl Buttons {
    pub fn is_down(self, button: Button) -> bool {
        self.0 & button.bit() != 0
    }

    pub fn set(&mut self, button: Button, down: bool) {
        if down {
            self.0 |= button.bit();
        } else {
            self.0 &= !button.bit();
        }
    }

    pub fn with(mut self, button: Button) -> Self {
        self.set(button, true);
        self
    }

    pub fn any(self) -> bool {
        self.0 != 0
    }

    pub fn union(self, other: Buttons) -> Self {
        Self(self.0 | other.0)
    }
}

/// A finger on a trackpad. `x` and `y` run −1..1 with +y up, as the Deck reports them.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Touch {
    pub x: f32,
    pub y: f32,
    pub touched: bool,
}

/// Rotation rates, degrees per second, in the holder's terms rather than a sensor's.
///
/// `pitch` is positive tipping the top away and the front up (aiming up), `yaw` positive turning
/// left, `roll` positive with the right side going down (steering right). Each source converts
/// its own axes into these before the mapper sees them, so a layout means the same thing for
/// the Deck and for the glasses.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rates {
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
}

/// Everything physical, as of one frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Snapshot {
    pub buttons: Buttons,
    /// −1..1, +y up.
    pub left_stick: (f32, f32),
    pub right_stick: (f32, f32),
    pub left_pad: Touch,
    pub right_pad: Touch,
    /// 0..1.
    pub left_trigger: f32,
    pub right_trigger: f32,
    /// The controller's own gyro.
    pub gyro: Option<Rates>,
    /// The glasses' gyro, bias removed, in head terms.
    pub glasses_gyro: Option<Rates>,
}

impl Snapshot {
    /// Fold a second device into this one, so both read as a single controller.
    ///
    /// Buttons are either-or. A stick or trigger takes whichever device is pushed further —
    /// summing would let two resting sticks' noise add up, and the Deck's own stick drifting a
    /// hair must not cancel a Bluetooth stick held hard over. A trackpad takes the device being
    /// touched, and a gyro the first one present.
    pub fn merge(&mut self, other: &Snapshot) {
        self.buttons = self.buttons.union(other.buttons);
        self.left_stick = further(self.left_stick, other.left_stick);
        self.right_stick = further(self.right_stick, other.right_stick);
        if other.left_pad.touched && !self.left_pad.touched {
            self.left_pad = other.left_pad;
        }
        if other.right_pad.touched && !self.right_pad.touched {
            self.right_pad = other.right_pad;
        }
        self.left_trigger = self.left_trigger.max(other.left_trigger);
        self.right_trigger = self.right_trigger.max(other.right_trigger);
        if self.gyro.is_none() {
            self.gyro = other.gyro;
        }
        if self.glasses_gyro.is_none() {
            self.glasses_gyro = other.glasses_gyro;
        }
    }
}

fn further(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    if b.0 * b.0 + b.1 * b.1 > a.0 * a.0 + a.1 * a.1 {
        b
    } else {
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_button_has_its_own_bit() {
        let mut seen = 0u64;
        for b in Button::ALL {
            assert_eq!(seen & b.bit(), 0, "{b:?} shares a bit");
            seen |= b.bit();
        }
    }

    #[test]
    fn each_pad_press_is_named_by_its_fingers_and_depth() {
        assert_eq!(Button::pad_press(1, false), Some(Button::PadClick1));
        assert_eq!(Button::pad_press(5, true), Some(Button::PadDeepClick5));
        assert_eq!(Button::pad_press(0, false), None);
        assert_eq!(Button::pad_press(6, false), None);
        assert_eq!(Button::PadDeepClick3.shallow(), Some(Button::PadClick3));
        let named: Vec<String> = [Button::PadClick2, Button::PadDeepClick2]
            .iter()
            .map(|b| toml::Value::try_from(b).unwrap().as_str().unwrap().to_string())
            .collect();
        assert_eq!(named, ["pad_click2", "pad_deep_click2"]);
    }

    #[test]
    fn a_second_pad_adds_its_buttons_and_wins_only_when_pushed_further() {
        let mut deck = Snapshot {
            left_stick: (0.02, 0.01),
            ..Default::default()
        };
        deck.buttons.set(Button::A, true);
        let mut bluetooth = Snapshot {
            left_stick: (-0.9, 0.0),
            right_trigger: 0.7,
            ..Default::default()
        };
        bluetooth.buttons.set(Button::R1, true);
        deck.merge(&bluetooth);
        assert!(deck.buttons.is_down(Button::A));
        assert!(deck.buttons.is_down(Button::R1));
        assert_eq!(deck.left_stick, (-0.9, 0.0));
        assert_eq!(deck.right_trigger, 0.7);

        // And a resting second pad does not pull a held stick back to centre.
        let resting = Snapshot {
            left_stick: (0.01, 0.0),
            ..Default::default()
        };
        deck.merge(&resting);
        assert_eq!(deck.left_stick, (-0.9, 0.0));
    }
}
