//! SpatiPal's voice: sentences said through hemera's speech model, played into the room under the
//! name the room places at its figure (`spatipal-voice`), and a soft tone while the larger model
//! thinks.
//!
//! Two threads. One asks hemera for each sentence as it comes, so the next is being made while
//! this one is heard; the other plays at the pace sound is heard, because the room drops what is
//! handed it more than a third of a second ahead -- and hemera makes speech two and a half times
//! faster than it is spoken. The second also keeps the time the last sound will have been heard
//! by, for the ears: what is heard while it speaks is its own voice, not the wearer's.
//!
//! **The voice shares its GPU with the models.** Alone it makes speech 2.5 times faster than it is
//! spoken; while the small model writes, 0.7 times; while the larger one does, 0.6 (measured
//! 2026-10-30), and played as it came it stuttered. So a sentence is not begun until the model is
//! done writing, if that is a moment away -- most answers are a sentence or two -- and it is not
//! played until enough has been made that the rest will arrive before it is needed.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use spatiand_voice::speech::{self, Voice as Spoken, SPEECH_RATE};
use spatiand_voice::Hemera;

/// The name its sound is handed to the room under: the room places it at the figure.
pub const SOUND_KEY: &str = "spatipal-voice";

enum Line {
    Say(String),
    /// Drop what is waiting to be said.
    Hush,
}

enum Sound {
    /// A sentence begins, about this many seconds long.
    Begin(f32),
    Pcm(Vec<i16>),
    /// All of it has come.
    End,
    Hush,
}

/// How long a sentence waits for a model that is writing to be done.
const WAIT_FOR_MODEL: Duration = Duration::from_millis(600);
/// At least this much is made before it is played, seconds.
const LEAST_AHEAD: f32 = 0.25;
/// And at most this much, whatever it looks like: the wearer is waiting.
const MOST_AHEAD: f32 = 4.0;
/// How long a slice is handed to the room at a time, in samples: it drops what is more than a
/// third of a second ahead, and speech comes in pieces of up to two thirds.
const SLICE: usize = SPEECH_RATE as usize / 20;

/// The voice, to be told what to say. Cheap to clone.
#[derive(Clone)]
pub struct Voice {
    lines: Sender<Line>,
    shared: Arc<Shared>,
}

struct Shared {
    /// When the last sound handed over will have been heard, in ms since `epoch`.
    quiet_at: AtomicU64,
    epoch: Instant,
    thinking: AtomicBool,
    spoken: AtomicBool,
    /// Sentences asked for and not yet heard.
    waiting: AtomicU64,
    voice: Mutex<Spoken>,
    hemera: Mutex<Hemera>,
    /// When the wearer stopped speaking, until the first sound of the answer: for the log.
    mark: Mutex<Option<Instant>>,
    /// A model is writing, on the GPU the voice needs.
    generating: AtomicBool,
}

impl Voice {
    pub fn start(hemera: Hemera, voice: Spoken, spoken: bool) -> Voice {
        let (lines, said) = channel::<Line>();
        let (sounds, heard) = channel::<Sound>();
        let shared = Arc::new(Shared {
            quiet_at: AtomicU64::new(0),
            epoch: Instant::now(),
            thinking: AtomicBool::new(false),
            spoken: AtomicBool::new(spoken),
            waiting: AtomicU64::new(0),
            voice: Mutex::new(voice),
            hemera: Mutex::new(hemera),
            mark: Mutex::new(None),
            generating: AtomicBool::new(false),
        });
        let s = shared.clone();
        std::thread::Builder::new().name("voice-say".into()).spawn(move || speak(said, sounds, s)).expect("a thread for the voice");
        let s = shared.clone();
        std::thread::Builder::new().name("voice-play".into()).spawn(move || play(heard, s)).expect("a thread for the voice");
        Voice { lines, shared }
    }

    /// Say this, after what is already being said. Nothing when the voice is off.
    pub fn say(&self, text: &str) {
        let text = text.trim();
        if text.is_empty() || !self.shared.spoken.load(Ordering::Relaxed) {
            return;
        }
        self.shared.waiting.fetch_add(1, Ordering::Relaxed);
        let _ = self.lines.send(Line::Say(text.to_string()));
    }

    /// Stop talking: what is waiting is not said.
    pub fn hush(&self) {
        let _ = self.lines.send(Line::Hush);
    }

    /// Whether a model is writing now: what is to be said gives it a moment to finish.
    pub fn set_generating(&self, on: bool) {
        self.shared.generating.store(on, Ordering::Relaxed);
    }

    /// A soft tone while the larger model thinks, when nothing is being said.
    pub fn set_thinking(&self, on: bool) {
        self.shared.thinking.store(on, Ordering::Relaxed);
    }

    pub fn set_spoken(&self, on: bool) {
        self.shared.spoken.store(on, Ordering::Relaxed);
        if !on {
            self.hush();
        }
    }

