//! SpatiPal: Spatiand's assistant (`docs/ai-integrations.md`).
//!
//! A voice, ears and hands in the room. It hears its name ("Hei Pixel, ..."), answers aloud from
//! where its figure stands, and does what it is asked with the room's windows as the wearer
//! would -- reads them, looks at them, clicks, scrolls, types -- through the room's assistant
//! protocol, which the room opens to this process alone. The everyday model is a small one; what
//! needs thought it hands to a larger one, which thinks out loud if the wearer likes.
//!
//! ```text
//! spatipal                    its window: the chats, and its settings
//! spatipal --background       no window: how the room starts it when it is switched on
//! ```
//!
//! One process at a time: started again, it asks the one running to show its window, and goes.

mod brain;
mod ears;
mod pack;
mod store;
mod time;
mod tools;
mod ui;
mod voice;
mod warm;

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use spatiand_app::ai::AiSettings;
use spatiand_app::ui::{theme, Input, Painter};
use spatiand_app::{App, Event, WindowId};
use spatiand_voice::speech::Voice as Spoken;
use spatiand_voice::{Hemera, VoiceSettings};

use brain::{Brain, Told, Turn};
use ears::{Conversation, Ear, Ears, Heard};
use store::{Chat, Said, Settings};
use tools::Room;
use ui::{Answering, Page, View, Want, Window};
use voice::Voice;

const APP_ID: &str = "id.prasetya.spatiand.spatipal";
const WINDOW: (u32, u32) = (1500, 1000);
/// How long after the room's microphone failed it is asked for again.
const LISTEN_AGAIN: Duration = Duration::from_secs(30);
/// The trouble line when the brain's thread has gone.
const BRAIN_GONE: &str = "Its answering stopped with a fault: quit SpatiPal and start it again.";

/// Something to wake the window's thread with, from any other: a byte down a pipe it watches.
#[derive(Clone)]
struct Waker(Arc<std::fs::File>);

impl Waker {
    fn wake(&self) {
        let _ = (&*self.0).write(&[1]);
    }
}

fn pipe() -> (std::fs::File, std::fs::File) {
    use std::os::fd::FromRawFd;
    let mut ends = [0i32; 2];
    unsafe {
        libc::pipe(ends.as_mut_ptr());
        for fd in ends {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
        }
        (std::fs::File::from_raw_fd(ends[0]), std::fs::File::from_raw_fd(ends[1]))
    }
}

struct Pal {
    app: App,
    window: Option<WindowId>,
    painter: Option<Painter>,
    size: (u32, u32),
    ready: bool,
    dirty: bool,
    ui: Window,
    /// The settings as used: the pal's (`pack`) over the wearer's own.
    settings: Settings,
    /// The wearer's own, as kept in `assistant.json`.
    saved: Settings,
    /// The pal it is, when one is chosen, and the pals there are.
    pack: Option<pack::Pack>,
    pals: Vec<String>,
    /// When the pal's files were last looked at for a change.
    pack_looked: Instant,
    /// The room's AI settings, which more than one app uses: hemera, the models, dictation's
    /// languages and words. Read again whenever the SDK says they changed.
    ai: AiSettings,
    chats: Vec<Chat>,
    /// The chat open, by its id: the list is sorted again whenever it is read.
    current: Option<String>,
    answering: Option<Answering>,
    look: bool,
    trouble: Option<(String, Instant)>,
    voice: Voice,
    brain: Brain,
    told: Receiver<(u64, Told)>,
    room_asks: Receiver<(String, Sender<String>)>,
    ears: Option<Ears>,
    heard_tx: Sender<Heard>,
    heard: Receiver<Heard>,
    conversation: Arc<Conversation>,
    /// What the ears are doing.
    ear: Ear,
    /// When hemera was last asked to have the models ready.
    warmed: Option<Instant>,
    /// When to ask hemera again, the settings having changed what it needs ready: a moment
    /// after the last change, so that stepping through the models warms only the one stopped at.
    warm_due: Option<Instant>,
    /// Whether its voice was busy when the window was last drawn.
    drawn_speaking: bool,
    /// When to ask the room for its microphone again, after it would not give it.
    listen_again: Option<Instant>,
    name: Arc<Mutex<String>>,
    waker: Waker,
    woken: std::fs::File,
    listener: Option<UnixListener>,
    reminders_looked: Instant,
    voices: Vec<String>,
    characters: Vec<String>,
    /// hemera's voices, as they come.
    lists: Receiver<Vec<String>>,
    lists_tx: Sender<Vec<String>>,
    /// Told to go: it does once it has finished speaking.
    leaving: bool,
    /// It went, but is switched on, so stays for its reminders: silent and unseen until opened.
    away: bool,
    /// What the figure was last told it is doing, and when the last answer finished (it smiles
    /// once it is heard out).
    state: &'static str,
    finished_at: Option<Instant>,
    /// When the wearer last spoke over it, and what it had been saying: what is heard next is
    /// checked against that.
    interrupted: Option<(Instant, String)>,
}

