//! SpatiPal's window: its chats down the left, the conversation in the middle, a field to type
//! in at the foot -- and its own settings behind the cog: the pal it is (Pixel, built in, or one
//! installed) with that pal's own name, personality, figure and voice, then SpatiPal's own lines,
//! the same for every pal. hemera, the models and dictation are the room's AI settings, which
//! more than one app uses: its settings say so, in one line.
//!
//! Drawn with the room's own widgets (`spatiand_app::ui`), so it looks like the rest of Spatiand.

use spatiand_app::ui::{look, theme, Button, Confirm, Field, Icon, Input, List, Painter, Rect, Row, Typed};
use spatiand_app::Gl;

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
    pub listening: bool,
    pub look: bool,
    pub status: &'a str,
    pub trouble: Option<&'a str>,
    /// Voices fetched from hemera's library, for the choice of one.
    pub voices: &'a [String],
    pub characters: &'a [String],
    /// The pal it is -- Pixel, or the one chosen (`settings.pal`) if it could be read -- and the
    /// pals installed, by folder.
    pub pack: &'a crate::pack::Pack,
    pub pals: &'a [String],
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
    Keep(Settings),
    /// A word to the wearer: a line that cannot be changed here, and why.
    Note(String),
}

#[derive(PartialEq, Clone, Copy)]
pub enum Page {
    Chat,
    Settings,
}

/// One line of the settings page.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Line {
    Pal,
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
}

/// The pal first, and its own lines -- each pal has its own name, personality, figure and voice
/// (the owner, 2026-11-19: "each pal... have it's own [voice] and personality") -- then
/// SpatiPal's, the same whichever pal it is.
const LINES: &[(&str, &[Line])] = &[
    ("Who it is", &[Line::Pal, Line::Name, Line::Persona, Line::Figure, Line::Voice, Line::Described]),
    ("Assistant", &[Line::On, Line::Shown, Line::Spoken, Line::Listening, Line::Thinking]),
    ("Chats", &[Line::Continue]),
];