    pub fn set_voice(&self, voice: Spoken, hemera: Hemera) {
        *self.shared.voice.lock().unwrap() = voice;
        *self.shared.hemera.lock().unwrap() = hemera;
    }

    /// Whether it is talking, or has something waiting to be said, or was heard less than a
    /// moment ago: the ears ignore the microphone then.
    pub fn busy(&self) -> bool {
        let now = self.shared.epoch.elapsed().as_millis() as u64;
        self.shared.waiting.load(Ordering::Relaxed) > 0 || now < self.shared.quiet_at.load(Ordering::Relaxed) + 350
    }

    /// Whether its voice is being heard now -- not only waiting to be made: the figure's mouth
    /// moves then and only then.
    pub fn speaking(&self) -> bool {
        let now = self.shared.epoch.elapsed().as_millis() as u64;
        now < self.shared.quiet_at.load(Ordering::Relaxed)
    }

    /// When the last thing it said will have been heard by, if it said anything: a follow-up
    /// without the name counts from then.
    pub fn quiet_at(&self) -> Option<Instant> {
        let at = self.shared.quiet_at.load(Ordering::Relaxed);
        (at > 0).then(|| self.shared.epoch + Duration::from_millis(at))
    }

    /// The wearer stopped speaking at `ended`: the first sound after this is logged with how long
    /// it took to come.
    pub fn mark(&self, ended: Instant) {
        *self.shared.mark.lock().unwrap() = Some(ended);
    }

    /// A voice that says nothing and plays nothing: for tests.
    #[cfg(test)]
    pub fn silent() -> Voice {
        let (lines, _) = channel::<Line>();
        let shared = Arc::new(Shared {
            quiet_at: AtomicU64::new(0),
            epoch: Instant::now(),
            thinking: AtomicBool::new(false),
            spoken: AtomicBool::new(false),
            waiting: AtomicU64::new(0),
            voice: Mutex::new(Spoken::default()),
            hemera: Mutex::new(Hemera::new("http://localhost:9")),
            mark: Mutex::new(None),
            generating: AtomicBool::new(false),
        });
        Voice { lines, shared }
    }
}