/// What the window is lent to draw from: built from the fields themselves, so that the app and
/// the window's own state can be borrowed beside it.
macro_rules! view {
    ($pal:expr) => {
        View {
            chats: &$pal.chats,
            current: $pal.current.as_ref().and_then(|id| $pal.chats.iter().position(|c| &c.id == id)),
            answering: $pal.answering.as_ref(),
            speaking: $pal.voice.busy(),
            settings: &$pal.settings,
            listening: $pal.ears.is_some(),
            look: $pal.look,
            status: status($pal.state, &$pal.settings),
            trouble: $pal.trouble.as_ref().filter(|(_, at)| at.elapsed() < Duration::from_secs(8)).map(|(t, _)| t.as_str()),
            voices: &$pal.voices,
            characters: &$pal.characters,
            pack: $pal.pack.as_ref(),
            pals: &$pal.pals,
        }
    };
}

/// What it is doing, beside its name: the figure's state in words.
fn status(state: &str, settings: &Settings) -> &'static str {
    match state {
        "talking" => "speaking",
        "working" => "working in the room",
        "pondering" => "thinking it through",
        "thinking" => "thinking",
        "transcribing" => "writing down what you said",
        "hearing" => "listening to you",
        "attentive" => "listening \u{2014} no need to say its name",
        "listening" | "checking" => "listening for its name",
        "idle" if !settings.listening => "not listening",
        _ => "",
    }
}

/// The figure's state as one of the five moods every room knows, for a room older than the
/// state's name.
fn mood_of(state: &str) -> &'static str {
    match state {
        "talking" => "talking",
        "working" | "pondering" | "thinking" | "transcribing" => "thinking",
        "hearing" | "attentive" | "listening" | "checking" => "listening",
        "happy" => "happy",
        _ => "idle",
    }
}

impl Pal {
    fn open_window(&mut self) {
        if self.window.is_some() {
            return;
        }
        let window = self.app.open(&self.settings.name, WINDOW);
        self.window = Some(window);
        self.ready = false;
        self.dirty = true;
        self.chats = store::chats_with(&self.settings.pal);
        // The chat that was open stays open, wherever the list now puts it.
        let kept = self.current.take().filter(|id| self.chats.iter().any(|c| &c.id == id));
        self.current = kept.or_else(|| self.chats.first().map(|c| c.id.clone()));
    }

    fn close_window(&mut self) {
        if let Some(w) = self.window.take() {
            self.app.close(w);
        }
        self.painter = None;
    }

    /// Dismissed, and done speaking: it goes. **Switched on it must stay** -- the room would only
    /// start it again, and its reminders need it -- but deaf and unseen, until it is opened again.
    /// `false` when the process should end.
    fn leave(&mut self) -> bool {
        self.leaving = false;
        log::info!("dismissed");
        if !self.settings.on {
            return false;
        }
        self.close_window();
        self.ears = None;
        self.ear = Ear::Quiet;
        self.away = true;
        let _ = self.app.room_ask(&json!({"do": "avatar", "visible": false}).to_string(), channel().0);
        true
    }

    /// Opened again after being dismissed.
    fn come_back(&mut self) {
        if std::mem::take(&mut self.away) {
            self.tend_ears();
            self.tell_figure();
        }
    }

    /// Listen for its name, or stop, as the settings say.
    fn tend_ears(&mut self) {
        if self.away {
            return;
        }
        match (self.settings.listening, self.ears.is_some()) {
            (true, false) => {
                let Some(microphone) = self.app.room_listen() else {
                    self.say_trouble("The room would not give its microphone.");
                    self.settings.listening = false;
                    return;
                };
                let waker = self.waker.clone();
                self.ears = Some(ears::start(
                    microphone,
                    VoiceSettings::of(&self.ai),
                    self.name.clone(),
                    self.voice.clone(),
                    self.conversation.clone(),
                    self.heard_tx.clone(),
                    move || waker.wake(),
                ));
                self.warm_soon();
                log::info!("listening for {:?}", self.settings.name);
            }
            (false, true) => {
                self.ears = None;
                self.ear = Ear::Quiet;
                log::info!("not listening");
            }
            _ => {}
        }
    }

    fn say_trouble(&mut self, why: &str) {
        log::warn!("{why}");
        // A whole sentence: its first letter capital, a full stop at its end.
        let mut said = why.trim().to_string();
        if let Some(first) = said.chars().next() {
            said.replace_range(..first.len_utf8(), &first.to_uppercase().to_string());
        }
        if !said.is_empty() && !said.ends_with(['.', '!', '?']) {
            said.push('.');
        }
        self.trouble = Some((said, Instant::now()));
        self.dirty = true;
    }

    /// The chat a new turn goes in. Typed, the one open (the wearer chose it). Spoken (`Some`,
    /// with whether it follows on in the conversation): a follow-up goes on in the chat open; a
    /// conversation begun anew goes on in it only if that was "Go on in the last chat"'s hours
    /// ago or less -- never, with Never -- and else in a new chat. With none open, the latest by
    /// when it was changed, if it is recent enough.
    fn chat_for_turn(&mut self, spoken: Option<bool>) -> usize {
        let i = match chat_to_go_on(&self.chats, self.current_index(), spoken, self.settings.continue_hours, store::now()) {
            Some(i) => i,
            None => {
                self.chats.insert(0, Chat { pal: self.settings.pal.clone(), ..Chat::new() });
                0
            }
        };
        self.current = Some(self.chats[i].id.clone());
        i
    }

    /// Where the chat open is in the list.
    fn current_index(&self) -> Option<usize> {
        self.current.as_ref().and_then(|id| self.chats.iter().position(|c| &c.id == id))
    }

