//! SpatiPal's window: its chats down the left, the conversation in the middle, a field to type
//! in at the foot -- and its settings, dictation's among them, behind the cog.
//!
//! Drawn with the room's own widgets (`spatiand_app::ui`), so it looks like the rest of Spatiand.

use spatiand_app::ui::{look, theme, Button, Confirm, Field, Icon, Input, List, Painter, Rect, Row, Typed};
use spatiand_app::Gl;
use spatiand_voice::VoiceSettings;

use crate::store::{Chat, Said, Settings};

/// The answer being given, as it comes.
#[derive(Default)]
pub struct Answering {
    /// The brain's number for the turn: what it says of any other is not this answer's.
    pub turn: u64,
    /// The chat it was asked in, by its id: it is kept there, whichever is open when it ends.
    pub chat: String,
    pub text: String,
    pub thinking: String,
    pub did: Vec<String>,
    pub model: String,
    pub handed_over: bool,
    /// The tool being done now, by name.
    pub doing: Option<String>,
}

/// What the window shows, lent to it each frame.
pub struct View<'a> {
    pub chats: &'a [Chat],
    pub current: Option<usize>,
    pub answering: Option<&'a Answering>,
    /// Its voice is speaking, or has something still to say: Stop stops it.
    pub speaking: bool,
    pub settings: &'a Settings,
    pub speech: &'a VoiceSettings,
    pub listening: bool,
    pub look: bool,
    pub status: &'a str,
    pub trouble: Option<&'a str>,
    /// Choices fetched from hemera: text models, voices.
    pub models: &'a [String],
    pub voices: &'a [String],
    pub characters: &'a [String],
}

/// What the wearer asked for.
pub enum Want {
    Ask(String),
    NewChat,
    OpenChat(usize),
    /// Forget a chat, by its id.
    ForgetChat(String),
    Listening,
    Spoken,
    Look,
    Stop,
    Settings,
    Chat,
    /// The settings, changed: to keep.
    Keep(Settings, VoiceSettings),
}

#[derive(PartialEq, Clone, Copy)]
pub enum Page {
    Chat,
    Settings,
}

/// One line of the settings page.
#[derive(Clone, Copy, PartialEq)]
enum Line {
    On,
    Shown,
    Name,
    Persona,
    Figure,
    Voice,
    Described,
    Spoken,
    Listening,
    Thinking,
    Continue,
    Model,
    Large,
    Languages,
    Words,
    Cleanup,
    Hemera,
}

const LINES: &[(&str, &[Line])] = &[
    ("Assistant", &[Line::On, Line::Name, Line::Persona, Line::Figure, Line::Shown]),
    ("Voice and ears", &[Line::Voice, Line::Described, Line::Spoken, Line::Listening, Line::Thinking]),
    ("Models and chats", &[Line::Model, Line::Large, Line::Continue]),
    ("Dictation", &[Line::Languages, Line::Words, Line::Cleanup, Line::Hemera]),
];

pub struct Window {
    pub page: Page,
    chats: List,
    pub field: Field,
    send: Button,
    listen: Button,
    mute: Button,
    look: Button,
    cog: Button,
    new_chat: Button,
    back: Button,
    stop: Button,
    /// How far the conversation is scrolled up from its foot, in pixels.
    scroll: f32,
    /// How tall the conversation was when last drawn, and the place it was drawn in.
    tall: f32,
    talk: Rect,
    // The settings page.
    lines: List,
    editing: Option<Line>,
    edit: Field,
    pointer: (f32, f32),
    /// The bin on the chat under the pointer: pressing it asks whether to delete the chat.
    forget: Button,
    forget_row: Option<usize>,
    /// The question being asked, and the id of the chat it is about.
    asking: Option<(String, Confirm)>,
}

