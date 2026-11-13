//! A pointing pad's press, by how many fingers made it and how deep it went.
//!
//! A Mac's Force Touch trackpad clicks at two depths: a click, and pressed on through it a
//! deeper one ("Force click"). macOS says which by a *stage* -- 0 nothing, 1 clicked, 2 the deep
//! click -- and how hard by a *pressure*, 0..1, and the room knows the fingers on the pad from
//! the pad's own frames. Measured on the owner's Magic Trackpad, macOS 27, 2026-11-14:
//!
//! * a click is stage 0 to 1 to 0, with one to five fingers;
//! * a deep click with one, three, four or five fingers is stage 1 to 2 (after the pressure in
//!   stage 1 reached about 0.3 to 0.7), then 2 to 1 to 0;
//! * with **two fingers macOS never says stage 2**; the pressure in stage 1 still climbs to 0.99
//!   on a hard press, where an ordinary click peaks at 0.01 to 0.12 (one hard press measured).
//!
//! [`PressReader`] turns those into a [`PadPress`]: the fingers counted as the press began and
//! kept until it ends (a finger landing or lifting mid-press does not make it another press),
//! deep from stage 2, and -- only where asked for, see [`PressReader::update`] -- a two-finger
//! deep press read from the pressure, confirmed by a tap of the pad because macOS gives none.
//! The platform only reports what its pad says; the deciding is here, the same for any pad
//! that one day says as much (the Deck's pads have a pressure too, `Pad::pressure`).

/// A press of a pointing pad going on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PadPress {
    /// 1 to 5.
    pub fingers: u8,
    /// Pressed on through the click: the deep press.
    pub deep: bool,
}

/// The most fingers a press is counted with; more are counted as this many.
pub const MOST_FINGERS: u8 = 5;

/// Where a two-finger deep press starts when nothing else is said: well above an ordinary
/// click's pressure (at most 0.12 measured) and well below a hard press's (0.99).
pub const TWO_FINGER_DEEP: f32 = 0.6;

/// A two-finger deep press ends when the pressure falls below this share of where it started,
/// so a press held near the line does not flicker between deep and not.
const TWO_FINGER_LET_GO: f32 = 0.5;

/// Reads one pad's presses from what its platform says of it, report by report.
#[derive(Debug, Default, Clone)]
pub struct PressReader {
    press: Option<PadPress>,
    /// The two-finger deep press may still happen in this press: once per press, again only
    /// after the press has ended.
    armed: bool,
    /// A two-finger deep press read from the pressure is going on.
    by_pressure: bool,
}

