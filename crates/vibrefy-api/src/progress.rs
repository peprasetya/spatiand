//! When to write a resume position back to `ProgressBean`.
//!
//! Ported from `player/ProgressReporter.kt`, which took its cadence from `scriptlib.js`.
//! The rhythm is copied deliberately rather than invented: the web player and this one
//! write to the same per-user store, so a file watched in both places should not produce
//! two different rhythms of update - or two different ideas of what "finished" means.
//!
//! Only the *decision* lives here, with the clock passed in, so the throttle can be tested
//! without waiting ten seconds or running a server.

/// `ProgressBean` removes any entry written below -10, which is how the web player clears
/// a file it has played to the end. -99 is the value `scriptlib.js` actually sends.
pub const FINISHED: i64 = -99;

const MIN_SAVE_INTERVAL_MS: u64 = 10_000;
const MIN_PROGRESS_DELTA_S: i64 = 5;

#[derive(Debug, Default)]
pub struct ProgressCadence {
    last_save_at: u64,
    /// `None` until something has been written, so the first position always goes.
    last_saved_position: Option<i64>,
}

impl ProgressCadence {
    pub fn new() -> Self {
        Self::default()
    }

    /// Called as playback advances; `true` means write it now.
    ///
    /// Mirrors `scriptlib.js`'s `timeupdate` guard: at most one write per ten seconds, and
    /// only once the position has actually moved by five - so pausing on a frame does not
    /// spam the server.
    pub fn on_position(&mut self, now_ms: u64, seconds: i64) -> bool {
        if now_ms.saturating_sub(self.last_save_at) <= MIN_SAVE_INTERVAL_MS {
            return false;
        }
        if let Some(last) = self.last_saved_position {
            if (seconds - last).abs() < MIN_PROGRESS_DELTA_S {
                return false;
            }
        }
        self.record(now_ms, seconds);
        true
    }

    /// Unconditional, for pause and for leaving the player. A negative position is not a
    /// place in the file and is never written by this route - [`FINISHED`] has its own.
    pub fn save_now(&mut self, now_ms: u64, seconds: i64) -> bool {
        if seconds < 0 {
            return false;
        }
        self.record(now_ms, seconds);
        true
    }

    /// Retires the entry, and forgets the throttle so whatever plays next starts clean.
    pub fn finished(&mut self) -> i64 {
        self.reset();
        FINISHED
    }

    /// A newly chained episode must not inherit the previous file's throttle state.
    pub fn reset(&mut self) {
        self.last_save_at = 0;
        self.last_saved_position = None;
    }

    fn record(&mut self, now_ms: u64, seconds: i64) {
        self.last_save_at = now_ms;
        self.last_saved_position = Some(seconds);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_position_is_always_written() {
        let mut cadence = ProgressCadence::new();
        assert!(cadence.on_position(11_000, 30));
    }

    #[test]
    fn no_more_than_one_write_in_ten_seconds() {
        let mut cadence = ProgressCadence::new();
        assert!(cadence.on_position(11_000, 30));
        assert!(!cadence.on_position(20_000, 39), "nine seconds later");
        assert!(cadence.on_position(21_001, 40), "just past ten");
    }

    /// Pausing on a frame leaves `timeupdate` firing with the same position. It must not
    /// turn into a write every ten seconds for as long as the film sits there.
    #[test]
    fn a_position_that_has_not_moved_is_not_written() {
        let mut cadence = ProgressCadence::new();
        assert!(cadence.on_position(11_000, 30));
        assert!(!cadence.on_position(30_000, 33), "moved only three seconds");
        assert!(cadence.on_position(30_000, 35), "moved five");
    }

    /// Seeking backwards moves the position by as much as seeking forwards does.
    #[test]
    fn a_backwards_seek_counts_as_movement() {
        let mut cadence = ProgressCadence::new();
        assert!(cadence.on_position(11_000, 600));
        assert!(cadence.on_position(30_000, 120));
    }

    #[test]
    fn pausing_writes_whatever_the_position_is() {
        let mut cadence = ProgressCadence::new();
        assert!(cadence.on_position(11_000, 30));
        assert!(cadence.save_now(11_500, 31), "not subject to the throttle");
        assert!(!cadence.save_now(11_500, -1), "not a place in the file");
    }

    #[test]
    fn finishing_asks_the_server_to_forget_the_file() {
        let mut cadence = ProgressCadence::new();
        cadence.on_position(11_000, 3_600);
        assert_eq!(FINISHED, cadence.finished());
        assert!(FINISHED < -10, "below the threshold ProgressBean deletes at");
        // The next episode starts with a clean throttle rather than the last one's.
        assert!(cadence.on_position(11_100, 0));
    }
}
