//! Cutting the microphone into phrases at the pauses.
//!
//! Done here rather than by hemera: each phrase is then one ordinary request, and hemera's
//! Realtime socket -- which holds every background job on deepmagpie for as long as it is open --
//! is never needed (`docs/ai-integrations.md`).
//!
//! **Loudness over the room's own noise**, in 20 ms windows. The noise floor falls at once to a
//! quieter window and rises only slowly, so a fan is learnt and a voice is not; a phrase starts
//! after three loud windows in a row and ends after a pause of `pause_ms`. The first moments
//! after the microphone opens only learn the room: a phrase begun before then would be the
//! room itself, and Whisper given a phrase of noise writes something anyway -- "Terima kasih
//! telah menonton!" is its favourite.

use std::collections::VecDeque;

/// One window of samples, in milliseconds.
const WINDOW_MS: u32 = 20;
/// Windows spent only learning the room after the microphone opens.
const WARM_UP: usize = 15;
/// A phrase begins with this much from before it was loud enough: the start of the first word
/// is quieter than its middle.
const PRE_ROLL_MS: u32 = 250;
/// And keeps this much of the pause that ended it, for the last word's tail.
const TAIL_MS: u32 = 200;
/// Shorter than this, of loud windows, is a click or a cough, not a phrase.
const SHORTEST_SPEECH_MS: u32 = 300;
/// How far over the floor starts a phrase, and keeps one going, in dB. The gap between them
/// keeps a phrase from flickering on and off at a word's quiet end.
const START_OVER: f32 = 10.0;
const KEEP_OVER: f32 = 6.0;
/// Quieter than this is never speech, whatever the floor: a dead-silent room has a floor so low
/// that breathing would be over it.
const QUIETEST_SPEECH: f32 = -55.0;
/// Whisper reads 30 s at most; a phrase this long is sent and the next carries on.
const LONGEST_MS: u32 = 25_000;

pub struct Phrases {
    window: usize,
    pause_windows: usize,
    pending: Vec<i16>,
    floor: f32,
    level: f32,
    seen: usize,
    loud_run: usize,
    pre: VecDeque<i16>,
    pre_len: usize,
    building: Option<Building>,
}

struct Building {
    samples: Vec<i16>,
    /// Quiet windows since the last loud one.
    quiet: usize,
    loud: usize,
}

impl Phrases {
    /// Phrases of 16-bit mono at `rate`, ended by `pause_ms` of quiet.
    pub fn new(rate: u32, pause_ms: u32) -> Phrases {
        let window = (rate * WINDOW_MS / 1000) as usize;
        Phrases {
            window,
            pause_windows: (pause_ms / WINDOW_MS).max(5) as usize,
            pending: Vec::with_capacity(window),
            floor: -60.0,
            level: -100.0,
            seen: 0,
            loud_run: 0,
            pre: VecDeque::new(),
            pre_len: (rate * PRE_ROLL_MS / 1000) as usize,
            building: None,
        }
    }

    /// Take more of the microphone; out come the phrases it finished.
    pub fn feed(&mut self, samples: &[i16]) -> Vec<Vec<i16>> {
        let mut done = Vec::new();
        for &s in samples {
            self.pending.push(s);
            if self.pending.len() == self.window {
                let window = std::mem::take(&mut self.pending);
                if let Some(phrase) = self.window_done(&window) {
                    done.push(phrase);
                }
                self.pending = window;
                self.pending.clear();
            }
        }
        done
    }

    /// What is being said when the microphone is closed: kept if it is speech.
    pub fn flush(&mut self) -> Option<Vec<i16>> {
        let b = self.building.take()?;
        (b.loud * WINDOW_MS as usize >= SHORTEST_SPEECH_MS as usize).then_some(b.samples)
    }

    /// The last window's loudness from 0 (the floor or quieter) to 1 (30 dB over it): a meter.
    pub fn meter(&self) -> f32 {
        ((self.level - self.floor) / 30.0).clamp(0.0, 1.0)
    }

    /// Whether a phrase is being heard.
    pub fn hearing(&self) -> bool {
        self.building.is_some()
    }

