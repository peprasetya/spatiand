//! Dictation in the room: the mic key, the microphone, and what was said typed into whatever
//! has focus (`docs/ai-integrations.md`).
//!
//! The work is `spatiand_voice`'s -- phrases, hemera, the word list, a terminal's symbols. What
//! is here is the room's part: opening the microphone when the key is pressed and closing it
//! when it is pressed again, saying what has focus, and handing back the text for the backend
//! to type as keystrokes. A terminal's text is not typed straight away: it waits on the notice
//! card, and pressing the card types it -- without Enter -- because a misheard `rm` is costly.

use std::time::{Duration, Instant};

use spatiand_shell::keyboard::Dictating;
use spatiand_voice::{Description, VoiceSettings};

use crate::listen::Microphone;

pub struct Dictation {
    running: Option<Running>,
    /// A terminal's text, waiting for the card to be pressed.
    review: Option<String>,
    trouble: Option<(String, Instant)>,
    /// The wearer's last phrases, which steer how the next is written. Kept for as long as the
    /// room runs, and nowhere else.
    recent: Vec<String>,
    /// Whether this dictation has typed anything: the next phrase then needs a space before it.
    typed: bool,
    /// The window the dictation started in.
    window: Option<usize>,
    /// What the room knows of the focused window by itself, while the window's own account of
    /// itself is awaited (`xr::ask_description`).
    asking: Option<(Description, std::sync::mpsc::Receiver<String>, Instant)>,
}

struct Running {
    voice: spatiand_voice::Dictation,
    /// `None` once the key has been pressed again: the last phrase is still being written.
    microphone: Option<Microphone>,
    /// Whether a phrase is being heard right now, for the card.
    hearing: bool,
    writing: usize,
}

/// How long trouble stays on the card.
const TROUBLE_FOR: Duration = Duration::from_secs(8);

impl Default for Dictation {
    fn default() -> Self {
        Dictation { running: None, review: None, trouble: None, recent: Vec::new(), typed: false, window: None, asking: None }
    }
}

impl Dictation {
    /// The mic key was pressed: start listening into what has focus, or stop.
    pub fn press(&mut self, state: &crate::state::Spatiand) {
        let (focus, window, answer) = focus_of(state, true);
        self.toggle(focus.clone(), window);
        self.asking = answer.filter(|_| self.running.is_some()).map(|rx| (focus, rx, Instant::now()));
    }

    /// Follow the focus while listening: the window's own account of itself is asked for only
    /// when the focus has moved.
    pub fn follow(&mut self, state: &crate::state::Spatiand) {
        if self.look() != Dictating::Listening || state.layout.focused_id() == self.window {
            return;
        }
        let (focus, window, answer) = focus_of(state, true);
        self.focus_changed(focus.clone(), window);
        self.asking = answer.map(|rx| (focus, rx, Instant::now()));
    }

    /// The mic key was pressed: start listening, or stop.
    pub fn toggle(&mut self, focus: Description, window: Option<usize>) {
        if let Some(running) = &mut self.running {
            if running.microphone.take().is_some() {
                running.voice.stop();
                log::info!("dictation: stopped listening");
            }
            return;
        }
        let settings = VoiceSettings::load(&spatiand_sync::default_folders().0);
        log::info!("dictation: listening for {} ({}, {:?})", focus.app, focus.kind, focus.title);
        let voice = spatiand_voice::Dictation::start(settings, focus, self.recent.clone());
        let feeder = voice.feeder();
        match Microphone::open(spatiand_voice::RATE, Box::new(move |samples: &[i16]| feeder.feed(samples))) {
            Ok(microphone) => {
                self.running = Some(Running { voice, microphone: Some(microphone), hearing: false, writing: 0 });
                self.typed = false;
                self.window = window;
                self.trouble = None;
            }
            Err(why) => {
                voice.stop();
                log::warn!("dictation: {why}");
                self.trouble = Some((why, Instant::now()));
            }
        }
    }

    /// Focus moved while listening: the next phrase is written for the new window.
    pub fn focus_changed(&mut self, focus: Description, window: Option<usize>) {
        if let Some(running) = &self.running {
            if window != self.window {
                self.window = window;
                self.typed = false;
                running.voice.set_focus(focus);
            }
        }
    }

