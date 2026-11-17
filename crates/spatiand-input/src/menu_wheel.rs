//! An open menu's list turned by a thumb on a pad, or a finger on the phone's touch area.
//!
//! The menus' lists move a row for every step of travel, the way a wheel turns past its
//! notches: the cursor walks, and the list scrolls once the cursor reaches its edge
//! (`spatiand_render::panel::keep_in_view`). What the hand gives is [`PadScroll`]'s, the same
//! filtered travel that scrolls a page, so a menu and a page are swiped alike on every pad.
//!
//! **A flick carries the list on by itself**, slowing as it goes, as a phone's list does: a
//! menu longer than the card was otherwise a swipe and a swipe and a swipe to its end. On a pad
//! it glides after [`Scroll::Fling`] -- the thumb left from the rim, still going. On a
//! touchscreen ([`MenuWheel::touchscreen`]) after any finger that lifts while still moving fast,
//! wherever it lifts, at the finger's own speed: a touchscreen has no rim to run out of and no
//! lift-off drift to be mistaken for speed, and a flick on one is over before [`PadScroll`],
//! which holds the last frames of every gesture back and drops them at the lift, has let much
//! of it out. A finger landing on a gliding list stops it, and that touch selects nothing
//! ([`MenuWheel::caught`]): on a phone, touching a list that is moving is how it is stopped.
//!
//! While the list is being turned ([`MenuWheel::turning`]) the pointer must not choose the row it
//! is on. On the Beam Pro the phone is the pointer *and* the touch area: a thumb swiping on it
//! tilts it, the pointer moves with it, and each frame it chose its row again -- the swipe moved
//! the highlight a row and the pointer put it straight back, so the list never scrolled and its
//! lower rows could not be reached (the owner, 2026-11-17).
//!
//! [`PadScroll`]: crate::scroll::PadScroll

use std::time::{Duration, Instant};

use crate::report::Pad;
use crate::scroll::Scroll;

/// How fast, in rows a second, a finger must still be going as it lifts from a touchscreen for
/// the list to glide on. A deliberate drag to a row is a few rows a second and lands where it
/// was left; a flick is tens.
pub const FLICK: f32 = 12.0;

/// How far a finger must have gone, in rows, for its lift to throw the list: less is a tap.
const THROW_AFTER: f32 = 0.5;

/// The fastest a list glides, in rows a second, however hard it was thrown: still rows that can
/// be read going past, and a long list crossed in a second or two.
pub const GLIDE_TOP: f32 = 40.0;

/// How quickly a glide slows: its speed falls by e (to about a third) in this long, in seconds.
/// A glide goes `speed × GLIDE_TAU` rows in all -- four from a flick at [`FLICK`], fourteen at
/// the top.
pub const GLIDE_TAU: f32 = 0.35;

/// Below this, in rows a second, a glide has stopped.
pub const GLIDE_STOPS: f32 = 3.0;

/// How quickly the speed estimates follow the hand, a share a frame.
const SPEED_ADAPT: f32 = 0.5;

/// How long after the touch that stopped a glide lifts its tap is still the stop's: a tap is
/// told a frame or two after its finger lifts.
const CATCH_HOLDS_TAP: Duration = Duration::from_millis(150);

/// The longest frame a speed is measured over: a stall is not a slow hand.
const LONGEST_FRAME: Duration = Duration::from_millis(100);

/// How far an open menu's cursor is to move this frame: rows down (up when negative) and, for
/// a grid, places right (left when negative).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Steps {
    pub down: i32,
    pub right: i32,
    /// Moved by a hand on the surface rather than by a glide: what a tick is felt for.
    pub by_hand: bool,
}

impl Steps {
    pub fn is_empty(&self) -> bool {
        self.down == 0 && self.right == 0
    }
}

/// One surface's turning of the open menu. See the module.
#[derive(Debug)]
pub struct MenuWheel {
    /// A row's worth of travel, in pad units (2 across the surface): the platform's, since a
    /// Deck's pad is a few centimetres and a phone's touch area most of its screen.
    row: f32,
    /// Whether a finger lifting while still fast glides, wherever it lifts.
    touchscreen: bool,
    /// Travel short of the next row, (across, up), pad units.
    owed: (f32, f32),
    /// What [`PadScroll`](crate::scroll::PadScroll) let out lately, (across, up), rows a second.
    speed: (f32, f32),
    /// The finger itself lately, (across, up), rows a second, and where it was last frame.
    finger: (f32, f32),
    at: Option<(f32, f32)>,
    /// Where this touch landed, and the furthest it has been from there, in rows.
    from: (f32, f32),
    travelled: f32,
    /// The glide under way, (across, up), in rows a second.
    glide: Option<(f32, f32)>,
    last: Option<Instant>,
    touched: bool,
    /// This touch has moved the cursor.
    turned: bool,
    /// This touch landed on a glide, and stopped it.
    caught: bool,
    /// The touch that stopped a glide lifted, and its tap is the stop's until then.
    caught_until: Option<Instant>,
}

