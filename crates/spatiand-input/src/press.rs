//! The two pads' clicks: one pad pressed is a mouse button, both pressed together a gesture.
//!
//! A thumb *resting* on each pad is nothing. The pads are held that way all the time -- one
//! scrolling, the other pointing -- and a toolkit that hears two contacts and waits for a gesture
//! (Chromium does, from a two-finger scroll) made that ordinary grip a back gesture that scrolled
//! nothing. Pressing both is deliberate, and that is what this reads: the thumbs then swept
//! sideways together are back or forward, and spread apart or drawn together are a pinch. Neither
//! press is a click.
//!
//! Either pad may go down first. A pad's press waits [`TOGETHER_WITHIN`] for the other's before
//! it is let through as a click -- late by that much, which a click does not notice -- and a tap
//! shorter than that is let through whole, pressed and released, the moment it lets go.

use std::time::{Duration, Instant};

use crate::report::Pad;

/// How long one pad's press waits for the other's before it is a click.
pub const TOGETHER_WITHIN: Duration = Duration::from_millis(80);

/// How far the thumbs move, in pad units (a pad spans −1..1), before the gesture is decided: a
/// sweep if they went the same way, a pinch if apart or towards each other. Short of it both
/// presses are only held, since a thumb shifts as its pad goes down.
const DECIDED_AFTER: f32 = 0.12;

/// How far the thumbs sweep together, in pad units, to go back or forward. Once a gesture.
const SWEEP: f32 = 0.35;

/// How strongly the thumbs' spread zooms: their separation changing by `d` is a scale of
/// `e^(d × rate)`. Each thumb moving 0.8 of a pad outwards -- a comfortable spread -- is about
/// twice the size, as a spread across a laptop's touchpad is; the whole width of both, six times.
const PINCH_RATE: f32 = 0.45;

/// Back or forward, as a sweep of both thumbs says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sweep {
    /// The thumbs swept right. The page goes the way they did and the one before it comes in
    /// from the left, as under a two-finger swipe on a laptop.
    Back,
    /// The thumbs swept left.
    Forward,
}

/// What the pads' presses are this frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pressed {
    /// The left pad's click, as it is to be delivered.
    pub left: bool,
    /// The right pad's click, as it is to be delivered.
    pub right: bool,
    /// Both pads were pressed together and one at least still is: no click, and the thumbs are
    /// neither pointing nor scrolling.
    pub together: bool,
    /// Back or forward, on the one frame the thumbs swept far enough.
    pub sweep: Option<Sweep>,
    /// While the thumbs pinch, how far: the scale since both pads went down, 1 at the start.
    pub pinch: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum Press {
    /// Not pressed, or pressed as half of a gesture.
    #[default]
    Up,
    /// Pressed at this instant with the other pad up, and waiting for it.
    Waiting(Instant),
    /// Delivered as a click, until it lets go.
    Down,
    /// Let go while still waiting: delivered pressed for this frame, and released the next.
    Tapped,
    /// Pressed before the pads were handed back -- the thumb that chose a menu's row, still down
    /// as the menu closes -- and nothing until it lets go, or it clicks whatever was behind.
    Spent,
}

