//! Live Text: the text in a picture chosen, copied and translated as text -- a photograph, a
//! screenshot, a scanned page, a paused film's frame -- in any script (the owner, 2026-11-19: "I
//! want it can select text from images (also on image of pdf)... even non latin, unicode character.
//! Like apple preview."). The same piece in SpatiLoupe and SpatiReel, on every platform.
//!
//! hemera reads the picture ([`Reading`], `POST /v1/ocr`) and says where each line, word and
//! grapheme is ([`ReadText`]); the application says where the picture is drawn ([`Mapping`]: its
//! orientation, quarter turns, crop, zoom and pan), hands this its input, and draws it over the
//! picture ([`LiveText::draw`]) and over its own controls ([`LiveText::draw_over`]).
//!
//! What the wearer does:
//!
//! - **press and drag on the text** chooses from grapheme to grapheme in reading order, across
//!   lines and blocks; a **double press** chooses a word, a **triple** a line. A drag that starts
//!   off the text is the application's (a pan, a swipe): the text never takes it.
//! - **Command/Control-C** copies what is chosen to the room's clipboard; **Command/Control-A**
//!   chooses all of it; **Escape** lets go of it.
//! - **the right button, or a press held still** (800 ms: a pad or a hand has no right button)
//!   opens the menu: Copy, Select all, Translate -- and Read the text, where the picture is read
//!   only when asked ([`LiveText::can_read`]).
//! - **Translate** puts what is chosen, or all of it, into the wearer's other language by the AI
//!   settings' rule ([`spatiand_voice::hemera::translate`]), shown in a pane with Copy and Done;
//!   Escape closes it.
//!
//! The line under the pointer is lit faintly -- the hint that there is text there, the room's
//! pointer being cast from the head -- and a word read with less certainty than [`SURE`] has a
//! dotted line under it.
//!
//! Nothing of what is read is logged or kept on disk: it is the application's, in memory.

mod place;
mod reader;
mod text;
#[cfg(test)]
mod truth;

pub use place::{turned_size, upright_size, Mapping};
pub use reader::{reads_by_itself, within_limit, Picture, Reading, Translating};
pub use spatiand_voice::hemera::ocr::Kind;
pub use text::{Direction, Grapheme, Line, Point, ReadText, Spot, Word, SURE};

use std::time::{Duration, Instant};

use crate::ui::{look, theme, Button, Icon, Input, Menu, MenuSaid, Painter, Rect, Row, DOUBLE_CLICK, DOUBLE_CLICK_SLOP, DRAG_AFTER};
use crate::{Gl, Key};

/// How long a press held still on the picture opens its menu: a pad's or a hand's right button.
pub const HOLD_FOR_MENU: Duration = Duration::from_millis(800);

/// What the room's speech is told before a picture's text ([`LiveText::described`]): that it
/// is what is printed, read from the picture, and how an unsure word is marked. The assistant
/// takes names and numbers from it rather than reading them off the picture again.
pub const HEADING: &str = "Text in the picture, read from it exactly as it is printed (a word in [? ?] was read with less certainty):";

/// How near the text a press counts as on it, in the window's pixels.
const SLACK: f32 = 6.0;

/// How long what was done (copied) is said, and what went wrong.
const SAID_FOR: Duration = Duration::from_secs(5);
const TROUBLE_FOR: Duration = Duration::from_secs(8);

/// What became of an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Said {
    /// Not the text's: the application does with it what it does.
    Pass,
    /// The text's: to be drawn again.
    Taken,
    /// A press let go of on the text where it went down, choosing nothing: the application's own
    /// tap (SpatiReel's, which puts its controls away).
    Tapped,
    /// Copied to the room's clipboard.
    Copied,
    /// "Read the text" chosen from the menu: the application reads the picture.
    Read,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Act {
    Copy,
    SelectAll,
    Translate,
    Read,
}

/// A press on the text, going on.
struct Grip {
    from: (f32, f32),
    when: Instant,
    moved: bool,
    /// What it went down on: a place (one press), a word (two), a line (three).
    unit: (Spot, Spot),
    clicks: u8,
    /// Held still long enough to have opened the menu: its release is nothing.
    held: bool,
}

/// A translation, and the pane it is shown in.
struct Pane {
    /// Where what was translated is on the window: the pane is put beside it.
    near: Rect,
    job: Option<Translating>,
    /// The language it went into, and the translation; or why it did not come.
    result: Option<Result<(String, String), String>>,
    copy: Button,
    done: Button,
    area: Rect,
    scroll: f32,
    /// How tall its text is, and how much of it is on show.
    tall: f32,
    shown: f32,
}