impl Window {
    pub fn new() -> Window {
        Window {
            page: Page::Chat,
            chats: List::default(),
            field: {
                let mut f = Field::default();
                f.hint = "Ask, or say its name\u{2026}".into();
                f.focused = true;
                f
            },
            send: Button::default(),
            listen: Button::default(),
            mute: Button::default(),
            look: Button::default(),
            cog: Button::default(),
            new_chat: Button::default(),
            back: Button::default(),
            stop: Button::default(),
            scroll: 0.0,
            tall: 0.0,
            talk: Rect::default(),
            lines: List::default(),
            editing: None,
            edit: Field::default(),
            pointer: (0.0, 0.0),
            forget: Button::default(),
            forget_row: None,
            asking: None,
        }
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, view: &View) {
        let mut all = p.bounds().inset(theme::PAD);
        // The app's name heads its first screen; Settings has its own head, with the way back.
        if self.page == Page::Chat {
            let beside = view.status.to_string();
            look::header(p, gl, &mut all, &view.settings.name, &beside);
        }
        match self.page {
            Page::Chat => self.draw_chat(p, gl, all, view),
            Page::Settings => self.draw_settings(p, gl, all, view),
        }
        if let Some((_, question)) = &mut self.asking {
            question.draw(p, gl, p.bounds());
        }
    }

    fn draw_chat(&mut self, p: &mut Painter, gl: &Gl, mut all: Rect, view: &View) {
        // The foot: the field and its buttons.
        let mut foot = all.take_bottom(Button::HEIGHT);
        all.take_bottom(16.0);
        let gap = 10.0;
        let round = Button::HEIGHT;
        let cog = foot.take_right(round);
        foot.take_right(gap);
        let new_chat = foot.take_right(round);
        foot.take_right(gap);
        let look_at = foot.take_right(round);
        foot.take_right(gap);
        let mute = foot.take_right(round);
        foot.take_right(gap);
        let listen = foot.take_right(round);
        foot.take_right(gap);
        let busy = view.answering.is_some() || view.speaking;
        let send_w = Button::width(p, gl, if busy { "Stop" } else { "Send" });
        let send = foot.take_right(send_w);
        foot.take_right(gap);
        self.field.draw(p, gl, foot);
        if busy {
            self.stop.draw(p, gl, send, "Stop");
        } else {
            self.send.draw(p, gl, send, "Send");
        }
        // Lit when on. Lit only to be drawn: a focused button also takes Space and Enter, and the
        // field is where those go.
        self.listen.focused = view.listening;
        self.listen.draw_icon(p, gl, listen, Icon::Microphone, "");
        self.mute.focused = !view.settings.spoken;
        self.mute.draw_icon(p, gl, mute, if view.settings.spoken { Icon::Speaker } else { Icon::Muted }, "");
        self.look.focused = view.look;
        self.look.draw_icon(p, gl, look_at, Icon::Eye, "");
        for b in [&mut self.listen, &mut self.mute, &mut self.look] {
            b.focused = false;
        }
        self.new_chat.draw_icon(p, gl, new_chat, Icon::Plus, "");
        self.cog.draw_icon(p, gl, cog, Icon::Cog, "");

        if let Some(trouble) = view.trouble {
            let line = all.take_bottom(theme::FOOTER_EM * 1.8);
            p.label(gl, trouble, line, theme::FOOTER_EM, look::trouble(), 0.0);
        }

        // The chats, down the left.
        let side = all.take_left((all.w * 0.26).clamp(260.0, 420.0));
        all.take_left(24.0);
        let rows: Vec<Row> = view
            .chats
            .iter()
            .map(|c| Row::with(if c.title.is_empty() { "New chat".to_string() } else { c.title.clone() }, crate::time::short(c.updated)))
            .collect();
        let hovered = self.chats.hovered();
        let mut rows = rows;
        // The chat under the pointer shows its bin where its time was.
        if let Some(row) = hovered.and_then(|i| rows.get_mut(i)) {
            row.trailing = None;
        }
        self.chats.draw(p, gl, side, &rows);
        if let Some(i) = view.current {
            if let Some(r) = self.chats.row_rect(i) {
                p.fill(gl, Rect { w: 4.0, ..r }, 2.0, look::TEAL);
            }
        }
        self.forget_row = None;
        if let Some(i) = hovered {
            if let Some(r) = self.chats.row_rect(i) {
                let w = 44.0;
                let at = Rect { x: r.x + r.w - w - 6.0, y: r.y + (r.h - 44.0).max(0.0) / 2.0, w, h: 44.0_f32.min(r.h) };
                self.forget.draw_icon(p, gl, at, Icon::Trash, "");
                self.forget_row = Some(i);
            }
        }

        // The conversation, its foot at the field.
        self.talk = all;
        let width = (all.w - 40.0).min(1100.0);
        let said: Vec<Said> = view.current.and_then(|i| view.chats.get(i)).map(|c| c.said.clone()).unwrap_or_default();
        let mut blocks: Vec<Block> = said.iter().map(Block::of).collect();
        if let Some(a) = view.answering {
            blocks.push(Block {
                mine: false,
                text: if a.text.is_empty() && a.thinking.is_empty() { "\u{2026}".into() } else { shown(&a.text) },
                thinking: a.thinking.clone(),
                did: a.did.clone(),
                model: if a.handed_over { a.model.clone() } else { String::new() },
            });
        }
        if blocks.is_empty() {
            look::message(p, gl, all, &format!("Say \u{201C}{}\u{201D}, or type below.", view.settings.name), theme::ink_hint());
            self.tall = 0.0;
            return;
        }
        let heights: Vec<f32> = blocks.iter().map(|b| b.height(p, gl, width)).collect();
        let tall: f32 = heights.iter().sum::<f32>() + 18.0 * blocks.len() as f32;
        self.tall = tall;
        self.scroll = self.scroll.clamp(0.0, (tall - all.h).max(0.0));
        p.clip(gl, all);
        let mut y = all.bottom() - tall + self.scroll;
        for (block, h) in blocks.iter().zip(heights) {
            if y + h >= all.y && y <= all.bottom() {
                block.draw(p, gl, Rect::new(all.x, y, all.w, h), width);
            }
            y += h + 18.0;
        }
        p.unclip(gl);
        if tall > all.h {
            look::scrollbar(p, gl, Rect { x: all.right() - theme::SCROLL_WIDTH, w: theme::SCROLL_WIDTH, ..all }, all.h, tall, tall - all.h - self.scroll);
        }
    }

