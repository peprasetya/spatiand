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
//! **One voice.** A voice described in words is made into a recorded voice once and every
//! sentence is said in that (`spatiand_voice::speech::made_voice`): given the description with
//! each sentence, the speech model made up a new voice for every one, and a long answer changed
//! voice from sentence to sentence (the owner, 2026-11-15). Each sentence's voice is in the log.
//!
//! **What it says is kept** ([`Played`]), sample by sample with when it is heard, so that the
//! ears can take it back out of the microphone and hear the wearer over it (`crate::ears`) --
//! what the room really played: not the soft tone while the larger model thinks, nor what a cut
//! dropped. [`Voice::interrupt`] stops it mid-word: what is waiting, what is being made, and what
//! the room has not yet played.
//!
//! **The voice shares its GPU with the models.** Alone it makes speech 2.5 times faster than it is
//! spoken; while the small model writes, 0.7 times; while the larger one does, 0.6 (measured
//! 2026-10-30), and played as it came it stuttered. So a sentence is not begun until the model is
//! done writing, if that is a moment away -- most answers are a sentence or two -- and it is not
//! played until enough has been made that the rest will arrive before it is needed.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spatiand_voice::echo::Played;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use spatiand_voice::speech::{self, Voice as Spoken, SPEECH_RATE};
use spatiand_voice::Hemera;

/// The name its sound is handed to the room under: the room places it at the figure.
pub const SOUND_KEY: &str = "spatipal-voice";

enum Line {
    /// What to say, and how many interruptions there had been when it was asked for: one since,
    /// and it is not said. (A hush needs no line of its own: what waits is dropped by this
    /// number as it comes up, and what was asked after the hush is said.)
    Say(String, u64),
}

/// To the player, each marked with the interruptions there had been when it was made: sound of
/// an older number than the last cut is not played.
enum Sound {
    /// A sentence begins, about this many seconds long.
    Begin(f32, u64),
    Pcm(Vec<i16>, u64),
    /// All of it has come.
    End(u64),
    /// Stop at once: what the room has not yet played is not played, nor anything made before
    /// this cut (its number) that is still on its way. Sent by [`Voice::hush`] itself, so that
    /// it is heard at once, whatever the sentences' thread is waiting on.
    Cut(u64),
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
/// What the room's output already has when it is told to drop the rest: still heard.
const HEARD_ANYWAY: Duration = Duration::from_millis(30);

/// The voice, to be told what to say. Cheap to clone.
#[derive(Clone)]
pub struct Voice {
    lines: Sender<Line>,
    sounds: Sender<Sound>,
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
    /// Counted up by each interruption: a sentence being made when it changes stops.
    cuts: AtomicU64,
    /// What was handed to the room, and when it is heard.
    played: Arc<Mutex<Played>>,
    /// The voice a description was made into, for the description and model it was made from.
    made: Mutex<Option<(String, String, String)>>,
    /// What it said, or was about to, the last half minute: the ears check what they heard over
    /// it against this, in case it was its own voice after all.
    said: Mutex<std::collections::VecDeque<(Instant, String)>>,
}

impl Voice {
    pub fn start(hemera: Hemera, voice: Spoken, spoken: bool) -> Voice {
        let (lines, said) = channel::<Line>();
        let (sounds, heard) = channel::<Sound>();
        let epoch = Instant::now();
        let shared = Arc::new(Shared {
            quiet_at: AtomicU64::new(0),
            epoch,
            thinking: AtomicBool::new(false),
            spoken: AtomicBool::new(spoken),
            waiting: AtomicU64::new(0),
            voice: Mutex::new(voice),
            hemera: Mutex::new(hemera),
            mark: Mutex::new(None),
            generating: AtomicBool::new(false),
            cuts: AtomicU64::new(0),
            played: Arc::new(Mutex::new(Played::new(epoch))),
            made: Mutex::new(None),
            said: Mutex::new(std::collections::VecDeque::new()),
        });
        let s = shared.clone();
        let to_player = sounds.clone();
        std::thread::Builder::new().name("voice-say".into()).spawn(move || speak(said, sounds, s)).expect("a thread for the voice");
        let s = shared.clone();
        std::thread::Builder::new().name("voice-play".into()).spawn(move || play(heard, s)).expect("a thread for the voice");
        let voice = Voice { lines, sounds: to_player, shared };
        voice.prepare();
        voice
    }

    /// Have the voice made now, if it is a described one not made yet: the first sentence then
    /// does not wait for it.
    fn prepare(&self) {
        if !self.shared.spoken.load(Ordering::Relaxed) {
            return;
        }
        let shared = self.shared.clone();
        let _ = std::thread::Builder::new().name("voice-make".into()).spawn(move || {
            let hemera = shared.hemera.lock().unwrap().clone();
            let _ = one_voice(&shared, &hemera);
        });
    }

