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
//! - **A press goes down where the pointer rested just before pressing moved it.** Pressing
//!   moves the pointer before the button is felt to go down: a thumb pushing a pad's click rolls
//!   on it, a finger tapping the phone tips it, the head nods with the effort. So a press looks
//!   back a little ([`Steadiness::press_settle`], 120 ms at the room's default) for the last
//!   moment the pointer was at rest, and goes down there if it has only moved a little since (a
//!   degree at the default): a pointer that was still when the press began is clicked where it
//!   was still. One that was moving all that while is clicked where it is, and so is one that
//!   came to rest where it is -- moved onto a button and pressed. Never for a mouse, which is
//!   clicked where its hand put it. The room's view does the same for the head first
//!   (`spatiand_track::Steady`), which is why this is one setting with it.
//!
//! Done on the ray's direction, in angle, because that is what the tremor is in: the same turn
//! of the head is a few pixels of a near window and a fraction of one far away.
//!
//! For what a wearer aims only ([`crate::pointer::PointerState::motion`]). What the room sends
//! on an application's behalf -- the assistant's clicks, a test's -- is exact already.

use std::collections::VecDeque;

use glam::DVec3;
use spatiand_track::Steadiness;

/// How far a pointer may turn from where its button went down before it is followed, in
/// degrees.
///
/// Chosen, not measured (2026-11-12, with neither the Deck nor the Mac reachable to record a
/// head on): from the sizes it has to sit between.
///
/// - Above what moves during a press. When this was chosen the room's view counted a head
///   within 0.15 degrees of where it was as still and one 0.6 away as turned (it now holds a
///   band of 0.5 to 2.5, `spatiand_track::Steady`); a thumb's band on a pad
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

/// A pointer that moved slower than this between two frames, in degrees a second, was at rest.
///
/// The room's view and a thumb on a pad are each held quite still when nothing means to move
/// them (`spatiand_track::Steady`, `spatiand_input::steady`), so a pointer at rest moves by
/// nothing at all; this leaves room for what the view does settling. A hand moving a pointer
/// onto something goes faster than this until it stops on it.
const RESTING_DEGREES_PER_SECOND: f64 = 2.0;

/// How long the pointer's past is kept for a press to look back on, in milliseconds: more than
/// the furthest any level looks back.
const PAST_MS: u32 = 300;

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
        /// How far the aim was from `anchor` as the button went down, in radians, when the press
        /// went down where the pointer rested before pressing moved it: the band is that much
        /// wider, so that the press's own jolt is not taken for the start of a drag.
        slack: f64,
    },
    /// Let go, and `lag` radians short of the aim, on the side of `toward`.
    Trailing { toward: DVec3, lag: f64 },
}

