//! A phone's touch screen as the Deck's left pad and clicks.
//!
//! On the Beam Pro the phone is the controller: the big touch area under the wearer's thumb is
//! the left trackpad, and the phone's orientation aims the pointer as the right one would (see
//! `spatiand_render::ray::pad_for_direction`). What a Deck says with pad clicks and triggers, a
//! touch screen says with gestures, and this turns one into the other so that everything above
//! it -- the pointer, the menus, window drags -- reads a Deck:
//!
//! | on the phone | as a Deck |
//! |---|---|
//! | one finger moving | the left pad moving: scroll |
//! | tap | right pad click: the left mouse button, a menu's accept |
//! | touch and hold | right pad held until the finger lifts: drag a title bar, an edge, or anything in a window -- select text, turn a camera; sliding the finger meanwhile sets a dragged window's distance, as the left thumb does on the Deck |
//! | two fingers tapped | left pad click: the right mouse button |
//! | two fingers apart or together | zoom: the ratio is handed out, for the window being pointed at |
//!
//! Coordinates in are the touch area's, 0..1 from its top-left, as Android reports them divided
//! by its size. Time is milliseconds on any steady clock.

use crate::report::Pad;

/// How long a touch may last and still be a tap.
pub const TAP_MS: u64 = 250;
/// How far a finger may wander, as a fraction of the area's width, and still be a tap or a
/// long press rather than a scroll.
pub const SLOP: f32 = 0.03;
/// How long a finger must rest, unmoved, to be a long press.
pub const LONG_PRESS_MS: u64 = 550;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Down,
    Move,
    Up,
    /// The system took the touch away, as it does for a gesture of its own.
    Cancel,
}

#[derive(Debug, Clone, Copy)]
struct Contact {
    id: i32,
    x: f32,
    y: f32,
    start: (f32, f32),
    since_ms: u64,
    moved: bool,
}

/// What the touch area says this frame, in the Deck's terms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// The left pad: touched while one finger scrolls.
    pub left_pad: Pad,
    /// The right pad's click, as a level: a tap is one frame down and one frame up.
    pub right_click: bool,
    /// The left pad's click, as a level, for a long press.
    pub left_click: bool,
    /// How much two fingers have spread since the last frame; 1 when they have not.
    pub pinch: f32,
    /// A long press whose finger is still down: what holds a window taken by one.
    pub long_held: bool,
}

#[derive(Debug, Default)]
pub struct PhoneTouch {
    contacts: Vec<Contact>,
    /// A tap waiting to be delivered: down this frame, up the next.
    tap_pending: u32,
    /// A tap delivered as down, owed its up.
    tap_down: bool,
    /// The current touch began as a held click.
    holding: bool,
    /// The current touch has been taken as a long press already.
    long_pressed: bool,
    long_pending: bool,
    long_down: bool,
    /// Two fingers' spread when last looked at.
    spread: Option<f32>,
    pinch: f32,
    /// The current touch has been two fingers at some point, so it is no tap or scroll.
    pinching: bool,
    /// When the second finger of the current touch came down, and whether any finger has
    /// travelled since: a quick, still pair is a two-finger tap.
    pair_since: Option<u64>,
    pair_moved: bool,
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

impl PhoneTouch {
    pub fn new() -> PhoneTouch {
        PhoneTouch {
            pinch: 1.0,
            ..Default::default()
        }
    }

