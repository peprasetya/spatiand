//! A pointer held where its button went down.
//!
//! Every pointer in the room is cast through the head: a thumb on a pad, a phone's laser, a
//! mouse are each a place in the view, and the view turns with the wearer. So a pointer nobody
//! is moving is never still, and least of all while a button is being pressed -- the press
//! itself moves the thumb, the finger, the hand, and the head goes on breathing. To an
//! application that is a button going down, a few pixels of movement, and the button coming up,
//! which is not a click:
//!
//! - a browser starts dragging a link after a few pixels with the button down, and the page
//!   never sees the click (nine such drags in one afternoon on the Mac, 2026-11-12);
//! - a second and third click count as a double and a triple only if they land within a pixel
//!   or two of the first, so a word or a line could not be selected.
//!
//! Taking the tremor out by smoothing would make every pointer swim. What sets it apart from a
//! hand that means to drag is its size, as with a thumb's wander on a pad
//! (`spatiand_input::steady`), so this is the same idea, turned to the press:
//!
//! - **While a button is down the pointer stays where it went down**, until what aims it has
//!   gone further than [`HOLD_DEGREES`] from there. Then it is let go, and what was held back
//!   is made up over the next of its travel rather than at once, so nothing jumps: a drag, a
//!   selection, a slider all begin where the button went down and end where the hand is.
//! - **It stays there after the button comes up too, for [`MULTI_CLICK_MS`] from the press**,
//!   and a button that goes down again inside that time and that angle goes down on the same
//!   point, to the pixel: a double click, a triple click.
//! - **A mouse is told apart from the head it rides on.** A hand on a mouse or a trackpad that
//!   moves it at all means to; it is let go once the hand's own part of the aim has gone
//!   [`MOUSE_HOLD_DEGREES`], while the head's part keeps the whole band.
//!
//! Done on the ray's direction, in angle, because that is what the tremor is in: the same turn
//! of the head is a few pixels of a near window and a fraction of one far away.
//!
//! For what a wearer aims only ([`crate::pointer::PointerState::motion`]). What the room sends
//! on an application's behalf -- the assistant's clicks, a test's -- is exact already.

use glam::DVec3;

/// How far a pointer may turn from where its button went down before it is followed, in
/// degrees.
///
/// Chosen, not measured (2026-11-12, with neither the Deck nor the Mac reachable to record a
/// head on): from the sizes it has to sit between.
///
/// - Above what moves during a press. `spatiand_track::Steady` counts a head within 0.15
///   degrees of where it was as still and one 0.6 away as turned; a thumb's band on a pad
///   (`spatiand_input::steady`, 0.008 of a pad at 19 degrees to the unit) is 0.15 degrees. A
///   browser takes a link for dragged after about four pixels, which at the 46 pixels a degree
///   of a window at its default distance is under a tenth of a degree.
/// - Below what is aimed at. The smallest thing on a window is about a degree
///   (`FURNITURE_FRACTION` in `pointer.rs`), a line of text about 0.4.
///
/// So 0.4: about 18 pixels of a window at its default distance, less of one further away. A
/// drag gives up that much of its start, and has it back within as much again of travel.
/// `pointer_hold_degrees` in `prefs.toml` changes it; 0 turns the hold off.
pub const HOLD_DEGREES: f64 = 0.4;

/// How far a mouse's own part of the aim may go before the pointer is followed, in degrees.
///
/// A mouse lies on a desk and a finger on a trackpad barely slides as it clicks, so what a
/// press moves of them is a pixel or two; anything more is the hand dragging, and a drag held
/// back for 18 pixels would feel stuck. 0.1 of a degree is four to five pixels of a window at
/// its default distance -- what toolkits themselves take as the start of a drag, so nothing an
/// application would have acted on is lost. The head under the mouse still has all of
/// [`HOLD_DEGREES`]. `pointer_hold_mouse_degrees` in `prefs.toml`.
pub const MOUSE_HOLD_DEGREES: f64 = 0.1;

/// How long after a press the pointer is still held for the next one, in milliseconds.
///
/// The time within which applications count a second press as a double click: Chromium's and
/// Qt's 500 ms, measured press to press (GTK's is 400). No longer, or a pointer at rest after a
/// click would seem dead to the hover it was moved for.
pub const MULTI_CLICK_MS: u32 = 500;

