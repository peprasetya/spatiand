//! SpatiPal's ears: the room's microphone, cut into phrases at the pauses, each written by Whisper,
//! and kept only when it was meant for SpatiPal.
//!
//! **Meant for it** (the owner's choice): it starts with its name ("Hei Pixel, ..."), or it is
//! part of a conversation -- begun within [`FOLLOW_UP`] of the last thing it said, heard or
//! answered, so a follow-up needs no name -- or the wearer asked it to listen to the next thing
//! said. Everything else said in the room is heard and let go: it is written by hemera, which
//! keeps nothing, and goes no further.
//!
//! **When a phrase began is what counts**, not when Whisper's text came back: a follow-up begun
//! three seconds after the answer and spoken for four would otherwise arrive after the window
//! had shut, and be dropped without a word (the owner's test, 2026-10-29).
//!
//! **Written while the pause is still being made sure of**: at a third of a second of quiet the
//! phrase so far goes to Whisper as a draft ([`Cut::Draft`]); when the pause turns out to be the
//! end, the draft's text is the phrase's, half a second sooner than writing it after.
//!
//! **Not its own voice**: a phrase heard while it is speaking, or a moment after, is dropped.

use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use spatiand_voice::phrases::{is_hallucination, Cut, Phrases};
use spatiand_voice::{Hemera, VoiceSettings, RATE};

use crate::voice::Voice;

/// A phrase begun within this long of the last thing in a conversation needs no name.
pub const FOLLOW_UP: Duration = Duration::from_secs(8);
/// How long a pause ends a phrase. Longer than dictation's: a phrase cut in two would be two
/// requests, the second interrupting the answer to the first.
const PAUSE_MS: u32 = 800;

/// What the ears heard.
#[derive(Clone, Debug, PartialEq)]
pub enum Heard {
    /// What they are doing now, for the figure and the window.
    Ear(Ear),
    /// Something asked: the words after the name, or all of a follow-up; and when the wearer
    /// stopped speaking, to know how long the answer took.
    Asked { text: String, ended: Instant },
    /// Only the name: the wearer wants its attention.
    Called,
}

/// What the ears are doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ear {
    Quiet,
    /// Someone is speaking.
    Hearing,
    /// What was said is with Whisper.
    Writing,
}

/// The ears, while listening. Dropped, they stop.
pub struct Ears {
    stop: Arc<AtomicBool>,
    /// The next phrase counts, name or not: the listen button was pressed.
    pub once: Arc<AtomicBool>,
}

impl Drop for Ears {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// When SpatiPal was last in a conversation: the latest of an answer finished, a phrase meant for
/// it, its name answered. With when its voice last went quiet, that is how long a follow-up may
/// come without the name.
pub struct Conversation {
    epoch: Instant,
    /// In ms since `epoch`; 0 for never.
    at: AtomicU64,
}

impl Conversation {
    pub fn new() -> Conversation {
        Conversation { epoch: Instant::now(), at: AtomicU64::new(0) }
    }

    /// Something happened in the conversation just now.
    pub fn touch(&self) {
        self.at.store((self.epoch.elapsed().as_millis() as u64).max(1), Ordering::Relaxed);
    }

    /// Until when a phrase may begin and still be part of it.
    pub fn open_until(&self, voice: &Voice) -> Option<Instant> {
        let at = self.at.load(Ordering::Relaxed);
        let touched = (at > 0).then(|| self.epoch + Duration::from_millis(at));
        let latest = match (touched, voice.quiet_at()) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        latest.map(|at| at + FOLLOW_UP)
    }