    /// One touch event.
    pub fn touch(&mut self, phase: Phase, id: i32, x: f32, y: f32, time_ms: u64) {
        match phase {
            Phase::Down => {
                if self.contacts.is_empty() {
                    self.pinching = false;
                    self.long_pressed = false;
                    // No hold begins here. A tap and then a hold used to, the way a laptop's pad
                    // drags -- but the tap had already clicked, so the application was told a
                    // click and then a press, which it took for a double click. A hold is a
                    // finger resting instead, and comes long enough after any tap to be a
                    // click of its own.
                    self.holding = false;
                }
                self.contacts.retain(|c| c.id != id);
                self.contacts.push(Contact {
                    id,
                    x,
                    y,
                    start: (x, y),
                    since_ms: time_ms,
                    moved: false,
                });
                if self.contacts.len() >= 2 {
                    if !self.pinching {
                        self.pair_since = Some(time_ms);
                        self.pair_moved = false;
                    }
                    self.pinching = true;
                    self.holding = false;
                    self.spread = self.spread_now();
                }
            }
            Phase::Move => {
                if let Some(c) = self.contacts.iter_mut().find(|c| c.id == id) {
                    c.x = x;
                    c.y = y;
                    if distance((x, y), c.start) > SLOP {
                        c.moved = true;
                        if self.pinching {
                            self.pair_moved = true;
                        }
                    }
                }
                if self.contacts.len() >= 2 {
                    if let (Some(before), Some(now)) = (self.spread, self.spread_now()) {
                        if before > 1e-3 {
                            self.pinch *= now / before;
                        }
                        self.spread = Some(now);
                    }
                }
            }
            Phase::Up | Phase::Cancel => {
                let ended = self.contacts.iter().position(|c| c.id == id).map(|i| self.contacts.remove(i));
                if self.contacts.len() < 2 {
                    self.spread = None;
                }
                let Some(c) = ended else { return };
                if self.contacts.is_empty() {
                    // Two fingers down and up together, without travelling: the right button.
                    if phase == Phase::Up
                        && !self.pair_moved
                        && self.pair_since.is_some_and(|at| time_ms.saturating_sub(at) <= TAP_MS * 2)
                    {
                        self.long_pending = true;
                    }
                    self.pair_since = None;
                    let quick = time_ms.saturating_sub(c.since_ms) <= TAP_MS;
                    if phase == Phase::Up
                        && quick
                        && !c.moved
                        && !self.pinching
                        && !self.holding
                        && !self.long_pressed
                    {
                        self.tap_pending += 1;
                    }
                    self.holding = false;
                }
            }
        }
    }

    fn spread_now(&self) -> Option<f32> {
        let a = self.contacts.first()?;
        let b = self.contacts.get(1)?;
        Some(distance((a.x, a.y), (b.x, b.y)))
    }

