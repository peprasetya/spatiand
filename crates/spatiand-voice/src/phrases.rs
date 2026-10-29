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
//!
//! **A draft before the end**: a pause has to be long to be sure a phrase is over, but most short
//! pauses are its end. So at a third of a second of quiet the phrase so far is handed out as a
//! draft, to be written while the cutter waits to be sure ([`Phrases::listen`]); speech going on
//! takes the draft back. A phrase that ends as drafted is the draft's samples exactly, so what was
//! written of the draft is what was written of the phrase -- half a second sooner.

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
/// This much quiet in a phrase hands it out as a draft.
const DRAFT_MS: u32 = 300;

/// What the cutter noticed in what it was fed.
#[derive(Clone, Debug, PartialEq)]
pub enum Cut {
    /// Speech began: a phrase is being heard.
    Began,
    /// A pause that may be the phrase's end: what was said so far, to be written while the cutter
    /// waits to be sure. The phrase, if it ends without [`Cut::Resumed`], is these samples.
    Draft(Vec<i16>),
    /// The speaker went on: the last draft is not the phrase.
    Resumed,
    /// A phrase, whole.
    Phrase(Vec<i16>),
    /// What was heard was too short to be speech -- a click, a cough -- and comes to nothing.
    Dropped,
}

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
    /// Handed out as a draft since the last loud window.
    drafted: bool,
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
        self.listen(samples)
            .into_iter()
            .filter_map(|cut| match cut {
                Cut::Phrase(samples) => Some(samples),
                _ => None,
            })
            .collect()
    }

    /// Take more of the microphone; out comes all it noticed, in order: speech beginning, drafts,
    /// phrases.
    pub fn listen(&mut self, samples: &[i16]) -> Vec<Cut> {
        let mut out = Vec::new();
        for &s in samples {
            self.pending.push(s);
            if self.pending.len() == self.window {
                let window = std::mem::take(&mut self.pending);
                self.window_done(&window, &mut out);
                self.pending = window;
                self.pending.clear();
            }
        }
        out
    }

    /// What is being said when the microphone is closed: kept if it is speech.
    pub fn flush(&mut self) -> Option<Vec<i16>> {
        let b = self.building.take()?;
        (b.loud * WINDOW_MS as usize >= SHORTEST_SPEECH_MS as usize).then_some(b.samples)
    }

    /// Whether what is being heard was handed out as a draft, and not taken back.
    pub fn drafted(&self) -> bool {
        self.building.as_ref().is_some_and(|b| b.drafted)
    }

    /// The last window's loudness from 0 (the floor or quieter) to 1 (30 dB over it): a meter.
    pub fn meter(&self) -> f32 {
        ((self.level - self.floor) / 30.0).clamp(0.0, 1.0)
    }

    /// Whether a phrase is being heard.
    pub fn hearing(&self) -> bool {
        self.building.is_some()
    }

    fn window_done(&mut self, window: &[i16], out: &mut Vec<Cut>) {
        let energy = window.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / window.len() as f64;
        let db = (10.0 * (energy / (32768.0 * 32768.0) + 1e-12).log10()) as f32;
        self.level = db;
        self.seen += 1;
        if self.seen <= WARM_UP {
            self.floor = if self.seen == 1 { db } else { self.floor * 0.5 + db * 0.5 };
            self.remember(window);
            return;
        }
        let rise = if self.building.is_some() { 0.001 } else { 0.01 };
        self.floor = if db < self.floor { self.floor * 0.7 + db * 0.3 } else { self.floor + (db - self.floor) * rise };
        self.floor = self.floor.clamp(-85.0, -30.0);

        let tail = (TAIL_MS / WINDOW_MS) as usize;
        let speech = |b: &Building| b.loud * WINDOW_MS as usize >= SHORTEST_SPEECH_MS as usize;
        match &mut self.building {
            None => {
                let loud = db > self.floor + START_OVER && db > QUIETEST_SPEECH;
                self.loud_run = if loud { self.loud_run + 1 } else { 0 };
                if self.loud_run >= 3 {
                    let mut samples: Vec<i16> = self.pre.drain(..).collect();
                    samples.extend_from_slice(window);
                    self.building = Some(Building { samples, quiet: 0, loud: self.loud_run, drafted: false });
                    self.loud_run = 0;
                    out.push(Cut::Began);
                } else {
                    self.remember(window);
                }
            }
            Some(b) => {
                b.samples.extend_from_slice(window);
                if db > self.floor + KEEP_OVER && db > QUIETEST_SPEECH {
                    b.loud += 1;
                    b.quiet = 0;
                    if std::mem::take(&mut b.drafted) {
                        out.push(Cut::Resumed);
                    }
                } else {
                    b.quiet += 1;
                }
                let longest = (LONGEST_MS / WINDOW_MS) as usize * self.window;
                // The phrase as it would end now: the pause cut to its tail.
                let ending = |b: &Building, window: usize| {
                    let cut = b.quiet.saturating_sub(tail) * window;
                    b.samples[..b.samples.len() - cut.min(b.samples.len())].to_vec()
                };
                if b.quiet >= self.pause_windows {
                    let b = self.building.take().unwrap();
                    out.push(if speech(&b) { Cut::Phrase(ending(&b, self.window)) } else { Cut::Dropped });
                } else if b.samples.len() >= longest {
                    // Mid-word, perhaps; better than Whisper dropping the end of a long phrase.
                    let b = self.building.replace(Building { samples: Vec::new(), quiet: 0, loud: 0, drafted: false }).unwrap();
                    if b.drafted {
                        out.push(Cut::Resumed);
                    }
                    out.push(Cut::Phrase(b.samples));
                    out.push(Cut::Began);
                } else if b.quiet == (DRAFT_MS / WINDOW_MS) as usize && !b.drafted && speech(b) && self.pause_windows > b.quiet {
                    b.drafted = true;
                    out.push(Cut::Draft(ending(b, self.window)));
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

    /// Everything the cutter noticed, for parts of noise at a loudness each.
    fn cuts(parts: &[(u32, f32)]) -> Vec<Cut> {
        let mut seed = 7;
        let mut p = Phrases::new(RATE, 700);
        parts.iter().flat_map(|&(ms, db)| p.listen(&noise(ms, db, &mut seed))).collect()
    }

    fn kinds(cuts: &[Cut]) -> Vec<&'static str> {
        cuts.iter()
            .map(|c| match c {
                Cut::Began => "began",
                Cut::Draft(_) => "draft",
                Cut::Resumed => "resumed",
                Cut::Phrase(_) => "phrase",
                Cut::Dropped => "dropped",
            })
            .collect()
    }

    #[test]
    fn a_phrase_that_ends_as_drafted_is_the_draft() {
        let c = cuts(&[(1000, -60.0), (1000, -20.0), (1500, -60.0)]);
        assert_eq!(kinds(&c), ["began", "draft", "phrase"]);
        let (Cut::Draft(draft), Cut::Phrase(phrase)) = (&c[1], &c[2]) else { unreachable!() };
        assert_eq!(draft, phrase);
    }

    #[test]
    fn speech_going_on_takes_the_draft_back() {
        let c = cuts(&[(800, -60.0), (600, -20.0), (400, -60.0), (600, -20.0), (1500, -60.0)]);
        assert_eq!(kinds(&c), ["began", "draft", "resumed", "draft", "phrase"]);
        let (Cut::Draft(second), Cut::Phrase(phrase)) = (&c[3], &c[4]) else { unreachable!() };
        assert_eq!(second, phrase);
    }

    #[test]
    fn a_click_begins_and_comes_to_nothing() {
        assert_eq!(kinds(&cuts(&[(800, -60.0), (80, -10.0), (1500, -60.0)])), ["began", "dropped"]);
    }

    #[test]
    fn whispers_favourites_are_nothing() {
        assert!(is_hallucination("Terima kasih telah menonton!"));
        assert!(is_hallucination(" Thank you. "));
        assert!(!is_hallucination("Terima kasih, sudah aku kirim ke email."));
    }
}