    /// Say this, after what is already being said. Nothing when the voice is off.
    pub fn say(&self, text: &str) {
        let text = text.trim();
        if text.is_empty() || !self.shared.spoken.load(Ordering::Relaxed) {
            return;
        }
        self.shared.waiting.fetch_add(1, Ordering::Relaxed);
        {
            let mut said = self.shared.said.lock().unwrap();
            said.push_back((Instant::now(), text.to_string()));
            while said.front().is_some_and(|(at, _)| at.elapsed() > Duration::from_secs(30)) {
                said.pop_front();
            }
        }
        let _ = self.lines.send(Line::Say(text.to_string(), self.shared.cuts.load(Ordering::Relaxed)));
    }

    /// What it said, or was about to, in the last half minute.
    pub fn lately_said(&self) -> String {
        self.shared.said.lock().unwrap().iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join(" ")
    }

    /// Stop talking: what is waiting is not said, nor the rest of what is being said.
    pub fn hush(&self) {
        let cut = self.shared.cuts.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.sounds.send(Sound::Cut(cut));
    }

    /// **Stop now**, mid-word: the wearer has begun speaking over it. The sentence being made is
    /// stopped, nothing waiting is said, and what the room has not played yet is dropped -- a
    /// few hundredths of a second already on their way are still heard.
    pub fn interrupt(&self) {
        self.shared.thinking.store(false, Ordering::Relaxed);
        self.hush();
    }

    /// What it has played, for the ears to take back out of the microphone.
    pub fn played(&self) -> Arc<Mutex<Played>> {
        self.shared.played.clone()
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
        self.prepare();
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
        let (sounds, _) = channel::<Sound>();
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
            cuts: AtomicU64::new(0),
            played: Arc::new(Mutex::new(Played::new(Instant::now()))),
            made: Mutex::new(None),
            said: Mutex::new(std::collections::VecDeque::new()),
        });
        Voice { lines, sounds, shared }
    }

    /// For tests: a line waiting to be said -- busy, and nothing heard -- or none.
    #[cfg(test)]
    pub fn pretend_waiting(&self, waiting: bool) {
        self.shared.waiting.store(waiting as u64, Ordering::Relaxed);
    }

    /// For tests: `pcm` (at the speech's rate) handed to the room to be heard from `at`, as the
    /// player hands it over.
    #[cfg(test)]
    pub fn pretend_played(&self, at: Instant, pcm: &[f32]) {
        self.shared.played.lock().unwrap().push(at, pcm, SPEECH_RATE);
        let end = at + Duration::from_secs_f64(pcm.len() as f64 / SPEECH_RATE as f64);
        self.shared.quiet_at.fetch_max(end.saturating_duration_since(self.shared.epoch).as_millis() as u64, Ordering::Relaxed);
    }

    /// For tests: how many times it was told to stop.
    #[cfg(test)]
    pub fn cuts(&self) -> u64 {
        self.shared.cuts.load(Ordering::Relaxed)
    }
}