    /// Have hemera ready with what the next answer needs (`crate::warm`); not again for a few
    /// minutes unless `now`.
    fn warm(&mut self, now: bool) {
        if !now && self.warmed.is_some_and(|at| at.elapsed() < warm::EVERY) {
            return;
        }
        self.warmed = Some(Instant::now());
        self.warm_due = None;
        let mut models = Vec::new();
        if self.ears.is_some() {
            models.push(self.ai.models.transcription.clone());
        }
        if self.settings.spoken {
            models.push(self.settings.speech_model.clone());
        }
        // The pal's own models, when it names any, over the room's.
        let ai = self.pal_ai();
        let wanted = warm::Wanted {
            models,
            small: ai.models.everyday.clone(),
            prime: brain::prime(&self.settings, &ai, false, self.appearance().as_ref()),
            large: ai.models.larger.clone(),
            prime_large: brain::prime(&self.settings, &ai, true, self.appearance().as_ref()),
        };
        warm::warm(Hemera::new(&self.ai.hemera), wanted, now);
    }

    /// The answer being given stops: the brain first, so that nothing more of it is said, and
    /// what of it was shown is kept in its chat as it is. What the brain still says of it is not
    /// this window's any more (`told` goes by the turn's number).
    fn cut_answer(&mut self) {
        self.brain.stop();
        let Some(a) = self.answering.take() else { return };
        let said = Said {
            role: "assistant".into(),
            text: a.text,
            thinking: a.thinking,
            did: a.did,
            model: if a.handed_over { a.model } else { String::new() },
            at: store::now(),
        };
        let a = Answering { chat: a.chat, ..Default::default() };
        self.keep_answer(&a.chat, said);
    }

    /// An answer, into the chat it was asked in -- by its id, whichever chat is open now. A chat
    /// forgotten meanwhile is not made again.
    fn keep_answer(&mut self, chat: &str, said: Said) {
        // Stopped before a word: nothing was answered, and nothing is kept.
        if said.text.trim().is_empty() && said.did.is_empty() {
            return;
        }
        match self.chats.iter_mut().find(|c| c.id == chat) {
            Some(chat) => {
                chat.said.push(said);
                chat.updated = store::now();
                if let Err(e) = chat.save() {
                    log::warn!("the chat could not be kept: {e}");
                }
            }
            None => log::info!("an answer for a chat since forgotten: not kept"),
        }
    }

    /// Ask the brain. `spoken`: when the wearer stopped speaking, and whether it followed on in
    /// the conversation; `None` when typed.
    /// Have hemera ready again once the settings have been left alone a moment.
    fn warm_soon(&mut self) {
        self.warm_due = Some(Instant::now() + Duration::from_secs(2));
    }

    fn ask(&mut self, text: String, spoken: Option<(Instant, bool)>) {
        let ended = spoken.map(|(ended, _)| ended);
        if self.answering.is_some() {
            self.cut_answer();
            self.voice.hush();
        }
        let i = self.chat_for_turn(spoken.map(|(_, follow_up)| follow_up));
        let chat_id = self.chats[i].id.clone();
        // Gone on with on another machine meanwhile (the folder is synchronised): that is the
        // chat, not what this one remembers of it, which would be saved over it.
        if let Some(disk) = Chat::load(&chat_id).filter(|disk| self.chats[i].older_than(disk)) {
            log::info!("chat {chat_id} changed elsewhere: taken as its file has it");
            self.chats[i] = disk;
        }
        let history = self.chats[i].said.clone();
        let chat = &mut self.chats[i];
        chat.said.push(Said { role: "user".into(), text: text.clone(), at: store::now(), ..Default::default() });
        if chat.title.is_empty() {
            chat.title = text.chars().take(48).collect();
        }
        chat.updated = store::now();
        if let Err(e) = chat.save() {
            log::warn!("the chat could not be kept: {e}");
        }
        let looks = self.pack.as_ref().map(pack::Pack::note).unwrap_or_default();
        let turn = Turn { number: 0, text, history, look: std::mem::take(&mut self.look), settings: self.settings.clone(), ai: self.pal_ai(), appearance: self.appearance(), looks, ended };
        self.answering = self.brain.ask(turn).map(|turn| Answering { turn, chat: chat_id, ..Default::default() });
        if self.answering.is_none() {
            self.say_trouble(BRAIN_GONE);
        }
        self.dirty = true;
    }

    /// What the brain said of turn `turn`: only the answer being given is shown.
    fn told(&mut self, turn: u64, told: Told) {
        let Some(a) = self.answering.as_mut().filter(|a| a.turn == turn) else { return };
        match told {
            Told::Text(t) => {
                a.doing = None;
                a.text.push_str(&t);
            }
            Told::Thinking(t) => a.thinking.push_str(&t),
            Told::Doing(name) => a.doing = Some(name),
            Told::Dismissed => self.leaving = true,
            Told::Did(d) => {
                a.doing = None;
                a.did.push(d);
            }
            Told::HandedOver(model) => {
                a.handed_over = true;
                a.model = model;
            }
            Told::Trouble(why) => {
                self.answering = None;
                self.say_trouble(&why);
            }
            Told::Done(said) => {
                let chat = std::mem::take(&mut a.chat);
                self.answering = None;
                self.conversation.touch();
                self.finished_at = Some(Instant::now());
                self.keep_answer(&chat, said);
            }
        }
        self.dirty = true;
    }