    /// Whether a phrase begun now would be part of it: the figure shows it is listening for more.
    pub fn open(&self, voice: &Voice) -> bool {
        voice.busy() || self.open_until(voice).is_some_and(|until| Instant::now() < until)
    }
}

/// To the thread that judges what was said: from the microphone's thread, and from Whisper's
/// helpers as they finish. Each piece of writing is a job, by number.
enum Said {
    Began,
    /// What was heard came to nothing, or was its own voice.
    Gone,
    Draft { job: u64, samples: Vec<i16> },
    Resumed,
    /// A phrase, whole: its job is its draft's when it ended as drafted.
    Phrase { job: u64, drafted: bool, samples: Vec<i16>, began: Instant, ended: Instant },
    /// Whisper's text for a job.
    Written { job: u64, text: Result<(String, f32), String> },
}

/// Start listening to `microphone` (16 kHz mono 16-bit, from the room).
#[allow(clippy::too_many_arguments)]
pub fn start(
    microphone: std::fs::File,
    settings: VoiceSettings,
    name: Arc<Mutex<String>>,
    voice: Voice,
    conversation: Arc<Conversation>,
    heard: Sender<Heard>,
    wake: impl Fn() + Send + 'static,
) -> Ears {
    let stop = Arc::new(AtomicBool::new(false));
    let once = Arc::new(AtomicBool::new(false));
    let (said_tx, said_rx) = channel::<Said>();
    let written = said_tx.clone();
    let stopped = stop.clone();
    let busy = voice.clone();
    std::thread::Builder::new()
        .name("ears".into())
        .spawn(move || cut(microphone, busy, stopped, said_tx))
        .expect("a thread for the ears");

    let listen_once = once.clone();
    std::thread::Builder::new()
        .name("ears-judge".into())
        .spawn(move || {
            let hemera = Hemera::new(&settings.hemera);
            let mut ears = Judge {
                hemera,
                settings,
                name,
                voice,
                conversation,
                once: listen_once,
                heard,
                wake: Box::new(wake),
                written_tx: written,
                hearing: false,
                writing: None,
                next: None,
                written: None,
                waiting: None,
            };
            ears.run(said_rx);
        })
        .expect("a thread for the ears");
    Ears { stop, once }
}

/// The microphone, cut into what was said.
fn cut(mut microphone: std::fs::File, voice: Voice, stopped: Arc<AtomicBool>, said: Sender<Said>) {
    let mut cutter = Phrases::new(RATE, PAUSE_MS);
    let mut bytes = vec![0u8; (RATE as usize / 50) * 2];
    let mut began = Instant::now();
    let mut hearing = false;
    let mut jobs = 0u64;
    let mut draft = None;
    while !stopped.load(Ordering::Relaxed) {
        if microphone.read_exact(&mut bytes).is_err() {
            log::info!("ears: the room's microphone closed");
            return;
        }
        let samples: Vec<i16> = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        // Its own voice in the room: what is heard then is not the wearer's. Still fed, so the
        // room's noise goes on being learnt.
        if voice.busy() {
            let _ = cutter.listen(&samples);
            let _ = cutter.flush();
            if std::mem::take(&mut hearing) {
                draft = None;
                let _ = said.send(Said::Gone);
            }
            continue;
        }
        for cut in cutter.listen(&samples) {
            let sent = match cut {
                Cut::Began => {
                    began = Instant::now();
                    hearing = true;
                    said.send(Said::Began)
                }
                Cut::Draft(samples) => {
                    jobs += 1;
                    draft = Some(jobs);
                    said.send(Said::Draft { job: jobs, samples })
                }
                Cut::Resumed => {
                    draft = None;
                    said.send(Said::Resumed)
                }
                Cut::Phrase(samples) => {
                    hearing = false;
                    let ended = Instant::now() - Duration::from_millis(PAUSE_MS as u64);
                    let (job, drafted) = match draft.take() {
                        Some(job) => (job, true),
                        None => {
                            jobs += 1;
                            (jobs, false)
                        }
                    };
                    said.send(Said::Phrase { job, drafted, samples, began, ended })
                }
                Cut::Dropped => {
                    hearing = false;
                    said.send(Said::Gone)
                }
            };
            if sent.is_err() {
                return;
            }
        }
    }
}

/// What was said, written by Whisper and judged. Whisper writes on helper threads, one job at a
/// time, so that this one always knows at once what the ears are doing; of the drafts waiting to
/// be written only the latest is kept.
struct Judge {
    hemera: Hemera,
    settings: VoiceSettings,
    name: Arc<Mutex<String>>,
    voice: Voice,
    conversation: Arc<Conversation>,
    once: Arc<AtomicBool>,
    heard: Sender<Heard>,
    wake: Box<dyn Fn() + Send>,
    /// For Whisper's helpers to answer by.
    written_tx: Sender<Said>,
    /// Someone is speaking.
    hearing: bool,
    /// The job Whisper is writing.
    writing: Option<u64>,
    /// The job to write next.
    next: Option<(u64, Vec<i16>)>,
    /// The last job written.
    written: Option<(u64, Result<(String, f32), String>)>,
    /// A phrase waiting for its job's text: the job, when it began and ended, how long it was,
    /// whether its draft is what is written.
    waiting: Option<(u64, Instant, Instant, f32, bool)>,
}

impl Judge {
    fn run(&mut self, said: Receiver<Said>) {
        while let Ok(next) = said.recv() {
            let listened = match next {
                Said::Began => {
                    self.hearing = true;
                    self.tell(Heard::Ear(Ear::Hearing))
                }
                Said::Gone => {
                    self.hearing = false;
                    self.next = None;
                    self.waiting = None;
                    self.tell(Heard::Ear(Ear::Quiet))
                }
                Said::Resumed => {
                    // The draft waiting to be written is not the phrase; one being written is let
                    // finish, and forgotten.
                    self.next = None;
                    true
                }
                Said::Draft { job, samples } => {
                    self.next = Some((job, samples));
                    self.start_writing();
                    true
                }
                Said::Phrase { job, drafted, samples, began, ended } => {
                    self.hearing = false;
                    let seconds = samples.len() as f32 / RATE as f32;
                    self.waiting = Some((job, began, ended, seconds, drafted));
                    match &self.written {
                        Some((done, _)) if *done == job => {}
                        _ => {
                            if self.writing != Some(job) && !(drafted && self.next.as_ref().is_some_and(|(n, _)| *n == job)) {
                                self.next = Some((job, samples));
                            }
                            self.start_writing();
                        }
                    }
                    let ready = self.written.as_ref().is_some_and(|(done, _)| *done == job);
                    if ready {
                        self.finish()
                    } else {
                        self.tell(Heard::Ear(Ear::Writing))
                    }
                }
                Said::Written { job, text } => {
                    self.writing = None;
                    self.written = Some((job, text));
                    self.start_writing();
                    if self.waiting.is_some_and(|(waited, ..)| waited == job) {
                        self.finish()
                    } else {
                        true
                    }
                }
            };
            if !listened {
                return;
            }
        }
    }