    fn draw_settings(&mut self, p: &mut Painter, gl: &Gl, mut all: Rect, view: &View) {
        look::top(p, gl, &mut all, &mut self.back, "Back", Icon::Back, "Settings", "");
        let mut foot = all.take_bottom(Button::HEIGHT);
        all.take_bottom(16.0);
        if let Some(line) = self.editing {
            self.edit.hint = label(line).to_string();
            self.edit.focused = true;
            self.edit.draw(p, gl, foot);
            let keys = all.take_bottom(theme::FOOTER_EM * 1.8);
            look::keys(p, gl, keys, "[Enter] Keep  [Esc] Drop", 0.0, 1.0);
        } else {
            p.label(gl, "Choose a line to change it.", foot, theme::FOOTER_EM, theme::ink_hint(), 0.0);
        }
        let mut rows = Vec::new();
        for (heading, lines) in LINES {
            rows.push(Row::heading(*heading));
            for line in *lines {
                rows.push(Row::with(label(*line), value(*line, view)));
            }
        }
        self.lines.draw(p, gl, all, &rows);
    }

    /// The line of the settings page at row `index`, headings counted.
    fn line_at(index: usize) -> Option<Line> {
        let mut at = 0;
        for (_, lines) in LINES {
            at += 1;
            if index < at + lines.len() && index >= at {
                return Some(lines[index - at]);
            }
            at += lines.len();
        }
        None
    }

    pub fn input(&mut self, input: Input, view: &View) -> Option<Want> {
        if let Input::Pointer { x, y } = input {
            self.pointer = (x, y);
        }
        // A question being asked has every input until it is answered.
        if let Some((id, question)) = &mut self.asking {
            match question.input(&input) {
                Some(yes) => {
                    let id = id.clone();
                    self.asking = None;
                    return yes.then_some(Want::ForgetChat(id));
                }
                None => return None,
            }
        }
        match self.page {
            Page::Chat => self.chat_input(input, view),
            Page::Settings => self.settings_input(input, view),
        }
    }

