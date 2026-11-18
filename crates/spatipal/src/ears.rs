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
//! **Not its own voice, but the wearer over it**: what it plays is taken back out of the
//! microphone (`spatiand_voice::echo`: a canceller against the very samples it played, and a
//! gate that knows what is left of them), so the wearer is heard while it speaks. Speaking over
//! it stops it at once -- its voice, what it was about to say, the model writing the rest -- and
//! what they say is heard from where they began ([`Heard::Interrupted`]; the owner, 2026-11-15:
//! "let user interrupt with voice if the spatipal is speaking"). Anything else heard while it
//! speaks is let go, as before.

use std::collections::VecDeque;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use spatiand_voice::echo::{Echo, MicClock, SimulatedRoom};
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
    /// Something asked: the words after the name, or all of a follow-up; when the wearer
    /// stopped speaking, to know how long the answer took; and whether it was begun in the
    /// conversation (a follow-up goes on in its chat, however long ago that chat began).
    Asked { text: String, ended: Instant, follow_up: bool },
    /// Only the name: the wearer wants its attention.
    Called,
    /// The wearer began speaking over it: it has stopped speaking, and stops answering.
    Interrupted,
    /// A phrase was heard and let go: not meant for it, not words, or not written.
    LetGo,
    /// What was said to it could not be written (Whisper or hemera failing): for the window's
    /// trouble line.
    Trouble(String),
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
    /// The wearer is speaking over its voice.
    Interrupted,
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
                troubled: None,
            };
            ears.run(said_rx);
        })
        .expect("a thread for the ears");
    Ears { stop, once }
}

/// The microphone, cut into what was said -- with its own voice taken out of it first
/// (`spatiand_voice::echo`), and the wearer heard over it: speaking while it speaks stops it.
fn cut(mut microphone: std::fs::File, voice: Voice, stopped: Arc<AtomicBool>, said: Sender<Said>) {
    let mut cutter = Cutter { phrases: Phrases::new(RATE, PAUSE_MS), began: Instant::now(), hearing: false, jobs: 0, draft: None, said };
    let mut bytes = vec![0u8; (RATE as usize / 50) * 2];
    let played = voice.played();
    let mut echo = Echo::new();
    let mut room = test_room().map(|r| (r, MicClock::new()));
    // What was heard while it spoke, cleaned of its voice: the start of what the wearer said
    // over it, which the barge-in is sure of only a quarter of a second in.
    let mut lately: VecDeque<i16> = VecDeque::new();
    // The wearer spoke over it: what follows is theirs, whatever the voice is still finishing.
    let mut over_it = false;
    // When the canceller's state was last put in the log.
    let mut told = Instant::now();
    while !stopped.load(Ordering::Relaxed) {
        if microphone.read_exact(&mut bytes).is_err() {
            log::info!("ears: the room's microphone closed");
            return;
        }
        let at = Instant::now();
        let mut samples: Vec<f32> = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
        let speaking = voice.busy();
        let blocks = {
            let played = played.lock().unwrap();
            if let Some((room, clock)) = room.as_mut() {
                // A test's room: its voice back in the microphone, as speakers by it would put it
                // -- in the microphone's own time, steady, as a device's samples are.
                let mut back = vec![0.0; samples.len()];
                let from = clock.arrived(&played, at, samples.len());
                room.echo(&played, from, &mut back);
                for (s, b) in samples.iter_mut().zip(back) {
                    *s += b;
                }
            }
            echo.hear(&played, at, &samples, speaking)
        };
        if speaking && told.elapsed() > Duration::from_secs(10) {
            told = Instant::now();
            log::info!("ears: while it speaks: {} (its voice back after {:?} ms)", echo.last, echo.delay_ms());
        }
        for heard in blocks {
            let clean: Vec<i16> = heard.left.iter().map(|s| (s * 32768.0).clamp(-32768.0, 32767.0) as i16).collect();
            if heard.barge_in && !over_it {
                over_it = true;
                voice.interrupt();
                log::info!("ears: the wearer began speaking over it: it stops ({}, its voice back after {:?} ms)", echo.last, echo.delay_ms());
                if cutter.said.send(Said::Interrupted).is_err() {
                    return;
                }
                // From where they began.
                let start: Vec<i16> = lately.drain(..).collect();
                if !cutter.feed(&start) {
                    return;
                }
            }
            if speaking && !over_it {
                // Its own voice in the room, or about to be: what is heard then is let go unless
                // it is the wearer over it. Still fed, so the room's noise goes on being learnt.
                lately.extend(&clean);
                while lately.len() > RATE as usize * 6 / 10 {
                    lately.pop_front();
                }
                if !cutter.quiet(&clean) {
                    return;
                }
                continue;
            }
            if !cutter.feed(&clean) {
                return;
            }
            if over_it && !speaking && !cutter.hearing {
                over_it = false;
            }
        }
        if !speaking {
            lately.clear();
        }
    }
}

