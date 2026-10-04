//! Putting the sidecar panel to sleep, and waking it with a touch.
//!
//! With the glasses on, the Deck's own screen is a sidecar nobody is looking at for most of a
//! session, and a lit LCD and backlight are the largest thing the machine can switch off
//! without losing anything. This decides *when*; `backend_drm` does the switching, by
//! blanking the panel's CRTC (DPMS off), which takes the panel and its backlight down
//! together. The touch controller is a separate device that is read directly, so it keeps
//! reporting while the screen is dark -- that is what makes "touch to wake" possible at all.
//!
//! ## The power button
//!
//!   * a tap puts the panel to sleep, or wakes it if it is already asleep;
//!   * holding it for [`HOLD_TO_QUIT`] ends the session, the way the sidecar's exit button does.
//!
//! "Held" has to be measured between the press and the release, which only one device on the
//! Deck reports honestly. The embedded controller's keyboard (`AT Translated Set 2 keyboard`)
//! sends the key down when the button goes down and the key up when it comes up. The two ACPI
//! devices called `Power Button` send both in the same instant -- SteamOS's own power daemon
//! ignores them for the same reason -- so `desk` does not pass their presses on, and this
//! module only ever sees the honest one.
//!
//! ## Waking
//!
//! Any finger waking the panel would press whatever is under it as the screen lights, so the
//! touch that wakes it is *swallowed*, together with everything the same finger does until it
//! lifts. Touches in the first moments after going to sleep do not wake it either: the hand
//! that pressed the button is usually still moving across the screen.

use std::time::{Duration, Instant};

use spatiand_input::TouchEvent;

/// How long the power button is held to end the session.
///
/// Long enough that a firm tap cannot reach it, short enough that nobody holding it thinks
/// nothing is happening. The screen is dark for most of the cases that matter, so there is no
/// progress bar to fill; the hold is simply this long.
pub const HOLD_TO_QUIT: Duration = Duration::from_millis(1500);

/// How long after going dark a touch is ignored rather than taken as a wish to wake.
pub const WAKE_GRACE: Duration = Duration::from_millis(600);

#[derive(Debug)]
pub struct ScreenSleep {
    /// What the wearer has asked for. The panel is brought into line with this by the caller,
    /// which may have to wait for a frame still in flight.
    want_off: bool,
    went_off: Option<Instant>,
    /// When the power button went down, while it is down.
    power_down: Option<Instant>,
    /// The hold already ended the session, so its release must not also toggle the screen.
    quit_sent: bool,
    /// Fingers currently on the glass, by slot -- tracked while asleep too, because a finger
    /// that came down in the dark is still down when the panel lights.
    live: Vec<usize>,
    /// Dropping touches until every finger has lifted.
    swallowing: bool,
}

impl ScreenSleep {
    pub fn new() -> Self {
        Self {
            want_off: false,
            went_off: None,
            power_down: None,
            quit_sent: false,
            live: Vec::new(),
            swallowing: false,
        }
    }

    pub fn want_off(&self) -> bool {
        self.want_off
    }

    /// The panel could not be blanked, so it stays lit and this stops saying it is asleep.
    pub fn cancel(&mut self) {
        self.wake();
    }

    /// The power button went down or came up.
    pub fn power(&mut self, pressed: bool, now: Instant) {
        if pressed {
            // A repeat while held is not a second press.
            if self.power_down.is_none() {
                self.power_down = Some(now);
                self.quit_sent = false;
            }
            return;
        }
        let Some(since) = self.power_down.take() else {
            return;
        };
        if self.quit_sent || now.duration_since(since) >= HOLD_TO_QUIT {
            return;
        }
        if self.want_off {
            self.wake();
        } else {
            self.sleep(now);
        }
    }

    /// Once a frame. True exactly once per hold that has reached [`HOLD_TO_QUIT`].
    ///
    /// On the clock rather than on the release: the button is held down when this fires, and
    /// the session should end while it is, not when the wearer lets go.
    pub fn held_to_quit(&mut self, now: Instant) -> bool {
        match self.power_down {
            Some(since) if !self.quit_sent && now.duration_since(since) >= HOLD_TO_QUIT => {
                self.quit_sent = true;
                true
            }
            _ => false,
        }
    }

    /// Filter what the touchscreen reported. Returns what the sidecar should see, which is
    /// nothing at all while the panel is asleep or while a waking finger is still down.
    pub fn touches(&mut self, events: Vec<TouchEvent>, now: Instant) -> Vec<TouchEvent> {
        let mut out = Vec::new();
        for event in events {
            match event {
                TouchEvent::Down(c) => {
                    if !self.live.contains(&c.slot) {
                        self.live.push(c.slot);
                    }
                    if self.want_off && self.grace_over(now) {
                        self.wake();
                    }
                }
                TouchEvent::Motion(_) => {}
                TouchEvent::Up { slot } => self.live.retain(|s| *s != slot),
            }
            if self.want_off || self.swallowing {
                continue;
            }
            out.push(event);
        }
        if self.swallowing && self.live.is_empty() {
            self.swallowing = false;
        }
        out
    }

    /// Fingers the sidecar was told about that it must now be told have lifted, for the moment
    /// the panel goes dark: a finger resting on the exit button would otherwise finish its
    /// hold in the dark and end the session.
    pub fn lifts(&self) -> Vec<TouchEvent> {
        self.live
            .iter()
            .map(|slot| TouchEvent::Up { slot: *slot })
            .collect()
    }

