//! Markup mode: the tools to draw with over a picture or a PDF's pages -- Preview's set: choose,
//! crop, words, a box, a circle, an arrow, a colour and a weight, undo and redo -- and what they
//! do as the wearer presses, drags and types. The marks are `markup`'s; they are painted by
//! `ink` on the CPU into a picture laid over what is shown, the same painting a saved picture
//! gets, so that the window shows them as the file will have them.
//!
//! A press and a drag draw only here. Outside Markup mode they keep their meaning (move about in
//! a picture, slide to the next), and a later job is to make a press-drag over a picture's words
//! choose them: nothing here gives the plain view's press-drag any other.

use std::hash::{Hash, Hasher};
use std::time::Instant;

use spatiand_app::glow::{self, HasContext};
use spatiand_app::ui::{look, theme, Button, Field, Icon, Input, Menu, MenuSaid, Painter, Rect, Row, Texture, Typed, DOUBLE_CLICK, DOUBLE_CLICK_SLOP};
use spatiand_app::{Gl, Key, Modifiers};

use crate::ink::{self, Map};
use crate::markup::{corner_to, Area, Handle, Ink, Markup, Marks, Note, Point, Shape, Weight};

/// The toolbar's height: a row of buttons, and room round them.
pub const BAR: f32 = 60.0 + 24.0;

/// A tool: what a press and a drag on a page do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Crop,
    Text,
    Box,
    Circle,
    Arrow,
}

impl Tool {
    pub const ALL: [Tool; 6] = [Tool::Select, Tool::Crop, Tool::Text, Tool::Box, Tool::Circle, Tool::Arrow];

    fn icon(self) -> Icon {
        match self {
            Tool::Select => Icon::Pointer,
            Tool::Crop => Icon::Crop,
            Tool::Text => Icon::Text,
            Tool::Box => Icon::Square,
            Tool::Circle => Icon::Oval,
            Tool::Arrow => Icon::Arrow,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Crop => "Crop",
            Tool::Text => "Text",
            Tool::Box => "Box",
            Tool::Circle => "Circle",
            Tool::Arrow => "Arrow",
        }
    }
}

/// A page as it is on the window: the rectangle it is drawn in, and the part of it, in its own
/// units, that rectangle shows (the whole page, or its crop).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub page: usize,
    pub rect: Rect,
    pub region: Area,
}

impl Placed {
    /// Logical pixels to one of the page's units.
    pub fn scale(&self) -> f32 {
        self.rect.w / self.region.w().max(1e-6)
    }

    pub fn to_page(self, at: (f32, f32)) -> Point {
        let s = self.scale();
        (self.region.x0 + (at.0 - self.rect.x) / s, self.region.y0 + (at.1 - self.rect.y) / s)
    }

    pub fn to_window(self, p: Point) -> (f32, f32) {
        let s = self.scale();
        (self.rect.x + (p.0 - self.region.x0) * s, self.rect.y + (p.1 - self.region.y0) * s)
    }

    pub fn area_on_window(&self, a: &Area) -> Rect {
        let (x0, y0) = self.to_window((a.x0, a.y0));
        let (x1, y1) = self.to_window((a.x1, a.y1));
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
}

/// What a press and a drag are doing.
#[derive(Debug, Clone)]
enum Gesture {
    /// A new shape, from where the press was to where the pointer is.
    Drawing { page: usize, start: Point, now: Point },
    /// The chosen note moved: where the pointer last was on its page, and the marks before.
    Moving { index: usize, last: Point, before: Marks },
    /// One of the chosen note's handles taken hold of.
    Reshaping { index: usize, handle: Handle, before: Marks },
    /// A crop's rectangle drawn, a corner of it moved, or the whole of it.
    Cropping { page: usize, start: Point },
    CropCorner { corner: usize },
    CropMoving { last: Point },
}

/// Words being typed onto a page: where, as which note if they are one already, and how.
pub struct Typing {
    pub page: usize,
    pub at: Point,
    pub index: Option<usize>,
    pub field: Field,
    pub size: f32,
    pub ink: Ink,
    pub weight: Weight,
}

/// The colours or the weights to choose from, open under their button.
enum Popup {
    Inks { cursor: usize, swatches: Vec<Button>, area: Rect },
    Weights(Menu),
}

/// The marks laid over the window, as a picture of its own: painted again only when they, or
/// where the pages are, change.
#[derive(Default)]
struct Overlay {
    texture: Option<Texture>,
    key: u64,
    pixels: Vec<u8>,
}

/// What the tools did with an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Said {
    /// Not theirs: the view does with it what it always does.
    Pass,
    Taken,
    /// The tools put away: Escape with nothing else to let go of.
    Done,
}

/// Markup on one file: what is drawn, and the tools, out (Markup mode) or put away with what was
/// drawn still to be saved.
pub struct Desk {
    pub markup: Markup,
    /// The tools are out.
    pub on: bool,
    pub tool: Tool,
    pub ink: Ink,
    /// The weight of lines drawn, and the size of words.
    pub line: Weight,
    pub words: Weight,
    gesture: Option<Gesture>,
    /// A crop drawn and not yet made (Enter or the Crop button make it).
    pub cropping: Option<(usize, Area)>,
    pub typing: Option<Typing>,
    popup: Option<Popup>,
    tools: Vec<Button>,
    ink_button: Button,
    weight_button: Button,
    undo_button: Button,
    redo_button: Button,
    crop_button: Button,
    /// Where the toolbar is, for what the pointer is over, and its colour and weight buttons,
    /// which their choices open under.
    pub bar: Rect,
    ink_at: Rect,
    weight_at: Rect,
    /// Each page's part shown, in its units -- the whole of it or its crop -- by its number: what
    /// a weight is a share of.
    pub regions: Vec<Area>,
    pointer: (f32, f32),
    shift: bool,
    last_press: Option<(Instant, (f32, f32))>,
    overlay: Overlay,
    /// Whether Crop is offered: not for a PDF whose owner does not allow its pages changed.
    pub may_crop: bool,
}