/// The cutter and what it has said of the phrase being heard.
struct Cutter {
    phrases: Phrases,
    began: Instant,
    hearing: bool,
    jobs: u64,
    draft: Option<u64>,
    said: Sender<Said>,
}

impl Cutter {
    /// What was heard, cut. `false` when no one is listening any more.
    fn feed(&mut self, samples: &[i16]) -> bool {
        for cut in self.phrases.listen(samples) {
            let sent = match cut {
                Cut::Began => {
                    self.began = Instant::now();
                    self.hearing = true;
                    self.said.send(Said::Began)
                }
                Cut::Draft(samples) => {
                    self.jobs += 1;
                    self.draft = Some(self.jobs);
                    self.said.send(Said::Draft { job: self.jobs, samples })
                }
                Cut::Resumed => {
                    self.draft = None;
                    self.said.send(Said::Resumed)
                }
                Cut::Phrase(samples) => {
                    self.hearing = false;
                    let ended = Instant::now() - Duration::from_millis(PAUSE_MS as u64);
                    let (job, drafted) = match self.draft.take() {
                        Some(job) => (job, true),
                        None => {
                            self.jobs += 1;
                            (self.jobs, false)
                        }
                    };
                    self.said.send(Said::Phrase { job, drafted, samples, began: self.began, ended })
                }
                Cut::Dropped => {
                    self.hearing = false;
                    self.said.send(Said::Gone)
                }
            };
            if sent.is_err() {
                return false;
            }
        }
        true
    }

    /// Heard, but not to be taken as said: the room's noise is learnt, nothing is cut.
    fn quiet(&mut self, samples: &[i16]) -> bool {
        let _ = self.phrases.listen(samples);
        let _ = self.phrases.flush();
        if std::mem::take(&mut self.hearing) {
            self.draft = None;
            return self.said.send(Said::Gone).is_ok();
        }
        true
    }
}

/// `SPATIPAL_TEST_ECHO=<gain>,<delay ms>[,<drift ppm>[,<distortion>]]`: **for tests only**, a
/// made-up room puts what it plays back into the microphone, as the glasses' speakers would put
/// it into theirs -- there are no speakers on a test machine, and none may be heard on the
/// owner's. (The proper test is the glasses on a head; this is how the canceller, the gate and the
/// barge-in are driven end to end without one.)
fn test_room() -> Option<SimulatedRoom> {
    let raw = std::env::var("SPATIPAL_TEST_ECHO").ok()?;
    let n: Vec<f64> = raw.split(',').filter_map(|v| v.trim().parse().ok()).collect();
    let mut room = SimulatedRoom::new(*n.first()? as f32, n.get(1).copied().unwrap_or(150.0) / 1000.0, n.get(2).copied().unwrap_or(0.0));
    room.distortion = n.get(3).copied().unwrap_or(0.5) as f32;
    log::warn!("ears: SPATIPAL_TEST_ECHO: a made-up room puts its voice back into the microphone ({raw})");
    Some(room)
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
    /// When the window was last told that writing failed: not more than once a minute.
    troubled: Option<Instant>,
}