    fn chat_input(&mut self, input: Input, view: &View) -> Option<Want> {
        if let Input::Scroll { dy, .. } = input {
            if self.talk.contains(self.pointer.0, self.pointer.1) {
                // Content follows the fingers: scrolling up shows what came before.
                self.scroll = (self.scroll - dy).clamp(0.0, (self.tall - self.talk.h).max(0.0));
                return None;
            }
        }
        // The bin on a chat, before the list: a press on it is not a press on the chat.
        if let Some(i) = self.forget_row {
            let on_bin = self.forget.contains(self.pointer.0, self.pointer.1);
            match input {
                Input::Press { pressed: true, .. } if on_bin => {
                    self.forget.input(&input);
                    return None;
                }
                Input::Press { pressed: false, .. } if self.forget.input(&input) => {
                    // By the chat's id: the list may have moved under the pointer between the
                    // two presses.
                    let Some(id) = view.chats.get(i).map(|c| c.id.clone()) else { return None };
                    return self.ask_to_forget(id);
                }
                Input::Pointer { .. } | Input::PointerLeft => {
                    self.forget.input(&input);
                }
                _ => {}
            }
        }
        if let Some(row) = self.chats.input(&input) {
            return Some(Want::OpenChat(row));
        }
        if self.listen.input(&input) {
            return Some(Want::Listening);
        }
        if self.mute.input(&input) {
            return Some(Want::Spoken);
        }
        if self.look.input(&input) {
            return Some(Want::Look);
        }
        if self.new_chat.input(&input) {
            self.scroll = 0.0;
            return Some(Want::NewChat);
        }
        if self.cog.input(&input) {
            return Some(Want::Settings);
        }
        if view.answering.is_some() || view.speaking {
            if self.stop.input(&input) {
                return Some(Want::Stop);
            }
        } else if self.send.input(&input) {
            return self.take_field();
        }
        // A chat is deleted with its bin, or Control/Super-Delete on the one open -- asked about
        // first, as the bin does, and only with nothing typed: in the field it is the field's
        // (Control-Delete deletes the word ahead). Control/Super-N starts a new chat.
        if let Input::Key { key: spatiand_app::Key::Char('n'), modifiers, .. } = &input {
            if modifiers.logo || modifiers.control {
                self.scroll = 0.0;
                return Some(Want::NewChat);
            }
        }
        if let Input::Key { key: spatiand_app::Key::Delete, modifiers, .. } = &input {
            if (modifiers.logo || modifiers.control) && self.field.text.is_empty() {
                if let Some(id) = view.current.and_then(|i| view.chats.get(i)).map(|c| c.id.clone()) {
                    return self.ask_to_forget(id);
                }
            }
        }
        if let Some(Typed::Entered) = self.field.input(&input) {
            return self.take_field();
        }
        None
    }

    /// Asks whether to delete the chat `id`: the bin and Control/Super-Delete come here.
    fn ask_to_forget(&mut self, id: String) -> Option<Want> {
        self.asking = Some((id, Confirm::careful("Delete this chat? It cannot be had back.", "Delete", "Cancel")));
        None
    }

    fn take_field(&mut self) -> Option<Want> {
        let text = self.field.text.trim().to_string();
        if text.is_empty() {
            return None;
        }
        self.field.set("");
        self.scroll = 0.0;
        Some(Want::Ask(text))
    }