/// The text of one picture, as the wearer works it. See the module's notes.
pub struct LiveText {
    read: Option<ReadText>,
    map: Mapping,
    unmap: Option<Mapping>,
    /// What is chosen: never empty, the earlier place first.
    selection: Option<(Spot, Spot)>,
    grip: Option<Grip>,
    /// The last press on the text: when, where, and which of two or three it was.
    last: Option<(Instant, (f32, f32), u8)>,
    pointer: (f32, f32),
    hovered: Option<usize>,
    menu: Option<(Menu, Vec<(Row, Act)>)>,
    pane: Option<Pane>,
    /// What was done, or went wrong, said in a pill at the foot: its words, whether it went wrong,
    /// and since when.
    said: Option<(String, bool, Instant)>,
    /// The picture is read only when asked (the AI settings' "read by itself" off) and has not
    /// been: its menu offers to.
    pub can_read: bool,
    /// The picture is being read: "Reading the text…" in the pill, for an application with no
    /// foot line of its own to say it in (a film's player).
    pub reading: bool,
    /// Where the pill's foot is, in the window: over the application's own foot (its strip, its
    /// controls). The window's foot, 52 up, if not said.
    pub note_foot: Option<f32>,
}

impl Default for LiveText {
    fn default() -> Self {
        LiveText::new()
    }
}