    /// Text to type into what has focus now, if a phrase has been written.
    pub fn tick(&mut self) -> Option<String> {
        // The window's account of itself, when it comes, steers the phrases after it.
        if let Some((basic, answer, asked)) = &self.asking {
            match answer.try_recv() {
                Ok(json) => {
                    if let Some(running) = &self.running {
                        let told = told(basic, &json);
                        log::info!(
                            "dictation: {} says it is {:?} ({} words of vocabulary, {} commands)",
                            told.app,
                            told.kind,
                            told.vocabulary.len(),
                            told.commands.len()
                        );
                        running.voice.set_focus(told);
                    }
                    self.asking = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.asking = None,
                Err(std::sync::mpsc::TryRecvError::Empty) if asked.elapsed() > Duration::from_secs(2) => self.asking = None,
                Err(_) => {}
            }
        }
        let running = self.running.as_mut()?;
        let mut out = String::new();
        let mut done = false;
        for event in running.voice.events() {
            match event {
                spatiand_voice::Event::Level { hearing, .. } => running.hearing = hearing,
                spatiand_voice::Event::Writing => running.writing += 1,
                spatiand_voice::Event::Text { text, review } => {
                    running.writing = running.writing.saturating_sub(1);
                    if review {
                        // One at a time on the card: the newest is what was just said.
                        self.review = Some(text);
                    } else {
                        if self.typed || !out.is_empty() {
                            out.push(' ');
                        }
                        out.push_str(&text);
                    }
                }
                spatiand_voice::Event::Trouble(why) => {
                    running.writing = running.writing.saturating_sub(1);
                    log::warn!("dictation: {why}");
                    self.trouble = Some((why, Instant::now()));
                }
                spatiand_voice::Event::Done => done = true,
            }
        }
        if done {
            self.recent = running.voice.recent();
            self.running = None;
            log::info!("dictation: finished");
        }
        if out.is_empty() {
            return None;
        }
        self.typed = true;
        Some(out)
    }

    /// The mic key's look.
    pub fn look(&self) -> Dictating {
        match &self.running {
            None => Dictating::Off,
            Some(r) if r.microphone.is_some() => Dictating::Listening,
            Some(_) => Dictating::Finishing,
        }
    }

    /// What the notice card says: a terminal's text waiting, trouble, or that the room is
    /// listening. Empty for nothing.
    pub fn notice(&self) -> String {
        if let Some(text) = &self.review {
            return format!("Dictation \u{2014} type into the terminal?\n{text}");
        }
        if let Some((why, at)) = &self.trouble {
            if at.elapsed() < TROUBLE_FOR {
                return format!("Dictation\n{why}");
            }
        }
        match &self.running {
            Some(r) if r.microphone.is_some() && r.writing > 0 => "Dictation\nWriting\u{2026}".into(),
            Some(r) if r.microphone.is_some() && r.hearing => "Dictation\nHearing you\u{2026}".into(),
            Some(r) if r.microphone.is_some() => "Dictation\nListening \u{2014} press \u{1F3A4} to stop".into(),
            Some(_) => "Dictation\nWriting the last of it\u{2026}".into(),
            None => String::new(),
        }
    }

    /// The card was pressed: a terminal's text waiting there, to be typed.
    pub fn take_review(&mut self) -> Option<String> {
        self.review.take()
    }

    /// Whether pressing the card is dictation's business.
    pub fn owns_notice(&self) -> bool {
        self.review.is_some() || self.running.is_some()
    }
}

/// `text` as the keyboard can type it: the punctuation Whisper writes in its typographer's
/// forms made plain, and accents dropped from the letters that have them. What is left that
/// the keyboard cannot type is skipped by the typing, and logged.
pub fn typeable(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{2032}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{2033}' => '"',
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            '\u{00A0}' | '\u{2009}' | '\u{202F}' => ' ',
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            'À' | 'Á' | 'Â' | 'Ä' => 'A',
            'É' | 'È' | 'Ê' => 'E',
            other => other,
        })
        .collect::<String>()
        .replace('\u{2026}', "...")
}