impl Desk {
    /// The tools, out, on a file shown turned `turns` quarters from how it was read.
    pub fn new(turns: u8) -> Desk {
        Desk {
            markup: Markup::new(turns),
            on: true,
            tool: Tool::Select,
            ink: Ink::default(),
            line: Weight::default(),
            words: Weight::default(),
            gesture: None,
            cropping: None,
            typing: None,
            popup: None,
            tools: Tool::ALL.iter().map(|_| Button::default()).collect(),
            ink_button: Button::default(),
            weight_button: Button::default(),
            undo_button: Button::default(),
            redo_button: Button::default(),
            crop_button: Button::default(),
            bar: Rect::default(),
            ink_at: Rect::default(),
            weight_at: Rect::default(),
            regions: Vec::new(),
            pointer: (0.0, 0.0),
            shift: false,
            last_press: None,
            overlay: Overlay::default(),
            may_crop: true,
        }
    }

    /// Whether anything is drawn or cut that the file has not got.
    pub fn edited(&self) -> bool {
        self.markup.edited()
    }

    fn short(&self, page: usize) -> f32 {
        self.regions.get(page).map_or(1000.0, |r| r.w().min(r.h()))
    }

    fn is_words(&self, index: usize) -> bool {
        matches!(self.markup.marks().notes.get(index).map(|n| &n.shape), Some(Shape::Text { .. }))
    }

    /// The tools put away: the words being typed kept, a crop not made let go of.
    pub fn put_away(&mut self) {
        self.finish_typing();
        self.cancel();
        self.cropping = None;
        self.popup = None;
        self.on = false;
    }

    /// What a press and a drag were doing, let go of as if they had not begun.
    fn cancel(&mut self) {
        match self.gesture.take() {
            Some(Gesture::Moving { before, .. } | Gesture::Reshaping { before, .. }) => {
                self.markup.adjust(|marks| *marks = before);
            }
            Some(Gesture::Cropping { .. }) => self.cropping = None,
            _ => {}
        }
    }

    /// A tool chosen from the bar.
    pub fn choose(&mut self, tool: Tool) {
        if tool == Tool::Crop && !self.may_crop {
            return;
        }
        self.finish_typing();
        if tool != Tool::Crop {
            self.cropping = None;
        }
        if tool != Tool::Select {
            self.markup.chosen = None;
        }
        self.tool = tool;
        self.markup.generation += 1;
    }

    /// The colour chosen: for what is drawn next, and for the note chosen.
    pub fn set_ink(&mut self, ink: Ink) {
        self.ink = ink;
        if let Some(typing) = &mut self.typing {
            typing.ink = ink;
        }
        if let Some(i) = self.markup.chosen {
            self.markup.change(|marks| {
                if let Some(note) = marks.notes.get_mut(i) {
                    note.ink = ink;
                }
            });
        }
    }

    /// Whether the weight in hand is the words' (a text note chosen, or the Text tool).
    fn weighing_words(&self) -> bool {
        self.tool == Tool::Text || self.typing.is_some() || self.markup.chosen.is_some_and(|i| self.is_words(i))
    }

    /// The weight chosen: for lines or words, whichever is in hand, and for the note chosen.
    pub fn set_weight(&mut self, weight: Weight) {
        if self.weighing_words() {
            self.words = weight;
        } else {
            self.line = weight;
        }
        if let Some(typing) = &mut self.typing {
            typing.weight = weight;
            typing.size = weight.text(self.regions.get(typing.page).map_or(1000.0, |r| r.w().min(r.h())));
            return;
        }
        let Some(i) = self.markup.chosen else { return };
        let Some(page) = self.markup.marks().notes.get(i).map(|n| n.page) else { return };
        let short = self.short(page);
        self.markup.change(|marks| {
            let note = &mut marks.notes[i];
            note.weight = weight;
            match &mut note.shape {
                Shape::Text { extent, .. } => {
                    let size = weight.text(short);
                    let by = size / note.size.max(1e-6);
                    *extent = (extent.0 * by, extent.1 * by);
                    note.size = size;
                }
                _ => note.size = weight.line(short),
            }
        });
    }

    pub fn undo(&mut self) {
        self.cancel();
        self.typing = None;
        self.markup.undo();
    }

    pub fn redo(&mut self) {
        self.cancel();
        self.typing = None;
        self.markup.redo();
    }

    /// The crop drawn, made: one step, as every change is.
    pub fn crop(&mut self) {
        if let Some((page, area)) = self.cropping.take() {
            if area.w() >= 1.0 && area.h() >= 1.0 {
                self.markup.change(|marks| {
                    marks.crops.retain(|(p, _)| *p != page);
                    marks.crops.push((page, area));
                });
            }
        }
    }

    /// The words of note `index` to be typed again.
    fn edit(&mut self, index: usize) {
        let Some(note) = self.markup.marks().notes.get(index).cloned() else { return };
        let Shape::Text { at, words, .. } = &note.shape else { return };
        let mut field = Field::default();
        field.set(words.clone());
        field.focused = true;
        self.markup.chosen = Some(index);
        self.typing = Some(Typing { page: note.page, at: *at, index: Some(index), field, size: note.size, ink: note.ink, weight: note.weight });
        self.markup.generation += 1;
    }