/// How fast a pointer that was let go closes on where it is aimed when the aim itself has
/// stopped, in degrees a second.
///
/// What was held back is made up by the aim's own travel, and that is all there is to it
/// while the hand moves. This is for the hand that stops short: without it the pointer would
/// stay a fraction of a degree off the aim until the next movement. A whole band in a fifth of
/// a second, which is seen as settling rather than as moving.
const SETTLE_DEGREES_PER_SECOND: f64 = 2.0;

#[derive(Debug, Clone, Copy, Default)]
enum State {
    /// Shown where it is aimed.
    #[default]
    Free,
    /// Held on `anchor`.
    Held {
        anchor: DVec3,
        /// The hand's own part of the aim when the button went down, when a mouse aims.
        hand: Option<DVec3>,
        /// When the button last went down.
        pressed_ms: u32,
        /// Whether a button is down now.
        down: bool,
    },
    /// Let go, and `lag` radians short of the aim, on the side of `toward`.
    Trailing { toward: DVec3, lag: f64 },
}

/// The rule; see the module.
#[derive(Debug, Clone)]
pub struct PressHold {
    /// [`HOLD_DEGREES`] and [`MOUSE_HOLD_DEGREES`], or what the wearer set, in radians.
    hold: f64,
    mouse: f64,
    state: State,
    /// Where the pointer was last aimed, where it was shown, the mouse's own part, and when.
    aimed: Option<DVec3>,
    shown: Option<DVec3>,
    hand: Option<DVec3>,
    at_ms: u32,
}

impl Default for PressHold {
    fn default() -> Self {
        Self::new(HOLD_DEGREES, MOUSE_HOLD_DEGREES)
    }
}

impl PressHold {
    pub fn new(hold_degrees: f64, mouse_degrees: f64) -> Self {
        let hold = if hold_degrees.is_finite() { hold_degrees.clamp(0.0, 5.0) } else { HOLD_DEGREES };
        let mouse = if mouse_degrees.is_finite() { mouse_degrees.clamp(0.0, 5.0) } else { MOUSE_HOLD_DEGREES };
        Self {
            hold: hold.to_radians(),
            // A mouse is never held further than anything else is.
            mouse: mouse.min(hold).to_radians(),
            state: State::Free,
            aimed: None,
            shown: None,
            hand: None,
            at_ms: 0,
        }
    }

    /// Where the pointer is to be shown, aimed along `direction` (unit length) at `time_ms`.
    ///
    /// `hand` is the mouse's own part of that aim -- the direction in the head's frame --
    /// when a mouse is what aims, and nothing for anything else.
    pub fn aim(&mut self, direction: DVec3, hand: Option<DVec3>, time_ms: u32) -> DVec3 {
        let was = self.aimed.replace(direction);
        let elapsed_ms = time_ms.wrapping_sub(self.at_ms);
        self.at_ms = time_ms;
        self.hand = hand;

        if let State::Held { anchor, hand: pressed_hand, pressed_ms, down } = self.state {
            let off = angle(direction, anchor);
            let over = !down && time_ms.wrapping_sub(pressed_ms) >= MULTI_CLICK_MS;
            let hand_moved = match (hand, pressed_hand) {
                (Some(now), Some(then)) => angle(now, then) > self.mouse,
                _ => false,
            };
            if !over && off <= self.hold && !hand_moved {
                self.shown = Some(anchor);
                return anchor;
            }
            // Let go. No further behind than the band: an aim that comes back from somewhere
            // else altogether -- a thumb lifted and put down again -- is simply there.
            self.state = State::Trailing { toward: anchor, lag: off.min(self.hold) };
        }

        let shown = match self.state {
            State::Trailing { toward, lag } => {
                // Made up by as much as the aim travels, so the pointer goes at most twice the
                // hand's pace until it has caught up; and a little by itself, for a hand that
                // has stopped. Never past where it was held: an aim that turns back to there
                // has nothing left to make up.
                let moved = was.map_or(0.0, |was| angle(direction, was));
                // A stall is not a quarter of a second of settling to be done at once.
                let settled = SETTLE_DEGREES_PER_SECOND.to_radians() * elapsed_ms.min(50) as f64 / 1000.0;
                let lag = (lag - moved - settled).min(angle(direction, toward));
                if lag <= 1e-7 {
                    self.state = State::Free;
                    direction
                } else {
                    self.state = State::Trailing { toward, lag };
                    turned(direction, toward, lag)
                }
            }
            _ => direction,
        };
        self.shown = Some(shown);
        shown
    }