    fn heard(&mut self, heard: Heard) {
        match heard {
            Heard::Ear(ear) => {
                // Someone speaking: a model stopped since is started while they talk.
                if ear == Ear::Hearing && self.ear != Ear::Hearing {
                    self.warm(false);
                }
                self.ear = ear;
            }
            // The phrase after speaking over it came to nothing: what is heard later is not
            // checked against its voice.
            Heard::LetGo => self.interrupted = None,
            Heard::Trouble(why) => self.say_trouble(&why),
            Heard::Called => {
                let _ = self.app.room_ask(&json!({"do": "avatar", "wave": true}).to_string(), channel().0);
                // Its name alone: it answers, and listens for what comes next without it --
                // attentive, not smiling as after an answer.
                self.voice.say(if self.ai.dictation.languages.iter().any(|l| l == "id") { "Ya?" } else { "Yes?" });
                self.conversation.touch();
                self.finished_at = None;
            }
            Heard::Interrupted => {
                // Its voice has stopped already (the ears stopped it); the answer stops too, and
                // what they say next is the next thing asked.
                self.interrupted = Some((Instant::now(), self.voice.lately_said()));
                self.cut_answer();
                self.voice.interrupt();
                self.brain.let_go();
                self.finished_at = None;
                log::info!("interrupted: listening to the wearer");
            }
            // Heard over its voice: what was its own voice is not asked.
            Heard::Asked { text, ended, .. } if self.interrupted.as_ref().is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(15)) => {
                let (_, said) = self.interrupted.take().unwrap();
                match ears::not_its_own(&text, &said) {
                    Some(rest) if rest != text => {
                        log::debug!("heard over it {} characters: its own words left out, {} are the wearer's", text.chars().count(), rest.chars().count());
                        self.heard(Heard::Asked { text: rest, ended, follow_up: true });
                    }
                    Some(_) => self.heard(Heard::Asked { text, ended, follow_up: true }),
                    None => log::debug!("heard over it {} characters: its own voice, not the wearer's -- nothing asked", text.chars().count()),
                }
            }
            // "Stop", "quiet", "diam": only that -- nothing is asked of the model.
            Heard::Asked { text, .. } if ears::is_hush(&text) => {
                log::info!("told to be quiet ({}): quiet", store::quoted(&text));
                self.cut_answer();
                self.voice.interrupt();
                // What waited for a yes is not waiting any more.
                self.brain.let_go();
                self.finished_at = None;
            }
            Heard::Asked { text, ended, follow_up } => self.ask(text, Some((ended, follow_up))),
        }
    }

    fn want(&mut self, want: Want) {
        match want {
            Want::Ask(text) => self.ask(text, None),
            Want::NewChat => {
                let chat = Chat { pal: self.settings.pal.clone(), ..Chat::new() };
                self.current = Some(chat.id.clone());
                self.chats.insert(0, chat);
            }
            Want::OpenChat(i) => {
                if let Some(c) = self.chats.get(i) {
                    self.current = Some(c.id.clone());
                }
            }
            // By its id: the chat the wearer chose, wherever it is in the list now. Only it is
            // closed if it was the one open.
            Want::ForgetChat(id) => {
                if let Some(i) = self.chats.iter().position(|c| c.id == id) {
                    self.chats.remove(i).forget();
                    if self.current.as_deref() == Some(id.as_str()) {
                        self.current = None;
                    }
                }
            }
            Want::Listening => {
                // The button listens to the next thing said, name or not, as well as for the name.
                self.settings.listening = true;
                self.tend_ears();
                if let Some(e) = &self.ears {
                    e.once.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            Want::Spoken => {
                self.settings.spoken = !self.settings.spoken;
                self.voice.set_spoken(self.settings.spoken);
                // The wearer's own settings are what is kept, never a pal's over them.
                self.saved.spoken = self.settings.spoken;
                let _ = self.saved.save();
            }
            Want::Look => self.look = !self.look,
            Want::Stop => {
                self.cut_answer();
                self.brain.let_go();
                self.voice.hush();
            }
            Want::Settings => {
                self.ui.page = Page::Settings;
                self.fetch_lists();
            }
            Want::Chat => {
                self.ui.page = Page::Chat;
                // Out of the settings: what they changed is made ready now.
                if self.warm_due.is_some() {
                    self.warm(true);
                }
            }
            Want::Keep(settings) => self.keep(settings),
            Want::Note(note) => {
                self.trouble = Some((note, Instant::now()));
            }
        }
        self.dirty = true;
    }

    fn keep(&mut self, settings: Settings) {
        // What the pal sets is the pal's: the wearer's own is what is kept.
        let saved = match &self.pack {
            Some(p) => p.under(&settings, &self.saved),
            None => settings,
        };
        if let Err(e) = saved.save() {
            self.say_trouble(&e);
        }
        let pal_changed = saved.pal != self.saved.pal;
        self.saved = saved;
        let before = self.settings.clone();
        if pal_changed {
            self.load_pack();
        }
        let settings = self.over(&self.saved);
        self.settings = before;
        self.use_settings(settings, pal_changed);
    }

    /// Settings as used from now on: its voice, ears, window, figure and models made to follow.
    fn use_settings(&mut self, settings: Settings, pal_changed: bool) {
        *self.name.lock().unwrap() = settings.name.clone();
        self.voice.set_spoken(settings.spoken);
        self.voice.set_voice(spoken_voice(&settings), Hemera::new(&self.ai.hemera));
        let listening_changed = settings.listening != self.settings.listening;
        let name_changed = settings.name != self.settings.name;
        // What hemera is asked to have ready: only then is it asked again.
        let wanted = |s: &Settings| (s.speech_model.clone(), s.spoken);
        let models_changed = wanted(&settings) != wanted(&self.settings);
        self.settings = settings;
        if listening_changed {
            self.tend_ears();
        }
        if pal_changed {
            // Another pal: its own chats are the ones shown.
            self.chats = store::chats_with(&self.settings.pal);
            self.current = self.chats.first().map(|c| c.id.clone());
        }
        if pal_changed || name_changed {
            if let Some(w) = self.window {
                self.app.set_title(w, &self.settings.name);
            }
        }
        self.tell_figure();
        // Another voice, or another pal's models and system message: have them ready, once the
        // wearer has stopped stepping through them.
        if models_changed || pal_changed {
            self.warm_soon();
        }
    }

    /// The AI settings changed -- on the room's AI card, or on another machine and synchronised
    /// here: hemera, the models, dictation's languages and words are taken up as they are now,
    /// with no restart. The next turn asks with them; the ears hear with them from now.
    fn follow_ai(&mut self) {
        let ai = spatiand_app::ai::settings();
        if ai == self.ai {
            return;
        }
        log::info!("the AI settings changed: hemera {}, everyday {}, larger {}", ai.hemera, ai.models.everyday, ai.models.larger);
        let hemera_changed = ai.hemera != self.ai.hemera;
        let ears_changed = VoiceSettings::of(&ai) != VoiceSettings::of(&self.ai);
        // What hemera is asked to have ready, or where hemera is.
        let wanted = |a: &AiSettings| (a.hemera.clone(), a.models.everyday.clone(), a.models.larger.clone(), a.models.transcription.clone());
        let models_changed = wanted(&ai) != wanted(&self.ai);
        self.ai = ai;
        if hemera_changed {
            self.voice.set_voice(spoken_voice(&self.settings), Hemera::new(&self.ai.hemera));
        }
        if ears_changed && self.ears.is_some() {
            self.ears = None;
            self.tend_ears();
        }
        if models_changed {
            self.warm_soon();
        }
        self.dirty = true;
    }

    /// The figure, as chosen: which, and whether it is shown.
    fn tell_figure(&mut self) {
        let figure = match &self.pack {
            Some(p) if p.figure.is_some() => p.figure_request(self.settings.shown),
            // A pal of no figure of its own has the one chosen in the settings.
            _ => json!({"do": "avatar", "character": self.settings.character, "path": store::character_path(&self.settings.character), "visible": self.settings.shown, "options": {}, "pose": "", "expression": "", "colors": {}, "size": 1.0, "signs": true}),
        };
        let _ = self.app.room_ask(&figure.to_string(), channel().0);
    }

    /// The room's AI settings as this pal has them: its own everyday and larger models, when it
    /// names any, over the room's.
    fn pal_ai(&self) -> AiSettings {
        match &self.pack {
            Some(p) => p.ai_over(&self.ai),
            None => self.ai.clone(),
        }
    }

    /// The settings as used: the pal's over `saved`.
    fn over(&self, saved: &Settings) -> Settings {
        match &self.pack {
            Some(p) => p.over(saved),
            None => saved.clone(),
        }
    }

    /// The pal chosen in the settings, read; and with it what is said kept out of the log or not.
    fn load_pack(&mut self) {
        self.pack = match self.saved.pal.trim() {
            "" => None,
            folder => match pack::Pack::load(folder) {
                Ok(p) => {
                    log::info!(
                        "pal {folder}: {} groups of options, {} animations, {} expressions{}",
                        p.offers.groups.len(),
                        p.offers.animations.len(),
                        p.offers.expressions.len(),
                        if p.about.private { ", private" } else { "" }
                    );
                    Some(p)
                }
                Err(why) => {
                    self.say_trouble(&format!("The pal {folder} could not be read: {why}"));
                    None
                }
            },
        };
        store::set_private(self.pack.as_ref().is_some_and(|p| p.about.private));
        self.settings = self.over(&self.saved);
        self.pack_looked = Instant::now();
    }

    /// The pal's files, edited (here, or on another machine): read again, and what changed told.
    /// A pal that cannot be read now (a file half synchronised) stays as it was.
    fn look_at_pack(&mut self) {
        if self.pack_looked.elapsed() < Duration::from_secs(10) {
            return;
        }
        self.pack_looked = Instant::now();
        let Some(old) = self.pack.as_mut().filter(|p| p.changed()) else { return };
        let folder = old.folder.clone();
        match pack::Pack::load(&folder) {
            Ok(_) => {
                log::info!("pal {folder}: its files changed: read again");
                let before = self.settings.clone();
                self.load_pack();
                let after = self.settings.clone();
                self.settings = before;
                self.use_settings(after, false);
                // Its persona may be new: the model reads it before it is needed.
                self.warm_soon();
            }
            Err(why) => {
                log::warn!("pal {folder}: its files changed but cannot be read ({why}): it stays as it was");
                old.mark_read();
            }
        }
    }

    /// The pal's `appearance` tool, when its figure offers anything.
    fn appearance(&self) -> Option<Value> {
        self.pack.as_ref().and_then(pack::Pack::tool)
    }

    /// A pal's look changed by its model (`tools.rs`'s `appearance`): kept, and the room told.
    fn appear(&mut self, with: &Value) -> Value {
        let Some(p) = self.pack.as_mut() else { return json!({"error": "there is no figure to change"}) };
        match p.apply(with) {
            Ok((room, did)) => {
                self.app.room_ask(&room.to_string(), channel().0);
                json!({"did": did})
            }
            Err(why) => json!({"error": why}),
        }
    }

    /// What the figure is doing, told to the room when it changes: by name (`state`), and as one
    /// of the moods every room knows (`mood`), for a room that does not know the name yet.
    ///
    /// Its voice heard is talking, whatever else; then what the wearer is saying, in a
    /// conversation (heard, being written); then the answer's work (a tool, the larger model
    /// thinking, the small one). Out of a conversation, a phrase being written is only checked
    /// for the name -- the figure barely stirs, as it may not be for it at all. After an answer
    /// is heard out it smiles a moment, then waits for more: no name needed, until the
    /// conversation lapses.
    fn tend_mood(&mut self) {
        let engaged = self.conversation.open(&self.voice);
        let heard_out = self.finished_at.map(|at| self.voice.quiet_at().map_or(at, |q| q.max(at)));
        let state = match (&self.answering, self.ear) {
            _ if self.voice.speaking() => "talking",
            (_, Ear::Hearing) if engaged => "hearing",
            (_, Ear::Writing) if engaged => "transcribing",
            (Some(a), _) if a.doing.is_some() => "working",
            (Some(a), _) if a.handed_over => "pondering",
            (Some(_), _) => "thinking",
            (None, Ear::Writing) => "checking",
            (None, _) if heard_out.is_some_and(|at| at.elapsed() < Duration::from_millis(1200)) => "happy",
            (None, _) if engaged => "attentive",
            (None, _) if self.ears.is_some() => "listening",
            (None, _) => "idle",
        };
        if state != self.state {
            self.state = state;
            self.dirty = true;
            let _ = self.app.room_ask(&json!({"do": "avatar", "mood": mood_of(state), "state": state}).to_string(), channel().0);
        }
    }

    /// hemera's voices and the figures there are, for the settings' choices. (Its models are
    /// chosen on the room's AI card.)
    fn fetch_lists(&mut self) {
        self.characters = store::characters();
        self.pals = pack::folders();
        let tx = self.lists_tx.clone();
        let waker = self.waker.clone();
        let hemera = Hemera::new(&self.ai.hemera);
        std::thread::spawn(move || {
            let _ = tx.send(ureq_voices(&hemera));
            waker.wake();
        });
    }

    /// Reminders that are due: said, and shown on the room's card.
    fn look_at_reminders(&mut self) {
        if self.reminders_looked.elapsed() < Duration::from_secs(10) {
            return;
        }
        self.reminders_looked = Instant::now();
        let now = store::now();
        // Marked said and saved first, under the reminders' lock: the brain may be setting one.
        let due = store::change_reminders(|all| {
            let mut due = Vec::new();
            for r in all.iter_mut().filter(|r| !r.done && r.due <= now) {
                r.done = true;
                // Long overdue -- the room was off -- is still said, as late.
                let late = now - r.due > 600;
                due.push(if late { format!("A reminder I could not give earlier: {}", r.text) } else { format!("Reminder: {}", r.text) });
            }
            due
        });
        let due = match due {
            Ok(due) => due,
            Err(why) => {
                log::warn!("reminders: {why}");
                return;
            }
        };
        for line in due {
            log::info!("{}", store::quoted(&line));
            self.voice.say(&line);
            let _ = self.app.room_ask(&json!({"do": "notice", "text": format!("{}\n{}", self.settings.name, line)}).to_string(), channel().0);
            self.conversation.touch();
        }
    }

    fn draw(&mut self) -> Result<(), String> {
        let Some(window) = self.window else { return Ok(()) };
        if !self.ready {
            return Ok(());
        }
        let animating = self.answering.is_some();
        // Stop shows while it speaks: drawn again when it starts and stops.
        let speaking = self.voice.busy();
        let speaking_changed = speaking != std::mem::replace(&mut self.drawn_speaking, speaking);
        if !std::mem::take(&mut self.dirty) && !animating && !speaking_changed {
            return Ok(());
        }
        let size = self.size;
        let view = view!(self);
        let (ui, painter) = (&mut self.ui, &mut self.painter);
        let result = self.app.draw(window, |gl, picture| {
            let p = painter.get_or_insert_with(|| Painter::new(gl).expect("the painter"));
            p.begin(gl, picture, picture.1 as f32 / size.1 as f32, theme::ground());
            spatiand_app::ui::look::backdrop(p, gl);
            ui.draw(p, gl, &view);
            p.end(gl);
        });
        result.map(|_| ())
    }

    fn handle(&mut self, event: Event) -> bool {
        if event == Event::AiSettings {
            self.follow_ai();
            return true;
        }
        if let Event::Room { json } = &event {
            if let Ok(said) = serde_json::from_str::<Value>(json) {
                if let Some(trouble) = said.get("trouble").and_then(Value::as_str) {
                    self.say_trouble(trouble);
                    // Its microphone would not open: deaf for now, and asked for again in a
                    // while (the room says nothing when it could open it again).
                    self.ears = None;
                    self.ear = Ear::Quiet;
                    self.listen_again = Some(Instant::now() + LISTEN_AGAIN);
                }
            }
            return true;
        }
        let Some(id) = event.window() else { return true };
        if Some(id) != self.window {
            return true;
        }
        self.dirty = true;
        match &event {
            Event::Ready { .. } => self.ready = true,
            Event::Resized { width, height, .. } => {
                self.size = ((*width).max(900), (*height).max(600));
                self.app.resize(id, self.size);
            }
            // Closed, or Command-Q or Control-Q (the SDK's `Event::Quit`): the same. Quitting is
            // the window's: the room's assistant, switched on in its settings, is not stopped by
            // a key -- switching it off is.
            Event::CloseRequested { .. } | Event::Quit { .. } => {
                self.close_window();
                // Switched on, it stays: its ears and reminders need no window. Otherwise closing
                // its window is how it is shut down.
                return self.settings.on;
            }
            Event::Describe { window } if store::private() => {
                self.app.describe(*window, &spatiand_app::Description { kind: "chat".into(), app: "SpatiPal".into(), title: self.settings.name.clone(), ..Default::default() });
                return true;
            }
            Event::Describe { window } => {
                let text = self.current_index().and_then(|i| self.chats.get(i)).map(|c| c.said.iter().rev().take(6).rev().map(|s| format!("{}: {}", s.role, s.text)).collect::<Vec<_>>().join("\n")).unwrap_or_default();
                self.app.describe(*window, &spatiand_app::Description { kind: "chat".into(), app: "SpatiPal".into(), title: self.settings.name.clone(), text, ..Default::default() });
                return true;
            }
            _ => {}
        }
        let Some(input) = Input::of(&event, 1.0) else { return true };
        let view = view!(self);
        let want = self.ui.input(input, &view);
        if let Some(want) = want {
            self.want(want);
        }
        true
    }

    fn run(&mut self) -> Result<(), String> {
        self.app.watch(self.woken.as_raw_fd());
        if let Some(l) = &self.listener {
            self.app.watch(l.as_raw_fd());
        }
        self.tend_ears();
        if self.warmed.is_none() {
            self.warm(true);
        }
        // Its figure in the room, as chosen.
        self.tell_figure();
        loop {
            let wait = if self.answering.is_some() { 33 } else { 250 };
            for event in self.app.wait(Some(Duration::from_millis(wait)))? {
                if !self.handle(event) {
                    return Ok(());
                }
            }
            let mut drained = [0u8; 64];
            while (&self.woken).read(&mut drained).is_ok_and(|n| n > 0) {}
            for (json, reply) in self.room_asks.try_iter().collect::<Vec<_>>() {
                let asked: Value = serde_json::from_str(&json).unwrap_or_default();
                if asked.get("do").and_then(Value::as_str) == Some("appearance") {
                    let answer = self.appear(asked.get("with").unwrap_or(&Value::Null));
                    let _ = reply.send(answer.to_string());
                    continue;
                }
                self.app.room_ask(&json, reply);
            }
            loop {
                match self.told.try_recv() {
                    Ok((turn, told)) => self.told(turn, told),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    // The brain's thread has ended (only a fault ends it): no answer is coming,
                    // and the window does not wait for one.
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        if self.answering.take().is_some() {
                            self.say_trouble(BRAIN_GONE);
                        }
                        break;
                    }
                }
            }
            for heard in self.heard.try_iter().collect::<Vec<_>>() {
                self.heard(heard);
            }
            if let Ok(voices) = self.lists.try_recv() {
                self.voices = voices;
                self.dirty = true;
            }
            if let Some(l) = &self.listener {
                if let Ok((mut stream, _)) = l.accept() {
                    let mut asked = String::new();
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
                    let _ = stream.read_to_string(&mut asked);
                    if asked.trim() == "window" {
                        self.come_back();
                        self.open_window();
                    }
                }
            }
            if self.leaving && self.answering.is_none() && !self.voice.busy() && !self.leave() {
                return Ok(());
            }
            if self.listen_again.is_some_and(|at| Instant::now() >= at) {
                self.listen_again = None;
                self.tend_ears();
            }
            if self.warm_due.is_some_and(|at| Instant::now() >= at) {
                self.warm(true);
            }
            self.look_at_reminders();
            self.look_at_pack();
            self.tend_mood();
            self.draw()?;
        }
    }
}