    /// The words typed, kept: a note made, or changed, or taken away if they are gone; then the
    /// note is chosen, as Preview chooses what was just drawn.
    pub fn finish_typing(&mut self) {
        let Some(typing) = self.typing.take() else { return };
        let words = typing.field.text.trim().to_string();
        match (typing.index, words.is_empty()) {
            (Some(i), true) => {
                self.markup.chosen = Some(i);
                self.markup.delete_chosen();
            }
            (Some(i), false) => {
                let extent = ink::measure(&mut ink::fonts(), &words, typing.size);
                self.markup.change(|marks| {
                    if let Some(Shape::Text { words: w, extent: e, .. }) = marks.notes.get_mut(i).map(|n| &mut n.shape) {
                        *w = words;
                        *e = extent;
                    }
                });
                self.markup.chosen = Some(i);
            }
            (None, true) => {}
            (None, false) => {
                let extent = ink::measure(&mut ink::fonts(), &words, typing.size);
                self.markup.add(Note { page: typing.page, shape: Shape::Text { at: typing.at, words, extent }, ink: typing.ink, weight: typing.weight, size: typing.size });
            }
        }
        self.tool = Tool::Select;
        self.markup.generation += 1;
    }

    /// The shape being drawn, as it is now.
    fn live(&self) -> Option<Note> {
        let Some(Gesture::Drawing { page, start, now }) = &self.gesture else { return None };
        let shape = match self.tool {
            Tool::Box => Shape::Box(corner_to(*start, *now, self.shift)),
            Tool::Circle => Shape::Oval(corner_to(*start, *now, self.shift)),
            Tool::Arrow => Shape::Arrow { from: *start, to: if self.shift { snapped(*start, *now) } else { *now } },
            _ => return None,
        };
        Some(Note { page: *page, shape, ink: self.ink, weight: self.line, size: self.line.line(self.short(*page)) })
    }

