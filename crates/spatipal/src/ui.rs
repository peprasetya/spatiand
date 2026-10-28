//! SpatiPal's window: its chats down the left, the conversation in the middle, a field to type
//! in at the foot -- and its settings, dictation's among them, behind the cog.
//!
//! Drawn with the room's own widgets (`spatiand_app::ui`), so it looks like the rest of Spatiand.

use spatiand_app::ui::{look, theme, Button, Field, Icon, Input, List, Painter, Rect, Row, Typed};
use spatiand_app::Gl;
use spatiand_voice::VoiceSettings;

use crate::store::{Chat, Said, Settings};

/// The answer being given, as it comes.
#[derive(Default)]
pub struct Answering {
    pub text: String,
    pub thinking: String,
    pub did: Vec<String>,
    pub model: String,
    pub handed_over: bool,
}

/// What the window shows, lent to it each frame.
pub struct View<'a> {
    pub chats: &'a [Chat],
    pub current: Option<usize>,
    pub answering: Option<&'a Answering>,
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
    ForgetChat(usize),
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
    ("Assistant", &[Line::On, Line::Name, Line::Persona, Line::Figure]),
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
        }
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, view: &View) {
        let mut all = p.bounds().inset(theme::PAD * 0.6);
        let beside = view.status.to_string();
        look::header(p, gl, &mut all, &view.settings.name, &beside);
        match self.page {
            Page::Chat => self.draw_chat(p, gl, all, view),
            Page::Settings => self.draw_settings(p, gl, all, view),
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
        let busy = view.answering.is_some();
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
        self.chats.draw(p, gl, side, &rows);
        if let Some(i) = view.current {
            if let Some(r) = self.chats.row_rect(i) {
                p.fill(gl, Rect { w: 4.0, ..r }, 2.0, look::TEAL);
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
    }

    fn draw_settings(&mut self, p: &mut Painter, gl: &Gl, mut all: Rect, view: &View) {
        let mut foot = all.take_bottom(Button::HEIGHT);
        all.take_bottom(16.0);
        let back_w = Button::width(p, gl, "Done");
        let back = foot.take_right(back_w);
        foot.take_right(10.0);
        self.back.draw(p, gl, back, "Done");
        if let Some(line) = self.editing {
            self.edit.hint = format!("{} \u{2014} Enter keeps it", label(line));
            self.edit.focused = true;
            self.edit.draw(p, gl, foot);
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
        if view.answering.is_some() {
            if self.stop.input(&input) {
                return Some(Want::Stop);
            }
        } else if self.send.input(&input) {
            return self.take_field();
        }
        // A chat is forgotten with the right button on it.
        if let Input::Key { key: spatiand_app::Key::Delete, modifiers, .. } = &input {
            if (modifiers.logo || modifiers.control) && view.current.is_some() {
                return view.current.map(Want::ForgetChat);
            }
        }
        if let Some(Typed::Entered) = self.field.input(&input) {
            return self.take_field();
        }
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