/// Which of `chats` a turn goes on in (`Pal::chat_for_turn`), if any: `None` for a new one.
fn chat_to_go_on(chats: &[Chat], open: Option<usize>, spoken: Option<bool>, continue_hours: u32, now: u64) -> Option<usize> {
    let hours = continue_hours as u64 * 3600;
    let recent = |c: &Chat| hours > 0 && now.saturating_sub(c.updated) <= hours;
    match (open, spoken) {
        (Some(i), None | Some(true)) => Some(i),
        (Some(i), Some(false)) => Some(i).filter(|i| recent(&chats[*i])),
        (None, _) => chats.iter().enumerate().max_by_key(|(_, c)| c.updated).filter(|(_, c)| recent(c)).map(|(i, _)| i),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_conversation_goes_on_in_the_last_chat_only_while_it_is_recent() {
        let now = 1_000_000;
        let chat = |hours_ago: u64| Chat { updated: now - hours_ago * 3600, ..Default::default() };
        let chats = [chat(30), chat(2)];
        // Spoken anew, days after the chat open: a new chat; hours after, within 4: that one.
        assert_eq!(chat_to_go_on(&chats, Some(0), Some(false), 4, now), None);
        assert_eq!(chat_to_go_on(&chats, Some(1), Some(false), 4, now), Some(1));
        // Never: a new chat for every conversation -- but a follow-up goes on in its own.
        assert_eq!(chat_to_go_on(&chats, Some(1), Some(false), 0, now), None);
        assert_eq!(chat_to_go_on(&chats, Some(1), Some(true), 0, now), Some(1));
        // Typed: the chat the wearer has open, however old.
        assert_eq!(chat_to_go_on(&chats, Some(0), None, 4, now), Some(0));
        // None open: the latest by when it changed, not the first in the list.
        assert_eq!(chat_to_go_on(&chats, None, Some(false), 4, now), Some(1));
        assert_eq!(chat_to_go_on(&chats, None, Some(false), 1, now), None);
    }
}

/// hemera's voice library, by name.
fn ureq_voices(hemera: &Hemera) -> Vec<String> {
    let Ok(reply) = ureq_get(&format!("{}/v1/audio/voices", hemera.base())) else { return Vec::new() };
    reply.get("data").and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.get("id").and_then(Value::as_str).map(str::to_string)).collect()
}