    /// The page under the pointer, as it is on the window.
    fn under<'a>(&self, placed: &'a [Placed]) -> Option<&'a Placed> {
        placed.iter().find(|p| p.rect.contains(self.pointer.0, self.pointer.1))
    }

    /// The pointer on the page a press began on, kept within what is shown of it.
    fn on_page(&self, placed: &[Placed], page: usize) -> Option<(Point, Area)> {
        let place = placed.iter().find(|p| p.page == page)?;
        let (x, y) = place.to_page(self.pointer);
        let r = place.region;
        Some(((x.clamp(r.x0, r.x1), y.clamp(r.y0, r.y1)), r))
    }

    /// An input, as the tools take it: `placed` is where each page shown is on the window.
    pub fn input(&mut self, input: &Input, placed: &[Placed]) -> Said {
        if let Input::Pointer { x, y } = input {
            self.pointer = (*x, *y);
        }
        if let Input::Key { modifiers, .. } | Input::Press { modifiers, .. } = input {
            self.shift = modifiers.shift;
        }
        if let Some(said) = self.popup_input(input) {
            return said;
        }
        if let Some(said) = self.typing_input(input) {
            return said;
        }
        if let Some(said) = self.bar_input(input) {
            return said;
        }
        match input {
            Input::Pointer { .. } => self.drag(placed),
            Input::PointerLeft => {
                self.release();
                Said::Pass
            }
            Input::Press { pressed: true, .. } => self.press(placed),
            Input::Press { pressed: false, .. } => self.release(),
            Input::Key { key, modifiers, .. } => self.key(*key, *modifiers, placed),
            _ => Said::Pass,
        }
    }

    /// The words being typed have the keys; a press elsewhere keeps them and then does what it
    /// does (the look and feel's rule 9).
    fn typing_input(&mut self, input: &Input) -> Option<Said> {
        let typing = self.typing.as_mut()?;
        match input {
            Input::Key { key: Key::Escape, .. } => {
                self.typing = None;
                self.markup.generation += 1;
                Some(Said::Taken)
            }
            Input::Press { pressed: true, .. } if !typing.field.contains(self.pointer.0, self.pointer.1) => {
                self.finish_typing();
                None
            }
            _ => {
                if typing.field.input(input) == Some(Typed::Entered) {
                    self.finish_typing();
                }
                matches!(input, Input::Key { .. } | Input::Press { .. }).then_some(Said::Taken)
            }
        }
    }

    fn popup_input(&mut self, input: &Input) -> Option<Said> {
        let pointer = self.pointer;
        match self.popup.as_mut()? {
            Popup::Weights(menu) => {
                match menu.input(input, pointer) {
                    Some(MenuSaid::Chose(row)) => {
                        self.popup = None;
                        self.set_weight(Weight::ALL[row.min(2)]);
                    }
                    Some(MenuSaid::Closed) => self.popup = None,
                    None => {}
                }
                Some(Said::Taken)
            }
            Popup::Inks { cursor, swatches, area } => {
                let mut chose = None;
                for (i, swatch) in swatches.iter_mut().enumerate() {
                    if swatch.input(input) {
                        chose = Some(i);
                    }
                }
                match input {
                    Input::Key { key: Key::Escape, .. } => self.popup = None,
                    Input::Key { key: Key::Left | Key::Up, .. } => *cursor = cursor.saturating_sub(1),
                    Input::Key { key: Key::Right | Key::Down, .. } => *cursor = (*cursor + 1).min(Ink::ALL.len() - 1),
                    Input::Key { key: Key::Enter | Key::Space, .. } => chose = Some(*cursor),
                    Input::Press { pressed: true, .. } if !area.contains(pointer.0, pointer.1) => self.popup = None,
                    _ => {}
                }
                if let Some(i) = chose {
                    self.popup = None;
                    self.set_ink(Ink::ALL[i]);
                }
                Some(Said::Taken)
            }
        }
    }

    fn bar_input(&mut self, input: &Input) -> Option<Said> {
        let mut tool = None;
        for (i, button) in self.tools.iter_mut().enumerate() {
            if button.input(input) {
                tool = Some(Tool::ALL[i]);
            }
        }
        let inks = self.ink_button.input(input);
        let weights = self.weight_button.input(input);
        let undo = self.undo_button.input(input) && self.markup.can_undo();
        let redo = self.redo_button.input(input) && self.markup.can_redo();
        let crop = self.crop_button.input(input) && self.cropping.is_some();
        if let Some(tool) = tool {
            self.choose(tool);
        } else if inks {
            let at = self.ink.index();
            self.popup = Some(Popup::Inks { cursor: at, swatches: Ink::ALL.iter().map(|_| Button::default()).collect(), area: Rect::default() });
        } else if weights {
            let mut menu = Menu::new((self.weight_at.x, self.bar.bottom() + 8.0));
            menu.list.cursor = Weight::ALL.iter().position(|w| *w == if self.weighing_words() { self.words } else { self.line }).unwrap_or(1);
            self.popup = Some(Popup::Weights(menu));
        } else if undo {
            self.undo();
        } else if redo {
            self.redo();
        } else if crop {
            self.crop();
        } else {
            // A press on the bar between its buttons presses nothing under it.
            let on_bar = self.bar.contains(self.pointer.0, self.pointer.1);
            return (matches!(input, Input::Press { pressed: true, .. }) && on_bar && self.gesture.is_none()).then_some(Said::Taken);
        }
        Some(Said::Taken)
    }

    fn press(&mut self, placed: &[Placed]) -> Said {
        let Some(place) = self.under(placed).copied() else { return Said::Pass };
        let at = place.to_page(self.pointer);
        let (slack, grip) = (10.0 / place.scale(), 22.0 / place.scale());
        let now = Instant::now();
        let double = self.last_press.is_some_and(|(when, p)| now.duration_since(when) < DOUBLE_CLICK && (p.0 - self.pointer.0).abs() <= DOUBLE_CLICK_SLOP && (p.1 - self.pointer.1).abs() <= DOUBLE_CLICK_SLOP);
        self.last_press = Some((now, self.pointer));
        self.popup = None;
        match self.tool {
            Tool::Select | Tool::Box | Tool::Circle | Tool::Arrow => {
                // The chosen note's handles, whatever the tool: it is reshaped.
                if let Some(index) = self.markup.chosen.filter(|&i| self.markup.marks().notes.get(i).is_some_and(|n| n.page == place.page)) {
                    if let Some(handle) = self.markup.handle_at(index, at, grip) {
                        self.gesture = Some(Gesture::Reshaping { index, handle, before: self.markup.begin() });
                        return Said::Taken;
                    }
                }
                if self.tool != Tool::Select {
                    self.markup.chosen = None;
                    self.gesture = Some(Gesture::Drawing { page: place.page, start: at, now: at });
                    return Said::Taken;
                }
                match self.markup.hit(place.page, at, slack) {
                    Some(index) => {
                        if double && self.markup.chosen == Some(index) && self.is_words(index) {
                            self.edit(index);
                            return Said::Taken;
                        }
                        self.markup.chosen = Some(index);
                        self.markup.generation += 1;
                        self.gesture = Some(Gesture::Moving { index, last: at, before: self.markup.begin() });
                        Said::Taken
                    }
                    None => {
                        // Nothing there: let go of what was chosen, and the press is the view's
                        // (a drag moves about in a zoomed picture, or along the pages).
                        if self.markup.chosen.take().is_some() {
                            self.markup.generation += 1;
                        }
                        Said::Pass
                    }
                }
            }
            Tool::Text => {
                match self.markup.hit(place.page, at, slack).filter(|&i| self.is_words(i)) {
                    Some(index) => self.edit(index),
                    None => {
                        let mut field = Field::default();
                        field.focused = true;
                        let size = self.words.text(self.short(place.page));
                        self.typing = Some(Typing { page: place.page, at, index: None, field, size, ink: self.ink, weight: self.words });
                    }
                }
                Said::Taken
            }
            Tool::Crop => {
                if let Some((page, area)) = self.cropping.filter(|(p, _)| *p == place.page) {
                    let corners = area.corners();
                    if let Some(corner) = (0..4).find(|&c| (corners[c].0 - at.0).abs() <= grip && (corners[c].1 - at.1).abs() <= grip) {
                        self.gesture = Some(Gesture::CropCorner { corner });
                        return Said::Taken;
                    }
                    if area.contains(at) && page == place.page {
                        self.gesture = Some(Gesture::CropMoving { last: at });
                        return Said::Taken;
                    }
                }
                self.cropping = None;
                self.gesture = Some(Gesture::Cropping { page: place.page, start: at });
                Said::Taken
            }
        }
    }

    fn drag(&mut self, placed: &[Placed]) -> Said {
        let page = match &self.gesture {
            None => return Said::Pass,
            Some(Gesture::Drawing { page, .. } | Gesture::Cropping { page, .. }) => *page,
            Some(Gesture::Moving { index, .. } | Gesture::Reshaping { index, .. }) => self.markup.marks().notes.get(*index).map_or(0, |n| n.page),
            Some(Gesture::CropCorner { .. } | Gesture::CropMoving { .. }) => self.cropping.map_or(0, |(p, _)| p),
        };
        let Some((at, region)) = self.on_page(placed, page) else { return Said::Taken };
        let even = self.shift;
        match &mut self.gesture {
            Some(Gesture::Drawing { now, .. }) => {
                *now = at;
                self.markup.generation += 1;
            }
            Some(Gesture::Moving { index, last, .. }) => {
                let (i, (dx, dy)) = (*index, (at.0 - last.0, at.1 - last.1));
                *last = at;
                self.markup.adjust(|marks| {
                    if let Some(note) = marks.notes.get_mut(i) {
                        note.shape = note.shape.moved(dx, dy);
                    }
                });
            }
            Some(Gesture::Reshaping { index, handle, .. }) => {
                let (i, handle) = (*index, *handle);
                self.markup.adjust(|marks| {
                    if let Some(note) = marks.notes.get_mut(i) {
                        let to = match (&note.shape, handle) {
                            (Shape::Arrow { from, .. }, Handle::To) if even => snapped(*from, at),
                            (Shape::Arrow { to, .. }, Handle::From) if even => snapped(*to, at),
                            _ => at,
                        };
                        note.shape = note.shape.with_handle(handle, to, even);
                    }
                });
            }
            Some(Gesture::Cropping { page, start }) => {
                self.cropping = Area::between(*start, at).meet(&region).map(|a| (*page, a));
                self.markup.generation += 1;
            }
            Some(Gesture::CropCorner { corner }) => {
                if let Some((page, area)) = self.cropping {
                    let fixed = area.corners()[(*corner + 2) % 4];
                    self.cropping = Area::between(fixed, at).meet(&region).map(|a| (page, a)).or(self.cropping);
                    // The corner held is the one nearest the pointer now, as the area turns over.
                    if let Some((_, a)) = self.cropping {
                        let c = a.corners();
                        *corner = (0..4).min_by(|&x, &y| (c[x].0 - at.0).hypot(c[x].1 - at.1).total_cmp(&(c[y].0 - at.0).hypot(c[y].1 - at.1))).unwrap_or(*corner);
                    }
                }
            }
            Some(Gesture::CropMoving { last }) => {
                if let Some((page, area)) = self.cropping {
                    let dx = (at.0 - last.0).clamp(region.x0 - area.x0, region.x1 - area.x1);
                    let dy = (at.1 - last.1).clamp(region.y0 - area.y0, region.y1 - area.y1);
                    self.cropping = Some((page, Area { x0: area.x0 + dx, y0: area.y0 + dy, x1: area.x1 + dx, y1: area.y1 + dy }));
                    *last = at;
                }
            }
            None => {}
        }
        Said::Taken
    }

    fn release(&mut self) -> Said {
        match self.gesture.take() {
            None => Said::Pass,
            Some(Gesture::Drawing { page, start, now }) => {
                self.gesture = Some(Gesture::Drawing { page, start, now });
                let live = self.live();
                self.gesture = None;
                if let Some(note) = live {
                    if !note.shape.is_speck(self.short(page) * 0.004) {
                        self.markup.add(note);
                        // As Preview does: what was drawn is chosen, to be changed or moved.
                        self.tool = Tool::Select;
                    }
                }
                self.markup.generation += 1;
                Said::Taken
            }
            Some(Gesture::Moving { before, .. } | Gesture::Reshaping { before, .. }) => {
                self.markup.finish(before);
                Said::Taken
            }
            Some(Gesture::Cropping { .. }) => {
                if self.cropping.is_some_and(|(page, a)| a.w() < self.short(page) * 0.01 || a.h() < self.short(page) * 0.01) {
                    self.cropping = None;
                }
                Said::Taken
            }
            Some(Gesture::CropCorner { .. } | Gesture::CropMoving { .. }) => Said::Taken,
        }
    }

    fn key(&mut self, key: Key, modifiers: Modifiers, placed: &[Placed]) -> Said {
        let command = modifiers.control || modifiers.logo;
        match key {
            Key::Escape => {
                if self.gesture.is_some() {
                    self.cancel();
                } else if self.cropping.is_some() {
                    self.cropping = None;
                } else if self.markup.chosen.take().is_none() {
                    return Said::Done;
                }
                self.markup.generation += 1;
                Said::Taken
            }
            Key::Enter => {
                if self.cropping.is_some() {
                    self.crop();
                } else if let Some(i) = self.markup.chosen.filter(|&i| self.is_words(i)) {
                    self.edit(i);
                }
                Said::Taken
            }
            Key::Delete | Key::Backspace => {
                self.markup.delete_chosen();
                Said::Taken
            }
            Key::Char('z') if command => {
                if modifiers.shift {
                    self.redo();
                } else {
                    self.undo();
                }
                Said::Taken
            }
            // The chosen note nudged: a logical pixel at a time, ten with Shift. With nothing
            // chosen they are the view's (along a document, about a zoomed picture), which keeps to
            // this file while the tools are out.
            Key::Left | Key::Right | Key::Up | Key::Down => {
                let Some(i) = self.markup.chosen else { return Said::Pass };
                let page = self.markup.marks().notes.get(i).map_or(0, |n| n.page);
                let step = placed.iter().find(|p| p.page == page).map_or(1.0, |p| 1.0 / p.scale()) * if modifiers.shift { 10.0 } else { 1.0 };
                let (dx, dy) = match key {
                    Key::Left => (-step, 0.0),
                    Key::Right => (step, 0.0),
                    Key::Up => (0.0, -step),
                    _ => (0.0, step),
                };
                self.markup.change(|marks| {
                    if let Some(note) = marks.notes.get_mut(i) {
                        note.shape = note.shape.moved(dx, dy);
                    }
                });
                Said::Taken
            }
            _ => Said::Pass,
        }
    }

    /// Which keys do what now, for the foot.
    pub fn hint(&self) -> String {
        let undo = format!("[{}] Undo", look::chord("Z"));
        if self.typing.is_some() {
            return "[Enter] Keep  [Esc] Drop".into();
        }
        if self.cropping.is_some() {
            return "[Drag] Change  [Enter] Crop  [Esc] Cancel".into();
        }
        match (self.tool, self.markup.chosen) {
            (_, Some(i)) if self.is_words(i) => format!("[Drag] Move  [Enter] Change the words  [Delete] Delete  {undo}  [Esc] Done"),
            (_, Some(_)) => format!("[Drag] Move  [Delete] Delete  {undo}  [Esc] Done"),
            (Tool::Select, None) => format!("{undo}  [Esc] Done"),
            (Tool::Crop, None) => "[Drag] Choose what to keep  [Esc] Done".into(),
            (Tool::Text, None) => "[Press] Write there  [Esc] Done".into(),
            (Tool::Box, None) => "[Drag] Draw  [Shift] Square  [Esc] Done".into(),
            (Tool::Circle, None) => "[Drag] Draw  [Shift] Round  [Esc] Done".into(),
            (Tool::Arrow, None) => "[Drag] Draw  [Shift] Straight  [Esc] Done".into(),
        }
    }

    /// What the room's speech is told of the tools.
    pub fn words_for_speech(&self) -> String {
        format!("Markup is on, with the {} tool, in {}.", self.tool.name().to_lowercase(), self.ink.name().to_lowercase())
    }
}

