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
    let mut heard = hemera.transcribe(&settings.transcription_model, phrase, RATE, settings.whisper_language(), &prompt)?;
    // **Whisper reads its prompt back.** The prompt ends with the wearer's last phrases and
    // what the window shows, which after a while of dictating is mostly what was dictated: of
    // a phrase it cannot make out -- a cough, a short word, a noise that moved like a voice --
    // it then writes one of those again, and that went into the last phrases in its turn, so
    // an old sentence came back again and again though it was said once. What was heard is
    // already in the prompt, then it is asked again with none of the wearer's words in it:
    // said twice, a sentence is heard twice; read back, it is not there the second time.
    if echoes(&heard, recent, &focus.text) {
        let mut bare = focus.clone();
        bare.text.clear();
        let prompt = whisper_prompt(&bare, &settings.words, &[], "");
        let again = hemera.transcribe(&settings.transcription_model, phrase, RATE, settings.whisper_language(), &prompt)?;
        log::info!("dictation: {heard:?} is in the prompt already; heard without it, {again:?}");
        heard = again;
    }
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

/// Whether what was heard is words the prompt already had, in a row: an earlier phrase, some of
/// one, or some of what the window shows. Three words or more -- "ya" and "oke lanjut" are said
/// again all day, and are too short to be told from themselves.
fn echoes(heard: &str, recent: &[String], shown: &str) -> bool {
    fn words(text: &str) -> Vec<String> {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect()
    }
    let heard = words(heard);
    if heard.len() < 3 {
        return false;
    }
    let has = |text: &str| words(text).windows(heard.len()).any(|run| run == heard.as_slice());
    // As the prompt has them: the last phrases run together, and the end of what is shown.
    has(&recent.join(" ")) || has(shown)
}

/// The small model's reading of a phrase said into a terminal.
///
/// **What it answers is checked against what was heard** ([`said_in`]), line by line, and the
/// first line that stands is typed. The model is a small one and does not always do as it is
/// told: given the terminal's folder as a line of its own (`Folder: ~`) it began its answer
/// with that very line for a phrase that was plain prose, and the line was typed in place of
/// the phrase (the owner's rig, 2026-11-17); asked "list the files" it answered `ls`; given a
/// word it could make nothing of, `cd ~`. So what the terminal is at is now fenced off from the
/// phrase and said to be for spelling only, and whatever comes back has to be made of the words
/// that were said. Nothing stands: the rules' guess is typed (the caller's).
fn cleanup(hemera: &Hemera, settings: &VoiceSettings, focus: &Description, heard: &str, ruled: &str) -> Result<String, String> {
    // Nothing but symbols was said: the rules have them, and the model has nothing to decide.
    if words_of(ruled).is_empty() {
        return Ok(ruled.to_string());
    }
    let system = "You turn dictated speech into the exact text to type into a terminal. Spoken symbols become \
                  symbols, letters spelled one by one become one word, number words become digits. The speaker \
                  mixes Indonesian and English: keep each word in the language it was said. Never add anything that \
                  was not said. Output only the text to type, on one line: no quotes around it, no explanation.";
    let mut terminal = String::new();
    if !focus.text.is_empty() {
        terminal.push_str(&format!("The terminal shows:\n{}\n\n", tail_lines(&focus.text, 15)));
    }
    if !focus.directory.is_empty() {
        terminal.push_str(&format!("Folder: {}\n", focus.directory));
    }
    if !focus.commands.is_empty() {
        terminal.push_str(&format!("Recent commands: {}\n", focus.commands.iter().rev().take(8).rev().cloned().collect::<Vec<_>>().join(" ; ")));
    }
    if !focus.vocabulary.is_empty() {
        terminal.push_str(&format!("Names here: {}\n", tail_words(&focus.vocabulary.join(" "), 60)));
    }
    let mut user = String::new();
    if !terminal.is_empty() {
        user.push_str(&format!("<terminal>\n{}\n</terminal>\n\n", terminal.trim_end()));
    }
    user.push_str(&format!(
        "<heard>{heard}</heard>\n<with-symbol-rules>{ruled}</with-symbol-rules>\n\nType what was heard. The terminal block is only \
         there to spell file names and commands that were said; none of it is typed unless it was said."
    ));
    let answer = hemera.complete(&settings.cleanup_model, &[Message::system(system), Message::user(user)], 160, CLEANUP_WAIT)?;
    let known = words_of(&format!("{} {} {} {}", tail_lines(&focus.text, 15), focus.directory, focus.commands.join(" "), focus.vocabulary.join(" ")));
    typed_line(&answer, heard, ruled, &known).ok_or_else(|| format!("the model's answer did not look like the phrase: {answer:?}"))
}

/// The line of the model's `answer` to type: the first that is made of what was said.
fn typed_line(answer: &str, heard: &str, ruled: &str, known: &[String]) -> Option<String> {
    let mut said = words_of(heard);
    said.extend(words_of(ruled));
    answer
        .lines()
        .map(|line| line.trim().trim_matches('`').trim())
        .filter(|line| !line.is_empty())
        // A model that explains is not to be typed into a shell.
        .filter(|line| line.chars().count() <= ruled.chars().count() * 3 + 20)
        .find(|line| said_in(line, &said, known))
        .map(str::to_string)
}

