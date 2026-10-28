//! Dictation: the microphone in, text to type out.
//!
//! Two threads. One cuts the microphone into phrases as it comes (`phrases.rs`), the other
//! sends each phrase to hemera and puts right what comes back, in order -- so a phrase is being
//! listened to while the one before is being written, and a slow answer never makes the cutting
//! fall behind the microphone.
//!
//! What comes back: the wearer's names put right (`words.rs`), and for a terminal the spoken
//! symbols (`spoken.rs`) and then the small model with the terminal's context, which makes the
//! decisions rules cannot ("cd slash etc" or "etc slash httpd"). A terminal's text is marked for
//! review: shown before it is typed, and never followed by Enter, because a misheard `rm` is
//! costly.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::context::{tail_words, whisper_prompt, Description};
use crate::hemera::{Hemera, Message};
use crate::phrases::{is_hallucination, Phrases};
use crate::settings::VoiceSettings;
use crate::{spoken, words, RATE};

/// What dictation has to say.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The microphone's loudness (0..1) and whether a phrase is being heard: for a meter.
    Level { meter: f32, hearing: bool },
    /// A phrase was heard and hemera is writing it.
    Writing,
    /// Text to type. `review`: show it first and let the wearer type it (a terminal's).
    Text { text: String, review: bool },
    /// Something went wrong with one phrase; dictation carries on.
    Trouble(String),
    /// Stopped, and everything heard has been written.
    Done,
}

enum Input {
    Samples(Vec<i16>),
    Stop,
}

/// One dictation, from the mic key pressed to pressed again.
pub struct Dictation {
    input: Sender<Input>,
    events: Receiver<Event>,
    focus: Arc<Mutex<Description>>,
    recent: Arc<Mutex<Vec<String>>>,
}

/// Where the microphone's samples go: cheap to clone, for the audio thread.
#[derive(Clone)]
pub struct Feeder(Sender<Input>);

impl Feeder {
    /// 16-bit mono at [`RATE`].
    pub fn feed(&self, samples: &[i16]) {
        let _ = self.0.send(Input::Samples(samples.to_vec()));
    }
}

/// How many of the wearer's own phrases steer the next.
const RECENT_KEPT: usize = 8;
/// A terminal's phrase waits this long for the small model before the rules' guess is used.
const CLEANUP_WAIT: Duration = Duration::from_secs(4);

impl Dictation {
    /// Start listening for phrases said into `focus`. `recent` is the wearer's last phrases from
    /// before, which steer how a mixed sentence is written.
    pub fn start(settings: VoiceSettings, focus: Description, recent: Vec<String>) -> Dictation {
        let (input, samples) = mpsc::channel::<Input>();
        let (phrases_out, phrases_in) = mpsc::channel::<Option<Vec<i16>>>();
        let (events_out, events) = mpsc::channel::<Event>();
        let focus = Arc::new(Mutex::new(focus));
        let recent = Arc::new(Mutex::new(recent));

        let level_events = events_out.clone();
        let pause = settings.pause_ms;
        std::thread::Builder::new()
            .name("dictation-cut".into())
            .spawn(move || {
                let mut cutter = Phrases::new(RATE, pause);
                let mut told = Instant::now();
                while let Ok(input) = samples.recv() {
                    match input {
                        Input::Samples(s) => {
                            for phrase in cutter.feed(&s) {
                                let _ = level_events.send(Event::Writing);
                                let _ = phrases_out.send(Some(phrase));
                            }
                            if told.elapsed() >= Duration::from_millis(50) {
                                told = Instant::now();
                                let _ = level_events.send(Event::Level { meter: cutter.meter(), hearing: cutter.hearing() });
                            }
                        }
                        Input::Stop => break,
                    }
                }
                if let Some(last) = cutter.flush() {
                    let _ = level_events.send(Event::Writing);
                    let _ = phrases_out.send(Some(last));
                }
                let _ = phrases_out.send(None);
            })
            .expect("a thread for dictation");

        let (writer_focus, writer_recent) = (focus.clone(), recent.clone());
        std::thread::Builder::new()
            .name("dictation-write".into())
            .spawn(move || {
                let hemera = Hemera::new(&settings.hemera);
                while let Ok(Some(phrase)) = phrases_in.recv() {
                    let focus = writer_focus.lock().unwrap().clone();
                    let mine = writer_recent.lock().unwrap().clone();
                    match write(&hemera, &settings, &focus, &mine, &phrase) {
                        Ok(Some((heard, text))) => {
                            let mut kept = writer_recent.lock().unwrap();
                            kept.push(heard);
                            let over = kept.len().saturating_sub(RECENT_KEPT);
                            kept.drain(..over);
                            let _ = events_out.send(Event::Text { text, review: focus.is_terminal() });
                        }
                        Ok(None) => {}
                        Err(why) => {
                            let _ = events_out.send(Event::Trouble(why));
                        }
                    }
                }
                let _ = events_out.send(Event::Done);
            })
            .expect("a thread for dictation");

        Dictation { input, events, focus, recent }
    }