    fn grace_over(&self, now: Instant) -> bool {
        self.went_off
            .map_or(true, |at| now.duration_since(at) >= WAKE_GRACE)
    }

    fn sleep(&mut self, now: Instant) {
        self.want_off = true;
        self.went_off = Some(now);
    }

    fn wake(&mut self) {
        self.want_off = false;
        self.went_off = None;
        // Whatever is on the glass now woke it, and is not a press on the panel.
        self.swallowing = !self.live.is_empty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spatiand_input::Contact;

    fn down(slot: usize) -> TouchEvent {
        TouchEvent::Down(Contact {
            slot,
            id: slot as i32,
            x: 0.5,
            y: 0.5,
        })
    }

    fn motion(slot: usize) -> TouchEvent {
        TouchEvent::Motion(Contact {
            slot,
            id: slot as i32,
            x: 0.6,
            y: 0.6,
        })
    }

    fn up(slot: usize) -> TouchEvent {
        TouchEvent::Up { slot }
    }

    fn tap(s: &mut ScreenSleep, at: Instant, held: Duration) {
        s.power(true, at);
        s.power(false, at + held);
    }

    #[test]
    fn a_tap_puts_the_panel_to_sleep_and_another_wakes_it() {
        let t = Instant::now();
        let mut s = ScreenSleep::new();
        tap(&mut s, t, Duration::from_millis(120));
        assert!(s.want_off());
        tap(&mut s, t + Duration::from_secs(5), Duration::from_millis(120));
        assert!(!s.want_off());
    }

    #[test]
    fn a_hold_ends_the_session_and_does_not_also_sleep() {
        let t = Instant::now();
        let mut s = ScreenSleep::new();
        s.power(true, t);
        assert!(!s.held_to_quit(t + Duration::from_millis(1000)));
        assert!(s.held_to_quit(t + HOLD_TO_QUIT));
        assert!(!s.held_to_quit(t + HOLD_TO_QUIT * 2), "once per hold");
        s.power(false, t + HOLD_TO_QUIT * 2);
        assert!(!s.want_off(), "letting go after a quit is not a tap");
    }

    #[test]
    fn a_hold_that_was_never_noticed_still_is_not_a_tap() {
        // The frame loop stalled across the whole hold, so `held_to_quit` never ran. The
        // release still has to know how long it was.
        let t = Instant::now();
        let mut s = ScreenSleep::new();
        tap(&mut s, t, HOLD_TO_QUIT + Duration::from_millis(1));
        assert!(!s.want_off());
    }

    #[test]
    fn a_release_with_no_press_does_nothing() {
        let mut s = ScreenSleep::new();
        s.power(false, Instant::now());
        assert!(!s.want_off());
    }

    #[test]
    fn a_touch_while_awake_reaches_the_sidecar() {
        let t = Instant::now();
        let mut s = ScreenSleep::new();
        let seen = s.touches(vec![down(0), motion(0), up(0)], t);
        assert_eq!(seen.len(), 3);
    }

    #[test]
    fn a_touch_wakes_the_panel_and_is_not_a_press_on_it() {
        let t = Instant::now();
        let mut s = ScreenSleep::new();
        tap(&mut s, t, Duration::from_millis(100));
        let later = t + Duration::from_secs(3);
        assert!(s.touches(vec![down(0)], later).is_empty());
        assert!(!s.want_off(), "the touch woke it");
        // The same finger carries on, and still reaches nothing...
        assert!(s.touches(vec![motion(0)], later).is_empty());
        assert!(s.touches(vec![up(0)], later).is_empty());
        // ...but the next touch does.
        assert_eq!(s.touches(vec![down(0)], later).len(), 1);
    }

    #[test]
    fn touches_just_after_going_dark_do_not_wake_it() {
        let t = Instant::now();
        let mut s = ScreenSleep::new();
        tap(&mut s, t, Duration::from_millis(100));
        let soon = t + Duration::from_millis(100) + WAKE_GRACE / 2;
        s.touches(vec![down(0), up(0)], soon);
        assert!(s.want_off());
    }

    #[test]
    fn a_finger_that_came_down_in_the_grace_period_does_not_wake_it_later_by_staying() {
        let t = Instant::now();
        let mut s = ScreenSleep::new();
        tap(&mut s, t, Duration::from_millis(100));
        s.touches(vec![down(0)], t + Duration::from_millis(200));
        // Motion after the grace is not a new touch.
        s.touches(vec![motion(0)], t + Duration::from_secs(2));
        assert!(s.want_off());
    }

    #[test]
    fn a_finger_down_when_the_panel_goes_dark_is_lifted_for_the_sidecar() {
        let t = Instant::now();
        let mut s = ScreenSleep::new();
        s.touches(vec![down(2)], t);
        tap(&mut s, t, Duration::from_millis(100));
        assert_eq!(s.lifts(), vec![up(2)]);
    }

    #[test]
    fn a_finger_down_before_the_panel_woke_stays_swallowed_until_it_lifts() {
        let t = Instant::now();
        let mut s = ScreenSleep::new();
        s.touches(vec![down(1)], t);
        tap(&mut s, t, Duration::from_millis(100));
        // Woken by the power button with a finger already on the glass.
        tap(&mut s, t + Duration::from_secs(3), Duration::from_millis(100));
        assert!(!s.want_off());
        assert!(s.touches(vec![motion(1)], t + Duration::from_secs(4)).is_empty());
        assert!(s.touches(vec![up(1)], t + Duration::from_secs(4)).is_empty());
        assert_eq!(s.touches(vec![down(1)], t + Duration::from_secs(5)).len(), 1);
    }
}