impl MenuWheel {
    /// A pad's: only [`Scroll::Fling`] glides. `row` is a row's travel, in pad units.
    pub fn pad(row: f32) -> MenuWheel {
        MenuWheel::new(row, false)
    }

    /// A touchscreen's: a finger lifting while fast glides too.
    pub fn touchscreen(row: f32) -> MenuWheel {
        MenuWheel::new(row, true)
    }

    fn new(row: f32, touchscreen: bool) -> MenuWheel {
        MenuWheel {
            row: row.max(1e-3),
            touchscreen,
            owed: (0.0, 0.0),
            speed: (0.0, 0.0),
            finger: (0.0, 0.0),
            at: None,
            from: (0.0, 0.0),
            travelled: 0.0,
            glide: None,
            last: None,
            touched: false,
            turned: false,
            caught: false,
            caught_until: None,
        }
    }

    /// Everything let go of, as when the menu closes.
    pub fn forget(&mut self) {
        *self = MenuWheel::new(self.row, self.touchscreen);
    }

    /// The cursor has reached the end of the list: a glide stops there, and what it still owed
    /// is not saved up for the way back.
    pub fn stop(&mut self) {
        if self.glide.take().is_some() {
            self.owed = (0.0, 0.0);
        }
    }

    /// The list is being turned: a hand on the surface has moved it, or it is gliding. The
    /// pointer chooses no row meanwhile. See the module.
    pub fn turning(&self) -> bool {
        (self.touched && self.turned) || self.glide.is_some()
    }

    /// The touch on the surface, or the one just lifted from it, stopped a glide: a tap it makes
    /// is that, and selects nothing.
    pub fn caught(&self, now: Instant) -> bool {
        (self.touched && self.caught) || self.caught_until.is_some_and(|until| now < until)
    }