impl PressReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// The press as it is now.
    pub fn press(&self) -> Option<PadPress> {
        self.press
    }

    /// One report: `fingers` on the pad as the press began (0 when no finger is known, as for
    /// a mouse's click, which is no pad press at all), the pad's `stage` (0 none, 1 clicked, 2
    /// deep) and its `pressure` (0..1). `two_finger_deep` is the pressure at which two fingers
    /// make a deep press, or `None` to leave two fingers to the stage alone (which macOS never
    /// raises to 2 for them).
    ///
    /// Returns the press as it now is, and whether the pad should tap under the fingers now:
    /// only as a two-finger deep press begins, because only that one has nothing from the pad
    /// itself to say it happened.
    pub fn update(&mut self, fingers: u8, stage: u8, pressure: f32, two_finger_deep: Option<f32>) -> (Option<PadPress>, bool) {
        if stage == 0 {
            self.press = None;
            self.armed = false;
            self.by_pressure = false;
            return (None, false);
        }
        let mut press = match self.press {
            Some(press) => press,
            None => {
                if fingers == 0 {
                    return (None, false);
                }
                self.armed = true;
                self.by_pressure = false;
                PadPress { fingers: fingers.min(MOST_FINGERS), deep: false }
            }
        };
        let mut tap = false;
        if press.fingers == 2 {
            match two_finger_deep.filter(|t| t.is_finite() && *t > 0.0) {
                Some(threshold) => {
                    if self.armed && pressure >= threshold {
                        self.armed = false;
                        self.by_pressure = true;
                        tap = true;
                    } else if self.by_pressure && pressure < threshold * TWO_FINGER_LET_GO {
                        self.by_pressure = false;
                    }
                }
                None => self.by_pressure = false,
            }
        }
        press.deep = stage >= 2 || self.by_pressure;
        self.press = Some(press);
        (self.press, tap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON: Option<f32> = Some(TWO_FINGER_DEEP);

    #[test]
    fn a_click_is_its_fingers_and_a_deep_click_follows_the_stage() {
        let mut r = PressReader::new();
        assert_eq!(r.update(3, 1, 0.05, ON), (Some(PadPress { fingers: 3, deep: false }), false));
        assert_eq!(r.update(3, 1, 0.5, ON).0, Some(PadPress { fingers: 3, deep: false }));
        // The pad's own second stage: no tap from here, macOS's pad gives its own.
        assert_eq!(r.update(3, 2, 0.6, ON), (Some(PadPress { fingers: 3, deep: true }), false));
        assert_eq!(r.update(3, 1, 0.3, ON).0, Some(PadPress { fingers: 3, deep: false }));
        assert_eq!(r.update(0, 0, 0.0, ON).0, None);
    }

    #[test]
    fn the_fingers_are_those_the_press_began_with() {
        let mut r = PressReader::new();
        r.update(1, 1, 0.05, ON);
        // A second finger lands mid-press: still the one-finger press.
        assert_eq!(r.update(2, 1, 0.9, ON), (Some(PadPress { fingers: 1, deep: false }), false));
        assert_eq!(r.update(2, 2, 0.9, ON).0, Some(PadPress { fingers: 1, deep: true }));
    }

    #[test]
    fn a_mouse_click_is_no_press_and_six_fingers_count_as_five() {
        let mut r = PressReader::new();
        assert_eq!(r.update(0, 1, 0.0, ON), (None, false));
        assert_eq!(r.update(7, 1, 0.0, ON).0.map(|p| p.fingers), Some(5));
    }

    #[test]
    fn two_fingers_go_deep_by_pressure_once_a_press_with_one_tap() {
        let mut r = PressReader::new();
        // An ordinary two-finger click peaks around 0.1: nothing.
        for p in [0.01, 0.05, 0.12, 0.08] {
            assert_eq!(r.update(2, 1, p, ON), (Some(PadPress { fingers: 2, deep: false }), false));
        }
        // Pressed on: deep, and the pad taps once, as the line is crossed.
        assert_eq!(r.update(2, 1, 0.65, ON), (Some(PadPress { fingers: 2, deep: true }), true));
        assert_eq!(r.update(2, 1, 0.99, ON), (Some(PadPress { fingers: 2, deep: true }), false));
        // Easing off a little is still deep (hysteresis); well below, it ends.
        assert_eq!(r.update(2, 1, 0.4, ON).0.unwrap().deep, true);
        assert_eq!(r.update(2, 1, 0.2, ON).0.unwrap().deep, false);
        // Pressed hard again in the same press: not again, and no second tap.
        assert_eq!(r.update(2, 1, 0.95, ON), (Some(PadPress { fingers: 2, deep: false }), false));
        // Let go, and the next press may.
        r.update(0, 0, 0.0, ON);
        r.update(2, 1, 0.05, ON);
        assert_eq!(r.update(2, 1, 0.7, ON), (Some(PadPress { fingers: 2, deep: true }), true));
    }

    #[test]
    fn two_fingers_pressed_hard_are_only_a_click_while_the_setting_is_off() {
        let mut r = PressReader::new();
        r.update(2, 1, 0.05, None);
        assert_eq!(r.update(2, 1, 0.99, None), (Some(PadPress { fingers: 2, deep: false }), false));
        // Nor does the pressure make anyone else's press deep.
        let mut r = PressReader::new();
        r.update(1, 1, 0.05, ON);
        assert_eq!(r.update(1, 1, 0.99, ON), (Some(PadPress { fingers: 1, deep: false }), false));
    }
}
