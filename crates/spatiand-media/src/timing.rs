//! Time, for a film whose file keeps none.
//!
//! A raw stream -- a bare `.h264` or `.hevc`, an MPEG-2 or MPEG-4 elementary stream, pictures
//! one after another with no container round them -- says what its pictures are and nothing of
//! when each is shown. Played by the times its packets carry, it has none, and every picture
//! was due at once: ten seconds of film went by in a third of a second.
//!
//! So the times are made here, on every platform alike, before any decoder sees a packet:
//!
//! * **How fast** is what the stream itself states ([`frame_rate`]): its codec's timing (an
//!   H.264 SPS's VUI, an HEVC VPS, an MPEG sequence header), and where it states nothing, 25
//!   pictures a second, which is what FFmpeg's own players take such a file to be.
//! * **When each picture is shown** is counted ([`Count`]): a decoder gives its pictures in the
//!   order they are shown, so the n-th since the reader last moved is n pictures after where it
//!   moved to. The packets are numbered too, in the order they are read, because a decoder
//!   wants every one told apart (Android's matches each picture to its packet's time); but
//!   that order is not the order of showing, so those numbers are not the pictures' times.
//! * **How long the film is, and where a moment of it is in the file**, is learnt as it is
//!   read ([`Index`]): the key frames passed, each with its place in the file and its time,
//!   and from how many bytes a second of it has taken so far, how long the rest will be.
//!   Moving to a moment already read goes to its key frame, exactly. Moving to one not read
//!   yet goes to where it would be if the rest of the file ran at the pace of what has been
//!   read, and the first key frame found there is taken to be the moment asked for.

/// Pictures a second, for a stream that states no rate: what `ffplay` and `ffmpeg` take a raw
/// stream to be.
pub const UNSTATED: f64 = 25.0;

/// How many pictures a second a film shows: the first of `stated` that is a rate at all (each
/// a fraction, as FFmpeg gives them -- the codec's own first, then the container's), and
/// [`UNSTATED`] where none is.
pub fn frame_rate(stated: &[(i32, i32)]) -> f64 {
    stated
        .iter()
        .filter(|(num, den)| *num > 0 && *den > 0)
        .map(|(num, den)| *num as f64 / *den as f64)
        .find(|rate| *rate <= 1000.0)
        .unwrap_or(UNSTATED)
}

/// The times of the pictures a decoder gives, where their own are missing.
#[derive(Debug, Default)]
pub struct Count {
    /// When the next picture is shown, if nothing says otherwise.
    next: Option<f64>,
}

impl Count {
    /// The reader has moved: the next picture is the first of somewhere else.
    pub fn restart(&mut self) {
        self.next = None;
    }

    /// The time of the first packet given to the decoder since the reader moved: where the
    /// count starts from.
    pub fn begin(&mut self, time: f64) {
        if self.next.is_none() && time.is_finite() {
            self.next = Some(time);
        }
    }

    /// When the picture the decoder has just given is shown: `stamped`, its own time, where it
    /// has one that is meant (a film whose file keeps times), and otherwise one picture after
    /// the one before. `repeat` is the stream asking for the picture to be held longer, in
    /// half pictures (a film telecined in its stream).
    pub fn time(&mut self, stamped: Option<f64>, rate: f64, repeat: i32) -> f64 {
        let time = stamped.or(self.next).unwrap_or(0.0);
        let rate = if rate > 0.0 { rate } else { UNSTATED };
        self.next = Some(time + (1.0 + repeat.clamp(0, 16) as f64 * 0.5) / rate);
        time
    }
}

/// Where to read from, to be at a moment of the film.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Landing {
    /// A key frame already passed: the packet at `pos` is at `time`, which is at or before the
    /// moment asked for.
    Key { pos: u64, time: f64 },
    /// A part of the file not read yet: from byte `pos` on, the first key frame found is taken
    /// to be the moment asked for.
    Bytes { pos: u64 },
}

/// A stretch of the file read from one key frame straight on.
#[derive(Debug, Clone)]
struct Run {
    /// Its key frames, in order: where in the file, and when in the film. Never empty.
    keys: Vec<(u64, f64)>,
    /// Where reading it stopped: the byte after its last packet, and the time after it.
    end: (u64, f64),
    /// It stopped at the end of the file.
    ended: bool,
}

impl Run {
    fn start(&self) -> (u64, f64) {
        self.keys[0]
    }