    /// A button went down.
    pub fn pressed(&mut self, time_ms: u32) {
        if self.hold <= 0.0 {
            return;
        }
        if let State::Held { pressed_ms, down, .. } = &mut self.state {
            // A second button beside the first, or the next click of several: on the same
            // point. (One that comes too late is found out here as well as in `aim`, for a
            // pointer that has not been aimed since.)
            if *down || time_ms.wrapping_sub(*pressed_ms) < MULTI_CLICK_MS {
                *down = true;
                *pressed_ms = time_ms;
                return;
            }
        }
        // Where the application last saw the pointer, which is where its click will be.
        self.state = match self.shown {
            Some(anchor) => State::Held { anchor, hand: self.hand, pressed_ms: time_ms, down: true },
            None => State::Free,
        };
    }

    /// The last button held came up.
    pub fn released(&mut self) {
        if let State::Held { down, .. } = &mut self.state {
            *down = false;
        }
    }

    /// Whether the pointer is being shown somewhere other than where it is aimed.
    #[cfg(test)]
    fn holding(&self) -> bool {
        !matches!(self.state, State::Free)
    }
}

/// The angle between two unit directions, in radians. By the cross product, since the angles
/// here are small and an arc cosine near one loses them.
fn angle(a: DVec3, b: DVec3) -> f64 {
    a.cross(b).length().atan2(a.dot(b))
}