    fn settings_input(&mut self, input: Input, view: &View) -> Option<Want> {
        // A setting being typed is kept when the wearer goes elsewhere -- a press outside its
        // field, or Back -- as well as on Enter; only Escape drops it. (It was kept on Enter
        // alone, and a hemera address typed and left was silently lost, 2026-11-15.)
        let leaving = matches!(input, Input::Press { pressed: true, .. }) && !self.edit.contains(self.pointer.0, self.pointer.1);
        if let (Some(line), true) = (self.editing, leaving) {
            let text = self.edit.text.trim().to_string();
            self.editing = None;
            let (mut s, mut v) = (view.settings.clone(), view.speech.clone());
            set_text(line, &text, &mut s, &mut v);
            // The press is also the start of what it was aimed at (Back, another line): the
            // field gives it up now and the release does the rest, as before.
            self.back.input(&input);
            self.lines.input(&input);
            return Some(Want::Keep(s, v));
        }
        if self.back.input(&input) {
            self.editing = None;
            return Some(Want::Chat);
        }
        if let Some(line) = self.editing {
            match self.edit.input(&input) {
                Some(Typed::Entered) => {
                    let text = self.edit.text.trim().to_string();
                    self.editing = None;
                    let (mut s, mut v) = (view.settings.clone(), view.speech.clone());
                    set_text(line, &text, &mut s, &mut v);
                    return Some(Want::Keep(s, v));
                }
                Some(Typed::Changed) => return None,
                None => {}
            }
            if let Input::Key { key: spatiand_app::Key::Escape, .. } = input {
                self.editing = None;
                return None;
            }
        } else if let Input::Key { key: spatiand_app::Key::Escape, .. } = input {
            return Some(Want::Chat);
        }
        let row = self.lines.input(&input)?;
        let line = Self::line_at(row)?;
        let (mut s, mut v) = (view.settings.clone(), view.speech.clone());
        match line {
            Line::Name | Line::Persona | Line::Described | Line::Words | Line::Hemera => {
                self.editing = Some(line);
                self.edit.set(text_of(line, view));
                self.edit.select_all();
                None
            }
            Line::On => {
                s.on = !s.on;
                Some(Want::Keep(s, v))
            }
            Line::Spoken => {
                s.spoken = !s.spoken;
                Some(Want::Keep(s, v))
            }
            Line::Shown => {
                s.shown = !s.shown;
                Some(Want::Keep(s, v))
            }
            Line::Listening => {
                s.listening = !s.listening;
                Some(Want::Keep(s, v))
            }
            Line::Thinking => {
                s.thinking = next(&["quiet", "tone", "read"], &s.thinking);
                Some(Want::Keep(s, v))
            }
            Line::Continue => {
                let choices = [0u32, 1, 4, 12, 24, 168, 100_000];
                let at = choices.iter().position(|c| *c == s.continue_hours).map(|i| (i + 1) % choices.len()).unwrap_or(2);
                s.continue_hours = choices[at];
                Some(Want::Keep(s, v))
            }
            Line::Figure => {
                s.character = next(view.characters, &s.character);
                Some(Want::Keep(s, v))
            }
            Line::Voice => {
                let mut choices = vec![String::new()];
                choices.extend(view.voices.iter().cloned());
                s.voice = next(&choices, &s.voice);
                Some(Want::Keep(s, v))
            }
            Line::Model => {
                s.model = next(view.models, &s.model);
                Some(Want::Keep(s, v))
            }
            Line::Large => {
                s.large_model = next(view.models, &s.large_model);
                Some(Want::Keep(s, v))
            }
            Line::Cleanup => {
                v.cleanup_model = next(view.models, &v.cleanup_model);
                Some(Want::Keep(s, v))
            }
            Line::Languages => {
                let presets: [&[&str]; 4] = [&["en", "id"], &["en"], &["id"], &[]];
                let now: Vec<&str> = v.languages.iter().map(String::as_str).collect();
                let at = presets.iter().position(|p| *p == now.as_slice()).map(|i| (i + 1) % presets.len()).unwrap_or(0);
                v.languages = presets[at].iter().map(|l| l.to_string()).collect();
                Some(Want::Keep(s, v))
            }
        }
    }
}

/// The one after `now` in `choices`, round again at the end.
fn next<S: AsRef<str>>(choices: &[S], now: &str) -> String {
    if choices.is_empty() {
        return now.to_string();
    }
    let at = choices.iter().position(|c| c.as_ref() == now).map(|i| (i + 1) % choices.len()).unwrap_or(0);
    choices[at].as_ref().to_string()
}

fn label(line: Line) -> &'static str {
    match line {
        Line::On => "Start with the room",
        Line::Shown => "Show its figure",
        Line::Name => "Name (what it answers to)",
        Line::Persona => "Personality",
        Line::Figure => "Figure",
        Line::Voice => "Voice",
        Line::Described => "Voice described",
        Line::Spoken => "Speak answers",
        Line::Listening => "Listen for its name",
        Line::Thinking => "While the larger model thinks",
        Line::Continue => "Go on in the last chat",
        Line::Model => "Everyday model",
        Line::Large => "Larger model",
        Line::Languages => "Languages spoken",
        Line::Words => "Your own words",
        Line::Cleanup => "Terminal clean-up model",
        Line::Hemera => "hemera",
    }
}

fn on(b: bool) -> String {
    if b { "On".into() } else { "Off".into() }
}