fn apart(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

impl LiveText {
    pub fn new() -> LiveText {
        LiveText {
            read: None,
            map: Mapping::IDENTITY,
            unmap: Some(Mapping::IDENTITY),
            selection: None,
            grip: None,
            last: None,
            pointer: (0.0, 0.0),
            hovered: None,
            menu: None,
            pane: None,
            said: None,
            can_read: false,
            reading: false,
            note_foot: None,
        }
    }

    /// What went wrong reading or translating, said in the pill for eight seconds (the rule for
    /// what went wrong), for an application with no foot line of its own.
    pub fn tell(&mut self, trouble: impl Into<String>) {
        self.said = Some((trouble.into(), true, Instant::now()));
    }

    /// The picture's text, as it has been read -- or none, for another picture: what was chosen,
    /// the menu and a translation of the last go with it.
    pub fn set_text(&mut self, read: Option<ReadText>) {
        self.read = read.filter(|r| !r.is_empty());
        self.selection = None;
        self.grip = None;
        self.hovered = None;
        self.menu = None;
        self.pane = None;
        if self.read.is_some() {
            self.can_read = false;
        }
    }

    pub fn text(&self) -> Option<&ReadText> {
        self.read.as_ref()
    }

    pub fn has_text(&self) -> bool {
        self.read.is_some()
    }

    /// Where the picture is drawn now: from its pixels as it was read to the window's.
    pub fn place(&mut self, map: Mapping) {
        if map != self.map {
            self.map = map;
            self.unmap = map.inverse();
        }
    }

    /// A menu or a translation is open: it has the input first (SpatiLoupe's controls stay
    /// meanwhile; SpatiReel's are left as they are, as they would lift its subtitles).
    pub fn busy(&self) -> bool {
        self.menu.is_some() || self.pane.is_some()
    }

    /// A press on the text is going on.
    pub fn holding(&self) -> bool {
        self.grip.is_some()
    }

    /// A press on the text is going on, or went down lately enough for another to make two or
    /// three of it: what is pressed on is to be kept where it is meanwhile (a subtitle the
    /// controls coming up would lift).
    pub fn pressed_lately(&self) -> bool {
        self.grip.is_some() || self.last.is_some_and(|(when, _, _)| when.elapsed() < DOUBLE_CLICK)
    }

    /// Whether anything is chosen.
    pub fn has_selection(&self) -> bool {
        self.selection.is_some()
    }

    /// What is chosen, as it is copied.
    pub fn selected(&self) -> Option<String> {
        let (from, to) = self.selection?;
        Some(self.read.as_ref()?.text(from, to)).filter(|t| !t.is_empty())
    }

    pub fn select_all(&mut self) -> bool {
        let Some(read) = &self.read else { return false };
        self.selection = Some((Spot::default(), read.end()));
        true
    }

    /// What is chosen, onto the room's clipboard.
    pub fn copy(&mut self) -> bool {
        let Some(text) = self.selected() else { return false };
        crate::clipboard::copy(text);
        self.said = Some(("Copied.".into(), false, Instant::now()));
        true
    }

    /// The picture's text as the room's speech is told it ([`HEADING`], then the text with its
    /// unsure words marked), for the application's description; nothing if none has been read.
    pub fn described(&self) -> Option<String> {
        let read = self.read.as_ref()?;
        Some(format!("{HEADING}\n{}", read.described()))
    }

    fn picture_point(&self, x: f32, y: f32) -> Option<Point> {
        Some(self.unmap?.apply([x, y]))
    }

    fn slack(&self) -> f32 {
        SLACK / self.map.scale().max(1e-4)
    }

    /// The place between graphemes at (`x`, `y`) on the window, if it is on the text.
    fn spot_at(&self, x: f32, y: f32) -> Option<Spot> {
        self.read.as_ref()?.hit(self.picture_point(x, y)?, self.slack())
    }

    /// Whether (`x`, `y`) on the window is on the text.
    pub fn over_text(&self, x: f32, y: f32) -> bool {
        self.spot_at(x, y).is_some()
    }

    fn line_at(&self, x: f32, y: f32) -> Option<usize> {
        self.read.as_ref()?.line_at(self.picture_point(x, y)?, self.slack())
    }

    /// The menu, at (`x`, `y`): the right button pressed there, or a press held. On the text, and
    /// not on what is chosen, the word there is chosen first. Whether there was one to open.
    pub fn context(&mut self, x: f32, y: f32) -> bool {
        if let (Some(spot), Some(point)) = (self.spot_at(x, y), self.picture_point(x, y)) {
            let inside = self.selection.is_some_and(|(a, b)| spot >= a && spot <= b);
            if !inside {
                let word = self.read.as_ref().map(|r| r.word(spot.line, point));
                self.selection = word.filter(|(a, b)| a != b);
            }
        }
        let mut rows = Vec::new();
        if self.selection.is_some() {
            rows.push((Row::with("Copy", look::chord("C")).icon(Icon::Copy), Act::Copy));
        }
        if self.read.is_some() {
            rows.push((Row::with("Select all", look::chord("A")).icon(Icon::SelectAll), Act::SelectAll));
            rows.push((Row::new("Translate").icon(Icon::Translate), Act::Translate));
        } else if self.can_read {
            rows.push((Row::new("Read the text").icon(Icon::Text), Act::Read));
        }
        if rows.is_empty() {
            return false;
        }
        self.menu = Some((Menu::new((x, y)), rows));
        true
    }

    /// What waits on time: a press held still opening the menu, a translation coming back, what
    /// was done no longer said. Whether anything changed, to be drawn again.
    pub fn tick(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if let Some(pane) = self.pane.as_mut() {
            if let Some(result) = pane.job.as_ref().and_then(Translating::poll) {
                pane.result = Some(result);
                pane.job = None;
                pane.scroll = 0.0;
                changed = true;
            }
        }
        let due = self.grip.as_ref().filter(|g| !g.moved && !g.held && now.saturating_duration_since(g.when) >= HOLD_FOR_MENU).map(|g| g.from);
        if let Some((x, y)) = due {
            if let Some(grip) = self.grip.as_mut() {
                grip.held = true;
            }
            self.context(x, y);
            changed = true;
        }
        if self.said.as_ref().is_some_and(|(_, trouble, at)| now.saturating_duration_since(*at) >= if *trouble { TROUBLE_FOR } else { SAID_FOR }) {
            self.said = None;
            changed = true;
        }
        changed
    }

    /// When [`LiveText::tick`] is next wanted, if anything waits on time.
    pub fn next_wake(&self) -> Option<Duration> {
        let hold = self.grip.as_ref().filter(|g| !g.moved && !g.held).map(|g| HOLD_FOR_MENU.saturating_sub(g.when.elapsed()));
        let translating = self.pane.as_ref().filter(|p| p.job.is_some()).map(|_| Duration::from_millis(100));
        let said = self.said.as_ref().map(|(_, trouble, at)| if *trouble { TROUBLE_FOR } else { SAID_FOR }.saturating_sub(at.elapsed()));
        [hold, translating, said].into_iter().flatten().min()
    }

    /// The pointer dragged to (`x`, `y`) from the press: what is chosen runs from what the press
    /// went down on to what is there now, a word or a line at a time after two or three presses.
    fn extend_to(&mut self, x: f32, y: f32) {
        let (Some(read), Some(point), Some(grip)) = (self.read.as_ref(), self.picture_point(x, y), self.grip.as_ref()) else { return };
        let focus = read.nearest(point);
        let (f0, f1) = match grip.clicks {
            1 => (focus, focus),
            2 => read.word(focus.line, point),
            _ => read.whole(focus.line),
        };
        let (a0, a1) = grip.unit;
        let (from, to) = if f1 >= a1 { (a0, f1) } else { (f0, a1) };
        self.selection = (from != to).then_some((from, to));
    }

    /// What the wearer did: see the module's notes for what each does.
    pub fn input(&mut self, input: &Input) -> Said {
        if let Input::Pointer { x, y } = input {
            self.pointer = (*x, *y);
        }
        // The release of a press that opened the menu by being held is nothing more.
        if matches!(input, Input::Press { pressed: false, .. }) && self.grip.as_ref().is_some_and(|g| g.held) {
            self.grip = None;
            return Said::Taken;
        }
        if self.pane.is_some() {
            return self.pane_input(input);
        }
        if let Some((menu, rows)) = self.menu.as_mut() {
            return match menu.input(input, self.pointer) {
                Some(MenuSaid::Chose(row)) => {
                    let act = rows[row].1;
                    self.menu = None;
                    self.act(act)
                }
                Some(MenuSaid::Closed) => {
                    self.menu = None;
                    Said::Taken
                }
                None => Said::Taken,
            };
        }
        if self.read.is_none() {
            return Said::Pass;
        }
        match input {
            Input::Pointer { x, y } => {
                self.hovered = self.line_at(*x, *y);
                let Some(grip) = self.grip.as_mut() else { return Said::Pass };
                if !grip.moved && apart(grip.from, (*x, *y)) > DRAG_AFTER {
                    grip.moved = true;
                }
                if grip.moved {
                    self.extend_to(*x, *y);
                }
                Said::Taken
            }
            Input::PointerLeft => {
                self.hovered = None;
                match self.grip.take() {
                    Some(_) => Said::Taken,
                    None => Said::Pass,
                }
            }
            Input::Press { pressed: true, modifiers } => {
                let (x, y) = self.pointer;
                let (Some(spot), Some(point)) = (self.spot_at(x, y), self.picture_point(x, y)) else {
                    // Off the text: what was chosen is let go of, and the press is the application's.
                    self.selection = None;
                    self.last = None;
                    return Said::Pass;
                };
                let read = self.read.as_ref().expect("text");
                let clicks = match self.last {
                    Some((when, at, n)) if when.elapsed() < DOUBLE_CLICK && apart(at, (x, y)) <= DOUBLE_CLICK_SLOP * 2.0 => n % 3 + 1,
                    _ => 1,
                };
                self.last = Some((Instant::now(), (x, y), clicks));
                let unit = match clicks {
                    1 => (spot, spot),
                    2 => read.word(spot.line, point),
                    _ => read.whole(spot.line),
                };
                // Shift: what is chosen runs on to here.
                self.selection = match (modifiers.shift, self.selection) {
                    (true, Some((a, b))) => Some(if spot < a { (spot, b) } else { (a, spot.max(a)) }).filter(|(a, b)| a != b),
                    _ if clicks == 1 => None,
                    _ => Some(unit).filter(|(a, b)| a != b),
                };
                let unit = if modifiers.shift { self.selection.unwrap_or(unit) } else { unit };
                self.grip = Some(Grip { from: (x, y), when: Instant::now(), moved: false, unit, clicks, held: false });
                Said::Taken
            }
            Input::Press { pressed: false, .. } => {
                let Some(grip) = self.grip.take() else { return Said::Pass };
                if !grip.moved && grip.clicks == 1 && self.selection.is_none() {
                    return Said::Tapped;
                }
                Said::Taken
            }
            Input::Key { key, modifiers, .. } => {
                let command = modifiers.control || modifiers.logo;
                match key {
                    Key::Char('c') if command && !modifiers.shift && self.selection.is_some() => {
                        self.copy();
                        Said::Copied
                    }
                    Key::Char('a') if command && !modifiers.shift => {
                        self.select_all();
                        Said::Taken
                    }
                    Key::Escape if self.selection.is_some() => {
                        self.selection = None;
                        Said::Taken
                    }
                    _ => Said::Pass,
                }
            }
            _ => Said::Pass,
        }
    }

    fn act(&mut self, act: Act) -> Said {
        match act {
            Act::Copy => {
                if self.copy() {
                    Said::Copied
                } else {
                    Said::Taken
                }
            }
            Act::SelectAll => {
                self.select_all();
                Said::Taken
            }
            Act::Translate => {
                self.translate();
                Said::Taken
            }
            Act::Read => Said::Read,
        }
    }

    /// What is chosen, or all of it, translated, in a pane beside it.
    fn translate(&mut self) {
        let Some(text) = self.selected().or_else(|| self.read.as_ref().map(ReadText::all)).filter(|t| !t.trim().is_empty()) else { return };
        let near = self.chosen_rect().unwrap_or_default();
        let mut done = Button::default();
        done.focused = true;
        self.pane = Some(Pane { near, job: Some(Translating::start(text)), result: None, copy: Button::default(), done, area: Rect::default(), scroll: 0.0, tall: 0.0, shown: 0.0 });
    }

    fn pane_input(&mut self, input: &Input) -> Said {
        let pointer = self.pointer;
        let Some(pane) = self.pane.as_mut() else { return Said::Pass };
        let translated = match &pane.result {
            Some(Ok((_, text))) => Some(text.clone()),
            _ => None,
        };
        match input {
            Input::Key { key: Key::Escape | Key::Enter, .. } => {
                self.pane = None;
                return Said::Taken;
            }
            Input::Key { key: Key::Char('c'), modifiers, .. } if modifiers.control || modifiers.logo => {
                if let Some(text) = translated {
                    crate::clipboard::copy(text);
                    self.said = Some(("Copied.".into(), false, Instant::now()));
                    return Said::Copied;
                }
                return Said::Taken;
            }
            // A press beside it closes it, as a menu's does, and does nothing else.
            Input::Press { pressed: true, .. } if !pane.area.contains(pointer.0, pointer.1) => {
                self.pane = None;
                return Said::Taken;
            }
            Input::Scroll { dy, .. } if pane.area.contains(pointer.0, pointer.1) => {
                pane.scroll = (pane.scroll + dy).clamp(0.0, (pane.tall - pane.shown).max(0.0));
                return Said::Taken;
            }
            _ => {}
        }
        if pane.copy.input(input) {
            if let Some(text) = translated {
                crate::clipboard::copy(text);
                self.said = Some(("Copied.".into(), false, Instant::now()));
                return Said::Copied;
            }
        }
        if pane.done.input(input) {
            self.pane = None;
        }
        Said::Taken
    }

    /// Where what is chosen is on the window -- or all the text, if nothing is.
    fn chosen_rect(&self) -> Option<Rect> {
        let read = self.read.as_ref()?;
        let (from, to) = self.selection.unwrap_or((Spot::default(), read.end()));
        let mut bounds: Option<(f32, f32, f32, f32)> = None;
        for (index, a, b) in self.stretches(from, to) {
            for (x, y) in self.map.corners(read.lines[index].quad(a, b)) {
                bounds = Some(match bounds {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
        bounds.map(|(x0, y0, x1, y1)| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    /// The lines from `from` to `to`, each with the graphemes of it inside: (line, from, to).
    fn stretches(&self, from: Spot, to: Spot) -> Vec<(usize, usize, usize)> {
        let Some(read) = self.read.as_ref().filter(|r| !r.is_empty()) else { return Vec::new() };
        let (from, to) = read.span(from, to);
        (from.line..=to.line.min(read.lines.len() - 1))
            .map(|i| (i, if i == from.line { from.at } else { 0 }, if i == to.line { to.at } else { read.lines[i].len() }))
            .filter(|(_, a, b)| a < b)
            .collect()
    }

    /// Over the picture: the dotted line under each word read unsurely, the faint light on the line
    /// under the pointer, and what is chosen. Drawn after the picture, before the application's
    /// controls.
    pub fn draw(&mut self, p: &mut Painter, gl: &Gl) {
        let Some(read) = self.read.as_ref() else { return };
        let map = self.map;
        let mark = theme::mark();
        let halo = if theme::dark() { [0.0, 0.0, 0.0, 0.55] } else { [1.0, 1.0, 1.0, 0.75] };
        // A word read unsurely: a row of dots under it, in the room's amber, each on a halo so that
        // it is seen on a dark picture and on white paper alike.
        for line in &read.lines {
            for word in line.words.iter().filter(|w| w.sure < SURE) {
                let [_, _, end, start] = map.corners(line.quad(word.graphemes.start, word.graphemes.end));
                let [top_start, ..] = map.corners(line.quad(word.graphemes.start, word.graphemes.end));
                let (dx, dy) = (end.0 - start.0, end.1 - start.1);
                let long = dx.hypot(dy);
                // Away from the text, under its bottom.
                let (ox, oy) = (start.0 - top_start.0, start.1 - top_start.1);
                let off = ox.hypot(oy).max(1e-3);
                let (ox, oy) = (ox / off * 3.5, oy / off * 3.5);
                let n = (long / 7.0).floor().max(1.0) as usize;
                for k in 0..=n {
                    let t = k as f32 / n as f32;
                    let (x, y) = (start.0 + dx * t + ox, start.1 + dy * t + oy);
                    p.fill(gl, Rect::new(x - 2.4, y - 2.4, 4.8, 4.8), 2.4, halo);
                    p.fill(gl, Rect::new(x - 1.5, y - 1.5, 3.0, 3.0), 1.5, look::unsure());
                }
            }
        }
        // There is text here: the line under the pointer, faintly lit.
        if let (Some(line), None) = (self.hovered.and_then(|i| read.lines.get(i)), &self.grip) {
            let corners = map.corners(line.quad(0, line.len()));
            let radius = (line.height() * map.scale() * 0.22).min(6.0);
            p.fill_quad(gl, corners, radius, [mark[0], mark[1], mark[2], 0.13]);
        }
        // What is chosen.
        if let Some((from, to)) = self.selection {
            for (index, a, b) in self.stretches(from, to) {
                let line = &read.lines[index];
                let radius = (line.height() * map.scale() * 0.18).min(5.0);
                p.fill_quad(gl, map.corners(line.quad(a, b)), radius, [mark[0], mark[1], mark[2], 0.40]);
            }
        }
    }

    /// Over everything, the application's controls too: the menu, the translation's pane, and
    /// what was done, within `all` (the window).
    pub fn draw_over(&mut self, p: &mut Painter, gl: &Gl, all: Rect) {
        if let Some((menu, rows)) = self.menu.as_mut() {
            let rows: Vec<Row> = rows.iter().map(|(row, _)| row.clone()).collect();
            menu.draw(p, gl, &rows, all);
        }
        if let Some(pane) = self.pane.as_mut() {
            draw_pane(p, gl, pane, all);
        }
        // What was done, what went wrong, or that the picture is being read: one pill, at the foot.
        let note = match &self.said {
            Some((said, trouble, _)) => Some((said.as_str(), *trouble)),
            None if self.reading => Some(("Reading the text\u{2026}", false)),
            None => None,
        };
        if let Some((words, trouble)) = note {
            let em = theme::FOOTER_EM;
            let w = (p.measure(gl, words, em, 8192.0 / p.scale()).0 + 44.0).min(all.w - 48.0);
            let foot = self.note_foot.unwrap_or(all.bottom() - 52.0);
            let near = self.pane.as_ref().map_or(foot - 44.0, |pane| (pane.area.bottom() + 12.0).min(foot - 44.0));
            let pill = Rect::new(all.x + (all.w - w) * 0.5, near, w, 44.0);
            look::glass(p, gl, pill, 22.0, 1.0);
            p.label(gl, words, pill.inset(4.0), em, if trouble { look::trouble() } else { theme::ink_hint() }, 0.5);
        }
    }
}

/// The translation's pane: beside what was translated, under it if there is room and over it if
/// not; its words, Copy and Done at its foot right.
fn draw_pane(p: &mut Painter, gl: &Gl, pane: &mut Pane, all: Rect) {
    let w = (all.w - 48.0).min(760.0);
    let em = theme::ROW_EM * 0.8;
    let inner_w = w - 56.0;
    let (body, ink) = match &pane.result {
        None => ("Translating\u{2026}".to_string(), theme::ink_hint()),
        Some(Ok((_, text))) => (text.clone(), theme::ink()),
        Some(Err(why)) => (why.clone(), look::trouble()),
    };
    let tall = p.measure_left(gl, &body, em, inner_w).1;
    let shown = tall.min((all.h * 0.45).max(em * 3.0));
    pane.tall = tall;
    pane.shown = shown;
    let title_h = theme::TITLE_EM * 0.8 * 1.5;
    let h = 28.0 + title_h + 10.0 + shown + 24.0 + Button::HEIGHT + 28.0;
    let near = pane.near;
    let y = if near.bottom() + 16.0 + h <= all.bottom() - 12.0 {
        near.bottom() + 16.0
    } else if near.y - 16.0 - h >= all.y + 12.0 {
        near.y - 16.0 - h
    } else {
        all.y + ((all.h - h) * 0.5).max(12.0)
    };
    let x = (near.x + near.w * 0.5 - w * 0.5).clamp(all.x + 24.0, (all.right() - 24.0 - w).max(all.x + 24.0));
    let area = Rect::new(x, y, w, h);
    pane.area = area;
    p.glow(gl, area, 24.0, [0.0, 0.0, 0.0, 0.45], 24.0);
    p.fill(gl, area, 24.0, theme::ground());
    look::glass(p, gl, area, 24.0, 1.0);
    let mut inner = area.inset(28.0);
    // Which way it went, as the rule decided it: "In English".
    let title = match &pane.result {
        Some(Ok((into, _))) => format!("In {}", spatiand_ai::language_name(into)),
        _ => "Translation".to_string(),
    };
    p.label(gl, &title, inner.take_top(title_h), theme::TITLE_EM * 0.8, theme::ink(), 0.0);
    inner.take_top(10.0);
    let words = inner.take_top(shown);
    p.clip(gl, words);
    p.text_left(gl, &body, words.x, words.y - pane.scroll, em, inner_w, ink);
    p.unclip(gl);
    if tall > shown {
        look::scrollbar(p, gl, Rect { x: words.right() + 14.0, w: theme::SCROLL_WIDTH, ..words }, shown, tall, pane.scroll);
    }
    let mut buttons = inner.take_bottom(Button::HEIGHT);
    let done_w = Button::width(p, gl, "Done").max(160.0);
    pane.done.draw(p, gl, Rect { x: buttons.right() - done_w, w: done_w, ..buttons }, "Done");
    buttons.w -= done_w + 12.0;
    if matches!(pane.result, Some(Ok(_))) {
        let copy_w = Button::icon_width(p, gl, "Copy");
        pane.copy.draw_icon(p, gl, Rect { x: buttons.right() - copy_w, w: copy_w, ..buttons }, Icon::Copy, "Copy");
    } else {
        pane.copy = Button::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Modifiers;
    use spatiand_voice::hemera::ocr;

    fn press(pressed: bool) -> Input {
        Input::Press { pressed, modifiers: Modifiers::default() }
    }

    fn at(x: f32, y: f32) -> Input {
        Input::Pointer { x, y }
    }

    fn chord(c: char) -> Input {
        Input::Key { key: Key::Char(c), text: None, modifiers: Modifiers { control: true, ..Default::default() }, letter: Some(c) }
    }

    /// Two lines in one block and a third in another, upright, ten pixels a grapheme.
    fn page() -> ReadText {
        let line = |text: &str, x: f32, y: f32| {
            let n = text.chars().count();
            ocr::Line {
                text: text.into(),
                polygon: vec![[x, y], [x + n as f32 * 10.0, y], [x + n as f32 * 10.0, y + 20.0], [x, y + 20.0]],
                confidence: 0.99,
                chars: (0..n).map(|i| [i as f32 * 10.0, (i + 1) as f32 * 10.0]).collect(),
                ..Default::default()
            }
        };
        let answer = ocr::Answer {
            image: ocr::Size { width: 400, height: 300 },
            blocks: vec![ocr::Block { lines: vec![line("hello world", 0.0, 0.0), line("second line", 0.0, 30.0)], ..Default::default() }, ocr::Block { lines: vec![line("third", 0.0, 100.0)], ..Default::default() }],
            ..Default::default()
        };
        ReadText::of(&answer)
    }

    /// The page drawn at twice its size, 100 pixels in.
    fn shown() -> LiveText {
        let mut live = LiveText::new();
        live.set_text(Some(page()));
        live.place(Mapping::onto((400.0, 300.0), Rect::new(100.0, 100.0, 800.0, 600.0)));
        live
    }

    #[test]
    fn a_drag_on_the_text_chooses_across_lines_and_one_off_it_is_not_the_text_s() {
        let mut live = shown();
        // "world" begins 6 graphemes in: x = 100 + 6*20 = 220; the first line is y 100..140.
        assert_eq!(live.input(&at(221.0, 120.0)), Said::Pass);
        assert_eq!(live.input(&press(true)), Said::Taken);
        assert_eq!(live.input(&at(260.0, 130.0)), Said::Taken);
        // To "second" on the next line (y 160..200): "sec".
        assert_eq!(live.input(&at(161.0, 180.0)), Said::Taken);
        assert_eq!(live.input(&press(false)), Said::Taken);
        assert_eq!(live.selected().as_deref(), Some("world\nsec"));
        // Copied, to the room's clipboard.
        assert_eq!(live.input(&chord('c')), Said::Copied);
        assert_eq!(crate::clipboard::text().as_deref(), Some("world\nsec"));
        // A press off the text lets go of it, and is the application's.
        assert_eq!(live.input(&at(950.0, 650.0)), Said::Pass);
        assert_eq!(live.input(&press(true)), Said::Pass);
        assert_eq!(live.selected(), None);
        assert_eq!(live.input(&at(990.0, 660.0)), Said::Pass, "its drag is the application's");
        assert_eq!(live.input(&press(false)), Said::Pass);
    }

    #[test]
    fn two_presses_choose_a_word_three_a_line_and_all_is_every_block() {
        let mut live = shown();
        live.input(&at(230.0, 120.0));
        for _ in 0..2 {
            live.input(&press(true));
            live.input(&press(false));
        }
        assert_eq!(live.selected().as_deref(), Some("world"));
        live.input(&press(true));
        live.input(&press(false));
        assert_eq!(live.selected().as_deref(), Some("hello world"));
        assert_eq!(live.input(&chord('a')), Said::Taken);
        assert_eq!(live.selected().as_deref(), Some("hello world\nsecond line\n\nthird"));
        // Escape lets go of it; a second Escape is the application's.
        let escape = Input::Key { key: Key::Escape, text: None, modifiers: Modifiers::default(), letter: None };
        assert_eq!(live.input(&escape), Said::Taken);
        assert_eq!(live.input(&escape), Said::Pass);
    }

    #[test]
    fn a_tap_on_the_text_is_the_application_s() {
        let mut live = shown();
        live.input(&at(130.0, 120.0));
        assert_eq!(live.input(&press(true)), Said::Taken);
        assert_eq!(live.input(&press(false)), Said::Tapped);
        assert_eq!(live.selected(), None);
    }

    #[test]
    fn a_press_held_still_opens_the_menu_on_the_word_and_its_release_is_nothing() {
        let mut live = shown();
        // On "line", the second line's second word (graphemes 7 to 11: x 240..320).
        live.input(&at(250.0, 180.0));
        live.input(&press(true));
        assert!(live.next_wake().is_some_and(|d| d <= HOLD_FOR_MENU));
        assert!(!live.tick(Instant::now()));
        assert!(live.tick(Instant::now() + HOLD_FOR_MENU));
        assert!(live.busy());
        assert_eq!(live.selected().as_deref(), Some("line"));
        assert_eq!(live.input(&press(false)), Said::Taken);
        assert!(live.busy(), "the menu stays open after the release");
        // Copy, Select all, Translate; Escape closes it and nothing else.
        let rows: Vec<&str> = live.menu.as_ref().unwrap().1.iter().map(|(row, _)| row.label.as_str()).collect();
        assert_eq!(rows, ["Copy", "Select all", "Translate"]);
        let escape = Input::Key { key: Key::Escape, text: None, modifiers: Modifiers::default(), letter: None };
        assert_eq!(live.input(&escape), Said::Taken);
        assert!(!live.busy());
        assert_eq!(live.selected().as_deref(), Some("line"));
    }

    #[test]
    fn the_right_button_off_the_text_offers_all_of_it_and_a_picture_not_read_offers_reading() {
        let mut live = shown();
        assert!(live.context(990.0, 690.0));
        let rows: Vec<&str> = live.menu.as_ref().unwrap().1.iter().map(|(row, _)| row.label.as_str()).collect();
        assert_eq!(rows, ["Select all", "Translate"]);
        let mut unread = LiveText::new();
        assert!(!unread.context(10.0, 10.0), "nothing to offer");
        unread.can_read = true;
        assert!(unread.context(10.0, 10.0));
        assert_eq!(unread.menu.as_ref().unwrap().1[0].1, Act::Read);
    }

    #[test]
    fn the_room_is_told_the_text_with_its_heading_and_nothing_when_none_was_read() {
        let live = shown();
        let told = live.described().unwrap();
        assert!(told.starts_with(HEADING));
        assert!(told.ends_with("hello world\nsecond line\n\nthird"));
        assert_eq!(LiveText::new().described(), None);
    }

    #[test]
    fn a_turned_picture_s_text_is_chosen_where_it_is_shown() {
        // The page turned a quarter clockwise and drawn 1:1 at the origin: "hello world" now runs
        // down the right side, from (300, 0) to (300, 110).
        let mut live = LiveText::new();
        live.set_text(Some(page()));
        live.place(Mapping::turned(1, (400.0, 300.0)).then(&Mapping::onto(turned_size(1, (400.0, 300.0)), Rect::new(0.0, 0.0, 300.0, 400.0))));
        // On "hello world" (x 280..300 once turned), at its 7th grapheme ("w" of world: 60..70 down).
        assert!(live.over_text(290.0, 65.0));
        assert!(!live.over_text(150.0, 65.0));
        live.input(&at(290.0, 64.0));
        live.input(&press(true));
        live.input(&press(false));
        live.input(&press(true));
        live.input(&press(false));
        assert_eq!(live.selected().as_deref(), Some("world"));
    }
}