fn speak(lines: Receiver<Line>, sounds: Sender<Sound>, shared: Arc<Shared>) {
    let mut sentences = 0u64;
    let cut_since = |cut: u64| shared.cuts.load(Ordering::Relaxed) != cut;
    while let Ok(line) = lines.recv() {
        match line {
            Line::Say(text, cut) => {
                if cut_since(cut) {
                    shared.waiting.fetch_sub(1, Ordering::Relaxed);
                    continue;
                }
                let asked = Instant::now();
                while shared.generating.load(Ordering::Relaxed) && asked.elapsed() < WAIT_FOR_MODEL && !cut_since(cut) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                let hemera = shared.hemera.lock().unwrap().clone();
                let voice = one_voice(&shared, &hemera);
                // Cut while it waited, or while its voice was being made: not begun.
                if cut_since(cut) {
                    shared.waiting.fetch_sub(1, Ordering::Relaxed);
                    continue;
                }
                let said = spoken_form(&text);
                sentences += 1;
                log::info!(
                    "voice: sentence {sentences} in {} ({}): {}",
                    voice.name.as_deref().map(|n| format!("the voice {n}")).unwrap_or_else(|| "a voice made up for it".into()),
                    voice.model,
                    crate::store::quoted(&said.chars().take(60).collect::<String>())
                );
                let _ = sounds.send(Sound::Begin(said.chars().count() as f32 / 17.0, cut));
                // `SPATIPAL_TEST_RECORD=<folder>`: each sentence's sound kept as a WAV file, for a
                // test to compare the voices of a long answer without anyone listening.
                let mut kept = std::env::var_os("SPATIPAL_TEST_RECORD").map(|_| Vec::new());
                let result = speech::say(&hemera, &voice, &said, |pcm| {
                    if let Some(k) = kept.as_mut() {
                        k.extend_from_slice(pcm);
                    }
                    !cut_since(cut) && sounds.send(Sound::Pcm(pcm.to_vec(), cut)).is_ok()
                });
                if let (Some(dir), Some(pcm)) = (std::env::var_os("SPATIPAL_TEST_RECORD"), kept) {
                    let path = std::path::Path::new(&dir).join(format!("sentence-{sentences:03}.wav"));
                    let _ = std::fs::write(&path, spatiand_voice::hemera::wav(&pcm, SPEECH_RATE));
                }
                let _ = sounds.send(Sound::End(cut));
                if let Err(why) = result {
                    log::warn!("voice: {why}");
                }
                shared.waiting.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }
}

/// The voice every sentence is said in: the one chosen from hemera's library, or the one the
/// description was made into -- made the first time it is wanted, then kept. If it cannot be made
/// (hemera unreachable, the library refusing it), the description is given with the sentence as
/// before, and the log says the voice may change: **a workaround** until it can be made, which
/// is tried again with the next sentence.
fn one_voice(shared: &Shared, hemera: &Hemera) -> Spoken {
    let mut voice = shared.voice.lock().unwrap().clone();
    let Some(described) = voice.described.clone().filter(|d| !d.trim().is_empty() && voice.name.as_deref().unwrap_or("").is_empty()) else {
        return voice;
    };
    let mut made = shared.made.lock().unwrap();
    if let Some((model, from, name)) = made.as_ref() {
        if *model == voice.model && *from == described {
            voice.name = Some(name.clone());
            return voice;
        }
    }
    match speech::made_voice(hemera, &voice.model, &described) {
        Ok(name) => {
            *made = Some((voice.model.clone(), described, name.clone()));
            voice.name = Some(name);
        }
        Err(why) => log::warn!("voice: the voice described could not be made ({why}); this sentence gets a voice of its own, which may not be the last one's"),
    }
    voice
}

/// Text as it is to be said: what is only for reading -- code marks, bullets, links -- left out.
fn spoken_form(text: &str) -> String {
    // A stage direction -- "*leans in, smiling*", as a model playing someone writes an action --
    // is not said: a pal's figure acts (its `appearance`), and its voice reading the words out was
    // the first answer heard in a pal's snapshot room test (2026-11-19). Doubled asterisks and a
    // lone one are left for the line below to take out.
    // A lone asterisk on each side -- not markdown's doubled ones -- with a few words between.
    // ('*' is one byte that is never part of a longer character, so bytes are safe to step by.)
    let b = text.as_bytes();
    let lone = |i: usize| b[i] == b'*' && (i == 0 || b[i - 1] != b'*') && b.get(i + 1) != Some(&b'*');
    let mut said = String::new();
    let mut from = 0;
    let mut i = 0;
    while i < b.len() {
        if lone(i) {
            if let Some(j) = (i + 1..b.len()).find(|&j| b[j] == b'*') {
                let between = &text[i + 1..j];
                if lone(j) && !between.trim().is_empty() && between.split_whitespace().count() <= 12 {
                    said.push_str(&text[from..i]);
                    from = j + 1;
                    i = j + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    said.push_str(&text[from..]);
    said.replace(['*', '`', '#', '_'], "").replace("\u{2014}", ", ").split_whitespace().collect::<Vec<_>>().join(" ")
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
    // The last cut: what was made before it is not played.
    let mut cut_at = 0u64;
    loop {
        match heard.recv_timeout(Duration::from_millis(10)) {
            Ok(Sound::Begin(_, n) | Sound::Pcm(_, n) | Sound::End(n)) if n < cut_at => {}
            Ok(Sound::Begin(seconds, _)) => {
                (expected, came, first_came, all_came) = (seconds, 0.0, None, false);
                // Going on from what is playing, there is nothing to hold; else, enough first.
                holding = waiting.is_empty() && Instant::now() >= heard_by;
            }
            Ok(Sound::Pcm(pcm, _)) => {
                let now = Instant::now();
                came += seconds_of(pcm.len());
                first_came.get_or_insert((now, came));
                waiting.extend(pcm);
            }
            Ok(Sound::End(_)) => {
                all_came = true;
                holding = false;
            }
            Ok(Sound::Cut(n)) => {
                cut_at = cut_at.max(n);
                // Not even what the room has yet to play.
                waiting.clear();
                stream.clear();
                heard_by = Instant::now();
                // And what the room dropped was not heard: kept as played, the canceller took
                // away an echo that never came -- "the canceller went wrong (4.5 dB added)" after
                // each interruption on the owner's Mac, 2026-11-19 -- and started again from
                // nothing. What was heard stays, for the canceller and the delay's matching.
                shared.played.lock().unwrap().cut(heard_by + HEARD_ANYWAY);
                holding = true;
                all_came = true;
                shared.quiet_at.store(heard_by.saturating_duration_since(shared.epoch).as_millis() as u64, Ordering::Relaxed);
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
                shared.played.lock().unwrap().push(heard_by, &slice, SPEECH_RATE);
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
            // Not kept with what it says: the tone is not its voice, and nothing is spoken over
            // it. Kept, the wearer could "speak over" a model thinking -- a key's click, under the
            // tone's echo (a pure tone, which repeats as a voice does), was taken for them -- the
            // delay was matched against two soft notes, and what was learnt of its voice from
            // them expected a roar from its next sentence.
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
    #[test]
    fn a_stage_direction_is_acted_not_said() {
        assert_eq!(super::spoken_form("There. *leans in, smiling* Now spin me around, darling."), "There. Now spin me around, darling.");
        assert_eq!(super::spoken_form("It is **really** late."), "It is really late.");
        assert_eq!(super::spoken_form("5 * 3 is 15."), "5 3 is 15.");
    }

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