fn value(line: Line, view: &View) -> String {
    let (s, v) = (view.settings, view.speech);
    match line {
        Line::On => on(s.on),
        Line::Shown => if s.shown { "Shown".into() } else { "Hidden (still listening)".into() },
        Line::Name => s.name.clone(),
        Line::Persona => short(&s.persona),
        Line::Figure => s.character.clone(),
        Line::Voice => if s.voice.is_empty() { "Described in words".into() } else { s.voice.clone() },
        Line::Described => short(&s.described),
        Line::Spoken => on(s.spoken),
        Line::Listening => on(s.listening),
        Line::Thinking => match s.thinking.as_str() {
            "quiet" => "Show it".into(),
            "read" => "Read the thinking aloud".into(),
            _ => "A soft tone".into(),
        },
        Line::Continue => match s.continue_hours {
            0 => "Never: a new chat each time".into(),
            h if h >= 100_000 => "Always".into(),
            h => format!("If within {h} h"),
        },
        Line::Model => s.model.clone(),
        Line::Large => s.large_model.clone(),
        Line::Languages => match v.languages.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["en", "id"] => "English and Indonesian, mixed".into(),
            ["en"] => "English".into(),
            ["id"] => "Indonesian".into(),
            [] => "Any (Whisper decides)".into(),
            other => other.join(", "),
        },
        Line::Words => short(&v.words.join(", ")),
        Line::Cleanup => v.cleanup_model.clone(),
        Line::Hemera => v.hemera.clone(),
    }
}

fn text_of(line: Line, view: &View) -> String {
    let (s, v) = (view.settings, view.speech);
    match line {
        Line::Name => s.name.clone(),
        Line::Persona => s.persona.clone(),
        Line::Described => s.described.clone(),
        Line::Words => v.words.join(", "),
        Line::Hemera => v.hemera.clone(),
        _ => String::new(),
    }
}

fn set_text(line: Line, text: &str, s: &mut Settings, v: &mut VoiceSettings) {
    match line {
        Line::Name if !text.is_empty() => s.name = text.to_string(),
        Line::Persona => s.persona = text.to_string(),
        Line::Described => s.described = text.to_string(),
        Line::Words => v.words = text.split(',').map(str::trim).filter(|w| !w.is_empty()).map(str::to_string).collect(),
        Line::Hemera if !text.is_empty() => v.hemera = text.to_string(),
        _ => {}
    }
}

fn short(text: &str) -> String {
    let mut t: String = text.chars().take(48).collect();
    if text.chars().count() > 48 {
        t.push('\u{2026}');
    }
    t
}

/// One thing said, as drawn.
struct Block {
    mine: bool,
    text: String,
    thinking: String,
    did: Vec<String>,
    /// The larger model's name, when it answered.
    model: String,
}

const TEXT_EM: f32 = theme::ROW_EM * 0.82;
const SMALL_EM: f32 = theme::DETAIL_EM * 0.9;
const BUBBLE_PAD: f32 = 18.0;
/// How much of the larger model's thinking is shown: the end of it, as it goes.
const THINKING_SHOWN: usize = 600;

impl Block {
    fn of(said: &Said) -> Block {
        Block { mine: said.role == "user", text: shown(&said.text), thinking: said.thinking.clone(), did: said.did.clone(), model: String::new() }
    }

    fn did_line(&self) -> String {
        format!("\u{2713} {}", self.did.join(" \u{00B7} "))
    }

    fn thinking_shown(&self) -> String {
        let t = self.thinking.trim();
        let n = t.chars().count();
        if n > THINKING_SHOWN {
            format!("\u{2026}{}", t.chars().skip(n - THINKING_SHOWN).collect::<String>())
        } else {
            t.to_string()
        }
    }

    fn height(&self, p: &mut Painter, gl: &Gl, width: f32) -> f32 {
        let inner = width * if self.mine { 0.78 } else { 0.92 } - BUBBLE_PAD * 2.0;
        let mut h = BUBBLE_PAD * 2.0;
        if !self.model.is_empty() {
            h += SMALL_EM * 1.4;
        }
        if !self.thinking.is_empty() {
            h += p.measure_left(gl, &self.thinking_shown(), SMALL_EM, inner).1 + 10.0;
        }
        if !self.text.is_empty() {
            h += p.measure_left(gl, &self.text, TEXT_EM, inner).1;
        }
        if !self.did.is_empty() {
            h += p.measure_left(gl, &self.did_line(), SMALL_EM, inner).1 + SMALL_EM * 0.3;
        }
        h
    }