    /// The pads and clicks as of now. Call once a frame.
    pub fn frame(&mut self, now_ms: u64) -> Frame {
        // A finger resting long enough without moving holds the button, as a tap and a hold
        // does: the one hold a thumb finds without being taught.
        if let [c] = self.contacts.as_slice() {
            if !c.moved
                && !self.holding
                && !self.long_pressed
                && !self.pinching
                && now_ms.saturating_sub(c.since_ms) >= LONG_PRESS_MS
            {
                self.long_pressed = true;
                self.holding = true;
            }
        }

        let right_click = if self.holding && !self.contacts.is_empty() {
            true
        } else if self.tap_down {
            // A tap's up, one frame after its down.
            self.tap_down = false;
            false
        } else if self.tap_pending > 0 {
            self.tap_pending -= 1;
            self.tap_down = true;
            true
        } else {
            false
        };
        let left_click = if self.long_down {
            self.long_down = false;
            false
        } else if self.long_pending {
            self.long_pending = false;
            self.long_down = true;
            true
        } else {
            false
        };

        // One finger is the pad; two are a pinch and touch no pad, so zooming never scrolls.
        let left_pad = match self.contacts.as_slice() {
            [c] if !self.pinching => Pad {
                x: c.x * 2.0 - 1.0,
                y: 1.0 - c.y * 2.0,
                touched: true,
                clicked: false,
                // Nothing measures pressure here. Anything steady will do for what reads it,
                // which looks at change rather than at the value.
                pressure: 8000,
            },
            _ => Pad::default(),
        };
        let pinch = std::mem::replace(&mut self.pinch, 1.0);
        let long_held = self.long_pressed && self.holding && self.contacts.len() == 1;
        Frame {
            left_pad,
            right_click,
            left_click,
            pinch,
            long_held,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_press_holds_the_click_until_the_finger_lifts() {
        let mut t = PhoneTouch::new();
        t.touch(Phase::Down, 0, 0.5, 0.5, 0);
        assert!(!t.frame(300).right_click);
        let f = t.frame(LONG_PRESS_MS + 10);
        assert!(f.right_click && f.long_held && !f.left_click);
        // Sliding afterwards -- dragging, or a dragged window's distance -- keeps it held.
        t.touch(Phase::Move, 0, 0.5, 0.8, LONG_PRESS_MS + 50);
        let f = t.frame(LONG_PRESS_MS + 60);
        assert!(f.right_click && f.long_held);
        t.touch(Phase::Up, 0, 0.5, 0.8, LONG_PRESS_MS + 100);
        let f = t.frame(LONG_PRESS_MS + 110);
        assert!(!f.right_click && !f.long_held);
        assert!(!t.frame(LONG_PRESS_MS + 120).right_click, "and no tap on the way out");
    }

    #[test]
    fn two_fingers_tapped_are_the_right_button() {
        let mut t = PhoneTouch::new();
        t.touch(Phase::Down, 0, 0.4, 0.5, 0);
        t.touch(Phase::Down, 1, 0.6, 0.5, 20);
        t.touch(Phase::Up, 1, 0.6, 0.5, 120);
        t.touch(Phase::Up, 0, 0.4, 0.5, 130);
        let f = t.frame(140);
        assert!(f.left_click && !f.right_click);
        assert!(!t.frame(150).left_click);
    }

    #[test]
    fn two_fingers_pinching_are_not_a_tap() {
        let mut t = PhoneTouch::new();
        t.touch(Phase::Down, 0, 0.4, 0.5, 0);
        t.touch(Phase::Down, 1, 0.6, 0.5, 20);
        t.touch(Phase::Move, 1, 0.8, 0.5, 80);
        t.touch(Phase::Up, 1, 0.8, 0.5, 120);
        t.touch(Phase::Up, 0, 0.4, 0.5, 130);
        assert!(!t.frame(140).left_click);
    }

    #[test]
    fn a_tap_is_one_frame_of_click_and_then_one_without() {
        let mut t = PhoneTouch::new();
        t.touch(Phase::Down, 0, 0.5, 0.5, 0);
        t.touch(Phase::Up, 0, 0.5, 0.5, 100);
        assert!(t.frame(110).right_click, "the down");
        assert!(!t.frame(120).right_click, "the up");
        assert!(!t.frame(130).right_click);
    }

    #[test]
    fn a_finger_that_travels_scrolls_and_does_not_click() {
        let mut t = PhoneTouch::new();
        t.touch(Phase::Down, 0, 0.5, 0.5, 0);
        let f = t.frame(10);
        assert!(f.left_pad.touched);
        t.touch(Phase::Move, 0, 0.5, 0.7, 60);
        assert!((t.frame(70).left_pad.y - (1.0 - 1.4)).abs() < 1e-6);
        t.touch(Phase::Up, 0, 0.5, 0.7, 120);
        assert!(!t.frame(130).right_click);
    }

    #[test]
    fn a_tap_and_then_a_hold_is_no_double_click() {
        let mut t = PhoneTouch::new();
        t.touch(Phase::Down, 0, 0.5, 0.5, 0);
        t.touch(Phase::Up, 0, 0.5, 0.5, 80);
        assert!(t.frame(90).right_click, "the tap");
        assert!(!t.frame(100).right_click);
        t.touch(Phase::Down, 0, 0.51, 0.5, 200);
        assert!(!t.frame(210).right_click, "not pressed again at once");
        assert!(!t.frame(500).right_click);
        assert!(t.frame(200 + LONG_PRESS_MS + 10).right_click, "held once it has rested");
    }

    #[test]
    fn a_finger_resting_long_enough_is_no_right_click() {
        let mut t = PhoneTouch::new();
        t.touch(Phase::Down, 0, 0.5, 0.5, 0);
        assert!(!t.frame(600).left_click);
        t.touch(Phase::Up, 0, 0.5, 0.5, 700);
        assert!(!t.frame(710).left_click);
    }

    #[test]
    fn two_fingers_spreading_zoom_and_touch_no_pad() {
        let mut t = PhoneTouch::new();
        t.touch(Phase::Down, 0, 0.4, 0.5, 0);
        t.touch(Phase::Down, 1, 0.6, 0.5, 10);
        t.touch(Phase::Move, 1, 0.8, 0.5, 50);
        let f = t.frame(60);
        assert!((f.pinch - 2.0).abs() < 1e-5, "{}", f.pinch);
        assert!(!f.left_pad.touched);
        assert_eq!(t.frame(70).pinch, 1.0, "handed out once");
        t.touch(Phase::Up, 1, 0.8, 0.5, 90);
        t.touch(Phase::Up, 0, 0.4, 0.5, 100);
        assert!(!t.frame(110).right_click, "a pinch is not a tap");
    }
}