impl Judge {
    fn run(&mut self, said: Receiver<Said>) {
        while let Ok(next) = said.recv() {
            let listened = match next {
                Said::Began => {
                    self.hearing = true;
                    self.tell(Heard::Ear(Ear::Hearing))
                }
                Said::Interrupted => {
                    self.conversation.touch();
                    self.tell(Heard::Interrupted)
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
                        // What was said in the room, not to it: not its words in the log.
                        log::debug!("ears: let go, {why}: {} characters ({seconds:.1} s, sure {sure:.2})", t.chars().count());
                    }
                    return self.tell(Heard::Ear(now)) && self.tell(Heard::LetGo);
                }
            },
            Err(why) => {
                log::warn!("ears: {why}");
                // Said to it -- the listen button pressed, or in the conversation -- and lost:
                // the wearer is told, not left waiting for an answer.
                let to_it = self.once.load(Ordering::Relaxed) || self.conversation.open_until(&self.voice).is_some_and(|until| began <= until);
                if to_it && self.troubled.is_none_or(|at| at.elapsed() >= Duration::from_secs(60)) {
                    self.troubled = Some(Instant::now());
                    if !self.tell(Heard::Trouble(format!("What you said could not be written down: {why}"))) {
                        return false;
                    }
                }
                return self.tell(Heard::Ear(now)) && self.tell(Heard::LetGo);
            }
        };
        let called = self.name.lock().unwrap().clone();
        let in_conversation = self.conversation.open_until(&self.voice).is_some_and(|until| began <= until);
        let meant = meant(&text, &called, self.once.swap(false, Ordering::Relaxed) || in_conversation);
        let Some(meant) = meant else {
            log::debug!("ears: not for us: {} characters", text.chars().count());
            return self.tell(Heard::Ear(now)) && self.tell(Heard::LetGo);
        };
        self.conversation.touch();
        log::info!("ears: heard {meant:?}{}", if in_conversation { " (in the conversation)" } else { "" });
        let meant = match meant {
            Heard::Asked { text, .. } => Heard::Asked { text, ended, follow_up: in_conversation },
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
        Some(rest) => Some(Heard::Asked { text: rest, ended: Instant::now(), follow_up: in_conversation }),
        None if in_conversation => Some(Heard::Asked { text: text.to_string(), ended: Instant::now(), follow_up: true }),
        None => None,
    }
}

/// Whether what was said is only "be quiet" -- "stop", "shh", "quiet please", "diam", "sudah,
/// stop" -- in either language: it is silenced, and nothing is asked (the owner, 2026-11-15:
/// "sometimes I just want it to be shut up"). Thanks or a goodbye with it ("sudah, makasih") is
/// not only that: the model hears it, and may go.
pub fn is_hush(text: &str) -> bool {
    let words: Vec<String> = text.split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()).filter(|w| !w.is_empty()).collect();
    if words.is_empty() || words.len() > 5 {
        return false;
    }
    // Words that say it.
    const HUSH: &[&str] = &[
        "stop", "quiet", "hush", "shh", "shhh", "sh", "ssst", "sst", "psst", "silence", "enough", "shut", "diam", "berhenti", "stopp", "cukup", "hentikan", "setop",
    ];
    // Words that may come with them.
    const WITH: &[&str] = &[
        "please", "now", "it", "be", "up", "talking", "ok", "okay", "oke", "that's", "thats", "that", "is", "pixel", "dong", "ya", "dulu", "sudah", "udah", "dah", "aja", "saja", "tolong", "deh", "you", "can", "just", "wait", "hey", "hei",
    ];
    words.iter().any(|w| HUSH.contains(&w.as_str())) && words.iter().all(|w| HUSH.contains(&w.as_str()) || WITH.contains(&w.as_str()))
}