    pub fn feeder(&self) -> Feeder {
        Feeder(self.input.clone())
    }

    /// What has focus now: the next phrase is steered by it.
    pub fn set_focus(&self, focus: Description) {
        *self.focus.lock().unwrap() = focus;
    }

    /// Stop listening. What was being said is still written; [`Event::Done`] follows it.
    pub fn stop(&self) {
        let _ = self.input.send(Input::Stop);
    }

    /// What has happened since last asked.
    pub fn events(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// The wearer's last phrases, to keep for the next dictation.
    pub fn recent(&self) -> Vec<String> {
        self.recent.lock().unwrap().clone()
    }
}

/// One phrase: what was heard (for steering the next), and what to type. `None` for nothing said.
fn write(hemera: &Hemera, settings: &VoiceSettings, focus: &Description, recent: &[String], phrase: &[i16]) -> Result<Option<(String, String)>, String> {
    let prompt = whisper_prompt(focus, &settings.words, recent, settings.primer());
    let heard = hemera.transcribe(&settings.transcription_model, phrase, RATE, settings.whisper_language(), &prompt)?;
    if is_hallucination(&heard) {
        return Ok(None);
    }
    let vocabulary: Vec<String> = focus.vocabulary.iter().chain(&settings.words).cloned().collect();
    let heard = words::correct(&heard, &vocabulary);
    if !focus.is_terminal() {
        return Ok(Some((heard.clone(), heard)));
    }
    let ruled = spoken::for_terminal(&heard);
    let text = match cleanup(hemera, settings, focus, &heard, &ruled) {
        Ok(text) => text,
        Err(why) => {
            log::warn!("dictation: the terminal clean-up failed ({why}); typing the rules' guess");
            ruled.clone()
        }
    };
    log::debug!("dictation: heard {heard:?}, by the rules {ruled:?}, typed {text:?}");
    Ok(Some((heard, text)))
}

/// The small model's reading of a phrase said into a terminal.
fn cleanup(hemera: &Hemera, settings: &VoiceSettings, focus: &Description, heard: &str, ruled: &str) -> Result<String, String> {
    let system = "You turn dictated speech into the exact text to type into a terminal. Spoken symbols become \
                  symbols, letters spelled one by one become one word, number words become digits. The speaker \
                  mixes Indonesian and English: keep each word in the language it was said. Never add anything that \
                  was not said. Output only the text to type, on one line: no quotes around it, no explanation.";
    let mut user = String::new();
    if !focus.text.is_empty() {
        user.push_str(&format!("The terminal shows:\n{}\n\n", tail_lines(&focus.text, 15)));
    }
    if !focus.directory.is_empty() {
        user.push_str(&format!("Folder: {}\n", focus.directory));
    }
    if !focus.commands.is_empty() {
        user.push_str(&format!("Recent commands: {}\n", focus.commands.iter().rev().take(8).rev().cloned().collect::<Vec<_>>().join(" ; ")));
    }
    if !focus.vocabulary.is_empty() {
        user.push_str(&format!("Names here: {}\n", tail_words(&focus.vocabulary.join(" "), 60)));
    }
    user.push_str(&format!("\nHeard: {heard}\nWith the symbol rules: {ruled}"));
    let answer = hemera.complete(&settings.cleanup_model, &[Message::system(system), Message::user(user)], 160, CLEANUP_WAIT)?;
    let line = answer.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line = line.trim_matches('`').trim();
    // A model that explains, or invents, is not to be typed into a shell.
    if line.is_empty() || line.chars().count() > ruled.chars().count() * 3 + 20 {
        return Err(format!("the model's answer did not look like the phrase: {line:?}"));
    }
    Ok(line.to_string())
}

fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}