/// Where what was moved out of these settings is now (the owner, 2026-11-19: "move shared
/// setting from spatipal to this card ... but keep setting specific spatipal in spatipal").
pub const ELSEWHERE: &str = "hemera, the models and dictation are in Spatiand's settings, under AI.";

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
    /// The Copy button on the answer under the pointer, that answer's text, and the last copied.
    copy: Button,
    copy_text: Option<String>,
    copied: Option<(std::time::Instant, String)>,
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
            copy: Button::default(),
            copy_text: None,
            copied: None,
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
        let mut answers = Vec::new();
        for (block, h) in blocks.iter().zip(heights) {
            if y + h >= all.y && y <= all.bottom() {
                let bubble = block.draw(p, gl, Rect::new(all.x, y, all.w, h), width);
                if !block.mine && !block.text.is_empty() && block.text != "\u{2026}" {
                    answers.push((bubble, block.text.clone()));
                }
            }
            y += h + 18.0;
        }
        // The answer under the pointer has a Copy button beside its top right: its text, onto the
        // room's clipboard (it cannot be chosen as a field's can).
        let (px, py) = self.pointer;
        self.copy_text = None;
        let beside = |b: &Rect| Rect::new(b.right() + 8.0, b.y + 4.0, 44.0, 44.0);
        if let Some((bubble, text)) = answers.into_iter().find(|(b, _)| (b.contains(px, py) || beside(b).contains(px, py)) && all.contains(px, py)) {
            let at = beside(&bubble);
            let copied = self.copied.as_ref().is_some_and(|(when, was)| when.elapsed() < std::time::Duration::from_secs(2) && *was == text);
            if copied {
                p.label(gl, "Copied", Rect { w: 124.0, ..at }, theme::FOOTER_EM, theme::ink_hint(), 0.0);
            } else {
                self.copy.draw_icon(p, gl, at, Icon::Copy, "");
            }
            self.copy_text = Some(text);
        }
        p.unclip(gl);
        if tall > all.h {
            look::scrollbar(p, gl, Rect { x: all.right() - theme::SCROLL_WIDTH, w: theme::SCROLL_WIDTH, ..all }, all.h, tall, tall - all.h - self.scroll);
        }
    }

    fn draw_settings(&mut self, p: &mut Painter, gl: &Gl, mut all: Rect, view: &View) {
        look::top(p, gl, &mut all, &mut self.back, "Back", Icon::Back, "Settings", "");
        let foot = all.take_bottom(Button::HEIGHT);
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
        // What is the room's now, said once, under the list.
        let elsewhere = all.take_bottom(theme::FOOTER_EM * 1.8);
        p.label(gl, ELSEWHERE, elsewhere, theme::FOOTER_EM, theme::ink_detail(), 0.0);
        all.take_bottom(8.0);
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
        // An answer's Copy button, before anything under it.
        if let Some(text) = self.copy_text.clone() {
            let on_copy = self.copy.contains(self.pointer.0, self.pointer.1);
            match input {
                Input::Press { pressed: true, .. } if on_copy => {
                    self.copy.input(&input);
                    return None;
                }
                Input::Press { pressed: false, .. } if self.copy.input(&input) => {
                    spatiand_app::clipboard::copy(text.clone());
                    self.copied = Some((std::time::Instant::now(), text));
                    return None;
                }
                Input::Pointer { .. } | Input::PointerLeft => {
                    self.copy.input(&input);
                }
                _ => {}
            }
        }
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
            let mut s = view.settings.clone();
            set_text(line, &text, &mut s);
            // The press is also the start of what it was aimed at (Back, another line): the
            // field gives it up now and the release does the rest, as before.
            self.back.input(&input);
            self.lines.input(&input);
            return Some(Want::Keep(s));
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
                    let mut s = view.settings.clone();
                    set_text(line, &text, &mut s);
                    return Some(Want::Keep(s));
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
        self.choose_line(line, view)
    }

    /// A line of the settings chosen: changed, opened to type in, or said why it cannot be.
    fn choose_line(&mut self, line: Line, view: &View) -> Option<Want> {
        let mut s = view.settings.clone();
        let p = view.pack;
        // An installed pal's own lines are its folder's: changed here and written there, unless
        // its files cannot be written; its figure is the file its folder has.
        if !p.is_pixel() && is_the_pals(line) {
            if line == Line::Figure {
                return Some(Want::Note(format!("A pal's figure is the file its pal.json names, in assistant/pals/{}: change it there.", p.folder)));
            }
            if !p.writable {
                return Some(Want::Note(format!("{}'s files cannot be changed here: assistant/pals/{} is not writable.", s.name, p.folder)));
            }
        }
        match line {
            Line::Pal => {
                // Pixel, built in, first; then each pal installed.
                let mut choices = vec![String::new()];
                choices.extend(view.pals.iter().cloned());
                if choices.len() == 1 && s.pal.is_empty() {
                    return Some(Want::Note(format!("{} is the only pal here: one of your own is a folder in assistant/pals/ (docs/spatipal-pals.md).", s.name)));
                }
                s.pal = next(&choices, &s.pal);
                Some(Want::Keep(s))
            }
            Line::Name | Line::Persona | Line::Described => {
                self.editing = Some(line);
                self.edit.set(text_of(line, view));
                self.edit.select_all();
                None
            }
            Line::On => {
                s.on = !s.on;
                Some(Want::Keep(s))
            }
            Line::Spoken => {
                s.spoken = !s.spoken;
                Some(Want::Keep(s))
            }
            Line::Shown => {
                s.shown = !s.shown;
                Some(Want::Keep(s))
            }
            Line::Listening => {
                s.listening = !s.listening;
                Some(Want::Keep(s))
            }
            Line::Thinking => {
                s.thinking = next(&["quiet", "tone", "read"], &s.thinking);
                Some(Want::Keep(s))
            }
            Line::Continue => {
                let choices = [0u32, 1, 4, 12, 24, 168, 100_000];
                let at = choices.iter().position(|c| *c == s.continue_hours).map(|i| (i + 1) % choices.len()).unwrap_or(2);
                s.continue_hours = choices[at];
                Some(Want::Keep(s))
            }
            Line::Figure => {
                s.character = next(view.characters, &s.character);
                Some(Want::Keep(s))
            }
            Line::Voice => {
                let mut choices = vec![String::new()];
                choices.extend(view.voices.iter().cloned());
                s.voice = next(&choices, &s.voice);
                Some(Want::Keep(s))
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

/// Whether the line is the pal's own -- its name, personality, figure, voice -- rather than
/// SpatiPal's.
fn is_the_pals(line: Line) -> bool {
    matches!(line, Line::Name | Line::Persona | Line::Figure | Line::Voice | Line::Described)
}

fn label(line: Line) -> &'static str {
    match line {
        Line::Pal => "Pal",
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
    }
}

fn on(b: bool) -> String {
    if b { "On".into() } else { "Off".into() }
}

fn value(line: Line, view: &View) -> String {
    let s = view.settings;
    let p = view.pack;
    match line {
        Line::Pal => match (p.is_pixel(), s.pal.is_empty()) {
            (true, true) if view.pals.is_empty() => format!("{} (built in; others: docs/spatipal-pals.md)", s.name),
            (true, true) => format!("{} (built in)", s.name),
            // Chosen, but its folder could not be read: Pixel, until it can.
            (true, false) => format!("{} cannot be read", s.pal),
            (false, _) if !p.writable => format!("{} (pals/{}, read-only)", s.name, p.folder),
            (false, _) => format!("{} (pals/{})", s.name, p.folder),
        },
        Line::On => on(s.on),
        Line::Shown => if s.shown { "Shown".into() } else { "Hidden (still listening)".into() },
        Line::Name => s.name.clone(),
        // A private pal's is not on show: pressed, it is there to change.
        Line::Persona if !p.is_pixel() && p.about.private => "Its own (private)".into(),
        Line::Persona if s.persona.trim().is_empty() => "None".into(),
        Line::Persona => short(&flat(&s.persona)),
        Line::Figure if p.is_pixel() => s.character.clone(),
        Line::Figure => match &p.figure {
            Some(_) => format!("{} (its folder's)", p.about.figure.trim()),
            None => "The robot (none in its folder)".into(),
        },
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
    }
}

fn text_of(line: Line, view: &View) -> String {
    let s = view.settings;
    match line {
        Line::Name => s.name.clone(),
        Line::Persona => flat(&s.persona),
        Line::Described => s.described.clone(),
        _ => String::new(),
    }
}

fn set_text(line: Line, text: &str, s: &mut Settings) {
    match line {
        Line::Name if !text.is_empty() => s.name = text.to_string(),
        Line::Persona => s.persona = unflat(text),
        Line::Described => s.described = text.to_string(),
        _ => {}
    }
}

/// A personality of more than one line -- a pal's persona.md is pages of its own -- in the
/// one-line field: each line break shown as a pilcrow, and put back where one is when it is kept.
const BREAK: &str = " \u{b6} ";

fn flat(text: &str) -> String {
    text.trim().replace("\r\n", "\n").replace('\n', BREAK)
}

fn unflat(text: &str) -> String {
    let parts: Vec<&str> = text.split('\u{b6}').collect();
    let last = parts.len().saturating_sub(1);
    parts
        .iter()
        .enumerate()
        .map(|(i, part)| {
            let part = if i > 0 { part.strip_prefix(' ').unwrap_or(part) } else { part };
            if i < last { part.strip_suffix(' ').unwrap_or(part) } else { part }
        })
        .collect::<Vec<_>>()
        .join("\n")
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

    /// Drawn in `rect`; where its bubble is.
    fn draw(&self, p: &mut Painter, gl: &Gl, rect: Rect, width: f32) -> Rect {
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
        bubble
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

    /// A folder of pals of a test's own.
    fn pals_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("spatipal-ui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn view<'a>(settings: &'a Settings, pack: &'a crate::pack::Pack, pals: &'a [String], voices: &'a [String]) -> View<'a> {
        View { chats: &[], current: None, answering: None, speaking: false, settings, listening: false, look: false, status: "", trouble: None, voices, characters: &[], pack, pals }
    }

    #[test]
    fn control_delete_forgets_the_open_chat_only_pressed_twice_and_with_nothing_typed() {
        let chats: Vec<Chat> = ["a", "b"].iter().map(|id| Chat { id: id.to_string(), ..Default::default() }).collect();
        let settings = Settings::default();
        let root = pals_root("delete");
        let pixel = crate::pack::Pack::pixel_at(&root, &settings);
        let view = View {
            chats: &chats,
            current: Some(1),
            answering: None,
            speaking: false,
            settings: &settings,
            listening: false,
            look: false,
            status: "",
            trouble: None,
            voices: &[],
            characters: &[],
            pack: &pixel,
            pals: &[],
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
        let settings = Settings::default();
        let root = pals_root("bin");
        let pixel = crate::pack::Pack::pixel_at(&root, &settings);
        let view = view(&settings, &pixel, &[], &[]);
        assert!(w.input(key(Key::Escape), &view).is_none());
        assert!(w.asking.is_none());
    }

    #[test]
    fn its_settings_keep_its_own_and_say_where_the_rest_went() {
        // The pal first, with its own lines; then SpatiPal's; hemera, the models and dictation
        // are the room's now.
        let labels: Vec<&str> = LINES.iter().flat_map(|(_, lines)| lines.iter().map(|l| label(*l))).collect();
        assert_eq!(
            labels,
            [
                "Pal",
                "Name (what it answers to)",
                "Personality",
                "Figure",
                "Voice",
                "Voice described",
                "Start with the room",
                "Show its figure",
                "Speak answers",
                "Listen for its name",
                "While the larger model thinks",
                "Go on in the last chat",
            ]
        );
        let headings: Vec<&str> = LINES.iter().map(|(h, _)| *h).collect();
        assert_eq!(headings, ["Who it is", "Assistant", "Chats"]);
        assert!(LINES[0].1.iter().all(|l| *l == Line::Pal || is_the_pals(*l)));
        assert!(ELSEWHERE.contains("Spatiand's settings, under AI"));
        assert!(ELSEWHERE.ends_with('.'));
    }

    #[test]
    fn the_pal_is_pixel_first_then_each_installed_and_never_none() {
        let root = pals_root("choice");
        std::fs::create_dir_all(root.join("mira")).unwrap();
        std::fs::write(root.join("mira").join("pal.json"), r#"{"name": "Mira"}"#).unwrap();
        let settings = Settings::default();
        let pixel = crate::pack::Pack::pixel_at(&root, &settings);
        let pals = vec!["mira".to_string()];
        let v = view(&settings, &pixel, &pals, &[]);
        assert_eq!(value(Line::Pal, &v), "Pixel (built in)");
        assert!(!LINES.iter().flat_map(|(_, l)| l.iter()).any(|l| value(*l, &v).starts_with("None (")));
        // Chosen: Pixel, then the pal, then Pixel again.
        let mut w = Window::new();
        let Some(Want::Keep(s)) = w.choose_line(Line::Pal, &v) else { panic!("the pal line should choose") };
        assert_eq!(s.pal, "mira");
        let mira = crate::pack::Pack::load_at(&root, "mira").unwrap();
        let as_mira = mira.over(&s);
        let v = view(&as_mira, &mira, &pals, &[]);
        assert_eq!(value(Line::Pal, &v), "Mira (pals/mira)");
        let Some(Want::Keep(s)) = w.choose_line(Line::Pal, &v) else { panic!("the pal line should choose") };
        assert_eq!(s.pal, "");
        // Pixel alone: said so, and how to have another.
        let v = view(&settings, &pixel, &[], &[]);
        assert!(value(Line::Pal, &v).starts_with("Pixel (built in"));
        assert!(matches!(w.choose_line(Line::Pal, &v), Some(Want::Note(n)) if n.contains("assistant/pals/")));
        // A pal chosen whose folder cannot be read: said so; Pixel is what is used meanwhile.
        let gone = Settings { pal: "gone".into(), ..Settings::default() };
        let v = view(&gone, &pixel, &pals, &[]);
        assert_eq!(value(Line::Pal, &v), "gone cannot be read");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_installed_pals_lines_are_its_own_and_pixels_are_the_settings() {
        let root = pals_root("lines");
        let dir = root.join("mira");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pal.json"), r#"{"name": "Mira", "persona": "persona.md", "voice": "alba"}"#).unwrap();
        std::fs::write(dir.join("persona.md"), "You are Mira.\n\nYou tease.").unwrap();
        let settings = Settings::default();
        let mira = crate::pack::Pack::load_at(&root, "mira").unwrap();
        let shown = mira.over(&settings);
        let voices = vec!["alba".to_string(), "bram".to_string()];
        let v = view(&shown, &mira, &[], &voices);
        assert_eq!(value(Line::Name, &v), "Mira");
        assert_eq!(value(Line::Voice, &v), "alba");
        assert_eq!(value(Line::Figure, &v), "The robot (none in its folder)");
        let mut w = Window::new();
        // Its figure is its folder's; its voice and its words are changed here.
        assert!(matches!(w.choose_line(Line::Figure, &v), Some(Want::Note(n)) if n.contains("assistant/pals/mira")));
        let Some(Want::Keep(s)) = w.choose_line(Line::Voice, &v) else { panic!("the voice should step") };
        assert_eq!(s.voice, "bram");
        assert!(w.choose_line(Line::Persona, &v).is_none());
        assert_eq!(w.editing, Some(Line::Persona));
        // Its personality's lines in the one-line field, and back as they were.
        assert_eq!(w.edit.text, "You are Mira. \u{b6}  \u{b6} You tease.");
        let mut kept = shown.clone();
        set_text(Line::Persona, &w.edit.text, &mut kept);
        assert_eq!(kept.persona, shown.persona);
        assert_eq!(unflat("a\u{b6}b \u{b6} c"), "a\nb\nc");
        // Pixel's figure is chosen here, among the room's.
        let pixel = crate::pack::Pack::pixel_at(&root, &settings);
        let characters = vec!["robot".to_string(), "hero.vrm".to_string()];
        let v = View { characters: &characters, ..view(&settings, &pixel, &[], &[]) };
        let Some(Want::Keep(s)) = w.choose_line(Line::Figure, &v) else { panic!("Pixel's figure should step") };
        assert_eq!(s.character, "hero.vrm");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_pal_that_cannot_be_written_shows_its_lines_and_says_why_they_stay() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let root = pals_root("readonly");
        let dir = root.join("mira");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pal.json"), r#"{"name": "Mira", "private": true}"#).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let mira = crate::pack::Pack::load_at(&root, "mira").unwrap();
        let shown = mira.over(&Settings::default());
        let v = view(&shown, &mira, &[], &[]);
        assert_eq!(value(Line::Pal, &v), "Mira (pals/mira, read-only)");
        assert_eq!(value(Line::Name, &v), "Mira");
        // A private one's personality is not on show in the room.
        assert_eq!(value(Line::Persona, &v), "Its own (private)");
        let mut w = Window::new();
        for line in [Line::Name, Line::Persona, Line::Voice, Line::Described] {
            assert!(matches!(w.choose_line(line, &v), Some(Want::Note(n)) if n.contains("cannot be changed")));
            assert!(w.editing.is_none());
        }
        // SpatiPal's own lines are still SpatiPal's to change.
        assert!(matches!(w.choose_line(Line::Spoken, &v), Some(Want::Keep(_))));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }
}