    fn window_done(&mut self, window: &[i16]) -> Option<Vec<i16>> {
        let energy = window.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / window.len() as f64;
        let db = (10.0 * (energy / (32768.0 * 32768.0) + 1e-12).log10()) as f32;
        self.level = db;
        self.seen += 1;
        if self.seen <= WARM_UP {
            self.floor = if self.seen == 1 { db } else { self.floor * 0.5 + db * 0.5 };
            self.remember(window);
            return None;
        }
        let rise = if self.building.is_some() { 0.001 } else { 0.01 };
        self.floor = if db < self.floor { self.floor * 0.7 + db * 0.3 } else { self.floor + (db - self.floor) * rise };
        self.floor = self.floor.clamp(-85.0, -30.0);

        match &mut self.building {
            None => {
                let loud = db > self.floor + START_OVER && db > QUIETEST_SPEECH;
                self.loud_run = if loud { self.loud_run + 1 } else { 0 };
                if self.loud_run >= 3 {
                    let mut samples: Vec<i16> = self.pre.drain(..).collect();
                    samples.extend_from_slice(window);
                    self.building = Some(Building { samples, quiet: 0, loud: self.loud_run });
                    self.loud_run = 0;
                } else {
                    self.remember(window);
                }
                None
            }
            Some(b) => {
                b.samples.extend_from_slice(window);
                if db > self.floor + KEEP_OVER && db > QUIETEST_SPEECH {
                    b.loud += 1;
                    b.quiet = 0;
                } else {
                    b.quiet += 1;
                }
                let longest = (LONGEST_MS / WINDOW_MS) as usize * self.window;
                if b.quiet >= self.pause_windows {
                    let mut b = self.building.take().unwrap();
                    let tail = (TAIL_MS / WINDOW_MS) as usize;
                    let cut = b.quiet.saturating_sub(tail) * self.window;
                    b.samples.truncate(b.samples.len() - cut.min(b.samples.len()));
                    (b.loud * WINDOW_MS as usize >= SHORTEST_SPEECH_MS as usize).then_some(b.samples)
                } else if b.samples.len() >= longest {
                    // Mid-word, perhaps; better than Whisper dropping the end of a long phrase.
                    let b = self.building.replace(Building { samples: Vec::new(), quiet: 0, loud: 0 }).unwrap();
                    Some(b.samples)
                } else {
                    None
                }
            }
        }
    }

    fn remember(&mut self, window: &[i16]) {
        self.pre.extend(window.iter().copied());
        while self.pre.len() > self.pre_len {
            self.pre.pop_front();
        }
    }
}

/// What Whisper writes when it was given no speech: its favourites, from the films and videos
/// it learnt on. A phrase that comes back as exactly one of these is taken for nothing.
pub fn is_hallucination(text: &str) -> bool {
    let t: String = text.trim().trim_end_matches(['.', '!']).to_lowercase();
    const SAID_OF_NOTHING: &[&str] = &[
        "terima kasih",
        "terima kasih telah menonton",
        "terima kasih sudah menonton",
        "sampai jumpa di video selanjutnya",
        "thank you",
        "thank you for watching",
        "thanks for watching",
        "you",
        "bye",
        "subtitles by the amara.org community",
    ];
    t.is_empty() || SAID_OF_NOTHING.contains(&t.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    /// Noise at about `db` dBFS, `ms` long: a simple generator, the same every run.
    fn noise(ms: u32, db: f32, seed: &mut u32) -> Vec<i16> {
        let amp = 32768.0 * 10f32.powf(db / 20.0) * 1.7;
        (0..RATE * ms / 1000)
            .map(|_| {
                *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (((*seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * 2.0 * amp) as i16
            })
            .collect()
    }

    fn run(parts: &[(u32, f32)]) -> Vec<Vec<i16>> {
        let mut seed = 7;
        let mut p = Phrases::new(RATE, 700);
        let mut out = Vec::new();
        for &(ms, db) in parts {
            out.extend(p.feed(&noise(ms, db, &mut seed)));
        }
        out.extend(p.flush());
        out
    }

    #[test]
    fn a_word_in_a_quiet_room_is_one_phrase() {
        let phrases = run(&[(1000, -60.0), (1000, -20.0), (1500, -60.0)]);
        assert_eq!(phrases.len(), 1);
        let ms = phrases[0].len() as u32 * 1000 / RATE;
        assert!((1300..=1600).contains(&ms), "{ms} ms");
    }

    #[test]
    fn a_short_pause_does_not_end_a_phrase_and_a_long_one_does() {
        assert_eq!(run(&[(800, -60.0), (600, -20.0), (300, -60.0), (600, -20.0), (1500, -60.0)]).len(), 1);
        assert_eq!(run(&[(800, -60.0), (600, -20.0), (1200, -60.0), (600, -20.0), (1500, -60.0)]).len(), 2);
    }

    #[test]
    fn a_click_is_not_a_phrase() {
        assert!(run(&[(800, -60.0), (60, -10.0), (1500, -60.0)]).is_empty());
    }

    #[test]
    fn a_noisy_room_is_learnt_not_heard() {
        // Fan noise from the moment the microphone opens, then a voice well over it.
        assert!(run(&[(3000, -40.0)]).is_empty());
        assert_eq!(run(&[(2000, -40.0), (1000, -15.0), (1500, -40.0)]).len(), 1);
    }

    #[test]
    fn speech_while_closing_is_kept() {
        let phrases = run(&[(800, -60.0), (900, -20.0)]);
        assert_eq!(phrases.len(), 1);
    }

    #[test]
    fn whispers_favourites_are_nothing() {
        assert!(is_hallucination("Terima kasih telah menonton!"));
        assert!(is_hallucination(" Thank you. "));
        assert!(!is_hallucination("Terima kasih, sudah aku kirim ke email."));
    }
}