fn ureq_get(url: &str) -> Result<Value, String> {
    // Through the voice crate's client, which already knows how hemera answers.
    let hemera = Hemera::new(url.rsplit_once("/v1/").map(|(b, _)| b).unwrap_or(url));
    hemera.get_json(&url[hemera.base().len()..])
}

fn spoken_voice(s: &Settings) -> Spoken {
    Spoken {
        model: s.speech_model.clone(),
        name: (!s.voice.is_empty()).then(|| s.voice.clone()),
        described: (!s.described.is_empty()).then(|| s.described.clone()),
    }
}

/// Where the one SpatiPal of this session listens for another start.
fn socket_path() -> std::path::PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join(format!("spatipal-{}.sock", unsafe { libc::getuid() }))
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let background = std::env::var("SPATIPAL_BACKGROUND").as_deref() == Ok("1") || std::env::args().any(|a| a == "--background");
    // Another start: the one running shows its window, and this one goes.
    let path = socket_path();
    if let Ok(mut other) = UnixStream::connect(&path) {
        let _ = other.write_all(b"window");
        return;
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).ok();
    if let Some(l) = &listener {
        let _ = l.set_nonblocking(true);
    }
    let run = || -> Result<(), String> {
        let app = App::connect(APP_ID)?;
        if !app.assisting() {
            log::warn!("not started by the room as its assistant: the room will refuse what it is asked");
        }
        let saved = Settings::load();
        let settings = saved.clone();
        // The room's AI settings: carried over from this app's older files the first time.
        let ai = spatiand_app::ai::settings();
        let (woken, wake_end) = pipe();
        let waker = Waker(Arc::new(wake_end));
        let voice = Voice::start(Hemera::new(&ai.hemera), spoken_voice(&settings), settings.spoken);
        let (asks_tx, room_asks) = channel();
        let w = waker.clone();
        let room = Room { ask: asks_tx, wake: Arc::new(move || w.wake()) };
        let (told_tx, told) = channel();
        let brain = Brain::start(room, voice.clone(), told_tx);
        let (heard_tx, heard) = channel();
        let (lists_tx, lists) = channel();
        let name = Arc::new(Mutex::new(settings.name.clone()));
        let mut pal = Pal {
            app,
            window: None,
            painter: None,
            size: WINDOW,
            ready: false,
            dirty: true,
            ui: Window::new(),
            settings,
            saved,
            pack: None,
            pals: pack::folders(),
            pack_looked: Instant::now(),
            ai,
            chats: Vec::new(),
            current: None,
            answering: None,
            look: false,
            trouble: None,
            voice,
            brain,
            told,
            room_asks,
            ears: None,
            heard_tx,
            heard,
            conversation: Arc::new(Conversation::new()),
            ear: Ear::Quiet,
            warmed: None,
            warm_due: None,
            drawn_speaking: false,
            listen_again: None,
            name,
            waker,
            woken,
            listener,
            reminders_looked: Instant::now() - Duration::from_secs(60),
            voices: Vec::new(),
            characters: store::characters(),
            lists,
            lists_tx,
            leaving: false,
            away: false,
            state: "",
            finished_at: None,
            interrupted: None,
        };
        // The pal chosen, over the settings: its name, voice and chats from the start.
        pal.load_pack();
        *pal.name.lock().unwrap() = pal.settings.name.clone();
        pal.voice.set_voice(spoken_voice(&pal.settings), Hemera::new(&pal.ai.hemera));
        pal.chats = store::chats_with(&pal.settings.pal);
        if !background {
            pal.open_window();
        }
        pal.run()
    };
    let result = run();
    let _ = std::fs::remove_file(&path);
    if let Err(e) = result {
        log::error!("{e}");
        std::process::exit(1);
    }
}