impl Press {
    fn delivered(self) -> bool {
        matches!(self, Press::Down | Press::Tapped)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Meaning {
    Undecided,
    /// Sweeping, and whether back or forward has been said yet.
    Sweep { said: bool },
    Pinch,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Together {
    /// Where each thumb was when both pads were down, left then right: the gesture is measured
    /// from there rather than frame to frame, so a slow pinch counts as much as a quick one.
    from: [(f32, f32); 2],
    meaning: Meaning,
    /// A pad has let go: the gesture is over, though the other is still pressed.
    over: bool,
}

impl Together {
    fn follow(&mut self, left: &Pad, right: &Pad) -> (Option<Sweep>, Option<f32>) {
        let (left_moved, right_moved) = (left.x - self.from[0].0, right.x - self.from[1].0);
        // Where the midpoint between the thumbs went, and how much further apart they are.
        let sweep = (left_moved + right_moved) * 0.5;
        let spread = right_moved - left_moved;
        if self.meaning == Meaning::Undecided {
            if spread.abs() >= DECIDED_AFTER {
                self.meaning = Meaning::Pinch;
            } else if sweep.abs() >= DECIDED_AFTER {
                self.meaning = Meaning::Sweep { said: false };
            }
        }
        match self.meaning {
            Meaning::Undecided => (None, None),
            Meaning::Sweep { said: false } if sweep.abs() >= SWEEP => {
                self.meaning = Meaning::Sweep { said: true };
                (Some(if sweep > 0.0 { Sweep::Back } else { Sweep::Forward }), None)
            }
            Meaning::Sweep { .. } => (None, None),
            Meaning::Pinch => (None, Some((spread * PINCH_RATE).exp())),
        }
    }
}

/// The pads' clicks across frames: see the module's documentation.
#[derive(Debug, Clone, Copy, Default)]
pub struct PadClicks {
    left: Press,
    right: Press,
    together: Option<Together>,
}

impl PadClicks {
    /// Fold in this frame: each pad as read, and whether it is pressed.
    ///
    /// `may_gesture` false lets every press straight through as a click, without waiting -- for
    /// typing, where both thumbs come down on keys together and each is a key. A gesture already
    /// under way runs on until both pads let go.
    pub fn update(
        &mut self,
        left: &Pad,
        left_pressed: bool,
        right: &Pad,
        right_pressed: bool,
        may_gesture: bool,
        now: Instant,
    ) -> Pressed {
        // A tap let through last frame is released now.
        for press in [&mut self.left, &mut self.right] {
            if *press == Press::Tapped {
                *press = Press::Up;
            }
        }

        if let Some(together) = self.together.as_mut() {
            if !left_pressed && !right_pressed {
                self.together = None;
                return Pressed::default();
            }
            let mut pressed = Pressed { together: true, ..Pressed::default() };
            if !(left_pressed && right_pressed) {
                together.over = true;
            }
            // A thumb off its pad has no position to measure; the gesture waits for it.
            if !together.over && left.touched && right.touched {
                (pressed.sweep, pressed.pinch) = together.follow(left, right);
            }
            return pressed;
        }

        if may_gesture
            && left_pressed
            && right_pressed
            && !matches!(self.left, Press::Down | Press::Tapped | Press::Spent)
            && !matches!(self.right, Press::Down | Press::Tapped | Press::Spent)
        {
            self.left = Press::Up;
            self.right = Press::Up;
            self.together = Some(Together {
                from: [(left.x, left.y), (right.x, right.y)],
                meaning: Meaning::Undecided,
                over: false,
            });
            return Pressed { together: true, ..Pressed::default() };
        }

        let (left_was, right_was) = (self.left, self.right);
        self.left = step(left_was, left_pressed, right_was.delivered() || !may_gesture, now);
        self.right = step(right_was, right_pressed, left_was.delivered() || !may_gesture, now);
        Pressed {
            left: self.left.delivered(),
            right: self.right.delivered(),
            ..Pressed::default()
        }
    }

    /// Drop every press, as when the pads are taken away (a menu opening): what was let through
    /// is released by whoever takes them, and what is still down when they come back is spent.
    pub fn forget(&mut self) {
        *self = Self { left: Press::Spent, right: Press::Spent, together: None };
    }
}

/// One pad's press, a frame on. `at_once` is a press that cannot be half of a gesture -- the
/// other pad is already a click, or gestures are off -- and so waits for nothing.
fn step(press: Press, pressed: bool, at_once: bool, now: Instant) -> Press {
    match (press, pressed) {
        (Press::Spent, true) => Press::Spent,
        (Press::Up | Press::Tapped, true) if at_once => Press::Down,
        (Press::Up | Press::Tapped, true) => Press::Waiting(now),
        (Press::Waiting(_), true) if at_once => Press::Down,
        (Press::Waiting(since), true) if now.duration_since(since) >= TOGETHER_WITHIN => Press::Down,
        (Press::Waiting(since), true) => Press::Waiting(since),
        (Press::Waiting(_), false) => Press::Tapped,
        (Press::Down, true) => Press::Down,
        (Press::Up | Press::Down | Press::Tapped | Press::Spent, false) => Press::Up,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32) -> Pad {
        Pad { x, y: 0.0, touched: true, clicked: false, pressure: 12_000 }
    }

    struct Run {
        clicks: PadClicks,
        start: Instant,
    }

    impl Run {
        fn new() -> Self {
            Self { clicks: PadClicks::default(), start: Instant::now() }
        }

        /// Both pads at these places and pressed or not, `ms` into the run.
        fn frame(&mut self, ms: u64, left: (f32, bool), right: (f32, bool)) -> Pressed {
            let now = self.start + Duration::from_millis(ms);
            self.clicks.update(&at(left.0), left.1, &at(right.0), right.1, true, now)
        }
    }

    #[test]
    fn one_pad_pressed_is_its_click_late_by_the_wait() {
        let mut run = Run::new();
        assert!(!run.frame(0, (0.0, false), (0.0, true)).right);
        assert!(!run.frame(42, (0.0, false), (0.0, true)).right, "still waiting for the other pad");
        let held = run.frame(84, (0.0, false), (0.0, true));
        assert!(held.right && !held.left && !held.together);
        assert!(run.frame(300, (0.0, false), (0.0, true)).right);
        assert!(!run.frame(314, (0.0, false), (0.0, false)).right);
    }

    #[test]
    fn a_tap_shorter_than_the_wait_is_a_whole_click() {
        let mut run = Run::new();
        run.frame(0, (0.0, true), (0.0, false));
        let tapped = run.frame(28, (0.0, false), (0.0, false));
        assert!(tapped.left, "let go before the wait was over: pressed now");
        assert!(!run.frame(42, (0.0, false), (0.0, false)).left, "and released the next frame");
    }

    #[test]
    fn both_pressed_in_either_order_is_no_click_at_all() {
        for left_first in [true, false] {
            let mut run = Run::new();
            let first = if left_first { ((0.0, true), (0.0, false)) } else { ((0.0, false), (0.0, true)) };
            run.frame(0, first.0, first.1);
            for ms in [56, 200, 600] {
                let p = run.frame(ms, (0.0, true), (0.0, true));
                assert!(p.together && !p.left && !p.right, "{ms} ms: {p:?}");
            }
            // One let go: still nothing, until the other is too.
            let p = run.frame(620, (0.0, false), (0.0, true));
            assert!(p.together && !p.left && !p.right);
            assert_eq!(run.frame(640, (0.0, false), (0.0, false)), Pressed::default());
        }
    }

    #[test]
    fn the_other_pad_after_the_wait_is_a_second_click_at_once() {
        // Holding one button and pressing the other, as a mouse can.
        let mut run = Run::new();
        run.frame(0, (0.0, false), (0.0, true));
        assert!(run.frame(84, (0.0, false), (0.0, true)).right);
        let both = run.frame(140, (0.0, true), (0.0, true));
        assert!(both.left && both.right && !both.together);
    }

    #[test]
    fn typing_never_waits() {
        // Both thumbs come down on keys together, and each is a key.
        let mut clicks = PadClicks::default();
        let p = clicks.update(&at(0.0), true, &at(0.0), true, false, Instant::now());
        assert!(p.left && p.right && !p.together);
    }

    #[test]
    fn sweeping_right_goes_back_once() {
        let mut run = Run::new();
        run.frame(0, (0.0, true), (0.0, true));
        let mut said = Vec::new();
        for (i, x) in [0.1, 0.2, 0.3, 0.4, 0.5, 0.7].into_iter().enumerate() {
            let p = run.frame(14 * (i as u64 + 1), (x, true), (x, true));
            assert!(p.pinch.is_none(), "a sweep is no pinch: {p:?}");
            said.extend(p.sweep);
        }
        assert_eq!(said, [Sweep::Back]);
    }

    #[test]
    fn sweeping_left_goes_forward() {
        let mut run = Run::new();
        run.frame(0, (0.2, true), (0.2, true));
        let p = run.frame(14, (0.0, true), (0.0, true));
        assert_eq!(p.sweep, None, "decided as a sweep, not yet far enough");
        assert_eq!(run.frame(28, (-0.2, true), (-0.2, true)).sweep, Some(Sweep::Forward));
    }

    #[test]
    fn spreading_zooms_in_and_closing_out() {
        let mut run = Run::new();
        run.frame(0, (0.0, true), (0.0, true));
        let spread = run.frame(14, (-0.3, true), (0.3, true)).pinch.expect("a pinch");
        assert!(spread > 1.0, "{spread}");
        assert!(run.frame(28, (-0.3, true), (0.3, true)).pinch.is_some(), "held: still pinching");
        let closed = run.frame(42, (0.2, true), (-0.2, true)).pinch.expect("a pinch");
        assert!(closed < 1.0, "{closed}");
        // Spread back to where it began: exactly the size it began.
        let back = run.frame(56, (0.0, true), (0.0, true)).pinch.unwrap();
        assert!((back - 1.0).abs() < 1e-6, "{back}");
        // A pad let go ends the pinch; the other is held, and still no click.
        let p = run.frame(70, (0.0, false), (0.3, true));
        assert!(p.pinch.is_none() && p.together && !p.right);
    }

    #[test]
    fn one_thumb_moving_away_from_the_other_is_a_pinch() {
        let mut run = Run::new();
        run.frame(0, (0.0, true), (0.0, true));
        let p = run.frame(14, (0.0, true), (0.3, true));
        assert!(p.pinch.is_some_and(|s| s > 1.0) && p.sweep.is_none(), "{p:?}");
    }

    #[test]
    fn thumbs_shifting_as_the_pads_go_down_are_no_gesture() {
        let mut run = Run::new();
        run.frame(0, (0.0, true), (0.0, true));
        let p = run.frame(14, (0.05, true), (-0.04, true));
        assert!(p.together && p.sweep.is_none() && p.pinch.is_none(), "{p:?}");
    }

    #[test]
    fn a_press_still_down_when_the_pads_come_back_is_no_click() {
        let mut run = Run::new();
        run.frame(0, (0.0, false), (0.0, true));
        // A menu took the pads, and its row was chosen with this press.
        run.clicks.forget();
        for ms in [14, 100, 200] {
            let p = run.frame(ms, (0.0, true), (0.0, true));
            assert!(!p.right && !p.left && !p.together, "{ms} ms: {p:?}");
        }
        run.frame(214, (0.0, false), (0.0, false));
        run.frame(228, (0.0, false), (0.0, true));
        assert!(run.frame(320, (0.0, false), (0.0, true)).right, "a new press is a click again");
    }

    #[test]
    fn after_a_gesture_a_press_is_a_click_again() {
        let mut run = Run::new();
        run.frame(0, (0.0, true), (0.0, true));
        run.frame(14, (0.5, true), (0.5, true));
        run.frame(28, (0.0, false), (0.0, false));
        run.frame(42, (0.0, true), (0.0, false));
        assert!(run.frame(130, (0.0, true), (0.0, false)).left);
    }
}