/// A text's words, lower case: its runs of letters and digits.
fn words_of(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// Whether `line` is made of what was said. Each of its words is one that was `said`, or
/// several of them run together (letters spelled one by one, "read me" for `README`), with
/// digits anywhere (number words become digits) -- or a name the terminal shows (`known`),
/// which is how a name heard nearly is spelled as it is there. No word of it may be anything
/// else, and at least half of them must be the wearer's own: a line that is the terminal's
/// and not the phrase -- its folder, its last command -- is not typed.
fn said_in(line: &str, said: &[String], known: &[String]) -> bool {
    let words = words_of(line);
    let own = words.iter().filter(|w| made_of(w, said)).count();
    let none_made_up = words.iter().all(|w| made_of(w, said) || known.contains(w));
    !words.is_empty() && none_made_up && own * 2 >= words.len()
}

/// Whether `word` is words of `said` run together, digits allowed anywhere among them.
fn made_of(word: &str, said: &[String]) -> bool {
    let letters: Vec<char> = word.chars().collect();
    // reach[i]: the first i letters are made of them.
    let mut reach = vec![false; letters.len() + 1];
    reach[0] = true;
    for at in 0..letters.len() {
        if !reach[at] {
            continue;
        }
        if letters[at].is_ascii_digit() {
            reach[at + 1] = true;
        }
        for piece in said {
            let piece: Vec<char> = piece.chars().collect();
            if !piece.is_empty() && letters[at..].starts_with(&piece) {
                reach[at + piece.len()] = true;
            }
        }
    }
    reach[letters.len()]
}

fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::{echoes, typed_line};

    /// Needs hemera (deepmagpie), so it is ignored by default:
    /// `cargo test -p spatiand-voice --lib the_small_model -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn the_small_model_types_the_phrase_and_never_the_terminals_folder() {
        use crate::{context::Description, hemera::Hemera, settings::VoiceSettings, spoken};
        let settings = VoiceSettings::default();
        let hemera = Hemera::new(&std::env::var("HEMERA").unwrap_or_else(|_| settings.hemera.clone()));
        let focus = Description { kind: "terminal".into(), title: "~".into(), directory: "~".into(), ..Default::default() };
        for (heard, wanted) in [
            ("Hello, this is a test.", "Hello, this is a test."),
            ("Oke.", "Oke."),
            ("list the files", "list the files"),
            ("you", "you"),
            ("ls dash l a", "ls -la"),
            ("cd slash etc slash httpd", "cd /etc/httpd"),
            ("tolong jelaskan kenapa build-nya gagal, then fix it", "tolong jelaskan kenapa build-nya gagal, then fix it"),
        ] {
            let ruled = spoken::for_terminal(heard);
            let typed = super::cleanup(&hemera, &settings, &focus, heard, &ruled).unwrap_or_else(|why| {
                println!("{heard:?}: {why}");
                ruled.clone()
            });
            println!("{heard:?} -> {typed:?}");
            // By its words: the rules drop a sentence's commas and full stop.
            assert_eq!(super::words_of(&typed), super::words_of(wanted));
        }
    }

    #[test]
    fn what_the_model_answers_is_typed_only_if_it_was_said() {
        let known: Vec<String> = ["readme", "md", "cargo", "toml", "spatiand"].iter().map(|w| w.to_string()).collect();
        let typed = |answer: &str, heard: &str, ruled: &str| typed_line(answer, heard, ruled, &known);
        // The small model's own answers (neohorse-1:4b, 2026-11-18). The terminal's folder read
        // back before the phrase: the phrase is typed, not the folder.
        assert_eq!(typed("Folder: ~\nHello, this is a test.", "Hello, this is a test.", "Hello, this is a test."), Some("Hello, this is a test.".into()));
        assert_eq!(typed("Folder: ~\nOke.\nOke.", "Oke.", "Oke."), Some("Oke.".into()));
        // A command it thought was meant, and ones made up for a word it could not place.
        assert_eq!(typed("ls", "list the files", "list the files"), None);
        assert_eq!(typed("cd ~", "you", "you"), None);
        assert_eq!(typed("~", "you", "you"), None);
        // What it is there for.
        assert_eq!(typed("ls -la", "ls dash l a", "ls -la"), Some("ls -la".into()));
        assert_eq!(typed("less README.md", "less read me dot m d", "less read me.m d"), Some("less README.md".into()));
        assert_eq!(typed("ssh deck@steamdeck", "s s h deck at steamdeck", "s s h deck@steamdeck"), Some("ssh deck@steamdeck".into()));
        assert_eq!(typed("kill -9 1234", "kill dash nine one two three four", "kill -nine one two three four"), Some("kill -9 1234".into()));
        assert_eq!(typed("`cd /etc/httpd`", "cd slash etc slash httpd", "cd /etc/httpd"), Some("cd /etc/httpd".into()));
        // A name heard nearly, spelled as the terminal has it.
        assert_eq!(typed("cd Spatiand", "cd spatial and", "cd spatial and"), Some("cd Spatiand".into()));
        // The terminal's own words alone are not the phrase.
        assert_eq!(typed("cargo toml readme", "open it", "open it"), None);
        // An explanation is not typed.
        assert_eq!(typed("The user said hello, so the text to type into the terminal is: hello", "hello", "hello"), None);
    }

    #[test]
    fn a_phrase_read_back_from_the_prompt_is_known_and_a_new_one_is_not() {
        let recent = vec!["Coba buka halaman yang tadi.".to_string(), "Terus scroll ke bawah sedikit.".to_string()];
        assert!(echoes("coba buka halaman yang tadi", &recent, ""));
        assert!(echoes("Halaman yang tadi. Terus scroll", &recent, ""));
        assert!(echoes("install it on the Deck", &[], "ok, install it on the deck then"));
        assert!(!echoes("Sekarang tutup jendelanya.", &recent, ""));
        // Short answers are said again and again, and are not asked about twice.
        assert!(!echoes("Terus scroll", &recent, ""));
    }
}