/// `from` turned `by` radians towards `to`; `to` itself if that is nearer.
fn turned(from: DVec3, to: DVec3, by: f64) -> DVec3 {
    let axis = from.cross(to);
    let length = axis.length();
    if length < 1e-12 || length.atan2(from.dot(to)) <= by {
        return to;
    }
    (glam::DQuat::from_axis_angle(axis / length, by) * from).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The room's frame time at the glasses' 72 Hz, near enough.
    const FRAME_MS: u32 = 14;

    /// A direction `right` and `up` degrees from straight ahead (+X ahead, +Y left, +Z up).
    fn towards(right: f64, up: f64) -> DVec3 {
        let (yaw, pitch) = (-right.to_radians(), -up.to_radians());
        glam::DQuat::from_axis_angle(DVec3::Z, yaw) * (glam::DQuat::from_axis_angle(DVec3::Y, pitch) * DVec3::X)
    }

    fn degrees(a: DVec3, b: DVec3) -> f64 {
        angle(a, b).to_degrees()
    }

    /// A head's and a hand's tremor, made up, the same every run: a pulse and some noise,
    /// `size` degrees at most from the middle.
    struct Tremor {
        seed: u32,
        size: f64,
    }

    impl Tremor {
        fn at(&mut self, frame: u32) -> (f64, f64) {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 17;
            self.seed ^= self.seed << 5;
            let noise = (self.seed as f64 / u32::MAX as f64) * 2.0 - 1.0;
            let t = frame as f64 * FRAME_MS as f64 / 1000.0;
            let pulse = (t * std::f64::consts::TAU * 1.2).sin();
            (self.size * (0.6 * pulse + 0.3 * noise), self.size * (0.5 * (t * 9.0).cos() + 0.3 * noise))
        }
    }

    #[test]
    fn a_pointer_nobody_presses_is_where_it_is_aimed() {
        let mut hold = PressHold::default();
        let mut tremor = Tremor { seed: 7, size: 0.3 };
        for frame in 0..200 {
            let (x, y) = tremor.at(frame);
            let aim = towards(3.0 + x, 1.0 + y);
            assert_eq!(hold.aim(aim, None, frame * FRAME_MS), aim);
        }
    }

    #[test]
    fn a_press_under_a_trembling_head_does_not_move_the_pointer() {
        // What made a link's click a drag: the button down, the pointer a few pixels on.
        let mut hold = PressHold::default();
        let mut tremor = Tremor { seed: 11, size: 0.3 };
        let down_at = hold.aim(towards(3.0, 1.0), None, 0);
        hold.pressed(0);
        for frame in 1..=20 {
            let (x, y) = tremor.at(frame);
            let shown = hold.aim(towards(3.0 + x, 1.0 + y), None, frame * FRAME_MS);
            assert_eq!(shown, down_at, "moved on frame {frame} of the press");
        }
        hold.released();
        // And the button came up where it went down.
        assert_eq!(hold.aim(towards(3.1, 1.1), None, 21 * FRAME_MS), down_at);
    }

    #[test]
    fn a_drag_is_let_go_and_catches_up_without_a_jump() {
        let mut hold = PressHold::default();
        let start = hold.aim(towards(0.0, 0.0), None, 0);
        hold.pressed(0);
        // A slow, deliberate drag to the right: three degrees a second.
        let step = 3.0 * FRAME_MS as f64 / 1000.0;
        let mut last_shown = start;
        let mut last_aim = start;
        let mut left_at = None;
        for frame in 1..=120u32 {
            let aim = towards(step * frame as f64, 0.0);
            let shown = hold.aim(aim, None, frame * FRAME_MS);
            let (pointer_went, hand_went) = (degrees(shown, last_shown), degrees(aim, last_aim));
            if pointer_went > 0.0 && left_at.is_none() {
                left_at = Some(degrees(aim, start));
            }
            // Never faster than twice the hand and the little it settles by itself.
            let settle = SETTLE_DEGREES_PER_SECOND * FRAME_MS as f64 / 1000.0;
            assert!(pointer_went <= 2.0 * hand_went + settle + 1e-9, "a jump of {pointer_went} degrees on frame {frame}");
            // Never backwards, never ahead of the hand.
            assert!(degrees(shown, start) + 1e-9 >= degrees(last_shown, start), "went back on frame {frame}");
            assert!(degrees(shown, start) <= degrees(aim, start) + 1e-9, "ahead of the hand on frame {frame}");
            last_shown = shown;
            last_aim = aim;
        }
        let left_at = left_at.expect("the drag never moved the pointer");
        assert!(left_at > HOLD_DEGREES && left_at < HOLD_DEGREES + 2.0 * step, "let go {left_at} degrees out");
        // Caught up, exactly: the end of a selection is where the hand is.
        assert_eq!(last_shown, last_aim);
        assert!(!hold.holding());
        // And it was within a band's travel of being let go.
        let mut hold = PressHold::default();
        hold.aim(towards(0.0, 0.0), None, 0);
        hold.pressed(0);
        let mut caught_at = None;
        for frame in 1..=120u32 {
            let aim = towards(step * frame as f64, 0.0);
            if hold.aim(aim, None, frame * FRAME_MS) == aim {
                caught_at = Some(step * frame as f64);
                break;
            }
        }
        assert!(caught_at.is_some_and(|at| at <= 2.0 * HOLD_DEGREES + 2.0 * step), "caught up only at {caught_at:?}");
    }

    #[test]
    fn a_double_and_a_triple_click_land_on_one_point() {
        let mut hold = PressHold::default();
        let mut tremor = Tremor { seed: 23, size: 0.25 };
        let mut frame = 0u32;
        let mut aim_now = |hold: &mut PressHold, frame: u32| {
            let (x, y) = tremor.at(frame);
            hold.aim(towards(-2.0 + x, 4.0 + y), None, frame * FRAME_MS)
        };
        let first = aim_now(&mut hold, frame);
        let mut clicks = Vec::new();
        for _ in 0..3 {
            // Down for six frames, up for ten: 220 ms from press to press.
            hold.pressed(frame * FRAME_MS);
            clicks.push(aim_now(&mut hold, frame));
            for _ in 0..6 {
                frame += 1;
                assert_eq!(aim_now(&mut hold, frame), first);
            }
            hold.released();
            for _ in 0..10 {
                frame += 1;
                assert_eq!(aim_now(&mut hold, frame), first);
            }
        }
        assert_eq!(clicks, vec![first; 3]);
    }

    #[test]
    fn a_click_somewhere_else_is_somewhere_else() {
        let mut hold = PressHold::default();
        let first = hold.aim(towards(0.0, 0.0), None, 0);
        hold.pressed(0);
        hold.released();
        // Moved a degree on within the double click's time: not the same click.
        let mut shown = first;
        for frame in 1..=10u32 {
            shown = hold.aim(towards(0.1 * frame as f64, 0.0), None, frame * FRAME_MS);
        }
        let there = towards(1.0, 0.0);
        for frame in 11..=14u32 {
            shown = hold.aim(there, None, frame * FRAME_MS);
        }
        assert_eq!(shown, there);
        hold.pressed(14 * FRAME_MS);
        assert_eq!(hold.aim(towards(1.05, 0.0), None, 15 * FRAME_MS), there);
    }

    #[test]
    fn a_late_second_click_is_a_new_one_and_the_pointer_settles_first() {
        let mut hold = PressHold::default();
        let first = hold.aim(towards(0.0, 0.0), None, 0);
        hold.pressed(0);
        hold.released();
        // The aim drifts a quarter of a degree and stays; held for the double click's time.
        let rest = towards(0.25, 0.0);
        let mut time = 0;
        while time + FRAME_MS < MULTI_CLICK_MS {
            time += FRAME_MS;
            assert_eq!(hold.aim(rest, None, time), first);
        }
        // Then it settles on the aim, by itself and a little at a time.
        let mut last = first;
        let mut frames = 0;
        loop {
            time += FRAME_MS;
            let shown = hold.aim(rest, None, time);
            assert!(degrees(shown, last) <= SETTLE_DEGREES_PER_SECOND * FRAME_MS as f64 / 1000.0 + 1e-9);
            last = shown;
            frames += 1;
            if shown == rest {
                break;
            }
            assert!(frames < 30, "never settled");
        }
        hold.pressed(time);
        assert_eq!(hold.aim(towards(0.3, 0.0), None, time + FRAME_MS), rest);
    }

    #[test]
    fn a_long_press_lets_go_as_the_button_comes_up() {
        let mut hold = PressHold::default();
        let first = hold.aim(towards(0.0, 0.0), None, 0);
        hold.pressed(0);
        let rest = towards(0.2, 0.1);
        assert_eq!(hold.aim(rest, None, 900), first);
        hold.released();
        // Nothing jumps: it is still there on the next frame, and settling.
        let shown = hold.aim(rest, None, 914);
        assert!(degrees(shown, first) < 0.05, "jumped {} degrees", degrees(shown, first));
    }

    #[test]
    fn a_mouse_moved_at_all_is_followed_and_the_head_under_it_is_not() {
        let head = |right: f64| glam::DQuat::from_axis_angle(DVec3::Z, -right.to_radians());
        // The head turns a third of a degree under a mouse that is not touched: held.
        let mut hold = PressHold::default();
        let hand = towards(2.0, 0.0);
        let first = hold.aim(head(0.0) * hand, Some(hand), 0);
        hold.pressed(0);
        for frame in 1..=10u32 {
            let turn = head(0.033 * frame as f64);
            assert_eq!(hold.aim(turn * hand, Some(hand), frame * FRAME_MS), first);
        }
        // The mouse moves on under a head that is still: followed, from
        // where it was held, a frame's travel at a time.
        let mut hold = PressHold::default();
        let first = hold.aim(hand, Some(hand), 0);
        hold.pressed(0);
        let mut last = first;
        let settle = SETTLE_DEGREES_PER_SECOND * FRAME_MS as f64 / 1000.0;
        for frame in 1..=14u32 {
            let moved = towards(2.0 + 0.0125 * frame as f64, 0.0);
            let shown = hold.aim(moved, Some(moved), frame * FRAME_MS);
            assert!(degrees(shown, last) <= 0.0125 * 2.0 + settle + 1e-9);
            last = shown;
            if frame <= 8 {
                assert_eq!(shown, first, "let go on frame {frame}, before a tenth of a degree");
            } else {
                assert_ne!(shown, first, "still held on frame {frame}, past a tenth of a degree");
            }
        }
        assert_eq!(last, towards(2.175, 0.0));
        // The same eighth of a degree from a thumb or a laser is inside the band.
        let mut hold = PressHold::default();
        let first = hold.aim(hand, None, 0);
        hold.pressed(0);
        assert_eq!(hold.aim(towards(2.125, 0.0), None, FRAME_MS), first);
    }

    #[test]
    fn an_aim_that_comes_back_elsewhere_is_there_within_the_band() {
        // A thumb lifted during the hold and put down across the view.
        let mut hold = PressHold::default();
        let first = hold.aim(towards(0.0, 0.0), None, 0);
        hold.pressed(0);
        hold.released();
        let far = towards(15.0, 5.0);
        let shown = hold.aim(far, None, 100);
        assert!(degrees(shown, far) <= HOLD_DEGREES + 1e-9);
        assert!(degrees(shown, first) > 10.0);
    }

    #[test]
    fn set_to_nothing_it_holds_nothing() {
        let mut hold = PressHold::new(0.0, 0.0);
        hold.aim(towards(0.0, 0.0), None, 0);
        hold.pressed(0);
        let aim = towards(0.01, 0.0);
        assert_eq!(hold.aim(aim, None, FRAME_MS), aim);
    }

    #[test]
    fn a_second_button_does_not_move_the_first() {
        let mut hold = PressHold::default();
        let first = hold.aim(towards(0.0, 0.0), None, 0);
        hold.pressed(0);
        hold.aim(towards(0.2, 0.0), None, 600);
        hold.pressed(600);
        assert_eq!(hold.aim(towards(0.3, 0.0), None, 614), first);
    }
}
