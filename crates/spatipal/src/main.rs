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
    settings: Settings,
    speech: VoiceSettings,
    chats: Vec<Chat>,
    current: Option<usize>,
    answering: Option<Answering>,
    look: bool,
    trouble: Option<(String, Instant)>,
    voice: Voice,
    brain: Brain,
    told: Receiver<Told>,
    room_asks: Receiver<(String, Sender<String>)>,
    ears: Option<Ears>,
    heard_tx: Sender<Heard>,
    heard: Receiver<Heard>,
    conversation: Arc<Conversation>,
    /// What the ears are doing.
    ear: Ear,
    /// When hemera was last asked to have the models ready.
    warmed: Option<Instant>,
    name: Arc<Mutex<String>>,
    waker: Waker,
    woken: std::fs::File,
    listener: Option<UnixListener>,
    reminders_looked: Instant,
    models: Vec<String>,
    voices: Vec<String>,
    characters: Vec<String>,
    lists: Receiver<(Vec<String>, Vec<String>)>,
    lists_tx: Sender<(Vec<String>, Vec<String>)>,
    /// What the figure was last told it is doing, and when the last answer finished (it smiles
    /// once it is heard out).
    state: &'static str,
    finished_at: Option<Instant>,
}

/// What the window is lent to draw from: built from the fields themselves, so that the app and
/// the window's own state can be borrowed beside it.
macro_rules! view {
    ($pal:expr) => {
        View {
            chats: &$pal.chats,
            current: $pal.current,
            answering: $pal.answering.as_ref(),
            settings: &$pal.settings,
            speech: &$pal.speech,
            listening: $pal.ears.is_some(),
            look: $pal.look,
            status: status($pal.state, &$pal.settings),
            trouble: $pal.trouble.as_ref().filter(|(_, at)| at.elapsed() < Duration::from_secs(12)).map(|(t, _)| t.as_str()),
            models: &$pal.models,
            voices: &$pal.voices,
            characters: &$pal.characters,
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
        self.chats = store::chats();
        self.current = self.current.or(if self.chats.is_empty() { None } else { Some(0) });
    }

    fn close_window(&mut self) {
        if let Some(w) = self.window.take() {
            self.app.close(w);
        }
        self.painter = None;
    }

    /// Listen for its name, or stop, as the settings say.
    fn tend_ears(&mut self) {
        match (self.settings.listening, self.ears.is_some()) {
            (true, false) => {
                let Some(microphone) = self.app.room_listen() else {
                    self.say_trouble("the room would not give its microphone");
                    self.settings.listening = false;
                    return;
                };
                let waker = self.waker.clone();
                self.ears = Some(ears::start(
                    microphone,
                    self.speech.clone(),
                    self.name.clone(),
                    self.voice.clone(),
                    self.conversation.clone(),
                    self.heard_tx.clone(),
                    move || waker.wake(),
                ));
                self.warm(true);
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
        self.trouble = Some((why.to_string(), Instant::now()));
        self.dirty = true;
    }

    /// The chat a new turn goes in: the last one if it was recent enough, else a new one.
    fn chat_for_turn(&mut self) -> usize {
        if let Some(i) = self.current {
            return i;
        }
        let recent = self.chats.first().is_some_and(|c| store::now().saturating_sub(c.updated) <= self.settings.continue_hours as u64 * 3600);
        if recent {
            self.current = Some(0);
            return 0;
        }
        self.chats.insert(0, Chat::new());
        self.current = Some(0);
        0
    }

    /// Have hemera ready with what the next answer needs (`crate::warm`); not again for a few
    /// minutes unless `now`.
    fn warm(&mut self, now: bool) {
        if !now && self.warmed.is_some_and(|at| at.elapsed() < warm::EVERY) {
            return;
        }
        self.warmed = Some(Instant::now());
        let mut models = Vec::new();
        if self.ears.is_some() {
            models.push(self.speech.transcription_model.clone());
        }
        if self.settings.spoken {
            models.push(self.settings.speech_model.clone());
        }
        let wanted = warm::Wanted {
            models,
            small: self.settings.model.clone(),
            prime: brain::prime(&self.settings, false),
            large: self.settings.large_model.clone(),
            prime_large: brain::prime(&self.settings, true),
        };
        warm::warm(Hemera::new(&self.speech.hemera), wanted, now);
    }

    fn ask(&mut self, text: String, ended: Option<Instant>) {
        if self.answering.is_some() {
            self.brain.stop();
            self.voice.hush();
        }
        let i = self.chat_for_turn();
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
        self.answering = Some(Answering::default());
        self.brain.ask(Turn { text, history, look: std::mem::take(&mut self.look), settings: self.settings.clone(), ended });
        self.dirty = true;
    }

    fn told(&mut self, told: Told) {
        let Some(a) = self.answering.as_mut() else { return };
        match told {
            Told::Text(t) => {
                a.doing = None;
                a.text.push_str(&t);
            }
            Told::Thinking(t) => a.thinking.push_str(&t),
            Told::Doing(name) => a.doing = Some(name),
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
                self.answering = None;
                self.conversation.touch();
                self.finished_at = Some(Instant::now());
                if let Some(chat) = self.current.and_then(|i| self.chats.get_mut(i)) {
                    chat.said.push(said);
                    chat.updated = store::now();
                    if let Err(e) = chat.save() {
                        log::warn!("the chat could not be kept: {e}");
                    }
                }
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
            Heard::Called => {
                let _ = self.app.room_ask(&json!({"do": "avatar", "wave": true}).to_string(), channel().0);
                // Its name alone: it answers, and listens for what comes next without it --
                // attentive, not smiling as after an answer.
                self.voice.say(if self.speech.languages.iter().any(|l| l == "id") { "Ya?" } else { "Yes?" });
                self.conversation.touch();
                self.finished_at = None;
            }
            Heard::Asked { text, ended } => self.ask(text, Some(ended)),
        }
    }

    fn want(&mut self, want: Want) {
        match want {
            Want::Ask(text) => self.ask(text, None),
            Want::NewChat => {
                self.current = None;
                self.chats.insert(0, Chat::new());
                self.current = Some(0);
            }
            Want::OpenChat(i) => self.current = Some(i).filter(|i| *i < self.chats.len()),
            Want::ForgetChat(i) => {
                if i < self.chats.len() {
                    self.chats.remove(i).forget();
                    self.current = None;
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
                let _ = self.settings.save();
            }
            Want::Look => self.look = !self.look,
            Want::Stop => {
                self.brain.stop();
                self.voice.hush();
            }
            Want::Settings => {
                self.ui.page = Page::Settings;
                self.fetch_lists();
            }
            Want::Chat => self.ui.page = Page::Chat,
            Want::Keep(settings, speech) => self.keep(settings, speech),
        }
        self.dirty = true;
    }

    fn keep(&mut self, settings: Settings, speech: VoiceSettings) {
        if let Err(e) = settings.save() {
            self.say_trouble(&e);
        }
        if let Err(e) = speech.save(&store::shared()) {
            self.say_trouble(&e);
        }
        *self.name.lock().unwrap() = settings.name.clone();
        self.voice.set_spoken(settings.spoken);
        self.voice.set_voice(spoken_voice(&settings), Hemera::new(&speech.hemera));
        let listening_changed = settings.listening != self.settings.listening;
        let ears_changed = speech != self.speech;
        self.settings = settings;
        self.speech = speech;
        if ears_changed {
            self.ears = None;
        }
        if listening_changed || ears_changed {
            self.tend_ears();
        }
        self.tell_figure();
        // Other models, perhaps: have them ready.
        self.warm(true);
    }

    /// The figure, as chosen: which, and whether it is shown.
    fn tell_figure(&mut self) {
        let figure = json!({"do": "avatar", "character": self.settings.character, "path": store::character_path(&self.settings.character), "visible": self.settings.shown});
        let _ = self.app.room_ask(&figure.to_string(), channel().0);
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

    /// hemera's text models and voices, for the settings' choices.
    fn fetch_lists(&mut self) {
        self.characters = store::characters();
        let tx = self.lists_tx.clone();
        let waker = self.waker.clone();
        let hemera = Hemera::new(&self.speech.hemera);
        std::thread::spawn(move || {
            let models = hemera.models("text").unwrap_or_default();
            let voices = ureq_voices(&hemera);
            let _ = tx.send((models, voices));
            waker.wake();
        });
    }

    /// Reminders that are due: said, and shown on the room's card.
    fn look_at_reminders(&mut self) {
        if self.reminders_looked.elapsed() < Duration::from_secs(10) {
            return;
        }
        self.reminders_looked = Instant::now();
        let mut all = store::reminders();
        let now = store::now();
        let mut changed = false;
        for r in all.iter_mut().filter(|r| !r.done && r.due <= now) {
            r.done = true;
            changed = true;
            // Long overdue -- the room was off -- is still said, as late.
            let late = now - r.due > 600;
            let line = if late { format!("A reminder I could not give earlier: {}", r.text) } else { format!("Reminder: {}", r.text) };
            log::info!("{line}");
            self.voice.say(&line);
            let _ = self.app.room_ask(&json!({"do": "notice", "text": format!("{}\n{}", self.settings.name, line)}).to_string(), channel().0);
            self.conversation.touch();
        }
        if changed {
            let _ = store::save_reminders(&all);
        }
    }

    fn draw(&mut self) -> Result<(), String> {
        let Some(window) = self.window else { return Ok(()) };
        if !self.ready {
            return Ok(());
        }
        let animating = self.answering.is_some();
        if !std::mem::take(&mut self.dirty) && !animating {
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
        if let Event::Room { json } = &event {
            if let Ok(said) = serde_json::from_str::<Value>(json) {
                if let Some(trouble) = said.get("trouble").and_then(Value::as_str) {
                    self.say_trouble(trouble);
                    self.ears = None;
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
            Event::CloseRequested { .. } => {
                self.close_window();
                // Switched on, it stays: its ears and reminders need no window.
                return self.settings.on || self.ears.is_some();
            }
            Event::Describe { window } => {
                let text = self.current.and_then(|i| self.chats.get(i)).map(|c| c.said.iter().rev().take(6).rev().map(|s| format!("{}: {}", s.role, s.text)).collect::<Vec<_>>().join("\n")).unwrap_or_default();
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
                self.app.room_ask(&json, reply);
            }
            for told in self.told.try_iter().collect::<Vec<_>>() {
                self.told(told);
            }
            for heard in self.heard.try_iter().collect::<Vec<_>>() {
                self.heard(heard);
            }
            if let Ok((models, voices)) = self.lists.try_recv() {
                self.models = models;
                self.voices = voices;
                self.dirty = true;
            }
            if let Some(l) = &self.listener {
                if let Ok((mut stream, _)) = l.accept() {
                    let mut asked = String::new();
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
                    let _ = stream.read_to_string(&mut asked);
                    if asked.trim() == "window" {
                        self.open_window();
                    }
                }
            }
            self.look_at_reminders();
            self.tend_mood();
            self.draw()?;
        }
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
        let settings = Settings::load();
        let speech = VoiceSettings::load(&store::shared());
        let (woken, wake_end) = pipe();
        let waker = Waker(Arc::new(wake_end));
        let voice = Voice::start(Hemera::new(&speech.hemera), spoken_voice(&settings), settings.spoken);
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
            speech,
            chats: store::chats(),
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
            name,
            waker,
            woken,
            listener,
            reminders_looked: Instant::now() - Duration::from_secs(60),
            models: Vec::new(),
            voices: Vec::new(),
            characters: store::characters(),
            lists,
            lists_tx,
            state: "",
            finished_at: None,
        };
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