    fn draw(&self, p: &mut Painter, gl: &Gl, rect: Rect, width: f32) {
        let w = width * if self.mine { 0.78 } else { 0.92 };
        let inner = w - BUBBLE_PAD * 2.0;
        // Its text, at the size it needs, at most `w` wide.
        let text_w = if self.text.is_empty() { 0.0 } else { p.measure_left(gl, &self.text, TEXT_EM, inner).0 };
        let bubble_w = if self.mine { (text_w + BUBBLE_PAD * 2.0).min(w) } else { w };
        let x = if self.mine { rect.x + rect.w - bubble_w } else { rect.x };
        let bubble = Rect::new(x, rect.y, bubble_w, rect.h);
        if self.mine {
            p.fill(gl, bubble, 22.0, theme::selected());
        } else {
            p.fill(gl, bubble, 22.0, theme::raised());
        }
        let mut y = rect.y + BUBBLE_PAD;
        let left = x + BUBBLE_PAD;
        if !self.model.is_empty() {
            p.text_left(gl, &format!("Thinking with {}", self.model), left, y, SMALL_EM, inner, look::VIOLET);
            y += SMALL_EM * 1.4;
        }
        if !self.thinking.is_empty() {
            let (_, h) = p.text_left(gl, &self.thinking_shown(), left, y, SMALL_EM, inner, theme::ink_hint());
            y += h + 10.0;
        }
        if !self.text.is_empty() {
            let (_, h) = p.text_left(gl, &self.text, left, y, TEXT_EM, inner, if self.mine { theme::ink() } else { theme::ink_row() });
            y += h;
        }
        if !self.did.is_empty() {
            p.text_left(gl, &self.did_line(), left, y + SMALL_EM * 0.3, SMALL_EM, inner, look::TEAL);
        }
    }
}

/// A model's text as shown: the marks small models put in for emphasis though told not to
/// (`**`, `__`, a backtick) taken out, and a trailing line's end.
fn shown(text: &str) -> String {
    text.replace("**", "").replace("__", "").replace('`', "").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use spatiand_app::{Key, Modifiers};

    fn key(key: Key) -> Input {
        Input::Key { key, text: None, modifiers: Modifiers::default(), letter: None }
    }

    #[test]
    fn control_delete_forgets_the_open_chat_only_pressed_twice_and_with_nothing_typed() {
        let chats: Vec<Chat> = ["a", "b"].iter().map(|id| Chat { id: id.to_string(), ..Default::default() }).collect();
        let (settings, speech) = (Settings::default(), VoiceSettings::default());
        let view = View {
            chats: &chats,
            current: Some(1),
            answering: None,
            speaking: false,
            settings: &settings,
            speech: &speech,
            listening: false,
            look: false,
            status: "",
            trouble: None,
            models: &[],
            voices: &[],
            characters: &[],
        };
        let delete = || Input::Key { key: Key::Delete, text: None, modifiers: Modifiers { control: true, ..Default::default() }, letter: None };
        let mut w = Window::new();
        // Something typed: the key is the field's.
        w.field.set("hello world");
        assert!(w.input(delete(), &view).is_none());
        assert!(w.asking.is_none());
        // Nothing typed: it asks, about the chat open; Return is Cancel and nothing is lost.
        w.field.set("");
        assert!(w.input(delete(), &view).is_none());
        assert!(w.asking.is_some());
        assert!(w.input(key(Key::Enter), &view).is_none());
        assert!(w.asking.is_none());
        // Asked again, the yes chosen: the chat is deleted, by its id.
        assert!(w.input(delete(), &view).is_none());
        assert!(w.input(key(Key::Right), &view).is_none());
        assert!(matches!(w.input(key(Key::Enter), &view), Some(Want::ForgetChat(id)) if id == "b"));
    }

    #[test]
    fn the_bin_asks_first_and_escape_is_cancel() {
        let mut w = Window::new();
        assert!(w.ask_to_forget("a".into()).is_none());
        let (settings, speech) = (Settings::default(), VoiceSettings::default());
        let view = View { chats: &[], current: None, answering: None, speaking: false, settings: &settings, speech: &speech, listening: false, look: false, status: "", trouble: None, models: &[], voices: &[], characters: &[] };
        assert!(w.input(key(Key::Escape), &view).is_none());
        assert!(w.asking.is_none());
    }
}
