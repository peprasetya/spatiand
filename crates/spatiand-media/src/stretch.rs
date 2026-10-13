//! Sound played faster or slower without its pitch moving.
//!
//! Played at half as fast again, a voice read out at half as fast again is a voice a fifth
//! higher. What is wanted is the same voice saying it sooner: short pieces of the sound laid
//! over one another closer together than they were (or further apart), each piece nudged by a
//! few milliseconds to where it best continues the one before -- waveform-similarity
//! overlap-add. Done here, in forty lines of arithmetic, because the platform that would lend
//! a filter for it differs on each of the three this plays on.

/// How long a piece is, in seconds, and how far either way one may be nudged.
const PIECE: f64 = 0.040;
const NUDGE: f64 = 0.012;

pub struct Stretch {
    channels: usize,
    /// Frames in a piece; half of it is laid over the piece before.
    piece: usize,
    nudge: usize,
    /// Sound in, not yet used up: frames of every channel in turn.
    input: Vec<f32>,
    /// Where in `input` the next piece is to be looked for, in frames, not yet nudged.
    at: f64,
    /// Where the last piece was taken from, in frames: its continuation is what the next
    /// should resemble.
    last: Option<usize>,
    /// The second half of the last piece, already faded, waiting for the next to be laid on it.
    tail: Vec<f32>,
    /// The raised-cosine fade over half a piece.
    fade: Vec<f32>,
}

impl Stretch {
    pub fn new(rate: u32, channels: u16) -> Stretch {
        let piece = ((rate as f64 * PIECE) as usize / 2 * 2).max(4);
        let half = piece / 2;
        Stretch {
            channels: channels.max(1) as usize,
            piece,
            nudge: (rate as f64 * NUDGE) as usize,
            input: Vec::new(),
            at: 0.0,
            last: None,
            tail: Vec::new(),
            fade: (0..half).map(|i| (0.5 - 0.5 * (std::f64::consts::PI * (i as f64 + 0.5) / half as f64).cos()) as f32).collect(),
        }
    }

    /// Forget everything: what comes next does not follow what came before.
    pub fn reset(&mut self) {
        self.input.clear();
        self.at = 0.0;
        self.last = None;
        self.tail.clear();
    }

    /// One channel-summed sample of the input, for comparing pieces by.
    fn mono(&self, frame: usize) -> f32 {
        self.input[frame * self.channels..(frame + 1) * self.channels].iter().sum()
    }

    /// Take `samples` in, and give back as much sound at `speed` as they complete.
    pub fn feed(&mut self, samples: &[f32], speed: f64) -> Vec<f32> {
        self.input.extend_from_slice(samples);
        let (c, half) = (self.channels, self.piece / 2);
        let step = half as f64 * speed.clamp(0.25, 4.0);
        let mut out = Vec::new();
        loop {
            let frames = self.input.len() / c;
            let wanted = self.at.round() as usize;
            // Everything a piece at the furthest nudge would read has to have arrived.
            if wanted + self.nudge + self.piece > frames {
                break;
            }
            // The piece that best carries on from where the last one left off: the place
            // within a nudge of where it ought to be whose sound most resembles what would
            // have come next had nothing been skipped.
            let from = match self.last {
                None => wanted,
                Some(last) => {
                    let natural = last + half;
                    let lowest = wanted.saturating_sub(self.nudge);
                    let mut best = (f32::MIN, wanted);
                    // Every other place and every other sample: near enough, at a quarter
                    // of the sums.
                    for candidate in (lowest..=wanted + self.nudge).step_by(2) {
                        let mut likeness = 0.0;
                        for i in (0..half).step_by(2) {
                            if natural + i >= frames {
                                break;
                            }
                            likeness += self.mono(candidate + i) * self.mono(natural + i);
                        }
                        if likeness > best.0 {
                            best = (likeness, candidate);
                        }
                    }
                    best.1
                }
            };
            // Its first half fades in over the last piece's fading out; its second half
            // waits, fading out, for the next.
            let piece = &self.input[from * c..(from + self.piece) * c];
            if self.tail.is_empty() {
                self.tail = vec![0.0; half * c];
            }
            for i in 0..half {
                for channel in 0..c {
                    out.push(self.tail[i * c + channel] + piece[i * c + channel] * self.fade[i]);
                    self.tail[i * c + channel] = piece[(half + i) * c + channel] * self.fade[half - 1 - i];
                }
            }
            self.last = Some(from);
            self.at += step;
        }
        // What can no longer be read is let go of, and everything that counts frames with it.
        let keep_from = (self.at.round() as usize).saturating_sub(self.nudge).min(self.last.map_or(usize::MAX, |last| last + half).min(self.input.len() / c));
        if keep_from > 0 {
            self.input.drain(..keep_from * c);
            self.at -= keep_from as f64;
            self.last = self.last.map(|last| last.saturating_sub(keep_from));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, seconds: f64, hertz: f64) -> Vec<f32> {
        (0..(rate as f64 * seconds) as usize).flat_map(|i| {
            let s = (i as f64 / rate as f64 * hertz * std::f64::consts::TAU).sin() as f32 * 0.5;
            [s, s]
        }).collect()
    }

    /// How many times a second the sound crosses nought going up: its pitch, near enough.
    fn pitch(samples: &[f32], rate: u32) -> f64 {
        let left: Vec<f32> = samples.chunks_exact(2).map(|pair| pair[0]).collect();
        let crossings = left.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        crossings as f64 / (left.len() as f64 / rate as f64)
    }

    #[test]
    fn faster_is_shorter_and_slower_is_longer_by_as_much() {
        for speed in [0.75, 1.25, 1.5, 2.0] {
            let mut stretch = Stretch::new(48_000, 2);
            let mut out = Vec::new();
            // In blocks, as a decoder gives it.
            for block in tone(48_000, 4.0, 440.0).chunks(2048) {
                out.extend(stretch.feed(block, speed));
            }
            let seconds = out.len() as f64 / 2.0 / 48_000.0;
            assert!((seconds - 4.0 / speed).abs() < 0.15, "at {speed}: {seconds:.2} s");
        }
    }

    #[test]
    fn the_pitch_stays_where_it_was() {
        let mut stretch = Stretch::new(48_000, 2);
        let out = stretch.feed(&tone(48_000, 3.0, 440.0), 1.5);
        let heard = pitch(&out[9600..], 48_000);
        assert!((heard - 440.0).abs() < 12.0, "a 440 Hz tone came out at {heard:.0} Hz");
    }

    #[test]
    fn nothing_falls_silent_where_pieces_meet() {
        let mut stretch = Stretch::new(48_000, 2);
        let out = stretch.feed(&tone(48_000, 2.0, 300.0), 1.25);
        // Over any fiftieth of a second, a steady tone is about as loud as anywhere else.
        let loudness: Vec<f32> = out[4800..].chunks(960 * 2).filter(|c| c.len() == 1920).map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt()).collect();
        let (quietest, loudest) = loudness.iter().fold((f32::MAX, 0f32), |(lo, hi), l| (lo.min(*l), hi.max(*l)));
        assert!(quietest > loudest * 0.6, "loudness ran from {quietest:.3} to {loudest:.3}");
    }
}