/// The point from `from` towards `to`, along the nearest eighth of a turn: a line kept level,
/// upright or at 45 degrees.
fn snapped(from: Point, to: Point) -> Point {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = dx.hypot(dy);
    let angle = (dy.atan2(dx) / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
    (from.0 + length * angle.cos(), from.1 + length * angle.sin())
}

impl Ink {
    fn index(self) -> usize {
        Ink::ALL.iter().position(|i| *i == self).unwrap_or(0)
    }

    /// The colour, as the painter takes one.
    pub fn colour(self) -> [f32; 4] {
        let [r, g, b] = self.rgb();
        [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
    }
}

// --- drawing ---

/// A rectangle's edge, `width` wide, inside it.
fn frame(p: &mut Painter, gl: &Gl, r: Rect, width: f32, colour: [f32; 4]) {
    p.fill(gl, Rect::new(r.x, r.y, r.w, width), 0.0, colour);
    p.fill(gl, Rect::new(r.x, r.bottom() - width, r.w, width), 0.0, colour);
    p.fill(gl, Rect::new(r.x, r.y, width, r.h), 0.0, colour);
    p.fill(gl, Rect::new(r.right() - width, r.y, width, r.h), 0.0, colour);
}

/// A handle to take hold of: a dot of the accent on a ring of the ground, its middle at (x, y).
fn knob(p: &mut Painter, gl: &Gl, x: f32, y: f32) {
    p.fill(gl, Rect::new(x - 9.0, y - 9.0, 18.0, 18.0), 9.0, theme::ground());
    p.fill(gl, Rect::new(x - 6.0, y - 6.0, 12.0, 12.0), 6.0, theme::mark());
}

/// A colour to choose, drawn in the middle of its button: a disc of it on a faint ring, so that
/// white and black show on either look.
fn swatch(p: &mut Painter, gl: &Gl, r: Rect, ink: Ink) {
    let side = r.w.min(r.h) * 0.52;
    let disc = Rect::new(r.x + (r.w - side) * 0.5, r.y + (r.h - side) * 0.5, side, side);
    p.fill(gl, disc.inset(-2.0), side * 0.5 + 2.0, theme::wash(0.35));
    p.fill(gl, disc, side * 0.5, ink.colour());
}

impl Desk {
    /// How wide the toolbar would like to be.
    pub fn bar_width(&self, p: &mut Painter, gl: &Gl) -> f32 {
        let crop = if self.cropping.is_some() { Button::icon_width(p, gl, "Crop") + 20.0 } else { 0.0 };
        10.0 * Button::HEIGHT + 96.0 + crop + 24.0
    }

    /// The toolbar, in `rect`: the tools, the colour and the weight, undo and redo -- each drawn
    /// only where it can act -- and, while a crop is drawn, the button that makes it.
    pub fn draw_bar(&mut self, p: &mut Painter, gl: &Gl, rect: Rect) {
        self.bar = rect;
        look::glass(p, gl, rect, 22.0, 1.0);
        let inner = rect.inset(12.0);
        let crop_w = if self.cropping.is_some() { Button::icon_width(p, gl, "Crop") } else { 0.0 };
        let side = ((inner.w - 96.0 - if crop_w > 0.0 { crop_w + 20.0 } else { 0.0 }) / 10.0).clamp(44.0, Button::HEIGHT);
        let y = inner.y + (inner.h - side) * 0.5;
        let mut x = inner.x;
        let mut next = |gap: f32| {
            let r = Rect::new(x, y, side, side);
            x += side + gap;
            r
        };
        for (i, tool) in Tool::ALL.iter().enumerate() {
            let r = next(if i + 1 == Tool::ALL.len() { 20.0 } else { 8.0 });
            // Not offered: its place left empty, and nothing there to press.
            if *tool == Tool::Crop && !self.may_crop {
                self.tools[i] = Button::default();
                continue;
            }
            // Lit to show the tool in hand, as a state only: not left focused for Enter.
            let button = &mut self.tools[i];
            button.focused = *tool == self.tool;
            button.draw_icon(p, gl, r, tool.icon(), "");
            button.focused = false;
        }
        let r = next(8.0);
        self.ink_at = r;
        self.ink_button.draw_icon(p, gl, r, Icon::Oval, "");
        swatch(p, gl, r, self.typing.as_ref().map_or(self.ink, |t| t.ink));
        let r = next(20.0);
        self.weight_at = r;
        let words = self.weighing_words();
        let weight = if words { self.typing.as_ref().map_or(self.words, |t| t.weight) } else { self.line };
        let icon = if words { Icon::Text } else { [Icon::LineThin, Icon::LineMedium, Icon::LineThick][Weight::ALL.iter().position(|w| *w == weight).unwrap_or(1)] };
        self.weight_button.draw_icon(p, gl, r, icon, "");
        // A button that cannot act is not drawn (the look and feel's rule 8), nor pressed.
        let r = next(8.0);
        if self.markup.can_undo() {
            self.undo_button.draw_icon(p, gl, r, Icon::Undo, "");
        } else {
            self.undo_button = Button::default();
        }
        let r = next(20.0);
        if self.markup.can_redo() {
            self.redo_button.draw_icon(p, gl, r, Icon::Redo, "");
        } else {
            self.redo_button = Button::default();
        }
        if self.cropping.is_some() {
            let r = Rect::new(inner.right() - crop_w, y, crop_w, side);
            self.crop_button.focused = true;
            self.crop_button.draw_icon(p, gl, r, Icon::Crop, "Crop");
            self.crop_button.focused = false;
        } else {
            self.crop_button = Button::default();
        }
    }

    /// The colours or the weights, open over everything in `all`.
    pub fn draw_popup(&mut self, p: &mut Painter, gl: &Gl, all: Rect) {
        let words = self.weighing_words();
        let (ink, ink_at, bar) = (self.typing.as_ref().map_or(self.ink, |t| t.ink), self.ink_at, self.bar);
        match &mut self.popup {
            Some(Popup::Weights(menu)) => {
                let rows: Vec<Row> = Weight::ALL
                    .iter()
                    .zip([Icon::LineThin, Icon::LineMedium, Icon::LineThick])
                    .map(|(w, icon)| if words { Row::new(w.text_name()).icon(Icon::Text) } else { Row::new(w.line_name()).icon(icon) })
                    .collect();
                menu.draw(p, gl, &rows, all);
            }
            Some(Popup::Inks { cursor, swatches, area }) => {
                let side = Button::HEIGHT;
                let w = Ink::ALL.len() as f32 * (side + 6.0) - 6.0 + 24.0;
                let x = (ink_at.x + ink_at.w * 0.5 - w * 0.5).clamp(all.x + 12.0, (all.right() - w - 12.0).max(all.x + 12.0));
                *area = Rect::new(x, bar.bottom() + 8.0, w, side + 24.0);
                p.fill(gl, *area, 20.0, theme::ground());
                look::glass(p, gl, *area, 20.0, 1.0);
                for (i, button) in swatches.iter_mut().enumerate() {
                    let r = Rect::new(x + 12.0 + i as f32 * (side + 6.0), area.y + 12.0, side, side);
                    button.focused = i == *cursor;
                    button.draw_icon(p, gl, r, Icon::Oval, "");
                    button.focused = false;
                    swatch(p, gl, r, Ink::ALL[i]);
                    if Ink::ALL[i] == ink {
                        frame(p, gl, r.inset(2.0), 2.0, theme::mark());
                    }
                }
            }
            None => {}
        }
    }

    /// The marks over the pages `placed` on the window `all`, and what goes with the tools: the
    /// chosen note's handles, the crop being drawn, the words being typed.
    pub fn paint(&mut self, p: &mut Painter, gl: &Gl, all: Rect, placed: &[Placed]) {
        self.paint_marks(p, gl, all, placed);
        self.paint_chosen(p, gl, placed);
        self.paint_crop(p, gl, placed);
        self.paint_typing(p, gl, placed);
    }

    fn paint_marks(&mut self, p: &mut Painter, gl: &Gl, all: Rect, placed: &[Placed]) {
        let live = self.live();
        let hidden = self.typing.as_ref().and_then(|t| t.index);
        let marks = self.markup.marks();
        let wanted = |page: usize| live.as_ref().is_some_and(|n| n.page == page) || marks.notes.iter().enumerate().any(|(i, n)| n.page == page && Some(i) != hidden);
        let shown: Vec<&Placed> = placed.iter().filter(|pl| wanted(pl.page)).collect();
        if shown.is_empty() {
            return;
        }
        let s = p.scale();
        let (w, h) = ((all.w * s).round().max(1.0) as u32, (all.h * s).round().max(1.0) as u32);
        // Painted again only when what is painted, or where, changed.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (w, h, self.markup.generation, hidden, s.to_bits()).hash(&mut hasher);
        for pl in &shown {
            (pl.page, [pl.rect.x, pl.rect.y, pl.rect.w, pl.rect.h, pl.region.x0, pl.region.y0, pl.region.x1, pl.region.y1].map(f32::to_bits)).hash(&mut hasher);
        }
        format!("{:?}", live.as_ref().map(|n| &n.shape)).hash(&mut hasher);
        let key = hasher.finish();
        let fits = self.overlay.texture.is_some_and(|t| t.width == w && t.height == h);
        if key != self.overlay.key || !fits {
            self.overlay.key = key;
            let pixels = &mut self.overlay.pixels;
            pixels.clear();
            pixels.resize((w * h * 4) as usize, 0);
            if let Some(mut pixmap) = resvg::tiny_skia::PixmapMut::from_bytes(pixels, w, h) {
                let mut fonts = ink::fonts();
                for pl in &shown {
                    let mut notes: Vec<&Note> = marks.notes.iter().enumerate().filter(|(i, n)| n.page == pl.page && Some(*i) != hidden).map(|(_, n)| n).collect();
                    if let Some(note) = live.as_ref().filter(|n| n.page == pl.page) {
                        notes.push(note);
                    }
                    let sc = pl.scale() * s;
                    let map = Map { scale: sc, dx: (pl.rect.x - all.x) * s - pl.region.x0 * sc, dy: (pl.rect.y - all.y) * s - pl.region.y0 * sc };
                    let clip = Area { x0: ((pl.rect.x - all.x) * s).max(0.0), y0: ((pl.rect.y - all.y) * s).max(0.0), x1: ((pl.rect.right() - all.x) * s).min(w as f32), y1: ((pl.rect.bottom() - all.y) * s).min(h as f32) };
                    if clip.w() > 0.0 && clip.h() > 0.0 {
                        ink::paint(&mut pixmap, &notes, map, Some(clip), &mut fonts);
                    }
                }
            }
            ink::unpremultiply(pixels);
            let texture = match self.overlay.texture {
                Some(t) if t.width == w && t.height == h => {
                    unsafe {
                        let g = gl.api();
                        g.bind_texture(glow::TEXTURE_2D, Some(t.raw()));
                        g.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                        g.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, w as i32, h as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(pixels)));
                    }
                    t
                }
                old => {
                    if let Some(old) = old {
                        p.forget(gl, old);
                    }
                    match p.texture(gl, pixels, w, h) {
                        Ok(t) => t,
                        Err(e) => {
                            log::warn!("the marks would not go on the GPU: {e}");
                            self.overlay.texture = None;
                            return;
                        }
                    }
                }
            };
            self.overlay.texture = Some(texture);
        }
        if let Some(texture) = self.overlay.texture {
            p.image(gl, texture, all, [0.0, 0.0, 1.0, 1.0], 0.0, [1.0, 1.0, 1.0, 1.0], false);
        }
    }

    fn paint_chosen(&self, p: &mut Painter, gl: &Gl, placed: &[Placed]) {
        let Some(index) = self.markup.chosen else { return };
        if self.typing.as_ref().is_some_and(|t| t.index == Some(index)) {
            return;
        }
        let Some(note) = self.markup.marks().notes.get(index) else { return };
        let Some(pl) = placed.iter().find(|pl| pl.page == note.page) else { return };
        if !matches!(note.shape, Shape::Arrow { .. }) {
            let pad = if matches!(note.shape, Shape::Text { .. }) { 6.0 } else { note.size * 0.5 * pl.scale() + 6.0 };
            frame(p, gl, pl.area_on_window(&note.shape.bounds()).inset(-pad), 1.5, theme::mark());
        }
        for (_, at) in note.shape.handles() {
            let (x, y) = pl.to_window(at);
            knob(p, gl, x, y);
        }
    }

    fn paint_crop(&self, p: &mut Painter, gl: &Gl, placed: &[Placed]) {
        let Some((page, area)) = self.cropping else { return };
        let Some(pl) = placed.iter().find(|pl| pl.page == page) else { return };
        let keep = pl.area_on_window(&area);
        let all = pl.rect;
        // What goes, dimmed; what stays, framed, its corners to take hold of.
        for r in [
            Rect::new(all.x, all.y, all.w, keep.y - all.y),
            Rect::new(all.x, keep.bottom(), all.w, all.bottom() - keep.bottom()),
            Rect::new(all.x, keep.y, keep.x - all.x, keep.h),
            Rect::new(keep.right(), keep.y, all.right() - keep.right(), keep.h),
        ] {
            if r.w > 0.0 && r.h > 0.0 {
                look::scrim(p, gl, r);
                look::scrim(p, gl, r);
            }
        }
        frame(p, gl, keep, 2.0, theme::mark());
        for corner in area.corners() {
            let (x, y) = pl.to_window(corner);
            knob(p, gl, x, y);
        }
    }

    fn paint_typing(&mut self, p: &mut Painter, gl: &Gl, placed: &[Placed]) {
        let Some(typing) = self.typing.as_mut() else { return };
        let Some(pl) = placed.iter().find(|pl| pl.page == typing.page) else { return };
        let (x, y) = pl.to_window(typing.at);
        let em = (typing.size * pl.scale()).max(12.0);
        typing.field.em = Some(em);
        let wide = 8192.0 / p.scale();
        let text_w = p.measure(gl, &format!("{}|", typing.field.text), em, wide).0;
        let w = (text_w + theme::ROW_INSET * 2.0 + em).max(em * 6.0).max(240.0);
        let h = em * 1.7;
        // Placed so that the words in it are where they will be drawn.
        let rect = Rect::new(x - theme::ROW_INSET, y + (em * ink::LEADING - h) * 0.5, w, h);
        typing.field.draw(p, gl, rect);
    }

    /// The GPU's copy of the marks let go of.
    pub fn forget(&mut self, gl: &Gl) {
        if let Some(texture) = self.overlay.texture.take() {
            unsafe { gl.api().delete_texture(texture.raw()) };
        }
    }
}
