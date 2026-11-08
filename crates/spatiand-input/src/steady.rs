//! A thumb held still, held still.
//!
//! The pads are absolute: where the right thumb is, is where the pointer aims, and how far the
//! left one goes is how far the page goes. So whatever the contact does, the pointer and the page
//! do too, and a thumb held still on a capacitive pad is not still. The sensor's own noise and the
//! thumb's pulse move its reported centre by a few thousandths of a pad, back and forth, all the
//! time. At 19 degrees a pad unit that is a few pixels of the window shaking under a pointer
//! nobody moved, and under a slow scroll, the page going back and forth with every beat.
//!
//! Smoothing over time cannot take that out. A pulse comes about once a second, and a filter slow
//! enough to hide it makes the pointer swim behind the thumb. What sets the wander apart from a
//! hand is its size, not its speed, so this is a band: the position handed on stays where it is
//! until the thumb has gone further than [`MOVING`] from it, and then follows, that far behind.
//! Moving steadily, nothing is lost and nothing is late -- the pointer moves exactly as the thumb
//! does, a hair behind it. What never gets through is a movement back of less than twice the
//! band, which is what a shake is; a hand that means to turn back has gone further than that
//! within a frame or two.
//!
//! Up and down and sideways each have the band to themselves. Held as one, a thumb going sideways
//! carried the up and down of its wobble with it, and the wheel, which scrolls along one of them,
//! ticked back against a swipe that never did.
//!
//! A thumb that has kept inside the band for [`REST_AFTER`] is resting, and the band widens to
//! [`RESTING`]: a pulse moves a resting thumb further than noise moves a moving one. It narrows
//! again over the first of the thumb's next movement rather than at once, so the pointer does not
//! jump to catch up.
//!
//! For the room's pointer and wheel only. A game's layout reads the pads as they are.

use std::time::{Duration, Instant};

use crate::report::Pad;

/// How far a moving thumb may go back before what it moves turns back with it, in pad units
/// (a pad spans −1..1).
///
/// Above the jitter of a thumb on the move -- a frame to the next, the recordings' slow swipes
/// step a median 0.001 and a tenth of the time over 0.003 -- and below anything a hand does on
/// purpose: 0.004 of a pad is about 0.07 mm of thumb, and on the pointer about four pixels.
///
/// Checked on 2026-11-08 against five minutes of the owner in the room (`tools/capture-pad-trace.py
/// --passive`, `tools/analyse-pad-rest.py`): the swings that turn back within 0.02 of a pad fell
/// from 3,040 to 44 on the right pad and from 1,264 to 219 up and down on the left. Those left are
/// around 0.01, larger than a pulse; a band of 0.006 took a quarter more of them, at half as much
/// again of the thumb's travel before a turn is followed.
pub const MOVING: f32 = 0.004;

/// How far a resting thumb may wander before the pointer follows it, in pad units.
///
/// A resting thumb's pulse moves it further than a moving thumb's noise. Twice the moving band:
/// a wander of up to twice this, side to side, is taken out once it has settled. The owner's
/// resting thumbs wandered a median 0.010 of a pad and at most 0.018 (2026-11-08). Leaving rest
/// costs this much of the thumb's travel before anything moves -- at a careful 0.1 of a pad a
/// second, 80 ms; at an ordinary pace, a frame or two.
pub const RESTING: f32 = 0.008;

/// How long a thumb must have kept inside the band to be resting.
///
/// Long enough that the pauses inside a slow swipe, where the thumb's own wobble takes it back
/// inside the band for a frame or several, do not count; short enough that a thumb that has
/// arrived on its target is held there before the wearer has noticed it moving.
pub const REST_AFTER: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy)]
struct Held {
    /// Where the thumb is said to be.
    x: f32,
    y: f32,
    /// How far the thumb may go from there before it is followed.
    band: f32,
    /// When it last had to be followed.
    followed: Instant,
}

/// One pad's held position.
#[derive(Debug, Default)]
pub struct SteadyPad {
    held: Option<Held>,
}