/// What the wearer said over it, without what was its own voice: Whisper, given what the
/// microphone heard after it stopped, may write the end of its own last words first ("...a woman
/// named Mera. Wait, what is the capital of France?"), or nothing but them when the gate took its
/// voice for the wearer's. Mostly its own words -- three in four of the longer words (four
/// letters or more, at least three of them) are words it said in the last half minute -- and it
/// was its own voice: `None`. Else its own words at the start are taken off: as many as are, in
/// that order, words it said one after another (the end of its sentence, heard before the
/// wearer's) -- not every word it happened to say somewhere ("Is that the only one?" is the
/// wearer's, though it said "is", "that" and "one"). A safeguard behind the canceller and the
/// gate; Whisper does not write its words as they were said ("that's" for "that"), so it goes by
/// words, not by the sentence.
pub fn not_its_own(text: &str, lately_said: &str) -> Option<String> {
    let plain = |w: &str| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase().replace('\'', "");
    let in_order: Vec<String> = lately_said.split_whitespace().map(plain).filter(|w| !w.is_empty()).collect();
    let said: std::collections::HashSet<&String> = in_order.iter().collect();
    let words: Vec<&str> = text.split_whitespace().collect();
    let long: Vec<String> = words.iter().map(|w| plain(w)).filter(|w| w.chars().count() >= 4).collect();
    let its = long.iter().filter(|w| said.contains(*w)).count();
    if long.len() >= 3 && its * 4 >= long.len() * 3 {
        return None;
    }
    // Its own words at the start: the longest run of them that it said one after another, if
    // more than one.
    let heard: Vec<String> = words.iter().map(|w| plain(w)).collect();
    let run = (0..in_order.len()).map(|from| in_order[from..].iter().zip(&heard).take_while(|(a, b)| a == b).count()).max().unwrap_or(0);
    let cut = if run >= 2 { run } else { 0 };
    let rest = words[cut..].join(" ");
    let rest = rest.trim_start_matches(|c: char| !c.is_alphanumeric()).trim().to_string();
    if rest.chars().filter(|c| c.is_alphanumeric()).count() < 2 {
        None
    } else {
        Some(rest)
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
    fn its_own_words_heard_over_it_are_not_the_wearer_s() {
        let said = "Once there was a lighthouse. A keeper of the light in those days was a woman named Mera. She lived alone.";
        assert_eq!(not_its_own("A keeper of the light in those days was a woman named Mera. Wait, what is the capital of France?", said).as_deref(), Some("Wait, what is the capital of France?"));
        assert_eq!(not_its_own("woman named Mera.", said), None);
        assert_eq!(not_its_own("What is the capital of France?", said).as_deref(), Some("What is the capital of France?"));
        assert_eq!(not_its_own("Stop.", said).as_deref(), Some("Stop."));
        // As Whisper wrote its own voice, heard as the wearer's (2026-11-15).
        let said = "Once, on a rock that the maps argued about, there stood a light. Iwan was not a large man, and his hair had gone the colour of the sea.";
        assert_eq!(not_its_own("Once on a rock that's- Iwan was not a large man, and his hair had gone the color of-", said), None);
        assert_eq!(not_its_own("and what does Iwan do at night?", said).as_deref(), Some("and what does Iwan do at night?"));
    }

    #[test]
    fn only_its_own_words_in_their_order_are_taken_off() {
        // Words it said, but not one after another: the wearer's question, whole.
        let said = "That is the only lighthouse on the coast, and one of the oldest. What a place it is.";
        assert_eq!(not_its_own("Is that the only one?", said).as_deref(), Some("Is that the only one?"));
        assert_eq!(not_its_own("What is the capital of France?", "Is it the right time? What a day.").as_deref(), Some("What is the capital of France?"));
        // The end of its own sentence first: that much is taken off.
        assert_eq!(not_its_own("lovely in spring. What is the capital of France?", "Paris is lovely in spring.").as_deref(), Some("What is the capital of France?"));
    }

    #[test]
    fn be_quiet_is_only_that() {
        for said in ["Stop.", "stop it", "Shh!", "quiet please", "Be quiet.", "shut up", "Diam!", "sudah, stop", "Cukup dulu ya", "Berhenti.", "okay stop", "that's enough"] {
            assert!(is_hush(said), "{said:?}");
        }
        for said in ["stop the music in the browser", "sudah, makasih", "berhenti di halaman dua", "what is a stop sign", "yes", "wait", "enough about that, tell me about the moon"] {
            assert!(!is_hush(said), "{said:?}");
        }
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