    /// Whisper writes the next job, if it is free.
    fn start_writing(&mut self) {
        if self.writing.is_some() {
            return;
        }
        let Some((job, samples)) = self.next.take() else { return };
        self.writing = Some(job);
        let hemera = self.hemera.clone();
        let model = self.settings.transcription_model.clone();
        let language = self.settings.whisper_language().map(str::to_string);
        let prompt = prompt(&self.name.lock().unwrap(), &self.settings);
        let answer = self.written_tx.clone();
        let spawned = std::thread::Builder::new().name("ears-write".into()).spawn(move || {
            let started = Instant::now();
            let text = hemera.transcribe_surely(&model, &samples, RATE, language.as_deref(), &prompt);
            log::debug!("ears: job {job} written in {:.2} s", started.elapsed().as_secs_f32());
            let _ = answer.send(Said::Written { job, text });
        });
        if spawned.is_err() {
            self.writing = None;
        }
    }

    /// The phrase waited for has its text: judge it. `false` when no one is listening any more.
    fn finish(&mut self) -> bool {
        let Some((job, began, ended, seconds, drafted)) = self.waiting.take() else { return true };
        let Some((_, text)) = self.written.take().filter(|(done, _)| *done == job) else { return true };
        log::info!(
            "ears: {seconds:.1} s of speech written {:.2} s after it ended{}",
            ended.elapsed().as_secs_f32(),
            if drafted { ", from its draft" } else { "" }
        );
        self.judge(text, began, ended, seconds)
    }

    /// Whether what was said was meant for SpatiPal, and if so, hand it on. `false` when no one
    /// is listening any more.
    fn judge(&mut self, text: Result<(String, f32), String>, began: Instant, ended: Instant, seconds: f32) -> bool {
        // What the ears are doing now: someone may have begun speaking again meanwhile.
        let now = if self.hearing { Ear::Hearing } else { Ear::Quiet };
        let text = match text {
            Ok((t, sure)) => match made_up(&t, seconds, sure) {
                None => t,
                Some(why) => {
                    if !t.is_empty() {
                        log::info!("ears: let go, {why}: {t:?} ({seconds:.1} s, sure {sure:.2})");
                    }
                    return self.tell(Heard::Ear(now));
                }
            },
            Err(why) => {
                log::warn!("ears: {why}");
                return self.tell(Heard::Ear(now));
            }
        };
        let called = self.name.lock().unwrap().clone();
        let in_conversation = self.conversation.open_until(&self.voice).is_some_and(|until| began <= until);
        let meant = meant(&text, &called, self.once.swap(false, Ordering::Relaxed) || in_conversation);
        let Some(meant) = meant else {
            log::debug!("ears: not for us: {text:?}");
            return self.tell(Heard::Ear(now));
        };
        self.conversation.touch();
        log::info!("ears: heard {meant:?}{}", if in_conversation { " (in the conversation)" } else { "" });
        let meant = match meant {
            Heard::Asked { text, .. } => Heard::Asked { text, ended },
            other => other,
        };
        // The ears first: the figure goes from hearing straight to what the asking makes it.
        self.tell(Heard::Ear(now)) && self.tell(meant)
    }