impl SteadyPad {
    /// The pad as the room should read it: its position held while the thumb only wanders, and
    /// everything else -- touched, clicked, pressure -- as it is.
    pub fn update(&mut self, pad: &Pad, now: Instant) -> Pad {
        if !pad.touched {
            self.held = None;
            return *pad;
        }
        let Some(held) = self.held.as_mut() else {
            // A thumb landing is where it lands: there is nothing yet to hold it against.
            self.held = Some(Held {
                x: pad.x,
                y: pad.y,
                band: MOVING,
                followed: now,
            });
            return *pad;
        };
        let band = held.band;
        let moved = follow(&mut held.x, pad.x, band).max(follow(&mut held.y, pad.y, band));
        if moved > 0.0 {
            // Narrowed by as much as it moved: a band wide from resting comes back to a moving
            // thumb's over the first of its travel, which the pointer makes up at most twice
            // the thumb's pace for a frame or so, instead of in one jump.
            held.band = (band - moved).max(MOVING);
            held.followed = now;
        } else if now.duration_since(held.followed) >= REST_AFTER {
            held.band = RESTING;
        }
        Pad {
            x: held.x,
            y: held.y,
            ..*pad
        }
    }
}

/// One axis followed to the band's edge, if the thumb has gone past it; how far that moved it.
fn follow(held: &mut f32, thumb: f32, band: f32) -> f32 {
    let off = thumb - *held;
    let moved = off.abs() - band;
    if moved <= 0.0 {
        return 0.0;
    }
    *held += moved.copysign(off);
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The compositor reads the pads once a frame, at the glasses' 72 Hz.
    const FRAME: Duration = Duration::from_micros(13_889);

    fn at(x: f32, y: f32) -> Pad {
        Pad {
            x,
            y,
            touched: true,
            clicked: false,
            pressure: 0,
        }
    }

    /// A thumb's wander, made up as a pulse and a sensor's noise would make it: the same every
    /// run, so a failure can be looked at again.
    struct Wander {
        seed: u32,
    }

    impl Wander {
        fn noise(&mut self) -> f32 {
            self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        }

        /// Where the thumb is pushed from where it means to be, `frame` frames in: a pulse of
        /// `pulse` either way at 1.2 beats a second, and noise of `noise` either way.
        fn at(&mut self, frame: usize, pulse: f32, noise: f32) -> (f32, f32) {
            let t = frame as f32 * FRAME.as_secs_f32();
            let beat = (t * 1.2 * std::f32::consts::TAU).sin() * pulse;
            (beat * 0.6 + self.noise() * noise, beat + self.noise() * noise)
        }
    }

    /// Drive `frames` frames of a thumb that means to be at `aim(frame)`, and say what came out.
    fn run(frames: usize, mut aim: impl FnMut(usize) -> (f32, f32)) -> Vec<Pad> {
        let mut steady = SteadyPad::default();
        let start = Instant::now();
        (0..frames)
            .map(|i| {
                let (x, y) = aim(i);
                steady.update(&at(x, y), start + FRAME * i as u32)
            })
            .collect()
    }

    #[test]
    fn a_thumb_landing_is_where_it_lands() {
        let mut steady = SteadyPad::default();
        let out = steady.update(&at(0.31, -0.42), Instant::now());
        assert_eq!((out.x, out.y), (0.31, -0.42));
    }

    #[test]
    fn a_resting_thumb_holds_still_through_its_pulse() {
        // The complaint: the thumb held on a button, and the pointer shaking on it with every
        // heartbeat. A pulse of 0.003 either way and noise of 0.0015 on top: wander of nearly a
        // hundredth of a pad, side to side, which is more than the recordings show.
        let mut wander = Wander { seed: 7 };
        let out = run(72 * 5, |i| {
            let (wx, wy) = wander.at(i, 0.003, 0.0015);
            (0.2 + wx, -0.1 + wy)
        });
        // Allowed to settle for the first second; still from then on, to the last bit.
        let settled = &out[72..];
        for pad in settled {
            assert_eq!((pad.x, pad.y), (settled[0].x, settled[0].y), "the pointer moved under a resting thumb");
        }
        // And it settled near where the thumb is, not wherever the first wander left it.
        assert!((settled[0].x - 0.2).abs() < RESTING && (settled[0].y + 0.1).abs() < RESTING);
    }

    #[test]
    fn a_slow_scroll_never_goes_back() {
        // A thumb drawn slowly up the pad, a tenth of a pad a second, with the pulse and the
        // noise on it. Its own reports go back as often as not, frame to frame; what comes out
        // only ever goes the one way, and gets as far as the thumb did, less the band.
        let mut wander = Wander { seed: 11 };
        let frames = 72 * 4;
        let speed = 0.1 * FRAME.as_secs_f32();
        let out = run(frames, |i| {
            let (wx, wy) = wander.at(i, 0.0015, 0.0015);
            (0.3 + wx * 0.2, -0.5 + i as f32 * speed + wy)
        });
        for pair in out.windows(2) {
            assert!(pair[1].y >= pair[0].y, "the scroll went back: {} after {}", pair[1].y, pair[0].y);
        }
        let travelled = out.last().unwrap().y - out[0].y;
        let meant = (frames - 1) as f32 * speed;
        assert!((travelled - meant).abs() < RESTING + 0.003, "went {travelled} of {meant}");
    }

    #[test]
    fn a_steady_movement_is_followed_exactly_and_never_late() {
        // Nothing is smoothed: past the band, each frame moves by exactly what the thumb did.
        let step = 0.01;
        let out = run(40, |i| (-0.5 + i as f32 * step, 0.2));
        for pair in out[2..].windows(2) {
            assert!((pair[1].x - pair[0].x - step).abs() < 1e-5, "moved {} for {step}", pair[1].x - pair[0].x);
        }
        let last = out.last().unwrap();
        assert!((-0.5 + 39.0 * step - last.x - MOVING).abs() < 1e-5, "behind by {}", -0.5 + 39.0 * step - last.x);
    }

    #[test]
    fn a_hand_that_turns_back_is_followed_back_at_once() {
        // Aiming past a button and coming back to it: the turn is taken within twice the band
        // of the thumb's travel back -- a frame at this pace.
        let step = 0.006;
        let out = run(40, |i| {
            let i = i as f32;
            (if i < 20.0 { i * step } else { (40.0 - i) * step }, 0.0)
        });
        let turned = out[20..].windows(2).position(|p| p[1].x < p[0].x).expect("never turned back");
        assert!(turned <= 2, "turned back {turned} frames late");
    }

    #[test]
    fn a_thumb_moving_off_from_rest_starts_smoothly() {
        // Rested long enough for the band to widen, then moved off at a careful pace: the
        // pointer starts a little later than the thumb and catches up without a jump.
        let step = 0.003;
        let out = run(72, |i| if i < 30 { (0.0, 0.0) } else { ((i - 30) as f32 * step, 0.0) });
        for pair in out.windows(2) {
            let moved = pair[1].x - pair[0].x;
            assert!(moved <= 2.0 * step + 1e-5, "jumped by {moved} for a step of {step}");
        }
        let last = out.last().unwrap();
        assert!((41.0 * step - last.x - MOVING).abs() < 1e-5, "still behind by {}", 41.0 * step - last.x);
    }

    #[test]
    fn a_thumb_that_lifts_lands_afresh() {
        let mut steady = SteadyPad::default();
        let now = Instant::now();
        steady.update(&at(0.5, 0.5), now);
        steady.update(&at(0.501, 0.5), now + FRAME);
        let lifted = steady.update(&Pad::default(), now + FRAME * 2);
        assert!(!lifted.touched);
        let out = steady.update(&at(-0.7, 0.1), now + FRAME * 3);
        assert_eq!((out.x, out.y), (-0.7, 0.1));
    }

    #[test]
    fn the_recorded_swipes_scroll_as_they_did() {
        // The wheel's own filter was set from recordings of this pad (`crate::scroll`); in front
        // of it, the band must leave what it decided alone: as far, the same way, the same end.
        use crate::scroll::{PadScroll, Scroll};
        use crate::scroll_trace::*;
        fn replay(frames: &[(f32, f32, bool, u16)], steadied: bool) -> (f32, f32, f32, usize, Scroll) {
            let mut scroll = PadScroll::default();
            let mut steady = SteadyPad::default();
            let start = Instant::now();
            let (mut sx, mut sy, mut fastest, mut turns, mut last_dy) = (0.0, 0.0, 0.0f32, 0, 0.0f32);
            let mut feed = |pad: Pad, i: usize| {
                let pad = if steadied { steady.update(&pad, start + FRAME * i as u32) } else { pad };
                scroll.update(&pad)
            };
            for (i, &(x, y, clicked, pressure)) in frames.iter().enumerate() {
                if let Scroll::By { dx, dy } = feed(Pad { x, y, touched: true, clicked, pressure }, i) {
                    sx += dx.abs();
                    sy += dy.abs();
                    fastest = fastest.max(dx.abs()).max(dy.abs());
                    if dy * last_dy < 0.0 {
                        turns += 1;
                    }
                    if dy != 0.0 {
                        last_dy = dy;
                    }
                }
            }
            (sx, sy, fastest, turns, feed(Pad::default(), frames.len()))
        }
        for (name, frames) in [
            ("contact 2", CONTACT_2),
            ("contact 3", CONTACT_3),
            ("contact 4", CONTACT_4),
            ("beginning sideways", SWIPE_BEGINNING_SIDEWAYS),
            ("on an arc", SWIPE_ON_AN_ARC),
            ("up and down", SWIPE_UP_AND_DOWN),
        ] {
            let (rx, ry, raw_fastest, raw_turns, raw_end) = replay(frames, false);
            let (fx, fy, fastest, turns, end) = replay(frames, true);
            assert_eq!(end, raw_end, "{name}: ended differently");
            // Nothing faster: the lift's lurch, which the wheel keeps from the page, kept from it
            // still.
            assert!(fastest <= raw_fastest + 1e-3, "{name}: a step of {fastest}, {raw_fastest} without");
            // Along the same axis, never further than the thumb went, and nearly as far as
            // without the band. Not exactly as far: a step the band shortens can come in under
            // the wheel's speed limit where the whole step did not -- a thumb's first after it
            // lands or turns, which is the hand, and was being lost (contact 2's landing, 0.027
            // of its 1.40; up and down's turns, 0.06 of 0.83).
            assert_eq!((fx == 0.0, fy == 0.0), (rx == 0.0, ry == 0.0), "{name}: scrolled along another axis");
            let thumb: f32 = frames.windows(2).map(|w| (w[1].0 - w[0].0).abs() + (w[1].1 - w[0].1).abs()).sum();
            assert!(fx + fy <= thumb, "{name}: scrolled {} of the thumb's {thumb}", fx + fy);
            assert!(fx + fy >= (rx + ry) * 0.9, "{name}: scrolled {} of the {} without", fx + fy, rx + ry);
            assert!(turns <= raw_turns, "{name}: turned {turns} times, {raw_turns} without");
        }
    }

    #[test]
    fn a_click_and_the_pressure_come_through_as_they_are() {
        let mut steady = SteadyPad::default();
        let now = Instant::now();
        steady.update(&at(0.1, 0.1), now);
        let pressed = Pad {
            clicked: true,
            pressure: 9000,
            ..at(0.1005, 0.1)
        };
        let out = steady.update(&pressed, now + FRAME);
        assert!(out.clicked && out.touched);
        assert_eq!(out.pressure, 9000);
        assert_eq!((out.x, out.y), (0.1, 0.1));
    }
}