/// What the focused window is, as far as the room knows by itself: its application and title,
/// and whether that application is a terminal. (Spatiand's own applications and Mac windows say
/// more about themselves; see `docs/ai-integrations.md`.)
pub fn describe_focus(state: &crate::state::Spatiand) -> (Description, Option<usize>) {
    let (focus, window, _) = focus_of(state, false);
    (focus, window)
}

/// The focused window by the room's own lights, and if `ask`, its own account of itself on the
/// way (see [`crate::xr::ask_description`]).
fn focus_of(state: &crate::state::Spatiand, ask: bool) -> (Description, Option<usize>, Option<std::sync::mpsc::Receiver<String>>) {
    let Some(id) = state.layout.focused_id() else {
        return (Description::default(), None, None);
    };
    let window = state.space.elements().find(|w| state.layout.id_of(w) == Some(id)).cloned();
    let app = state.app_id_of_id(id).unwrap_or_default();
    let title = window.as_ref().and_then(|w| state.title_of(w)).unwrap_or_default();
    let answer = if ask {
        window.as_ref().and_then(|w| w.toplevel()).and_then(|t| crate::xr::ask_description(t.wl_surface()))
    } else {
        None
    };
    let mut description = Description::titled(&app, &title);
    if is_terminal(&app) {
        description.kind = "terminal".into();
        // A shell's title is usually where it is.
        if title.starts_with('~') || title.starts_with('/') {
            description.directory = title.clone();
        } else if let Some((_, place)) = title.split_once(':') {
            description.directory = place.trim().to_string();
        }
    }
    (description, Some(id), answer)
}

/// A window's own account of itself, over what the room knew of it: what it says wins, what it
/// leaves out the room fills in.
fn told(basic: &Description, json: &str) -> Description {
    let Some(mut said) = Description::from_json(json) else {
        log::warn!("dictation: a window described itself in something that is not JSON: {:?}", json.chars().take(80).collect::<String>());
        return basic.clone();
    };
    if said.app.is_empty() {
        said.app = basic.app.clone();
    }
    if said.title.is_empty() {
        said.title = basic.title.clone();
    }
    if said.kind.is_empty() {
        said.kind = basic.kind.clone();
    }
    if said.directory.is_empty() {
        said.directory = basic.directory.clone();
    }
    said
}

/// Whether an application id is a terminal's: ours, the Mac's own and the usual Linux ones, by
/// any of the names they go by.
fn is_terminal(app: &str) -> bool {
    let app = app.to_lowercase();
    const TERMINALS: &[&str] = &[
        "spatiquill", "com.apple.terminal", "iterm", "ghostty", "kitty", "alacritty", "wezterm", "konsole", "gnome.terminal",
        "ptyxis", "kgx", "org.gnome.console", "foot", "xterm", "tilix", "terminator", "warp",
    ];
    TERMINALS.iter().any(|t| app.contains(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_s_own_account_wins_and_the_room_fills_the_gaps() {
        let basic = Description { kind: "terminal".into(), app: "id.prasetya.spatiand.spatiquill".into(), title: "~".into(), ..Default::default() };
        let said = told(&basic, r#"{"directory":"~/Documents","commands":["ls"],"vocabulary":["Cargo.toml"]}"#);
        assert_eq!(said.kind, "terminal");
        assert_eq!(said.app, basic.app);
        assert_eq!(said.directory, "~/Documents");
        assert_eq!(said.commands, vec!["ls".to_string()]);
        assert_eq!(told(&basic, "not json"), basic);
    }

    #[test]
    fn what_whisper_writes_is_made_typeable() {
        assert_eq!(typeable("It\u{2019}s \u{201C}café\u{201D} \u{2014} ok\u{2026}"), "It's \"cafe\" - ok...");
    }

    #[test]
    fn terminals_are_known_by_any_of_their_names() {
        assert!(is_terminal("spatiquill"));
        assert!(is_terminal("remote.darkphoenix.com.apple.Terminal"));
        assert!(is_terminal("remote.deepmagpie.org.gnome.Ptyxis"));
        assert!(!is_terminal("spatimesh"));
        assert!(!is_terminal("remote.darkphoenix.com.apple.Safari"));
    }
}
