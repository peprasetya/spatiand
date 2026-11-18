//! Controls that hide themselves over what they control: SpatiReel's player and SpatiLoupe's
//! picture and object views.

use std::time::{Duration, Instant};

/// How long controls stay after the wearer last did something.
pub const CONTROLS_SHOWN: Duration = Duration::from_secs(3);
/// How long after a tap has put the controls away the hands that made it are not taken to want
/// them back: the room says `hands` for the tap's press and release a frame or a few later.
const PUT_AWAY_DEAF: Duration = Duration::from_millis(600);
/// How long the controls must have been on show for a tap on the picture to put them away.
const TAP_SETTLED: Duration = Duration::from_millis(300);

/// When controls over a film or a picture are on show: for [`CONTROLS_SHOWN`] after the wearer
/// last did something, and then not -- paused as much as playing, since a film is often paused
/// to look at something in the frame -- or until a tap on the picture puts them away at once
/// (SpatiReel's; SpatiLoupe has no such tap). A press, a key, a scroll [`wake`](Self::wake)
/// them; the room's word that the hands stirred [`stir`](Self::stir)s them; pointer motion
/// alone does not, once the room has said the hands. The app keeps them up while a menu, a
/// pane or a field of its own is open, and draws its own fade.
#[derive(Debug, Clone, Copy)]
pub struct AutoHide {
    until: Instant,
    /// When they last came on show.
    since: Instant,
    /// Put away by a tap: until then, the hands that made it do not bring them back.
    deaf_until: Instant,
}

impl AutoHide {
    pub fn new(now: Instant) -> AutoHide {
        AutoHide { until: now + CONTROLS_SHOWN, since: now, deaf_until: now }
    }

    pub fn on(&self, now: Instant) -> bool {
        now < self.until
    }

    /// On show, and for long enough that a tap on the picture is meant for them: not brought
    /// back a moment ago by the very hand that is tapping -- the move to get there, or the
    /// room's `hands` for the press arriving before the press itself.
    pub fn settled(&self, now: Instant) -> bool {
        self.on(now) && now.saturating_duration_since(self.since) >= TAP_SETTLED
    }

    /// A press, a key, a scroll: on show from now.
    pub fn wake(&mut self, now: Instant) {
        if !self.on(now) {
            self.since = now;
        }
        self.until = now + CONTROLS_SHOWN;
    }

    /// The hands stirred (or, in a room too old to say, the pointer moved): the same, unless a
    /// tap has only just put the controls away.
    pub fn stir(&mut self, now: Instant) {
        if now >= self.deaf_until {
            self.wake(now);
        }
    }

    /// A tap on the picture: gone at once.
    pub fn put_away(&mut self, now: Instant) {
        self.until = now;
        self.deaf_until = now + PUT_AWAY_DEAF;
    }

    /// How long until they go by themselves, while they are on show.
    pub fn left(&self, now: Instant) -> Option<Duration> {
        self.on(now).then(|| self.until - now)
    }
}

impl AutoHide {
    /// How long ago they went by themselves (or were put away), while they are not on show: for
    /// an app that fades them out over a moment.
    pub fn off_since(&self, now: Instant) -> Option<Duration> {
        (!self.on(now)).then(|| now.saturating_duration_since(self.until))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_controls_go_by_themselves_and_come_back_for_the_hands() {
        let start = Instant::now();
        let mut clock = AutoHide::new(start);
        assert!(clock.on(start));
        assert_eq!(clock.left(start), Some(CONTROLS_SHOWN));
        // Nothing done for the while they stay: gone, whatever the film is doing.
        let later = start + CONTROLS_SHOWN;
        assert!(!clock.on(later));
        assert_eq!(clock.left(later), None);
        // The hands stir: back, for as long again.
        clock.stir(later);
        assert!(clock.on(later + CONTROLS_SHOWN - Duration::from_millis(1)));
        assert!(!clock.on(later + CONTROLS_SHOWN));
    }

    #[test]
    fn a_tap_puts_the_controls_away_and_its_own_hands_do_not_bring_them_back() {
        let start = Instant::now();
        let mut clock = AutoHide::new(start);
        let tapped = start + Duration::from_millis(500);
        clock.put_away(tapped);
        assert!(!clock.on(tapped));
        // The room's word that the hands stirred, for the tap itself, a few frames on.
        clock.stir(tapped + Duration::from_millis(120));
        assert!(!clock.on(tapped + Duration::from_millis(120)));
        // The hands moving afterwards bring them back.
        let moved = tapped + PUT_AWAY_DEAF;
        clock.stir(moved);
        assert!(clock.on(moved));
        // A second tap, a press, a key: back at once, however soon.
        clock.put_away(moved);
        clock.wake(moved + Duration::from_millis(50));
        assert!(clock.on(moved + Duration::from_millis(50)));
    }

    #[test]
    fn a_tap_puts_away_only_controls_that_were_already_there() {
        let start = Instant::now();
        let mut clock = AutoHide::new(start);
        assert!(!clock.settled(start));
        assert!(clock.settled(start + TAP_SETTLED));
        // Gone, then brought back by the hand on its way to tap: that tap is not for them.
        let back = start + CONTROLS_SHOWN + Duration::from_secs(1);
        clock.stir(back);
        assert!(clock.on(back) && !clock.settled(back + Duration::from_millis(50)));
        assert!(clock.settled(back + TAP_SETTLED));
        // Kept on show by more stirring, they stay settled.
        clock.stir(back + Duration::from_secs(1));
        assert!(clock.settled(back + Duration::from_secs(1)));
    }

    #[test]
    fn how_long_ago_they_went() {
        let start = Instant::now();
        let clock = AutoHide::new(start);
        assert_eq!(clock.off_since(start), None);
        assert_eq!(clock.off_since(start + CONTROLS_SHOWN + Duration::from_millis(250)), Some(Duration::from_millis(250)));
    }
}