    fn shift(&mut self, by: f64) {
        for key in &mut self.keys {
            key.1 += by;
        }
        self.end.1 += by;
    }
}

/// What has been learnt of a file with no times: see the module.
#[derive(Debug)]
pub struct Index {
    /// The file's length in bytes; 0 where it is not known (a stream with no end).
    size: u64,
    /// The stretches read, in the file's order, none overlapping another.
    runs: Vec<Run>,
    /// Which of them is being read on from; none just after a jump, until a key frame is found.
    current: Option<usize>,
}

/// How much of a file must have been read before the rest is judged by it.
const JUDGED_BY_BYTES: u64 = 16 * 1024;
const JUDGED_BY_SECONDS: f64 = 0.5;

impl Index {
    pub fn new(size: u64) -> Index {
        Index { size, runs: Vec::new(), current: None }
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    /// The reader has jumped to a byte of the file: what it finds starts a stretch of its own.
    pub fn jumped(&mut self) {
        self.current = None;
    }

    /// The time already given to the key frame at `pos`, if it has been passed before.
    pub fn key_time(&self, pos: u64) -> Option<f64> {
        self.runs.iter().find_map(|run| run.keys.binary_search_by_key(&pos, |key| key.0).ok().map(|at| run.keys[at].1))
    }

    /// The reader has gone back to the key frame at `pos`, and reads on from it.
    pub fn went_to(&mut self, pos: u64) {
        self.current = self.runs.iter().position(|run| run.start().0 <= pos && pos < run.end.0);
    }

    /// A packet has been read, straight after the one before unless [`Index::jumped`] or
    /// [`Index::went_to`] said otherwise: `bytes` of the file from `pos`, shown at `time` for
    /// `lasts`. `key` is whether reading can start from it; the first after a jump must be.
    pub fn passed(&mut self, pos: u64, bytes: u64, time: f64, lasts: f64, key: bool) {
        let end = (pos + bytes.max(1), time + lasts);
        let Some(at) = self.current else {
            if !key {
                return;
            }
            // A stretch of its own, in its place among the others.
            let at = self.runs.partition_point(|run| run.start().0 < pos);
            self.runs.insert(at, Run { keys: vec![(pos, time)], end, ended: false });
            self.current = Some(at);
            self.join(at, pos, time);
            return;
        };
        let run = &mut self.runs[at];
        if end.0 <= run.end.0 {
            // Read before: gone back to, and being read again.
            return;
        }
        if key && run.keys.last().is_some_and(|last| last.0 < pos) {
            run.keys.push((pos, time));
        }
        run.end = end;
        if key {
            self.join(at, pos, time);
        }
    }

    /// The stretch `at` has reached the key frame at `pos`, at `time` by its own count. If that
    /// is where a later stretch begins, the two are one from here: the later one's times were
    /// counted from a guess, and are moved to carry on from these.
    fn join(&mut self, at: usize, pos: u64, time: f64) {
        while let Some(next) = self.runs.get(at + 1) {
            if next.start().0 > pos {
                return;
            }
            let Ok(found) = next.keys.binary_search_by_key(&pos, |key| key.0) else {
                // Read past its start without meeting a key frame of it: what of it is behind
                // is forgotten, and the rest waits to be met.
                let behind = next.keys.partition_point(|key| key.0 <= pos);
                if behind == next.keys.len() || next.end.0 <= self.runs[at].end.0 {
                    self.runs.remove(at + 1);
                    continue;
                }
                self.runs[at + 1].keys.drain(..behind);
                return;
            };
            let by = time - next.keys[found].1;
            for later in &mut self.runs[at + 1..] {
                later.shift(by);
            }
            let next = self.runs.remove(at + 1);
            let run = &mut self.runs[at];
            run.keys.extend_from_slice(&next.keys[found + 1..]);
            if next.end.0 > run.end.0 {
                run.end = next.end;
                run.ended = next.ended;
            }
        }
    }

    /// The file has no more: the stretch being read is its last.
    pub fn ended(&mut self) {
        if let Some(at) = self.current {
            self.runs[at].ended = true;
            self.runs.truncate(at + 1);
        }
    }

    /// How long the film is, and whether that is a guess: known once the file has been read
    /// from its start to its end, and judged until then from its size and the pace of what has
    /// been read. None while there is nothing to judge by.
    pub fn length(&self) -> Option<(f64, bool)> {
        let last = self.runs.last()?;
        if last.ended {
            let whole = self.runs.len() == 1 && last.start().1 == 0.0;
            return Some((last.end.1, !whole));
        }
        if self.size == 0 {
            return None;
        }
        let bytes: u64 = self.runs.iter().map(|run| run.end.0 - run.start().0).sum();
        let seconds: f64 = self.runs.iter().map(|run| run.end.1 - run.start().1).sum();
        if bytes < JUDGED_BY_BYTES.min(self.size) || seconds < JUDGED_BY_SECONDS {
            return None;
        }
        let rest = self.size.saturating_sub(last.end.0) as f64;
        Some((last.end.1 + rest * seconds / bytes as f64, true))
    }

    /// Where to read from to be at `to` seconds. None while that cannot be judged: nothing
    /// read yet to go by, or a stream with no end.
    pub fn seek(&self, to: f64) -> Option<Landing> {
        // In a stretch that has been read: its key frame at or before.
        for run in &self.runs {
            if run.start().1 <= to && to < run.end.1 {
                let (pos, time) = run.keys[run.keys.partition_point(|key| key.1 <= to) - 1];
                return Some(Landing::Key { pos, time });
            }
        }
        // Between what is known either side of it, as far through the bytes as through the time.
        let before = self.runs.iter().rev().find(|run| run.end.1 <= to).map_or((0, 0.0), |run| run.end);
        if to <= before.1 {
            return Some(Landing::Bytes { pos: before.0 });
        }
        if self.size == 0 {
            return None;
        }
        let after = match self.runs.iter().find(|run| run.start().1 > to) {
            Some(run) => run.start(),
            None => (self.size, self.length()?.0),
        };
        if after.1 <= before.1 || after.0 <= before.0 {
            return Some(Landing::Bytes { pos: before.0 });
        }
        let through = ((to - before.1) / (after.1 - before.1)).clamp(0.0, 1.0);
        Some(Landing::Bytes { pos: before.0 + (through * (after.0 - before.0) as f64) as u64 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rate_is_the_streams_own_and_25_where_it_states_none() {
        // The codec's statement first (an SPS's VUI), whatever the container supposes.
        assert_eq!(frame_rate(&[(24, 1), (25, 1)]), 24.0);
        assert_eq!(frame_rate(&[(30000, 1001), (25, 1)]), 30000.0 / 1001.0);
        assert_eq!(frame_rate(&[(60, 1)]), 60.0);
        // Nothing stated by the codec: the container's (a raw demuxer's is 25).
        assert_eq!(frame_rate(&[(0, 1), (25, 1)]), 25.0);
        assert_eq!(frame_rate(&[(0, 0), (50, 1)]), 50.0);
        // Nothing stated anywhere, or nonsense: 25, as ffplay has it.
        assert_eq!(frame_rate(&[]), UNSTATED);
        assert_eq!(frame_rate(&[(0, 1), (0, 0)]), UNSTATED);
        assert_eq!(frame_rate(&[(-24, 1), (1, 0)]), UNSTATED);
        assert_eq!(frame_rate(&[(90000, 1)]), UNSTATED);
    }

    #[test]
    fn pictures_with_no_times_are_one_frame_apart_from_where_reading_began() {
        let mut count = Count::default();
        count.begin(0.0);
        let times: Vec<f64> = (0..5).map(|_| count.time(None, 24.0, 0)).collect();
        for (n, time) in times.iter().enumerate() {
            assert!((time - n as f64 / 24.0).abs() < 1e-9, "{times:?}");
        }
        // Ten seconds of a 60-a-second stream are 600 pictures, the last a sixtieth short of ten.
        let mut count = Count::default();
        count.begin(0.0);
        let last = (0..600).map(|_| count.time(None, 60.0, 0)).last().unwrap();
        assert!((last - (10.0 - 1.0 / 60.0)).abs() < 1e-6, "{last}");
    }

    #[test]
    fn a_count_starts_again_from_where_a_seek_lands() {
        let mut count = Count::default();
        count.begin(0.0);
        count.time(None, 25.0, 0);
        count.time(None, 25.0, 0);
        count.restart();
        // Only the first packet after the move says where: later ones do not move it.
        count.begin(42.0);
        count.begin(42.04);
        assert_eq!(count.time(None, 25.0, 0), 42.0);
        assert!((count.time(None, 25.0, 0) - 42.04).abs() < 1e-9);
    }

    #[test]
    fn a_film_with_times_keeps_them_and_a_picture_missing_one_follows_the_last() {
        let mut count = Count::default();
        count.begin(10.0);
        // B-frames: the packets were at 10.0, 10.04, ... but the pictures say their own.
        assert_eq!(count.time(Some(10.08), 25.0, 0), 10.08);
        assert_eq!(count.time(Some(10.12), 25.0, 0), 10.12);
        // One with no time of its own: a frame after the one before, not at nought.
        assert!((count.time(None, 25.0, 0) - 10.16).abs() < 1e-9);
        assert_eq!(count.time(Some(10.2), 25.0, 0), 10.2);
    }

    #[test]
    fn a_picture_the_stream_holds_longer_puts_the_next_later() {
        // 3:2 pulldown carried in the stream: every other picture held half as long again.
        let mut count = Count::default();
        count.begin(0.0);
        assert_eq!(count.time(None, 30.0, 1), 0.0);
        assert!((count.time(None, 30.0, 0) - 1.5 / 30.0).abs() < 1e-9);
        assert!((count.time(None, 30.0, 1) - 2.5 / 30.0).abs() < 1e-9);
        assert!((count.time(None, 30.0, 0) - 4.0 / 30.0).abs() < 1e-9);
    }

    /// Whether a landing is the key frame at `pos`, at `time` to within what adding up
    /// twenty-fifths of a second leaves.
    fn is_key(landing: Option<Landing>, pos: u64, time: f64) -> bool {
        matches!(landing, Some(Landing::Key { pos: p, time: t }) if p == pos && (t - time).abs() < 1e-6)
    }

    /// Whether a landing is by bytes, within a picture's worth of `pos`.
    fn is_bytes(landing: Option<Landing>, pos: u64) -> bool {
        matches!(landing, Some(Landing::Bytes { pos: p }) if p.abs_diff(pos) < 1000)
    }

    /// A file of `seconds` at 25 pictures a second, `bytes` each, a key frame every second:
    /// read from packet `from` to packet `to`, the clock starting at `clock`.
    fn read(index: &mut Index, from: u64, to: u64, bytes: u64, clock: f64) -> f64 {
        let mut time = clock;
        for n in from..to {
            index.passed(n * bytes, bytes, time, 0.04, n % 25 == 0);
            time += 0.04;
        }
        time
    }

    #[test]
    fn the_length_is_judged_from_what_has_been_read_and_known_at_the_end() {
        // 100 seconds, 1000 bytes a picture: 2 500 000 bytes.
        let mut index = Index::new(2_500_000);
        assert_eq!(index.length(), None);
        read(&mut index, 0, 5, 1000, 0.0);
        // A fifth of a second is too little to judge by.
        assert_eq!(index.length(), None);
        read(&mut index, 5, 100, 1000, 0.2);
        let (length, guessed) = index.length().unwrap();
        assert!((length - 100.0).abs() < 1e-6 && guessed, "{length}");
        let end = read(&mut index, 100, 2500, 1000, 4.0);
        index.ended();
        let (length, guessed) = index.length().unwrap();
        assert!((length - end).abs() < 1e-9 && (length - 100.0).abs() < 1e-6 && !guessed, "{length}");
    }

    #[test]
    fn a_film_whose_pictures_get_bigger_is_guessed_shorter_as_it_is_read() {
        // 10 seconds of small pictures, then 10 of pictures three times the size: 1 000 000 bytes.
        let mut index = Index::new(250 * 1000 + 250 * 3000);
        let mut time = 0.0;
        for n in 0..250u64 {
            index.passed(n * 1000, 1000, time, 0.04, n % 25 == 0);
            time += 0.04;
        }
        // From the small ones alone the film looks twice as long as it is.
        assert!((index.length().unwrap().0 - 40.0).abs() < 1e-6);
        for n in 0..250u64 {
            index.passed(250_000 + n * 3000, 3000, time, 0.04, n % 25 == 0);
            time += 0.04;
        }
        index.ended();
        assert!((index.length().unwrap().0 - 20.0).abs() < 1e-6);
    }

    #[test]
    fn a_moment_already_read_is_gone_back_to_by_its_key_frame() {
        let mut index = Index::new(2_500_000);
        read(&mut index, 0, 250, 1000, 0.0);
        // 7.3 s: the key frame at 7 s, which is the 175th packet.
        assert!(is_key(index.seek(7.3), 175_000, 7.0), "{:?}", index.seek(7.3));
        assert_eq!(index.seek(0.0), Some(Landing::Key { pos: 0, time: 0.0 }));
        // Read again from there: nothing is learnt twice, and the times stay what they were.
        index.went_to(175_000);
        read(&mut index, 175, 300, 1000, 7.0);
        assert!(is_key(index.seek(7.3), 175_000, 7.0), "{:?}", index.seek(7.3));
        assert!(is_key(index.seek(11.9), 275_000, 11.0), "{:?}", index.seek(11.9));
        assert!((index.key_time(275_000).unwrap() - 11.0).abs() < 1e-6);
        assert_eq!(index.key_time(275_500), None);
    }

    #[test]
    fn a_moment_not_read_yet_is_gone_to_by_bytes() {
        let mut index = Index::new(2_500_000);
        // Nothing read: only the start can be gone to.
        assert_eq!(index.seek(0.0), Some(Landing::Bytes { pos: 0 }));
        assert_eq!(index.seek(50.0), None);
        read(&mut index, 0, 100, 1000, 0.0);
        // Half way through the film is half way through the file.
        assert!(is_bytes(index.seek(50.0), 1_250_000), "{:?}", index.seek(50.0));
        // The reader jumps; the first key frame it finds there is taken to be 50 s.
        index.jumped();
        index.passed(1_250_500, 1000, 50.0, 0.04, false);
        assert!(is_bytes(index.seek(50.0), 1_250_000), "{:?}", index.seek(50.0));
        read(&mut index, 1275, 1400, 1000, 50.0);
        assert!(is_key(index.seek(52.5), 1_325_000, 52.0), "{:?}", index.seek(52.5));
        // Between the two stretches: between the end of the one and the start of the other.
        let Some(Landing::Bytes { pos }) = index.seek(27.0) else { panic!() };
        assert!(pos > 100_000 && pos < 1_275_000, "{pos}");
        // A stream with no end has nowhere to jump to.
        let mut endless = Index::new(0);
        read(&mut endless, 0, 100, 1000, 0.0);
        assert_eq!(endless.seek(50.0), None);
        assert_eq!(endless.length(), None);
        assert!(is_key(endless.seek(2.3), 50_000, 2.0), "{:?}", endless.seek(2.3));
    }

    #[test]
    fn a_stretch_read_on_into_the_next_makes_one_of_them() {
        let mut index = Index::new(2_500_000);
        read(&mut index, 0, 100, 1000, 0.0);
        // A jump to "50 s" that landed on what is really 51 s (packet 1275).
        index.jumped();
        read(&mut index, 1275, 1400, 1000, 50.0);
        // Back to the start, and read all the way to where the jump landed and past it.
        index.went_to(0);
        let time = read(&mut index, 0, 1300, 1000, 0.0);
        assert!((time - 52.0).abs() < 1e-6);
        // One stretch now, by the count from the start: the key frame at packet 1275 is 51 s,
        // and those after it moved by as much.
        assert!((index.key_time(1_275_000).unwrap() - 51.0).abs() < 1e-6);
        assert!((index.key_time(1_350_000).unwrap() - 54.0).abs() < 1e-6);
        let Some(Landing::Key { pos, time }) = index.seek(55.5) else { panic!() };
        assert_eq!(pos, 1_375_000);
        assert!((time - 55.0).abs() < 1e-6);
        read(&mut index, 1300, 2500, 1000, 52.0);
        index.ended();
        let (length, guessed) = index.length().unwrap();
        assert!((length - 100.0).abs() < 1e-6 && !guessed, "{length}");
    }

    #[test]
    fn the_end_reached_after_a_jump_is_the_length_but_still_a_guess() {
        let mut index = Index::new(2_500_000);
        read(&mut index, 0, 100, 1000, 0.0);
        index.jumped();
        // Taken to be 90 s; really 91 s. Read to the end from there.
        read(&mut index, 2275, 2500, 1000, 90.0);
        index.ended();
        let (length, guessed) = index.length().unwrap();
        assert!((length - 99.0).abs() < 1e-6 && guessed, "{length}");
        // Past the end there is only the end.
        assert_eq!(index.seek(120.0), Some(Landing::Bytes { pos: 2_500_000 }));
    }
}