    /// Once a frame: what [`PadScroll`] made of the surface this frame, the surface as it is
    /// (its touch and where), and the time. Returns how far the cursor is to move.
    ///
    /// [`PadScroll`]: crate::scroll::PadScroll
    pub fn update(&mut self, scroll: Scroll, pad: &Pad, now: Instant) -> Steps {
        let touched = pad.touched;
        let dt = self
            .last
            .map(|t| now.saturating_duration_since(t).min(LONGEST_FRAME))
            .unwrap_or_default()
            .as_secs_f32();
        self.last = Some(now);
        let landed = touched && !self.touched;
        let lifted = !touched && self.touched;
        self.touched = touched;
        if self.caught_until.is_some_and(|until| now >= until) {
            self.caught_until = None;
        }

        if landed {
            self.caught = self.glide.take().is_some();
            self.caught_until = None;
            self.turned = false;
            self.speed = (0.0, 0.0);
            self.finger = (0.0, 0.0);
            self.at = None;
            self.from = (pad.x, pad.y);
            self.travelled = 0.0;
            self.owed = (0.0, 0.0);
        }

        let follow = |speed: &mut (f32, f32), moved: (f32, f32), row: f32| {
            if dt > 0.0 {
                speed.0 += (moved.0 / row / dt - speed.0) * SPEED_ADAPT;
                speed.1 += (moved.1 / row / dt - speed.1) * SPEED_ADAPT;
            }
        };
        let mut by_hand = false;
        if touched {
            let (dx, dy) = match scroll {
                Scroll::By { dx, dy } => (dx, dy),
                _ => (0.0, 0.0),
            };
            self.owed.0 += dx;
            self.owed.1 += dy;
            follow(&mut self.speed, (dx, dy), self.row);
            if let Some((x, y)) = self.at {
                follow(&mut self.finger, (pad.x - x, pad.y - y), self.row);
            }
            self.at = Some((pad.x, pad.y));
            let away = (pad.x - self.from.0).hypot(pad.y - self.from.1) / self.row;
            self.travelled = self.travelled.max(away);
            by_hand = true;
        } else {
            if lifted {
                let glide = if self.touchscreen {
                    // The finger's own speed, along whichever way it was mostly going.
                    let (vx, vy) = self.finger;
                    let fast = vx.hypot(vy) >= FLICK;
                    let swiped = self.turned || self.travelled >= THROW_AFTER;
                    (swiped && (fast || scroll == Scroll::Fling))
                        .then(|| if vy.abs() >= vx.abs() { (0.0, vy) } else { (vx, 0.0) })
                } else {
                    (self.turned && scroll == Scroll::Fling).then_some(self.speed)
                };
                self.glide = glide.map(|(gx, gy)| {
                    let top = gx.hypot(gy);
                    let scale = if top > GLIDE_TOP { GLIDE_TOP / top } else { 1.0 };
                    (gx * scale, gy * scale)
                });
                if self.caught {
                    self.caught_until = Some(now + CATCH_HOLDS_TAP);
                }
                self.caught = false;
                self.turned = false;
                self.at = None;
            }
            if let Some((gx, gy)) = self.glide {
                self.owed.0 += gx * self.row * dt;
                self.owed.1 += gy * self.row * dt;
                let slowed = (-dt / GLIDE_TAU).exp();
                let (gx, gy) = (gx * slowed, gy * slowed);
                self.glide = (gx.hypot(gy) >= GLIDE_STOPS).then_some((gx, gy));
            }
        }

        let mut steps = Steps { by_hand, ..Steps::default() };
        while self.owed.1.abs() >= self.row {
            let way = self.owed.1.signum();
            self.owed.1 -= way * self.row;
            // Up the surface reveals what is below, as a page does.
            steps.down += way as i32;
        }
        while self.owed.0.abs() >= self.row {
            let way = self.owed.0.signum();
            self.owed.0 -= way * self.row;
            // And to the right, what is on the left: the list follows the finger.
            steps.right -= way as i32;
        }
        if self.glide.is_none() && !touched {
            // Travel short of a row is not saved up for the next swipe.
            self.owed = (0.0, 0.0);
        }
        if touched && !steps.is_empty() {
            self.turned = true;
        }
        steps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: f32 = 0.18;
    const FRAME: Duration = Duration::from_micros(16_667);

    /// A finger or thumb on the surface, fed to a wheel a frame at a time, with what
    /// `PadScroll` let out taken to be the finger's own travel.
    struct Hand {
        w: MenuWheel,
        t: Instant,
        x: f32,
        y: f32,
        on: bool,
    }

    impl Hand {
        fn new(w: MenuWheel) -> Hand {
            Hand { w, t: Instant::now(), x: 0.0, y: -0.6, on: false }
        }

        fn pad(&self) -> Pad {
            Pad { x: self.x, y: self.y, touched: self.on, ..Pad::default() }
        }

        /// Down where it is, the frame it lands.
        fn land(&mut self) -> Steps {
            self.on = true;
            self.t += FRAME;
            let pad = self.pad();
            self.w.update(Scroll::Idle, &pad, self.t)
        }

        /// One frame, the finger going this far (up the surface for +dy).
        fn moves(&mut self, dx: f32, dy: f32) -> Steps {
            if !self.on {
                self.land();
            }
            self.x += dx;
            self.y += dy;
            self.t += FRAME;
            let pad = self.pad();
            self.w.update(Scroll::By { dx, dy }, &pad, self.t)
        }

        fn swipe(&mut self, frames: usize, dy: f32) -> i32 {
            (0..frames).map(|_| self.moves(0.0, dy).down).sum()
        }

        fn lift(&mut self, scroll: Scroll) -> i32 {
            self.on = false;
            self.t += FRAME;
            self.w.update(scroll, &Pad::default(), self.t).down
        }

        /// Frames with nothing on the surface until any glide is over; the rows it moved.
        fn glide_out(&mut self) -> i32 {
            let mut rows = 0;
            for _ in 0..600 {
                self.t += FRAME;
                let s = self.w.update(Scroll::Idle, &Pad::default(), self.t);
                assert!(!s.by_hand, "a glide is not the hand");
                rows += s.down;
            }
            rows
        }
    }

    #[test]
    fn a_row_for_every_rows_worth_of_travel() {
        let mut h = Hand::new(MenuWheel::touchscreen(ROW));
        // Slowly: 0.6 of a pad in a second, about three rows a second.
        let mut rows = 0;
        for _ in 0..60 {
            let s = h.moves(0.0, 0.01);
            assert!(s.by_hand);
            rows += s.down;
        }
        assert_eq!(rows, 3, "0.6 of a pad is three rows of 0.18");
        assert_eq!(h.lift(Scroll::Idle), 0, "a slow drag lands where it was left");
        assert!(!h.w.turning());
        assert_eq!(h.glide_out(), 0);
    }

    #[test]
    fn down_the_surface_is_up_the_list() {
        let mut h = Hand::new(MenuWheel::pad(0.16));
        assert_eq!(h.swipe(20, -0.02), -2);
    }

    #[test]
    fn a_flick_on_a_touchscreen_glides_on_and_slows() {
        let mut h = Hand::new(MenuWheel::touchscreen(ROW));
        // A flick: 0.08 of a pad a frame, about 27 rows a second, lifted mid-screen.
        let by_hand = h.swipe(8, 0.08);
        h.lift(Scroll::Idle);
        assert!(h.w.turning(), "gliding");
        let glided = h.glide_out();
        assert!(!h.w.turning(), "and stopped");
        assert!(by_hand >= 3, "{by_hand}");
        // speed × τ: 27 × 0.35 is about nine, less what slows below the stop.
        assert!((5..=12).contains(&glided), "glided {glided} rows");
    }

    #[test]
    fn a_pad_glides_only_when_the_thumb_left_from_its_rim() {
        let mut h = Hand::new(MenuWheel::pad(0.16));
        h.swipe(8, 0.08);
        h.lift(Scroll::Idle);
        assert!(!h.w.turning(), "a thumb lifted mid-pad has arrived");
        assert_eq!(h.glide_out(), 0);
        h.swipe(8, 0.08);
        h.lift(Scroll::Fling);
        assert!(h.w.turning());
        assert!(h.glide_out() > 0);
    }

    #[test]
    fn a_flick_that_stopped_before_lifting_does_not_glide() {
        let mut h = Hand::new(MenuWheel::touchscreen(ROW));
        h.swipe(8, 0.08);
        // Held still a moment, then lifted.
        h.swipe(10, 0.0);
        h.lift(Scroll::Idle);
        assert_eq!(h.glide_out(), 0);
    }

    #[test]
    fn a_quick_tap_throws_nothing() {
        let mut h = Hand::new(MenuWheel::touchscreen(ROW));
        // A tap that rolls a little as it lands: fast, but hardly anywhere.
        h.moves(0.0, 0.03);
        h.moves(0.0, 0.02);
        h.lift(Scroll::Idle);
        assert!(!h.w.turning());
        assert_eq!(h.glide_out(), 0);
    }

    #[test]
    fn a_glide_is_no_faster_than_the_top() {
        let mut h = Hand::new(MenuWheel::touchscreen(ROW));
        // As fast as a hand goes: 0.3 of a pad a frame, a hundred rows a second.
        h.swipe(6, 0.3);
        h.lift(Scroll::Idle);
        let mut first_second = 0;
        for _ in 0..60 {
            h.t += FRAME;
            first_second += h.w.update(Scroll::Idle, &Pad::default(), h.t).down;
        }
        assert!(first_second <= GLIDE_TOP as i32, "{first_second} rows in its first second");
        assert!(first_second >= 10, "{first_second}");
    }

    #[test]
    fn a_finger_landing_on_a_glide_stops_it_and_its_tap_selects_nothing() {
        let mut h = Hand::new(MenuWheel::touchscreen(ROW));
        h.swipe(8, 0.08);
        h.lift(Scroll::Idle);
        h.t += FRAME;
        h.w.update(Scroll::Idle, &Pad::default(), h.t);
        assert!(h.w.turning());
        // A tap: down for a few frames without moving, and up.
        assert!(h.land().is_empty());
        assert!(!h.w.turning(), "stopped");
        assert!(h.w.caught(h.t));
        h.swipe(4, 0.0);
        h.lift(Scroll::Idle);
        assert!(h.w.caught(h.t), "the tap, told as the finger lifts, is the catch's");
        assert!(h.w.caught(h.t + FRAME * 2), "and a frame or two after");
        assert_eq!(h.glide_out(), 0, "nothing moves on");
        assert!(!h.w.caught(h.t), "a later tap is a tap");
        // And a tap on a list at rest is the pointer's.
        h.land();
        assert!(!h.w.caught(h.t));
        h.lift(Scroll::Idle);
        assert!(!h.w.caught(h.t));
    }

    #[test]
    fn the_pointer_is_free_until_the_hand_has_turned_the_list() {
        let mut h = Hand::new(MenuWheel::touchscreen(ROW));
        // A finger resting, or a tap: the pointer still chooses.
        h.land();
        assert!(!h.w.turning());
        h.moves(0.0, 0.05);
        assert!(!h.w.turning(), "not a row yet");
        assert_eq!(h.moves(0.0, 0.15).down, 1);
        assert!(h.w.turning(), "from the first row on, until the finger lifts");
        h.swipe(10, 0.0);
        assert!(h.w.turning(), "resting mid-swipe is still the swipe");
        h.lift(Scroll::Idle);
        assert!(!h.w.turning());
    }

    #[test]
    fn the_end_of_the_list_stops_a_glide() {
        let mut h = Hand::new(MenuWheel::touchscreen(ROW));
        h.swipe(8, 0.08);
        h.lift(Scroll::Idle);
        assert!(h.w.turning());
        h.w.stop();
        assert!(!h.w.turning());
        assert_eq!(h.glide_out(), 0);
    }

    #[test]
    fn sideways_follows_the_finger() {
        let mut h = Hand::new(MenuWheel::touchscreen(ROW));
        let right: i32 = (0..10).map(|_| h.moves(0.04, 0.0).right).sum();
        assert_eq!(right, -2, "a finger going right shows what is on the left");
    }

    #[test]
    fn travel_short_of_a_row_is_not_saved_for_the_next_swipe() {
        let mut h = Hand::new(MenuWheel::pad(0.16));
        assert_eq!(h.swipe(3, 0.05), 0);
        h.lift(Scroll::Idle);
        assert_eq!(h.swipe(1, 0.05), 0, "0.15 then 0.05 is two swipes, not a row");
    }

    /// The phone's whole path: its touches (`phone::PhoneTouch`), the travel a page is scrolled by
    /// (`PadScroll`), and the rows. `fingers` is the touch area's y (0 top, 1 bottom) a frame.
    fn through_the_phone(fingers: &[f32]) -> (i32, i32, bool) {
        use crate::phone::{Phase, PhoneTouch};
        let mut touch = PhoneTouch::new();
        let mut scroll = crate::scroll::PadScroll::default();
        let mut w = MenuWheel::touchscreen(ROW);
        let start = Instant::now();
        let (mut by_hand, mut glided) = (0, 0);
        let mut tapped = false;
        for i in 0..fingers.len() + 300 {
            let ms = i as u64 * 16;
            match (i, fingers.get(i)) {
                (0, Some(y)) => touch.touch(Phase::Down, 0, 0.5, *y, ms),
                (_, Some(y)) => touch.touch(Phase::Move, 0, 0.5, *y, ms),
                (i, None) if i == fingers.len() => touch.touch(Phase::Up, 0, 0.5, fingers[i - 1], ms),
                _ => {}
            }
            let f = touch.frame(ms);
            tapped |= f.right_click;
            let s = w.update(scroll.update(&f.left_pad), &f.left_pad, start + Duration::from_millis(ms));
            if s.by_hand {
                by_hand += s.down;
            } else {
                glided += s.down;
            }
        }
        (by_hand, glided, tapped)
    }

    #[test]
    fn a_thumb_flicked_up_the_phone_turns_the_list_and_it_glides_on() {
        // Up a third of the touch area in a tenth of a second, lifted mid-screen.
        let flick: Vec<f32> = (0..7).map(|i| 0.7 - i as f32 * 0.05).collect();
        let (by_hand, glided, tapped) = through_the_phone(&flick);
        assert!(!tapped, "a swipe is no tap");
        assert!(by_hand + glided >= 6, "{by_hand} by hand, {glided} gliding");
        assert!(glided >= 4, "glided {glided}");
    }

    #[test]
    fn a_thumb_drawn_slowly_down_the_phone_walks_up_the_list_and_stops_with_it() {
        // Down half the touch area in a second and a half.
        let drag: Vec<f32> = (0..90).map(|i| 0.25 + i as f32 * 0.0056).collect();
        let (by_hand, glided, tapped) = through_the_phone(&drag);
        assert!(!tapped);
        assert!((-6..=-4).contains(&by_hand), "{by_hand}: a pad unit is about five rows");
        assert_eq!(glided, 0);
    }

    #[test]
    fn a_tap_on_the_phone_is_a_tap_and_turns_nothing() {
        let (by_hand, glided, tapped) = through_the_phone(&[0.5, 0.5, 0.501, 0.501]);
        assert!(tapped);
        assert_eq!((by_hand, glided), (0, 0));
    }
}