/// One frame of the pointer's past.
#[derive(Debug, Clone, Copy)]
struct Shown {
    at_ms: u32,
    direction: DVec3,
    /// Shown where it was aimed, rather than held.
    free: bool,
    mouse: bool,
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
    /// Where it was shown over the last [`PAST_MS`] and when, whether that was where it was aimed
    /// (not held or making up a drag), and whether a mouse aimed it: what a press looks back on.
    /// Oldest first, one a frame.
    past: VecDeque<Shown>,
    /// How far back a press looks, in milliseconds, and how far the pointer may have gone since,
    /// in radians: the wearer's [`Steadiness`].
    settle: (u32, f64),
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
        let mut held = Self {
            hold: hold.to_radians(),
            // A mouse is never held further than anything else is.
            mouse: mouse.min(hold).to_radians(),
            state: State::Free,
            aimed: None,
            shown: None,
            hand: None,
            at_ms: 0,
            past: VecDeque::new(),
            settle: (0, 0.0),
        };
        held.set_steadiness(Steadiness::default());
        held
    }

    /// How far back a press looks for where the pointer rested: the wearer's level, as the room's
    /// view is held by it.
    pub fn set_steadiness(&mut self, level: Steadiness) {
        let (ms, degrees) = level.press_settle();
        self.settle = (ms.min(PAST_MS), degrees.to_radians());
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

        if let State::Held { anchor, hand: pressed_hand, pressed_ms, down, slack } = self.state {
            let off = angle(direction, anchor);
            let over = !down && time_ms.wrapping_sub(pressed_ms) >= MULTI_CLICK_MS;
            let hand_moved = match (hand, pressed_hand) {
                (Some(now), Some(then)) => angle(now, then) > self.mouse,
                _ => false,
            };
            let band = self.hold + slack;
            if !over && off <= band && !hand_moved {
                self.shown = Some(anchor);
                self.remember(anchor, time_ms);
                return anchor;
            }
            // Let go. No further behind than the band: an aim that comes back from somewhere
            // else altogether -- a thumb lifted and put down again -- is simply there.
            self.state = State::Trailing { toward: anchor, lag: off.min(band) };
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
        self.remember(shown, time_ms);
        shown
    }

    /// Keep where the pointer was shown at `time_ms` for a press to look back on: once a frame,
    /// the last word for a frame aimed twice.
    fn remember(&mut self, shown: DVec3, time_ms: u32) {
        let frame = Shown {
            at_ms: time_ms,
            direction: shown,
            free: self.aimed == Some(shown),
            mouse: self.hand.is_some(),
        };
        match self.past.back_mut() {
            Some(last) if last.at_ms == time_ms => *last = frame,
            _ => self.past.push_back(frame),
        }
        while self.past.front().is_some_and(|f| time_ms.wrapping_sub(f.at_ms) > PAST_MS) {
            self.past.pop_front();
        }
    }

    /// Where the pointer rested before a press at `time_ms` moved it, if it did; see the module.
    ///
    /// Nothing if it is resting now (it is where it rested), if a mouse aims it, if it has been
    /// moving all the while the press looks back over, if it has gone further from where it
    /// rested than a press moves it, or if it was not shown where it was aimed meanwhile -- held
    /// for a click, or making up a drag, which is the room's own doing and not a press's.
    pub fn rested(&self, time_ms: u32) -> Option<DVec3> {
        let (look, reach) = self.settle;
        if look == 0 || self.past.len() < 2 {
            return None;
        }
        let past: Vec<Shown> = self.past.iter().copied().collect();
        let now = past[past.len() - 1];
        // Not for an aim from some other time than this press's: that is not what it moved.
        if now.mouse || !now.free || time_ms.wrapping_sub(now.at_ms) > look {
            return None;
        }
        let resting = |i: usize| {
            let (from, to) = (past[i - 1], past[i]);
            let seconds = to.at_ms.wrapping_sub(from.at_ms).max(1) as f64 / 1000.0;
            from.free && angle(from.direction, to.direction).to_degrees() / seconds < RESTING_DEGREES_PER_SECOND
        };
        if resting(past.len() - 1) {
            return None;
        }
        for i in (1..past.len() - 1).rev() {
            let then = past[i];
            if then.mouse
                || !then.free
                || time_ms.wrapping_sub(then.at_ms) > look
                || angle(then.direction, now.direction) > reach
            {
                return None;
            }
            if resting(i) {
                return Some(then.direction);
            }
        }
        None
    }

    /// Whether a button going down at `time_ms` is the next of several clicks, or a second button
    /// beside the first: on the same point as the first, wherever the pointer is.
    fn again(&self, time_ms: u32) -> bool {
        match self.state {
            State::Held { pressed_ms, down, .. } => down || time_ms.wrapping_sub(pressed_ms) < MULTI_CLICK_MS,
            _ => false,
        }
    }

    /// Where a button going down at `time_ms` should go down instead of where the pointer is
    /// shown, if anywhere: where it rested before pressing moved it ([`Self::rested`]). Only for
    /// a pointer shown where it is aimed -- one still held, or making up a drag, is where the
    /// room put it -- and never for the next of several clicks, which goes where the first did.
    pub fn put_back(&self, time_ms: u32) -> Option<DVec3> {
        if self.hold <= 0.0 || self.again(time_ms) || !matches!(self.state, State::Free) {
            return None;
        }
        self.rested(time_ms)
    }

    /// A button went down: where the pointer was last shown, or at `back` (from
    /// [`Self::put_back`]) once the application has been told the pointer is there again.
    pub fn pressed_at(&mut self, time_ms: u32, back: Option<DVec3>) {
        if self.hold <= 0.0 {
            return;
        }
        if self.again(time_ms) {
            // (One that comes too late is found out here as well as in `aim`, for a pointer that
            // has not been aimed since.)
            if let State::Held { pressed_ms, down, .. } = &mut self.state {
                *down = true;
                *pressed_ms = time_ms;
            }
            return;
        }
        // The press's own jolt, which the band then allows for.
        let slack = match (back, self.aimed) {
            (Some(back), Some(aimed)) => angle(back, aimed),
            _ => 0.0,
        };
        // Where the application last saw the pointer, which is where its click will be.
        self.state = match back.or(self.shown) {
            Some(anchor) => State::Held { anchor, hand: self.hand, pressed_ms: time_ms, down: true, slack },
            None => State::Free,
        };
        if let Some(back) = back {
            self.shown = Some(back);
            self.remember(back, time_ms);
        }
    }

    /// Both at once, for the tests, where there is no application to tell: a button went down
    /// where [`Self::put_back`] says, and that is returned.
    #[cfg(test)]
    pub fn pressed(&mut self, time_ms: u32) -> Option<DVec3> {
        let back = self.put_back(time_ms);
        self.pressed_at(time_ms, back);
        back
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

    /// A pointer at rest at `rest` for half a second, then moved by a press over the `jolt` frames
    /// before it to `to`, step by step: where the press went down, and the hold after it.
    fn jolted(level: Steadiness, rest: DVec3, to: DVec3, jolt: u32) -> (Option<DVec3>, PressHold, u32) {
        let mut hold = PressHold::default();
        hold.set_steadiness(level);
        let mut frame = 0;
        for _ in 0..36 {
            hold.aim(rest, None, frame * FRAME_MS);
            frame += 1;
        }
        for step in 1..=jolt {
            hold.aim(turned(rest, to, angle(rest, to) * step as f64 / jolt as f64), None, frame * FRAME_MS);
            frame += 1;
        }
        let at = (frame - 1) * FRAME_MS;
        (hold.pressed(at), hold, frame)
    }

    #[test]
    fn a_press_that_jolts_the_pointer_goes_down_where_it_rested() {
        // A thumb rolling as it pushes the pad's click down: the pointer had been still, and in
        // the 70 ms before the button was felt to go down it went half a degree -- 23 pixels of a
        // window at its default distance, a link's width.
        let rest = towards(3.0, 1.0);
        let to = towards(3.4, 0.7);
        let (back, mut hold, frame) = jolted(Steadiness::Medium, rest, to, 5);
        assert_eq!(back, Some(rest));
        // And held there while the button is down, the thumb still rolled over: the press's own
        // jolt is not the start of a drag.
        for f in frame..frame + 10 {
            assert_eq!(hold.aim(towards(3.45, 0.68), None, f * FRAME_MS), rest);
        }
        hold.released();
        // A real drag from there is one, from where it went down.
        let mut shown = rest;
        for f in frame + 10..frame + 60 {
            shown = hold.aim(towards(3.45 + 0.05 * (f - frame - 10) as f64, 0.68), None, f * FRAME_MS);
        }
        assert!(degrees(shown, rest) > 2.0, "never let go: {}", degrees(shown, rest));
    }

    #[test]
    fn a_pointer_moved_onto_something_and_pressed_there_is_not_put_back() {
        // Moved onto a button, at rest there for a frame, pressed: there.
        let mut hold = PressHold::default();
        for frame in 0..20u32 {
            hold.aim(towards(0.1 * frame as f64, 0.0), None, frame * FRAME_MS);
        }
        let there = towards(1.9, 0.0);
        hold.aim(there, None, 20 * FRAME_MS);
        assert_eq!(hold.pressed(20 * FRAME_MS), None);
        assert_eq!(hold.aim(towards(1.95, 0.0), None, 21 * FRAME_MS), there);
        // Pressed on the way, never at rest: where it is.
        let mut hold = PressHold::default();
        let mut last = DVec3::X;
        for frame in 0..20u32 {
            last = hold.aim(towards(0.1 * frame as f64, 0.0), None, frame * FRAME_MS);
        }
        assert_eq!(hold.pressed(19 * FRAME_MS), None);
        assert_eq!(hold.aim(towards(1.95, 0.0), None, 20 * FRAME_MS), last);
    }

    #[test]
    fn a_rest_too_long_ago_or_too_far_away_is_not_gone_back_to() {
        let rest = towards(0.0, 0.0);
        // Moving for 200 ms before the press: a hand going somewhere, not a press's jolt.
        let (back, ..) = jolted(Steadiness::Medium, rest, towards(0.6, 0.0), 15);
        assert_eq!(back, None);
        // Two degrees in 70 ms: further than a press moves anything.
        let (back, ..) = jolted(Steadiness::Medium, rest, towards(2.0, 0.0), 5);
        assert_eq!(back, None);
        // The stronger the level, the further back and the further away it is found.
        let (back, ..) = jolted(Steadiness::Light, rest, towards(0.8, 0.0), 5);
        assert_eq!(back, None);
        let (back, ..) = jolted(Steadiness::Strong, rest, towards(1.4, 0.0), 9);
        assert_eq!(back, Some(rest));
    }

    #[test]
    fn a_mouse_is_clicked_where_its_hand_put_it() {
        let mut hold = PressHold::default();
        let rest = towards(1.0, 0.0);
        for frame in 0..20u32 {
            hold.aim(rest, Some(rest), frame * FRAME_MS);
        }
        let mut aim = rest;
        for frame in 20..24u32 {
            aim = towards(1.0 + 0.1 * (frame - 19) as f64, 0.0);
            hold.aim(aim, Some(aim), frame * FRAME_MS);
        }
        assert_eq!(hold.pressed(23 * FRAME_MS), None);
        assert_eq!(hold.aim(aim, Some(aim), 24 * FRAME_MS), aim);
    }

    #[test]
    fn off_it_looks_back_at_nothing() {
        let (back, ..) = jolted(Steadiness::Off, towards(0.0, 0.0), towards(0.3, 0.0), 5);
        assert_eq!(back, None);
    }

    #[test]
    fn a_press_that_jolts_the_head_clicks_where_the_wearer_pointed() {
        // The whole of it, with the room's view: a body swaying a little, a thumb resting on a
        // pad three degrees right of and one above the middle of the view, and a press that nods
        // the head down by 0.5 to 1.5 degrees over the 80 ms before the button is felt to go
        // down. Where the click lands against where the pointer was when the nod began, in
        // degrees, for each level; and whether the pointer stays put while the button is down.
        use spatiand_track::smoothing::motion::{head, Sway};
        let pad = towards(3.0, 1.0);
        let rate = 72.0;
        let dt = 1.0 / rate;
        let ms = |frame: u32| (frame as f64 * 1000.0 / rate) as u32;
        println!("{:>8} {:>6} {:>8} {:>8}", "level", "nod", "missed", "moved");
        for level in Steadiness::ALL {
            for nod in [0.5, 1.0, 1.5] {
                let mut view = level.hold().map(spatiand_track::Steady::new);
                let mut hold = PressHold::default();
                hold.set_steadiness(level);
                let mut sway = Sway::new(0.3, 0.8);
                let begin = 5.0;
                let (mut meant, mut clicked, mut moved) = (None, None, 0.0f64);
                for frame in 0..(6.0 * rate) as u32 {
                    let t = frame as f64 * dt;
                    let s = t - begin;
                    let bump = if s < 0.0 {
                        0.0
                    } else if s < 0.08 {
                        nod * (std::f64::consts::FRAC_PI_2 * s / 0.08).sin()
                    } else if s < 0.23 {
                        nod * (std::f64::consts::FRAC_PI_2 * (0.23 - s) / 0.15).sin()
                    } else {
                        0.0
                    };
                    let (right, up, roll) = sway.at(t);
                    let h = head(right, up - bump, roll);
                    let drawn = view.as_mut().map_or(h, |v| v.filter(h, dt));
                    let shown = hold.aim(drawn * pad, None, ms(frame));
                    if s < 0.0 {
                        meant = Some(shown);
                    }
                    if clicked.is_none() && s >= 0.08 - dt / 2.0 {
                        let back = hold.pressed(ms(frame));
                        clicked = Some(back.unwrap_or(shown));
                    } else if let Some(at) = clicked.filter(|_| s < 0.3) {
                        moved = moved.max(degrees(shown, at));
                    }
                    if s >= 0.18 && s < 0.18 + dt {
                        hold.released();
                    }
                }
                let missed = degrees(clicked.unwrap(), meant.unwrap());
                println!("{:>8} {:>5.1}° {:>8.3} {:>8.3}", level.name(), nod, missed, moved);
                // Off, the click is where the nod took the pointer, as it always was; the lightest
                // takes a nod of a degree, not one of one and a half.
                match level {
                    Steadiness::Off => assert!(missed > nod * 0.8, "off, and yet {missed}"),
                    Steadiness::Light if nod > 1.0 => {}
                    _ => {
                        // Two pixels of a window at its default distance: what the lightest's band
                        // lets through of a nod is too slow to count as moving.
                        assert!(missed < 0.05, "{level:?}: a nod of {nod} missed by {missed:.3} degrees");
                        assert_eq!(moved, 0.0, "{level:?}: the pointer moved while the button was down");
                    }
                }
            }
        }
    }
}