    fn tell(&self, heard: Heard) -> bool {
        let gone = self.heard.send(heard).is_err();
        (self.wake)();
        !gone
    }
}

/// What Whisper is given to lean on: its name as the wearer says it -- calling it, at the start of
/// a sentence -- before a sentence in the way they speak.
///
/// **Not the name last**: Whisper given noise reads back the end of its prompt, and "Pixel, can you
/// check the log?" there was heard, over and over, in a silent room (2026-10-30). And not the name
/// alone: given "Pixel." it left the name out of "Pixel, write a short Python function...".
fn prompt(name: &str, settings: &VoiceSettings) -> String {
    let indonesian = settings.languages.iter().any(|l| l == "id");
    let first = if indonesian { format!("{name}, buka browser.") } else { format!("{name}, open the browser.") };
    format!("{first} {name}, what do you think? {}", settings.primer())
}

/// Why this is not something said, if it is not: Whisper writes something for anything it is given.
///
/// - One of its favourites.
/// - Words it barely believes (noise came back at 0.06-0.2, speech at 0.88 or more).
/// - Far too few for the time they took: "Okay." from 25 s of a silent room.
/// - One sentence over and over, which is how it runs away with a sentence from its prompt.
fn made_up(text: &str, seconds: f32, sure: f32) -> Option<&'static str> {
    if is_hallucination(text) {
        return Some("one of Whisper's favourites");
    }
    if sure < 0.4 {
        return Some("Whisper barely believes it");
    }
    let letters = text.chars().filter(|c| c.is_alphanumeric()).count();
    if seconds > 2.0 && (letters as f32) < 1.5 * seconds {
        return Some("too few words for the time");
    }
    let lower = text.to_lowercase();
    let sentences: Vec<&str> = lower.split(['.', '?', '!']).map(str::trim).filter(|s| !s.is_empty()).collect();
    if sentences.iter().any(|s| sentences.iter().filter(|o| o == &s).count() >= 3) {
        return Some("one sentence over and over");
    }
    None
}

/// What `text` asks of a figure called `name`: anything at all when it is in a conversation (or
/// was asked to listen), else only what begins with its name.
fn meant(text: &str, name: &str, in_conversation: bool) -> Option<Heard> {
    match addressed(text, name) {
        Some(rest) if rest.is_empty() => Some(Heard::Called),
        Some(rest) => Some(Heard::Asked { text: rest, ended: Instant::now() }),
        None if in_conversation => Some(Heard::Asked { text: text.to_string(), ended: Instant::now() }),
        None => None,
    }
}

/// Whether `text` begins with `name` (after a greeting, perhaps): the rest of it, if so.
///
/// By sound as well as by letters: Whisper writes "Pixel" as "Piksel" when the sentence around it
/// is Indonesian, so both are put into one spelling first (`x` as `ks`, `ph` as `f`, ...).
pub fn addressed(text: &str, name: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let plain = |w: &str| -> String { sounded(&w.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>()) };
    let wanted = plain(name);
    if wanted.is_empty() {
        return None;
    }
    const GREETINGS: &[&str] = &["hei", "hey", "hai", "hi", "halo", "hallo", "hello", "oke", "ok", "okay", "eh", "ya", "yo"];
    let mut at = 0;
    while at < words.len() && GREETINGS.contains(&plain(words[at]).as_str()) && at < 2 {
        at += 1;
    }
    // The name may come as two words ("Pi xel") or one.
    for n in [1usize, 2] {
        if at + n > words.len() {
            continue;
        }
        let said: String = words[at..at + n].iter().map(|w| plain(w)).collect();
        if near(&said, &wanted, n) {
            let rest = words[at + n..].join(" ");
            let rest = rest.trim_start_matches([',', '.', '!', '?', ':', ';', ' ']).trim().to_string();
            return Some(rest);
        }
    }
    None
}

