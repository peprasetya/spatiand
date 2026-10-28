//! SpatiPal's ears: the room's microphone, cut into phrases at the pauses, each written by Whisper,
//! and kept only when it was meant for SpatiPal.
//!
//! **Meant for it** (the owner's choice): it starts with its name ("Hei Pixel, ..."), or it comes
//! within a few seconds of its last answer -- a follow-up needs no name -- or the wearer asked it
//! to listen to the next thing said. Everything else said in the room is heard and let go: it is
//! written by hemera, which keeps nothing, and goes no further.
//!
//! **Not its own voice**: a phrase heard while it is speaking, or a moment after, is dropped.

use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use spatiand_voice::phrases::{is_hallucination, Phrases};
use spatiand_voice::{Hemera, VoiceSettings, RATE};

use crate::voice::Voice;

/// A follow-up within this long of the last answer needs no name.
pub const FOLLOW_UP: Duration = Duration::from_secs(8);

/// What the ears heard that was meant for SpatiPal.
#[derive(Clone, Debug, PartialEq)]
pub enum Heard {
    /// Something asked: the words after the name, or all of a follow-up.
    Asked(String),
    /// Only the name: the wearer wants its attention.
    Called,
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

/// When the last answer was given, in ms since `epoch`: for follow-ups when it answered in
/// writing only.
pub struct Answered {
    pub epoch: Instant,
    pub at: AtomicU64,
}

impl Answered {
    pub fn new() -> Answered {
        Answered { epoch: Instant::now(), at: AtomicU64::new(0) }
    }

    pub fn now(&self) {
        self.at.store(self.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    fn since(&self) -> Duration {
        let at = self.at.load(Ordering::Relaxed);
        if at == 0 {
            return Duration::MAX;
        }
        Duration::from_millis(self.epoch.elapsed().as_millis() as u64 - at)
    }
}

/// Start listening to `microphone` (16 kHz mono 16-bit, from the room).
pub fn start(
    mut microphone: std::fs::File,
    settings: VoiceSettings,
    name: Arc<Mutex<String>>,
    voice: Voice,
    answered: Arc<Answered>,
    heard: Sender<Heard>,
    wake: impl Fn() + Send + 'static,
) -> Ears {
    let stop = Arc::new(AtomicBool::new(false));
    let once = Arc::new(AtomicBool::new(false));
    let (phrases_out, phrases_in) = channel::<Vec<i16>>();
    let stopped = stop.clone();
    let busy = voice.clone();
    std::thread::Builder::new()
        .name("ears".into())
        .spawn(move || {
            let mut cutter = Phrases::new(RATE, 800);
            let mut bytes = vec![0u8; (RATE as usize / 50) * 2];
            let mut talking_since: Option<Instant> = None;
            while !stopped.load(Ordering::Relaxed) {
                if microphone.read_exact(&mut bytes).is_err() {
                    log::info!("ears: the room's microphone closed");
                    return;
                }
                let samples: Vec<i16> = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
                // Its own voice in the room: what is heard then is not the wearer's.
                if busy.busy() {
                    talking_since.get_or_insert_with(Instant::now);
                    let _ = cutter.feed(&samples);
                    let _ = cutter.flush();
                    continue;
                }
                talking_since = None;
                for phrase in cutter.feed(&samples) {
                    let _ = phrases_out.send(phrase);
                }
            }
        })
        .expect("a thread for the ears");

    let listen_once = once.clone();
    std::thread::Builder::new()
        .name("ears-write".into())
        .spawn(move || {
            let hemera = Hemera::new(&settings.hemera);
            for phrase in phrases_in {
                let called = name.lock().unwrap().clone();
                // Its name in the prompt, so that Whisper spells it as it is called.
                let prompt = format!("{called}. {}", settings.primer());
                let text = match hemera.transcribe(&settings.transcription_model, &phrase, RATE, settings.whisper_language(), &prompt) {
                    Ok(t) => t,
                    Err(why) => {
                        log::warn!("ears: {why}");
                        continue;
                    }
                };
                if is_hallucination(&text) {
                    continue;
                }
                let follow_up = voice.quiet_for() < FOLLOW_UP || answered.since() < FOLLOW_UP;
                let meant = if listen_once.swap(false, Ordering::Relaxed) || follow_up {
                    match addressed(&text, &called) {
                        Some(rest) if rest.is_empty() => Some(Heard::Called),
                        Some(rest) => Some(Heard::Asked(rest)),
                        None => Some(Heard::Asked(text.clone())),
                    }
                } else {
                    match addressed(&text, &called) {
                        Some(rest) if rest.is_empty() => Some(Heard::Called),
                        Some(rest) => Some(Heard::Asked(rest)),
                        None => None,
                    }
                };
                match meant {
                    Some(m) => {
                        log::info!("ears: heard {m:?}");
                        if heard.send(m).is_err() {
                            return;
                        }
                        wake();
                    }
                    None => log::debug!("ears: not for us: {text:?}"),
                }
            }
        })
        .expect("a thread for the ears");
    Ears { stop, once }
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
    use super::addressed;

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
}