fn speak(lines: Receiver<Line>, sounds: Sender<Sound>, shared: Arc<Shared>) {
    while let Ok(line) = lines.recv() {
        match line {
            Line::Hush => {
                // Everything already waiting goes with it.
                while let Ok(Line::Say(_)) = lines.try_recv() {
                    shared.waiting.fetch_sub(1, Ordering::Relaxed);
                }
                let _ = sounds.send(Sound::Hush);
            }
            Line::Say(text) => {
                let asked = Instant::now();
                while shared.generating.load(Ordering::Relaxed) && asked.elapsed() < WAIT_FOR_MODEL {
                    std::thread::sleep(Duration::from_millis(10));
                }
                let voice = shared.voice.lock().unwrap().clone();
                let hemera = shared.hemera.lock().unwrap().clone();
                let said = spoken_form(&text);
                let _ = sounds.send(Sound::Begin(said.chars().count() as f32 / 17.0));
                let result = speech::say(&hemera, &voice, &said, |pcm| sounds.send(Sound::Pcm(pcm.to_vec())).is_ok());
                let _ = sounds.send(Sound::End);
                if let Err(why) = result {
                    log::warn!("voice: {why}");
                }
                shared.waiting.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }
}

/// Text as it is to be said: what is only for reading -- code marks, bullets, links -- left out.
fn spoken_form(text: &str) -> String {
    text.replace(['*', '`', '#', '_'], "").replace("\u{2014}", ", ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn play(heard: Receiver<Sound>, shared: Arc<Shared>) {
    let group = spatiand_media::SoundGroup::keyed(SOUND_KEY);
    let mut stream = match spatiand_media::Stream::open_in(&group, 1) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("voice: no way to be heard: {e}");
            for _ in heard {}
            return;
        }
    };
    // The moment the sound handed over so far will have been heard by.
    let mut heard_by = Instant::now();
    let mut next_tone = Instant::now();
    let seconds_of = |n: usize| n as f32 / SPEECH_RATE as f32;
    // What has come and not been handed over; whether it is being held to let more come.
    let mut waiting: std::collections::VecDeque<i16> = std::collections::VecDeque::new();
    let mut holding = true;
    // The sentence being made: how long it should be, how much has come, since when, and whether
    // all of it has.
    let (mut expected, mut came, mut first_came) = (0.0f32, 0.0f32, None::<(Instant, f32)>);
    let mut all_came = true;
    loop {
        match heard.recv_timeout(Duration::from_millis(10)) {
            Ok(Sound::Begin(seconds)) => {
                (expected, came, first_came, all_came) = (seconds, 0.0, None, false);
                // Going on from what is playing, there is nothing to hold; else, enough first.
                holding = waiting.is_empty() && Instant::now() >= heard_by;
            }
            Ok(Sound::Pcm(pcm)) => {
                let now = Instant::now();
                came += seconds_of(pcm.len());
                first_came.get_or_insert((now, came));
                waiting.extend(pcm);
            }
            Ok(Sound::End) => {
                all_came = true;
                holding = false;
            }
            Ok(Sound::Hush) => {
                // What was handed over is heard out; nothing more is.
                waiting.clear();
                heard_by = Instant::now();
                holding = true;
                all_came = true;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
        let now = Instant::now();
        if holding {
            let rate = first_came.and_then(|(at, then)| {
                let time = now.duration_since(at).as_secs_f32();
                (time > 0.05).then(|| (came - then) / time)
            });
            holding = !enough(seconds_of(waiting.len()), (expected - came).max(0.0), rate);
        }
        if !holding {
            // Not more than a fifth of a second ahead of the ear.
            while !waiting.is_empty() && heard_by.saturating_duration_since(now) < Duration::from_millis(200) {
                let slice: Vec<f32> = waiting.drain(..SLICE.min(waiting.len())).map(|s| s as f32 / 32768.0).collect();
                if heard_by < now {
                    heard_by = now;
                }
                stream.push(&slice, 1, SPEECH_RATE);
                if let Some(ended) = shared.mark.lock().unwrap().take() {
                    log::info!("voice: first sound {:.2} s after the wearer stopped speaking", ended.elapsed().as_secs_f32());
                }
                heard_by += Duration::from_secs_f64(slice.len() as f64 / SPEECH_RATE as f64);
                next_tone = heard_by + Duration::from_millis(900);
            }
            // Run dry with more to come: hold again rather than stutter on.
            if waiting.is_empty() && !all_came && now >= heard_by {
                log::info!("voice: ran dry, holding for more ({:.1} s of about {expected:.1} s came)", came);
                holding = true;
            }
        }
        if heard_by > shared.epoch {
            let at = heard_by + Duration::from_secs_f32(seconds_of(waiting.len()));
            shared.quiet_at.store(at.saturating_duration_since(shared.epoch).as_millis() as u64, Ordering::Relaxed);
        }
        if waiting.is_empty() && shared.thinking.load(Ordering::Relaxed) && shared.waiting.load(Ordering::Relaxed) == 0 && now >= next_tone && now >= heard_by {
            let tone = thinking_tone();
            stream.push(&tone, 1, SPEECH_RATE);
            heard_by = now + Duration::from_secs_f64(tone.len() as f64 / SPEECH_RATE as f64);
            next_tone = now + Duration::from_millis(1800);
        }
    }
}

/// Whether what has been made is enough to start playing: `ahead` seconds of it, `left` still to
/// come at `rate` seconds of speech a second (`None` while too little has come to tell). It lasts
/// `ahead + left` seconds, and the rest takes `left / rate` to come.
fn enough(ahead: f32, left: f32, rate: Option<f32>) -> bool {
    if ahead < LEAST_AHEAD {
        return false;
    }
    if ahead >= MOST_AHEAD {
        return true;
    }
    match rate {
        Some(rate) if rate > 0.01 => ahead + left >= left / rate * 1.1,
        _ => false,
    }
}

/// Two soft notes, rising: "hmm?", a figure thinking. Quiet, and shaped so neither note clicks.
fn thinking_tone() -> Vec<f32> {
    let rate = SPEECH_RATE as f32;
    let mut out = Vec::new();
    for (frequency, length) in [(523.25f32, 0.16f32), (659.25, 0.22)] {
        let n = (length * rate) as usize;
        for i in 0..n {
            let t = i as f32 / rate;
            let envelope = (std::f32::consts::PI * i as f32 / n as f32).sin().powi(2);
            out.push(0.05 * envelope * (2.0 * std::f32::consts::PI * frequency * t).sin());
        }
        out.extend(std::iter::repeat(0.0).take((0.05 * rate) as usize));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thinking_tone_is_quiet_and_starts_and_ends_at_rest() {
        let t = thinking_tone();
        assert!(t.iter().all(|s| s.abs() <= 0.05));
        assert!(t[0].abs() < 1e-3 && t.last().unwrap().abs() < 1e-3);
    }

    #[test]
    fn it_plays_once_the_rest_will_come_in_time() {
        // Alone on the GPU: as soon as there is anything to speak of.
        assert!(!enough(0.16, 4.8, None));
        assert!(enough(0.3, 4.7, Some(2.5)));
        // A model writing beside it, at 0.7: the 5 s sentence wants about 2 s made first.
        assert!(!enough(0.5, 4.5, Some(0.7)));
        assert!(enough(2.0, 3.0, Some(0.7)));
        // But the wearer is not kept waiting for ever.
        assert!(enough(4.0, 20.0, Some(0.1)));
    }

    #[test]
    fn what_is_only_for_reading_is_not_said() {
        assert_eq!(spoken_form("**Siap** \u{2014} `ls` sudah"), "Siap , ls sudah");
    }
}