/// One spelling for what sounds alike, in English and Indonesian.
fn sounded(w: &str) -> String {
    w.replace("ph", "f").replace('x', "ks").replace("ck", "k").replace('q', "k").replace('c', "k").replace('v', "f").replace('z', "s")
}

/// Near enough to be the name: a letter off, two for a long name said as one word. Said as two
/// words, only a letter off -- "pick sale" is not "Pixel".
fn near(a: &str, b: &str, words: usize) -> bool {
    if a == b {
        return true;
    }
    let len = a.chars().count().max(b.chars().count());
    let allowed = if words == 1 && len >= 7 { 2 } else { 1 };
    len >= 4 && distance(a, b) <= allowed
}

fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut diagonal = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let above = row[j];
            row[j] = (diagonal + (a[i - 1] != b[j - 1]) as usize).min(row[j - 1] + 1).min(above + 1);
            diagonal = above;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_name_at_the_start_is_for_it() {
        assert_eq!(addressed("Hei Pixel, buka SpatiQuill.", "Pixel").as_deref(), Some("buka SpatiQuill."));
        assert_eq!(addressed("Piksel tolong ingatkan aku", "Pixel").as_deref(), Some("tolong ingatkan aku"));
        assert_eq!(addressed("Pixel.", "Pixel").as_deref(), Some(""));
        assert_eq!(addressed("Oke Pixel open the browser", "Pixel").as_deref(), Some("open the browser"));
    }

    #[test]
    fn its_name_elsewhere_is_not() {
        assert_eq!(addressed("Aku suka pixel art", "Pixel"), None);
        assert_eq!(addressed("Hei, ini pixel", "Pixel"), None);
        assert_eq!(addressed("Pick sale itu murah", "Pixel"), None);
    }

    #[test]
    fn in_a_conversation_anything_said_is_for_it() {
        let asked = |h: Option<Heard>| match h {
            Some(Heard::Asked { text, .. }) => Some(text),
            _ => None,
        };
        assert_eq!(asked(meant("and the browser too", "Pixel", true)).as_deref(), Some("and the browser too"));
        assert_eq!(meant("and the browser too", "Pixel", false), None);
        assert_eq!(asked(meant("Pixel, buka browser", "Pixel", false)).as_deref(), Some("buka browser"));
        assert_eq!(meant("Pixel?", "Pixel", true), Some(Heard::Called));
    }

    #[test]
    fn whisper_hears_the_name_as_it_is_said() {
        let both = VoiceSettings::default();
        assert_eq!(
            prompt("Pixel", &both),
            format!("Pixel, buka browser. Pixel, what do you think? {}", both.primer())
        );
        let english = VoiceSettings { languages: vec!["en".into()], ..Default::default() };
        assert!(prompt("Pixel", &english).starts_with("Pixel, open the browser. "));
    }

    #[test]
    fn what_whisper_makes_of_a_silent_room_is_let_go() {
        // As they came from the owner's room, 2026-10-30.
        assert!(made_up("Okay.", 25.0, 0.73).is_some());
        assert!(made_up("the", 3.0, 0.9).is_some());
        assert!(made_up("Pixel, can you check the log? Pixel, can you check the log? Pixel, can you check the log?", 25.0, 0.9).is_some());
        assert!(made_up("Oke.", 1.0, 0.06).is_some());
        // What people say.
        assert_eq!(made_up("Oke.", 0.8, 0.9), None);
        assert_eq!(made_up("Pixel, can you check the log?", 2.0, 0.95), None);
        assert_eq!(made_up("ya", 1.2, 0.7), None);
    }

    #[test]
    fn a_follow_up_counts_from_when_it_began() {
        let voice = Voice::silent();
        let c = Conversation::new();
        assert!(c.open_until(&voice).is_none());
        c.touch();
        let until = c.open_until(&voice).unwrap();
        // Begun just inside the window: in the conversation, however long Whisper then takes.
        assert!(Instant::now() + FOLLOW_UP - Duration::from_millis(100) <= until);
        assert!(Instant::now() + FOLLOW_UP + Duration::from_millis(100) > until);
        assert!(c.open(&voice));
    }
}
