//! Markup mode: the tools to draw with over a picture or a PDF's pages, and what they do as the
//! wearer presses, drags and types. A picture's are Chromium's PDF viewer's -- its pen, its
//! highlighter, its eraser and its words, with their typeface, size, styles and alignment, in its
//! own colours and sizes (`markup`) -- and SpatiLoupe's own Select, Box, Circle, Arrow and Crop,
//! in the room's own look. A PDF's are Preview's set as they were (choose, crop, words, a box, a
//! circle, an arrow, seven colours and three weights), until PDFs are shown by SpatiMesh. Undo and
//! redo for both. The marks are `markup`'s; they are painted by `ink` on the CPU into a picture
//! laid over what is shown, the same painting a saved picture gets, so that the window shows them
//! as the file will have them.
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
use crate::markup::{
    corner_to, held_at, left_of, on_picture, Alignment, Area, Brush, Colour, Face, Handle, Ink, Markup, Marks, Note, Pick, Point, Shape, Size, Swatch, Typeface, Weight, ERASER, HIGHLIGHTER,
    HIGHLIGHTER_COLOURS, HIGHLIGHTER_SIZES, PEN, PEN_COLOURS, PEN_SIZES, TEXT_COLOURS, TEXT_SIZES, WORDS,
};

/// The toolbar's height: a row of buttons, and room round them.
pub const BAR: f32 = 60.0 + 24.0;

/// A tool: what a press and a drag on a page do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Pen,
    Highlighter,
    Eraser,
    Text,
    Box,
    Circle,
    Arrow,
    Crop,
}

impl Tool {
    /// A picture's tools, in the bar's order: choosing, Chromium's four, then SpatiLoupe's shapes
    /// and its crop.
    pub const PICTURE: [Tool; 9] = [Tool::Select, Tool::Pen, Tool::Highlighter, Tool::Eraser, Tool::Text, Tool::Box, Tool::Circle, Tool::Arrow, Tool::Crop];
    /// A PDF's, as they were.
    pub const PDF: [Tool; 6] = [Tool::Select, Tool::Crop, Tool::Text, Tool::Box, Tool::Circle, Tool::Arrow];

    fn icon(self) -> Icon {
        match self {
            Tool::Select => Icon::Pointer,
            Tool::Pen => Icon::Pen,
            Tool::Highlighter => Icon::Highlighter,
            Tool::Eraser => Icon::Eraser,
            Tool::Text => Icon::Text,
            Tool::Box => Icon::Square,
            Tool::Circle => Icon::Oval,
            Tool::Arrow => Icon::Arrow,
            Tool::Crop => Icon::Crop,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Pen => "Pen",
            Tool::Highlighter => "Highlighter",
            Tool::Eraser => "Eraser",
            Tool::Text => "Text",
            Tool::Box => "Box",
            Tool::Circle => "Circle",
            Tool::Arrow => "Arrow",
            Tool::Crop => "Crop",
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

/// A brush as it is set: its colour, and its size as Chromium's number.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Brushing {
    pub colour: Colour,
    pub size: f32,
}

/// Words as they are set: their colour, their size as Chromium's number, and their face.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lettering {
    pub colour: Colour,
    pub size: f32,
    pub face: Face,
}

/// What the colour and size buttons are for now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hand {
    /// Nothing with a colour or a size: Select with nothing chosen, the eraser, Crop.
    Nothing,
    /// A pen's line, or a box, an oval or an arrow on a picture: the pen's colours and sizes.
    Pen,
    Highlighter,
    /// Words on a picture.
    Words,
    /// A PDF's lines and words, as they are marked up for now: seven inks and three weights.
    PdfLines,
    PdfWords,
}

/// What a press and a drag are doing.
#[derive(Debug, Clone)]
enum Gesture {
    /// A new shape, from where the press was to where the pointer is.
    Drawing { page: usize, start: Point, now: Point },
    /// A line drawn by hand: the points it has come through, a pointer's move apart, and where
    /// the pointer is now.
    Stroking { page: usize, brush: Brush, points: Vec<Point>, tail: Point },
    /// The eraser drawn over a page: where it last was, and the marks before it began.
    Erasing { page: usize, last: Point, before: Marks },
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
    /// Where the words are held: the place across them their alignment holds them at, and their
    /// top.
    pub anchor: Point,
    pub index: Option<usize>,
    pub field: Field,
    /// Their size, in the page's units.
    pub size: f32,
    pub colour: Colour,
    pub face: Face,
    pub pick: Pick,
}

/// The colours, sizes or words' face to choose from, open under their button.
enum Popup {
    /// Colours `columns` across: Chromium's grids of five, or a PDF's seven in a row.
    Colours { cursor: usize, buttons: Vec<Button>, area: Rect, columns: usize },
    /// A brush's five sizes in a row, or words' sixteen four across.
    Sizes { cursor: usize, buttons: Vec<Button>, area: Rect, columns: usize },
    /// Words' typeface, bold and italic, and alignment: the buttons in that order.
    Face { cursor: usize, buttons: Vec<Button>, area: Rect },
    /// A PDF's three weights.
    Weights(Menu),
}

/// The face panel's buttons: three typefaces, bold, italic, three alignments.
const FACE_BUTTONS: usize = 8;

/// The marks laid over the window, as a picture of its own -- and what the highlighters
/// multiply the picture under it by, as another: painted again only when they, or where the
/// pages are, change.
#[derive(Default)]
struct Overlay {
    texture: Option<Texture>,
    tint: Option<Texture>,
    tinting: bool,
    key: u64,
    pixels: Vec<u8>,
    tinted: Vec<u8>,
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
    /// A picture's tools, Chromium's; or a PDF's, as they were.
    pub picture: bool,
    pub tool: Tool,
    /// A picture's pen -- which a box, an oval and an arrow are drawn with too -- its
    /// highlighter and its words, each as it was last set.
    pub pen: Brushing,
    pub highlighter: Brushing,
    pub lettering: Lettering,
    /// A PDF's colour, the weight of its lines, and the size of its words.
    pub ink: Ink,
    pub line: Weight,
    pub words: Weight,
    gesture: Option<Gesture>,
    /// A crop drawn and not yet made (Enter or the Crop button make it).
    pub cropping: Option<(usize, Area)>,
    pub typing: Option<Typing>,
    popup: Option<Popup>,
    tools: Vec<Button>,
    colour_button: Button,
    size_button: Button,
    face_button: Button,
    undo_button: Button,
    redo_button: Button,
    crop_button: Button,
    /// Where the toolbar is, for what the pointer is over, and its colour, size and face
    /// buttons, which their choices open under.
    pub bar: Rect,
    colour_at: Rect,
    size_at: Rect,
    face_at: Rect,
    /// Each page's part shown, in its units -- the whole of it or its crop -- by its number: what
    /// a size is a share of.
    pub regions: Vec<Area>,
    pointer: (f32, f32),
    shift: bool,
    last_press: Option<(Instant, (f32, f32))>,
    overlay: Overlay,
    /// Whether Crop is offered: not for a PDF whose owner does not allow its pages changed.
    pub may_crop: bool,
}

impl Desk {
    /// The tools, out, on a picture (`picture`) or a PDF shown turned `turns` quarters from how
    /// it was read.
    pub fn new(turns: u8, picture: bool) -> Desk {
        Desk {
            markup: Markup::new(turns),
            on: true,
            picture,
            tool: Tool::Select,
            pen: Brushing { colour: PEN.0, size: PEN.1 },
            highlighter: Brushing { colour: HIGHLIGHTER.0, size: HIGHLIGHTER.1 },
            lettering: Lettering { colour: WORDS.0, size: WORDS.1, face: Face::default() },
            ink: Ink::default(),
            line: Weight::default(),
            words: Weight::default(),
            gesture: None,
            cropping: None,
            typing: None,
            popup: None,
            tools: Tool::PICTURE.iter().map(|_| Button::default()).collect(),
            colour_button: Button::default(),
            size_button: Button::default(),
            face_button: Button::default(),
            undo_button: Button::default(),
            redo_button: Button::default(),
            crop_button: Button::default(),
            bar: Rect::default(),
            colour_at: Rect::default(),
            size_at: Rect::default(),
            face_at: Rect::default(),
            regions: Vec::new(),
            pointer: (0.0, 0.0),
            shift: false,
            last_press: None,
            overlay: Overlay::default(),
            may_crop: true,
        }
    }

    /// The tools on offer, in the bar's order.
    pub fn offered(&self) -> &'static [Tool] {
        if self.picture { &Tool::PICTURE } else { &Tool::PDF }
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

    fn chosen_note(&self) -> Option<&Note> {
        self.markup.chosen.and_then(|i| self.markup.marks().notes.get(i))
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
            Some(Gesture::Moving { before, .. } | Gesture::Reshaping { before, .. } | Gesture::Erasing { before, .. }) => {
                self.markup.adjust(|marks| *marks = before);
            }
            Some(Gesture::Cropping { .. }) => self.cropping = None,
            Some(Gesture::Stroking { .. } | Gesture::Drawing { .. }) => self.markup.generation += 1,
            _ => {}
        }
    }

    /// A tool chosen from the bar.
    pub fn choose(&mut self, tool: Tool) {
        if (tool == Tool::Crop && !self.may_crop) || !self.offered().contains(&tool) {
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

    /// What the colour and size buttons are for: what is being typed, the note chosen, or what
    /// the tool in hand draws.
    fn hand(&self) -> Hand {
        let of_shape = |shape: &Shape| match (self.picture, shape) {
            (true, Shape::Text { .. }) => Hand::Words,
            (true, Shape::Stroke { brush: Brush::Highlighter, .. }) => Hand::Highlighter,
            (true, _) => Hand::Pen,
            (false, Shape::Text { .. }) => Hand::PdfWords,
            (false, _) => Hand::PdfLines,
        };
        if self.typing.is_some() {
            return if self.picture { Hand::Words } else { Hand::PdfWords };
        }
        if let Some(note) = self.chosen_note() {
            return of_shape(&note.shape);
        }
        match (self.picture, self.tool) {
            (true, Tool::Pen | Tool::Box | Tool::Circle | Tool::Arrow) => Hand::Pen,
            (true, Tool::Highlighter) => Hand::Highlighter,
            (true, Tool::Text) => Hand::Words,
            (true, _) => Hand::Nothing,
            (false, Tool::Text) => Hand::PdfWords,
            (false, _) => Hand::PdfLines,
        }
    }

    /// The colours there are to choose from now: each, whether it is a highlighter's (shown on
    /// white, as it lies), and its name.
    fn colours(&self) -> Vec<(Colour, bool, &'static str)> {
        let of = |swatches: &[Swatch], blended: bool| swatches.iter().map(|s| (s.colour, blended, s.name)).collect();
        match self.hand() {
            Hand::Pen => of(&PEN_COLOURS, false),
            Hand::Highlighter => of(&HIGHLIGHTER_COLOURS, true),
            Hand::Words => of(&TEXT_COLOURS, false),
            Hand::PdfLines | Hand::PdfWords => Ink::ALL.iter().map(|i| (i.colour(), false, i.name())).collect(),
            Hand::Nothing => Vec::new(),
        }
    }

    /// The sizes there are to choose from now, Chromium's numbers: none for a PDF, whose are
    /// its three weights.
    fn sizes(&self) -> Vec<f32> {
        match self.hand() {
            Hand::Pen => PEN_SIZES.iter().map(|s| s.value).collect(),
            Hand::Highlighter => HIGHLIGHTER_SIZES.iter().map(|s| s.value).collect(),
            Hand::Words => TEXT_SIZES.to_vec(),
            _ => Vec::new(),
        }
    }

    /// The colour in hand: of the words being typed, the note chosen, or what is drawn next.
    fn colour_now(&self) -> Option<Colour> {
        if let Some(typing) = &self.typing {
            return Some(typing.colour);
        }
        if let Some(note) = self.chosen_note() {
            return Some(note.colour);
        }
        match self.hand() {
            Hand::Pen => Some(self.pen.colour),
            Hand::Highlighter => Some(self.highlighter.colour),
            Hand::Words => Some(self.lettering.colour),
            Hand::PdfLines | Hand::PdfWords => Some(self.ink.colour()),
            Hand::Nothing => None,
        }
    }

    /// The size in hand, as it was chosen.
    fn size_now(&self) -> Option<Pick> {
        if let Some(typing) = &self.typing {
            return Some(typing.pick);
        }
        if let Some(note) = self.chosen_note() {
            return Some(note.pick);
        }
        match self.hand() {
            Hand::Pen => Some(Pick::Chromium(self.pen.size)),
            Hand::Highlighter => Some(Pick::Chromium(self.highlighter.size)),
            Hand::Words => Some(Pick::Chromium(self.lettering.size)),
            Hand::PdfLines => Some(Pick::Weight(self.line)),
            Hand::PdfWords => Some(Pick::Weight(self.words)),
            Hand::Nothing => None,
        }
    }

    /// Words' face in hand.
    fn face_now(&self) -> Face {
        if let Some(typing) = &self.typing {
            return typing.face;
        }
        match self.chosen_note().map(|n| &n.shape) {
            Some(Shape::Text { face, .. }) => *face,
            _ => self.lettering.face,
        }
    }

    /// The colour chosen: for what is drawn next, and for the note chosen.
    pub fn set_colour(&mut self, colour: Colour) {
        match self.hand() {
            Hand::Pen => self.pen.colour = colour,
            Hand::Highlighter => self.highlighter.colour = colour,
            Hand::Words => self.lettering.colour = colour,
            Hand::PdfLines | Hand::PdfWords => self.ink = Ink::of(colour).unwrap_or(self.ink),
            Hand::Nothing => return,
        }
        if let Some(typing) = &mut self.typing {
            typing.colour = colour;
        }
        if let Some(i) = self.markup.chosen {
            self.markup.change(|marks| {
                if let Some(note) = marks.notes.get_mut(i) {
                    note.colour = colour;
                }
            });
        }
    }

    /// One of Chromium's sizes chosen, on a picture: for what is drawn next, and the note chosen.
    pub fn set_size(&mut self, value: f32) {
        match self.hand() {
            Hand::Pen => self.pen.size = value,
            Hand::Highlighter => self.highlighter.size = value,
            Hand::Words => self.lettering.size = value,
            _ => return,
        }
        if let Some(typing) = &mut self.typing {
            typing.pick = Pick::Chromium(value);
            typing.size = on_picture(value, self.regions.get(typing.page).map_or(1000.0, |r| r.w().min(r.h())));
            return;
        }
        let Some(i) = self.markup.chosen else { return };
        let Some(page) = self.chosen_note().map(|n| n.page) else { return };
        let size = on_picture(value, self.short(page));
        self.refit(i, |note| {
            note.size = size;
            note.pick = Pick::Chromium(value);
        });
    }

    /// The weight chosen, on a PDF: for lines or words, whichever is in hand, and for the note
    /// chosen.
    pub fn set_weight(&mut self, weight: Weight) {
        let words = self.hand() == Hand::PdfWords;
        if words {
            self.words = weight;
        } else {
            self.line = weight;
        }
        if let Some(typing) = &mut self.typing {
            typing.pick = Pick::Weight(weight);
            typing.size = weight.text(self.regions.get(typing.page).map_or(1000.0, |r| r.w().min(r.h())));
            return;
        }
        let Some(i) = self.markup.chosen else { return };
        let Some(page) = self.chosen_note().map(|n| n.page) else { return };
        let short = self.short(page);
        self.refit(i, |note| {
            note.pick = Pick::Weight(weight);
            note.size = if words { weight.text(short) } else { weight.line(short) };
        });
    }

    /// Words' face chosen: for what is written next, the words being typed, and those chosen.
    pub fn set_face(&mut self, face: Face) {
        if self.hand() != Hand::Words {
            return;
        }
        self.lettering.face = face;
        if let Some(typing) = &mut self.typing {
            typing.face = face;
            return;
        }
        if let Some(i) = self.markup.chosen {
            self.refit(i, |note| {
                if let Shape::Text { face: f, .. } = &mut note.shape {
                    *f = face;
                }
            });
        }
    }

    /// Note `index` changed by `change` as one step; words set again in their new size or face,
    /// still held where their alignment held them -- the edge or the middle it said, in the new
    /// alignment if that is what changed (as point text moves about its point).
    fn refit(&mut self, index: usize, change: impl FnOnce(&mut Note)) {
        let Some(mut note) = self.markup.marks().notes.get(index).cloned() else { return };
        let held = match &note.shape {
            Shape::Text { at, extent, face, .. } => Some(held_at(at.0, extent.0, face.alignment)),
            _ => None,
        };
        change(&mut note);
        if let (Some(held), Shape::Text { at, words, extent, face }) = (held, &mut note.shape) {
            *extent = ink::measure(&mut ink::fonts(), words, note.size, face);
            at.0 = left_of(held, extent.0, face.alignment);
        }
        self.markup.change(|marks| {
            if let Some(n) = marks.notes.get_mut(index) {
                *n = note;
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
        let Shape::Text { at, words, extent, face } = &note.shape else { return };
        let mut field = Field::default();
        field.set(words.clone());
        field.focused = true;
        self.markup.chosen = Some(index);
        let anchor = (held_at(at.0, extent.0, face.alignment), at.1);
        self.typing = Some(Typing { page: note.page, anchor, index: Some(index), field, size: note.size, colour: note.colour, face: *face, pick: note.pick });
        self.markup.generation += 1;
    }

    /// The words typed, kept: a note made, or changed, or taken away if they are gone; then the
    /// note is chosen, as Preview chooses what was just drawn.
    pub fn finish_typing(&mut self) {
        let Some(typing) = self.typing.take() else { return };
        let words = typing.field.text.trim().to_string();
        let extent = ink::measure(&mut ink::fonts(), &words, typing.size, &typing.face);
        let at = (left_of(typing.anchor.0, extent.0, typing.face.alignment), typing.anchor.1);
        let note = Note { page: typing.page, shape: Shape::Text { at, words: words.clone(), extent, face: typing.face }, colour: typing.colour, size: typing.size, pick: typing.pick };
        match (typing.index, words.is_empty()) {
            (Some(i), true) => {
                self.markup.chosen = Some(i);
                self.markup.delete_chosen();
            }
            (Some(i), false) => {
                self.markup.change(|marks| {
                    if let Some(n) = marks.notes.get_mut(i) {
                        *n = note;
                    }
                });
                self.markup.chosen = Some(i);
            }
            (None, true) => {}
            (None, false) => {
                self.markup.add(note);
            }
        }
        self.tool = Tool::Select;
        self.markup.generation += 1;
    }

    /// The shape being drawn, as it is now.
    fn live(&self) -> Option<Note> {
        match &self.gesture {
            Some(Gesture::Drawing { page, start, now }) => {
                let shape = match self.tool {
                    Tool::Box => Shape::Box(corner_to(*start, *now, self.shift)),
                    Tool::Circle => Shape::Oval(corner_to(*start, *now, self.shift)),
                    Tool::Arrow => Shape::Arrow { from: *start, to: if self.shift { snapped(*start, *now) } else { *now } },
                    _ => return None,
                };
                Some(if self.picture {
                    Note { page: *page, shape, colour: self.pen.colour, size: on_picture(self.pen.size, self.short(*page)), pick: Pick::Chromium(self.pen.size) }
                } else {
                    Note { page: *page, shape, colour: self.ink.colour(), size: self.line.line(self.short(*page)), pick: Pick::Weight(self.line) }
                })
            }
            Some(Gesture::Stroking { page, brush, points, tail }) => {
                let mut points = points.clone();
                if points.last() != Some(tail) {
                    points.push(*tail);
                }
                let brushing = if *brush == Brush::Pen { self.pen } else { self.highlighter };
                Some(Note { page: *page, shape: Shape::Stroke { brush: *brush, points }, colour: brushing.colour, size: on_picture(brushing.size, self.short(*page)), pick: Pick::Chromium(brushing.size) })
            }
            _ => None,
        }
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

    /// How far the eraser reaches either way on `place`'s page: Chromium's 3 there, but never less
    /// than 8 of the window's pixels, so that it can be aimed by a hand's ray.
    fn eraser_reach(&self, place: &Placed) -> f32 {
        on_picture(ERASER, self.short(place.page)).max(8.0 / place.scale())
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
        // Arrows over a grid: along it, and up and down its rows (along, for a single row).
        let step = |cursor: usize, key: Key, columns: usize, count: usize| -> usize {
            let columns = columns.max(1);
            let rows = count.div_ceil(columns);
            match key {
                Key::Left => cursor.saturating_sub(1),
                Key::Right => (cursor + 1).min(count - 1),
                Key::Up if rows == 1 => cursor.saturating_sub(1),
                Key::Down if rows == 1 => (cursor + 1).min(count - 1),
                Key::Up => cursor.checked_sub(columns).unwrap_or(cursor),
                Key::Down => (cursor + columns).min(count - 1),
                _ => cursor,
            }
        };
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
            Popup::Colours { cursor, buttons, area, columns } | Popup::Sizes { cursor, buttons, area, columns } => {
                let mut chose = None;
                for (i, button) in buttons.iter_mut().enumerate() {
                    if button.input(input) {
                        chose = Some(i);
                    }
                }
                let count = buttons.len();
                let mut close = false;
                match input {
                    Input::Key { key: Key::Escape, .. } => close = true,
                    Input::Key { key: key @ (Key::Left | Key::Right | Key::Up | Key::Down), .. } => *cursor = step(*cursor, *key, *columns, count),
                    Input::Key { key: Key::Enter | Key::Space, .. } => chose = Some(*cursor),
                    Input::Press { pressed: true, .. } if !area.contains(pointer.0, pointer.1) => close = true,
                    _ => {}
                }
                let sizes = matches!(self.popup, Some(Popup::Sizes { .. }));
                if close {
                    self.popup = None;
                } else if let Some(i) = chose {
                    self.popup = None;
                    if sizes {
                        if let Some(value) = self.sizes().get(i).copied() {
                            self.set_size(value);
                        }
                    } else if let Some((colour, _, _)) = self.colours().get(i).copied() {
                        self.set_colour(colour);
                    }
                }
                Some(Said::Taken)
            }
            Popup::Face { cursor, buttons, area } => {
                let mut chose = None;
                for (i, button) in buttons.iter_mut().enumerate() {
                    if button.input(input) {
                        chose = Some(i);
                    }
                }
                match input {
                    Input::Key { key: Key::Escape, .. } => {
                        self.popup = None;
                        return Some(Said::Taken);
                    }
                    Input::Key { key: Key::Left, .. } => *cursor = cursor.saturating_sub(1),
                    Input::Key { key: Key::Right, .. } => *cursor = (*cursor + 1).min(FACE_BUTTONS - 1),
                    // The typefaces above, the styles and alignments below.
                    Input::Key { key: Key::Up, .. } if *cursor >= 3 => *cursor = (*cursor - 3).min(2),
                    Input::Key { key: Key::Down, .. } if *cursor < 3 => *cursor += 3,
                    Input::Key { key: Key::Enter | Key::Space, .. } => chose = Some(*cursor),
                    Input::Press { pressed: true, .. } if !area.contains(pointer.0, pointer.1) => {
                        self.popup = None;
                        return Some(Said::Taken);
                    }
                    _ => {}
                }
                // Each choice is made at once, and the panel stays for the next.
                if let Some(i) = chose {
                    let mut face = self.face_now();
                    match i {
                        0..=2 => face.typeface = Typeface::ALL[i],
                        3 => face.bold = !face.bold,
                        4 => face.italic = !face.italic,
                        _ => face.alignment = Alignment::ALL[(i - 5).min(2)],
                    }
                    self.set_face(face);
                }
                Some(Said::Taken)
            }
        }
    }

    fn bar_input(&mut self, input: &Input) -> Option<Said> {
        let offered = self.offered();
        let mut tool = None;
        for (i, button) in self.tools.iter_mut().enumerate().take(offered.len()) {
            if button.input(input) {
                tool = Some(offered[i]);
            }
        }
        let colours = self.colour_button.input(input);
        let sizes = self.size_button.input(input);
        let faces = self.face_button.input(input);
        let undo = self.undo_button.input(input) && self.markup.can_undo();
        let redo = self.redo_button.input(input) && self.markup.can_redo();
        let crop = self.crop_button.input(input) && self.cropping.is_some();
        if let Some(tool) = tool {
            self.choose(tool);
        } else if colours {
            let list = self.colours();
            if !list.is_empty() {
                let now = self.colour_now();
                let cursor = list.iter().position(|c| Some(c.0) == now).unwrap_or(0);
                let columns = if self.picture { 5 } else { list.len() };
                self.popup = Some(Popup::Colours { cursor, buttons: list.iter().map(|_| Button::default()).collect(), area: Rect::default(), columns });
            }
        } else if sizes {
            if self.picture {
                let list = self.sizes();
                if !list.is_empty() {
                    let now = self.size_now();
                    let cursor = list.iter().position(|v| Some(Pick::Chromium(*v)) == now).unwrap_or(0);
                    let columns = if list.len() > 5 { 4 } else { list.len() };
                    self.popup = Some(Popup::Sizes { cursor, buttons: list.iter().map(|_| Button::default()).collect(), area: Rect::default(), columns });
                }
            } else {
                let mut menu = Menu::new((self.size_at.x, self.bar.bottom() + 8.0));
                let now = match self.size_now() {
                    Some(Pick::Weight(w)) => w,
                    _ => Weight::Medium,
                };
                menu.list.cursor = Weight::ALL.iter().position(|w| *w == now).unwrap_or(1);
                self.popup = Some(Popup::Weights(menu));
            }
        } else if faces {
            if self.hand() == Hand::Words {
                let cursor = Typeface::ALL.iter().position(|t| *t == self.face_now().typeface).unwrap_or(0);
                self.popup = Some(Popup::Face { cursor, buttons: (0..FACE_BUTTONS).map(|_| Button::default()).collect(), area: Rect::default() });
            }
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
            Tool::Pen | Tool::Highlighter => {
                self.markup.chosen = None;
                let brush = if self.tool == Tool::Pen { Brush::Pen } else { Brush::Highlighter };
                self.gesture = Some(Gesture::Stroking { page: place.page, brush, points: vec![at], tail: at });
                self.markup.generation += 1;
                Said::Taken
            }
            Tool::Eraser => {
                let before = self.markup.begin();
                let reach = self.eraser_reach(&place);
                self.markup.adjust(|marks| {
                    marks.erase(place.page, at, reach);
                });
                self.gesture = Some(Gesture::Erasing { page: place.page, last: at, before });
                Said::Taken
            }
            Tool::Text => {
                match self.markup.hit(place.page, at, slack).filter(|&i| self.is_words(i)) {
                    Some(index) => self.edit(index),
                    None => {
                        let mut field = Field::default();
                        field.focused = true;
                        let short = self.short(place.page);
                        self.typing = Some(if self.picture {
                            let l = self.lettering;
                            Typing { page: place.page, anchor: at, index: None, field, size: on_picture(l.size, short), colour: l.colour, face: l.face, pick: Pick::Chromium(l.size) }
                        } else {
                            Typing { page: place.page, anchor: at, index: None, field, size: self.words.text(short), colour: self.ink.colour(), face: Face::default(), pick: Pick::Weight(self.words) }
                        });
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
            Some(Gesture::Drawing { page, .. } | Gesture::Cropping { page, .. } | Gesture::Stroking { page, .. } | Gesture::Erasing { page, .. }) => *page,
            Some(Gesture::Moving { index, .. } | Gesture::Reshaping { index, .. }) => self.markup.marks().notes.get(*index).map_or(0, |n| n.page),
            Some(Gesture::CropCorner { .. } | Gesture::CropMoving { .. }) => self.cropping.map_or(0, |(p, _)| p),
        };
        let Some(place) = placed.iter().find(|p| p.page == page).copied() else { return Said::Taken };
        let Some((at, region)) = self.on_page(placed, page) else { return Said::Taken };
        // A line drawn by hand, or the eraser, goes where the pointer goes, off the page too:
        // what is off it is not painted, and nothing there is erased.
        let free = place.to_page(self.pointer);
        let even = self.shift;
        let reach = self.eraser_reach(&place);
        match &mut self.gesture {
            Some(Gesture::Drawing { now, .. }) => {
                *now = at;
                self.markup.generation += 1;
            }
            Some(Gesture::Stroking { points, tail, .. }) => {
                *tail = free;
                // A point kept every two of the window's pixels: a hand's line, not its every
                // tremble.
                let last = *points.last().unwrap_or(&free);
                if (free.0 - last.0).hypot(free.1 - last.1) * place.scale() >= 2.0 {
                    points.push(free);
                }
                self.markup.generation += 1;
            }
            Some(Gesture::Erasing { page, last, .. }) => {
                // From where it was to where it is, a step at most its reach at a time: a quick
                // sweep misses nothing between.
                let (from, to, page) = (*last, free, *page);
                *last = free;
                let steps = ((to.0 - from.0).hypot(to.1 - from.1) / reach).ceil().clamp(1.0, 400.0) as usize;
                self.markup.adjust(|marks| {
                    for k in 1..=steps {
                        let t = k as f32 / steps as f32;
                        marks.erase(page, (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t), reach);
                    }
                });
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
            Some(stroking @ Gesture::Stroking { .. }) => {
                self.gesture = Some(stroking);
                let live = self.live();
                self.gesture = None;
                // Kept as drawn, the pen still in hand for the next: a line is not chosen.
                if let Some(note) = live {
                    self.markup.change(|marks| marks.notes.push(note));
                }
                Said::Taken
            }
            Some(Gesture::Erasing { before, .. } | Gesture::Moving { before, .. } | Gesture::Reshaping { before, .. }) => {
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
            (Tool::Pen, None) => format!("[Drag] Draw  {undo}  [Esc] Done"),
            (Tool::Highlighter, None) => format!("[Drag] Highlight  {undo}  [Esc] Done"),
            (Tool::Eraser, None) => format!("[Drag] Erase what it touches  {undo}  [Esc] Done"),
            (Tool::Crop, None) => "[Drag] Choose what to keep  [Esc] Done".into(),
            (Tool::Text, None) => "[Press] Write there  [Esc] Done".into(),
            (Tool::Box, None) => "[Drag] Draw  [Shift] Square  [Esc] Done".into(),
            (Tool::Circle, None) => "[Drag] Draw  [Shift] Round  [Esc] Done".into(),
            (Tool::Arrow, None) => "[Drag] Draw  [Shift] Straight  [Esc] Done".into(),
        }
    }

    /// The name of the colour in hand, if it has one.
    fn colour_name(&self) -> Option<&'static str> {
        let now = self.colour_now()?;
        self.colours().into_iter().find(|c| c.0 == now).map(|c| c.2)
    }

    /// What the room's speech is told of the tools: the tool, and what it draws with.
    pub fn words_for_speech(&self) -> String {
        let tool = self.tool.name().to_lowercase();
        let colour = self.colour_name().map(str::to_lowercase).unwrap_or_default();
        let what = match (self.hand(), self.size_now()) {
            (Hand::Nothing, _) => String::new(),
            (Hand::Pen, Some(Pick::Chromium(v))) => format!(", in {colour}, {}", size_name(&PEN_SIZES, v)),
            (Hand::Highlighter, Some(Pick::Chromium(v))) => format!(", in {colour}, {}", size_name(&HIGHLIGHTER_SIZES, v)),
            (Hand::Words, Some(Pick::Chromium(v))) => {
                let face = self.face_now();
                let mut said = format!(", in {colour}, {} {}", number(v), face.typeface.name().to_lowercase());
                for (on, style) in [(face.bold, "bold"), (face.italic, "italic")] {
                    if on {
                        said.push_str(", ");
                        said.push_str(style);
                    }
                }
                if face.alignment != Alignment::Left {
                    said.push_str(&format!(", {} aligned", face.alignment.name().to_lowercase()));
                }
                said
            }
            _ => format!(", in {colour}"),
        };
        format!("Markup is on, with the {tool} tool{what}.")
    }
}

/// A size's name among `sizes`, or its number.
fn size_name(sizes: &[Size], value: f32) -> String {
    sizes.iter().find(|s| s.value == value).map_or_else(|| number(value), |s| s.name.to_lowercase())
}

/// A size as it is written: "12", "2.5".
fn number(value: f32) -> String {
    if value.fract() == 0.0 { format!("{value:.0}") } else { format!("{value}") }
}

/// The point from `from` towards `to`, along the nearest eighth of a turn: a line kept level,
/// upright or at 45 degrees.
fn snapped(from: Point, to: Point) -> Point {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = dx.hypot(dy);
    let angle = (dy.atan2(dx) / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
    (from.0 + length * angle.cos(), from.1 + length * angle.sin())
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

/// Where the keys are among a popup's buttons: the room's blue spilling round the button in `r`,
/// as round a tile with the keys, drawn before it.
fn keys_here(p: &mut Painter, gl: &Gl, r: Rect, radius: f32) {
    p.glow(gl, r, radius, theme::glow(0.45), 16.0);
}

/// A colour to choose, drawn in the middle of its button: a disc of it on a faint ring, so that
/// white and black show on either look; a highlighter's as it lies on white. `chosen` rings it in
/// the accent.
fn swatch(p: &mut Painter, gl: &Gl, r: Rect, colour: Colour, blended: bool, chosen: bool) {
    let side = r.w.min(r.h) * 0.52;
    let disc = Rect::new(r.x + (r.w - side) * 0.5, r.y + (r.h - side) * 0.5, side, side);
    if chosen {
        p.fill(gl, disc.inset(-6.0), side * 0.5 + 6.0, theme::mark());
        p.fill(gl, disc.inset(-3.5), side * 0.5 + 3.5, theme::ground());
    }
    p.fill(gl, disc.inset(-2.0), side * 0.5 + 2.0, theme::wash(0.35));
    p.fill(gl, disc, side * 0.5, if blended { colour.on_white().unit() } else { colour.unit() });
}

/// A brush's size, drawn in the middle of its button as Chromium shows one: a slanting line that
/// thick, round at its ends for a pen, square for a highlighter.
fn size_mark(p: &mut Painter, gl: &Gl, r: Rect, thick: f32, square: bool, colour: [f32; 4]) {
    let c = (r.x + r.w * 0.5, r.y + r.h * 0.5);
    let half = r.w.min(r.h) * 0.2;
    let (a, b) = ((c.0 - half, c.1 + half), (c.0 + half, c.1 - half));
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx.hypot(dy).max(1e-3);
    let (nx, ny) = (-dy / length * thick * 0.5, dx / length * thick * 0.5);
    // Its own top left first, clockwise: along it, then across.
    let corners = [(a.0 + nx, a.1 + ny), (b.0 + nx, b.1 + ny), (b.0 - nx, b.1 - ny), (a.0 - nx, a.1 - ny)];
    p.fill_quad(gl, corners, if square { thick.min(2.0) * 0.5 } else { thick * 0.5 }, colour);
}

/// How thick a brush's size is drawn on its button: Chromium's sizes, as the pen's and the
/// highlighter's icons draw them, side by side within a button.
fn size_thickness(value: f32, highlighter: bool) -> f32 {
    if highlighter { 2.0 + value * 0.85 } else { 1.5 + value * 1.6 }
}

impl Desk {
    /// How wide the toolbar would like to be.
    pub fn bar_width(&self, p: &mut Painter, gl: &Gl) -> f32 {
        let crop = if self.cropping.is_some() { Button::icon_width(p, gl, "Crop") + 20.0 } else { 0.0 };
        let (square, gaps) = self.bar_room();
        square * Button::HEIGHT + gaps + crop + 24.0
    }

    /// The toolbar's square buttons and the room between them: the tools, then the colour and
    /// size (and on a picture the face), then undo and redo.
    fn bar_room(&self) -> (f32, f32) {
        let tools = self.offered().len() as f32;
        let options = if self.picture { 3.0 } else { 2.0 };
        (tools + options + 2.0, (tools - 1.0) * 8.0 + 20.0 + (options - 1.0) * 8.0 + 20.0 + 8.0)
    }

    /// The toolbar, from `rect`'s left: the tools, the colour and the size (and words' face), undo
    /// and redo -- each drawn only where it can act, its place kept so that nothing moves -- and,
    /// while a crop is drawn, the button that makes it. Its glass reaches as far as what is drawn
    /// on it: `rect` is the most it takes.
    pub fn draw_bar(&mut self, p: &mut Painter, gl: &Gl, rect: Rect) {
        let inner = rect.inset(12.0);
        let crop_w = if self.cropping.is_some() { Button::icon_width(p, gl, "Crop") } else { 0.0 };
        let (square, gaps) = self.bar_room();
        let side = ((inner.w - gaps - if crop_w > 0.0 { crop_w + 20.0 } else { 0.0 }) / square).clamp(44.0, Button::HEIGHT);
        let y = inner.y + (inner.h - side) * 0.5;
        let mut x = inner.x;
        let mut next = |gap: f32| {
            let r = Rect::new(x, y, side, side);
            x += side + gap;
            r
        };
        let offered = self.offered();
        let tool_rects: Vec<Rect> = (0..offered.len()).map(|i| next(if i + 1 == offered.len() { 20.0 } else { 8.0 })).collect();
        let hand = self.hand();
        let colour_rect = next(8.0);
        let size_rect = next(if self.picture { 8.0 } else { 20.0 });
        let face_rect = if self.picture { next(20.0) } else { Rect::default() };
        let undo_rect = next(8.0);
        let redo_rect = next(20.0);
        let crop_rect = Rect::new(inner.right() - crop_w, y, crop_w, side);
        // As far as the last thing drawn on it.
        let colour_now = self.colour_now();
        let size_now = self.size_now().filter(|_| hand != Hand::Nothing);
        let drawn = [
            (true, tool_rects.last().copied().unwrap_or_default()),
            (colour_now.is_some(), colour_rect),
            (size_now.is_some(), size_rect),
            (self.picture && hand == Hand::Words, face_rect),
            (self.markup.can_undo(), undo_rect),
            (self.markup.can_redo(), redo_rect),
            (self.cropping.is_some(), crop_rect),
        ];
        let end = drawn.iter().filter(|(shown, _)| *shown).map(|(_, r)| r.right()).fold(inner.x, f32::max);
        self.bar = Rect { w: (end + 12.0 - rect.x).min(rect.w), ..rect };
        look::glass(p, gl, self.bar, 22.0, 1.0);
        for (i, tool) in offered.iter().enumerate() {
            let r = tool_rects[i];
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
        // The colour: a disc of it.
        let r = colour_rect;
        self.colour_at = r;
        match colour_now {
            Some(colour) => {
                self.colour_button.draw_bare(p, gl, r);
                swatch(p, gl, r, colour, hand == Hand::Highlighter, false);
            }
            None => self.colour_button = Button::default(),
        }
        // The size: a line that thick, the words' number, or a PDF's weight.
        let r = size_rect;
        self.size_at = r;
        match (hand, size_now) {
            (Hand::Nothing, _) | (_, None) => self.size_button = Button::default(),
            (Hand::PdfLines | Hand::PdfWords, Some(pick)) => {
                let weight = match pick {
                    Pick::Weight(w) => w,
                    Pick::Chromium(_) => Weight::Medium,
                };
                let icon = if hand == Hand::PdfWords { Icon::Text } else { [Icon::LineThin, Icon::LineMedium, Icon::LineThick][Weight::ALL.iter().position(|w| *w == weight).unwrap_or(1)] };
                self.size_button.draw_icon(p, gl, r, icon, "");
            }
            (Hand::Words, Some(pick)) => {
                let ink = self.size_button.draw_bare(p, gl, r);
                let value = match pick {
                    Pick::Chromium(v) => v,
                    Pick::Weight(_) => WORDS.1,
                };
                p.label(gl, &number(value), r, theme::DETAIL_EM, ink, 0.5);
            }
            (_, Some(pick)) => {
                let ink = self.size_button.draw_bare(p, gl, r);
                let value = match pick {
                    Pick::Chromium(v) => v,
                    Pick::Weight(_) => PEN.1,
                };
                let highlighter = hand == Hand::Highlighter;
                size_mark(p, gl, r, size_thickness(value, highlighter), highlighter, ink);
            }
        }
        // Words' face.
        self.face_at = face_rect;
        if self.picture && hand == Hand::Words {
            self.face_button.draw_icon(p, gl, face_rect, Icon::Typeface, "");
        } else {
            self.face_button = Button::default();
        }
        // A button that cannot act is not drawn (the look and feel's rule 8), nor pressed.
        if self.markup.can_undo() {
            self.undo_button.draw_icon(p, gl, undo_rect, Icon::Undo, "");
        } else {
            self.undo_button = Button::default();
        }
        if self.markup.can_redo() {
            self.redo_button.draw_icon(p, gl, redo_rect, Icon::Redo, "");
        } else {
            self.redo_button = Button::default();
        }
        if self.cropping.is_some() {
            self.crop_button.focused = true;
            self.crop_button.draw_icon(p, gl, crop_rect, Icon::Crop, "Crop");
            self.crop_button.focused = false;
        } else {
            self.crop_button = Button::default();
        }
    }

    /// A pane under the button at `under`, `w` by `h`, kept in `all`: its rectangle, drawn.
    fn pane(p: &mut Painter, gl: &Gl, under: Rect, bar: Rect, all: Rect, w: f32, h: f32) -> Rect {
        let x = (under.x + under.w * 0.5 - w * 0.5).clamp(all.x + 12.0, (all.right() - w - 12.0).max(all.x + 12.0));
        let area = Rect::new(x, bar.bottom() + 8.0, w, h);
        p.glow(gl, area, 20.0, [0.0, 0.0, 0.0, 0.45], 24.0);
        p.fill(gl, area, 20.0, theme::ground());
        look::glass(p, gl, area, 20.0, 1.0);
        area
    }

    /// The colours, the sizes, the words' face or a PDF's weights, open over everything in `all`.
    pub fn draw_popup(&mut self, p: &mut Painter, gl: &Gl, all: Rect) {
        let hand = self.hand();
        let (colours, sizes, colour_now, size_now, face) = (self.colours(), self.sizes(), self.colour_now(), self.size_now(), self.face_now());
        let (colour_at, size_at, face_at, bar) = (self.colour_at, self.size_at, self.face_at, self.bar);
        let side = Button::HEIGHT;
        let gap = 6.0;
        match &mut self.popup {
            Some(Popup::Weights(menu)) => {
                let words = hand == Hand::PdfWords;
                let rows: Vec<Row> = Weight::ALL
                    .iter()
                    .zip([Icon::LineThin, Icon::LineMedium, Icon::LineThick])
                    .map(|(w, icon)| if words { Row::new(w.text_name()).icon(Icon::Text) } else { Row::new(w.line_name()).icon(icon) })
                    .collect();
                menu.draw(p, gl, &rows, all);
            }
            Some(Popup::Colours { cursor, buttons, area, columns }) => {
                let columns = (*columns).max(1);
                let rows = buttons.len().div_ceil(columns);
                let (w, h) = (columns as f32 * (side + gap) - gap + 24.0, rows as f32 * (side + gap) - gap + 24.0);
                *area = Desk::pane(p, gl, colour_at, bar, all, w, h);
                for (i, button) in buttons.iter_mut().enumerate() {
                    let r = Rect::new(area.x + 12.0 + (i % columns) as f32 * (side + gap), area.y + 12.0 + (i / columns) as f32 * (side + gap), side, side);
                    if i == *cursor {
                        keys_here(p, gl, r, side * 0.5);
                    }
                    button.draw_bare(p, gl, r);
                    if let Some((colour, blended, _)) = colours.get(i) {
                        swatch(p, gl, r, *colour, *blended, Some(*colour) == colour_now);
                    }
                }
            }
            Some(Popup::Sizes { cursor, buttons, area, columns }) => {
                let columns = (*columns).max(1);
                let rows = buttons.len().div_ceil(columns);
                let (w, h) = (columns as f32 * (side + gap) - gap + 24.0, rows as f32 * (side + gap) - gap + 24.0);
                *area = Desk::pane(p, gl, size_at, bar, all, w, h);
                for (i, button) in buttons.iter_mut().enumerate() {
                    let r = Rect::new(area.x + 12.0 + (i % columns) as f32 * (side + gap), area.y + 12.0 + (i / columns) as f32 * (side + gap), side, side);
                    // The size in hand lit; where the keys are, glowing.
                    let value = sizes.get(i).copied().unwrap_or(0.0);
                    if i == *cursor {
                        keys_here(p, gl, r, side * 0.5);
                    }
                    button.focused = Some(Pick::Chromium(value)) == size_now;
                    let ink = button.draw_bare(p, gl, r);
                    button.focused = false;
                    match hand {
                        Hand::Words => {
                            p.label(gl, &number(value), r, theme::DETAIL_EM, ink, 0.5);
                        }
                        _ => {
                            let highlighter = hand == Hand::Highlighter;
                            size_mark(p, gl, r, size_thickness(value, highlighter), highlighter, ink);
                        }
                    }
                }
            }
            Some(Popup::Face { cursor, buttons, area }) => {
                let label_h = 30.0;
                let names: Vec<&str> = Typeface::ALL.iter().map(|t| t.name()).collect();
                let widths: Vec<f32> = names.iter().map(|n| Button::width(p, gl, n)).collect();
                let font_w: f32 = widths.iter().sum::<f32>() + gap * 2.0;
                let below_w = 2.0 * side + gap + 36.0 + 3.0 * side + 2.0 * gap;
                let (w, h) = (font_w.max(below_w) + 40.0, 20.0 + label_h + side + 16.0 + label_h + side + 20.0);
                *area = Desk::pane(p, gl, face_at, bar, all, w, h);
                let mut inner = area.inset(20.0);
                let heading = |p: &mut Painter, gl: &Gl, text: &str, r: Rect| {
                    p.label(gl, &text.to_uppercase(), r, theme::FOOTER_EM * 0.82, theme::ink_hint(), 0.0);
                };
                heading(p, gl, "Font", inner.take_top(label_h));
                let row = inner.take_top(side);
                let mut x = row.x;
                for (i, name) in names.iter().enumerate() {
                    let r = Rect::new(x, row.y, widths[i], side);
                    x += widths[i] + gap;
                    if i == *cursor {
                        keys_here(p, gl, r, theme::ROW_RADIUS);
                    }
                    let button = &mut buttons[i];
                    button.focused = face.typeface == Typeface::ALL[i];
                    button.draw(p, gl, r, name);
                    button.focused = false;
                }
                inner.take_top(16.0);
                let labels = inner.take_top(label_h);
                let styles_x = labels.x;
                let align_x = labels.x + 2.0 * side + gap + 36.0;
                heading(p, gl, "Styles", Rect { w: align_x - styles_x, ..labels });
                heading(p, gl, "Text alignment", Rect { x: align_x, w: labels.right() - align_x, ..labels });
                let row = inner.take_top(side);
                let lit = [face.bold, face.italic, face.alignment == Alignment::Left, face.alignment == Alignment::Center, face.alignment == Alignment::Right];
                let icons = [Icon::Bold, Icon::Italic, Icon::AlignLeft, Icon::AlignCenter, Icon::AlignRight];
                for k in 0..5 {
                    let x = if k < 2 { styles_x + k as f32 * (side + gap) } else { align_x + (k - 2) as f32 * (side + gap) };
                    let r = Rect::new(x, row.y, side, side);
                    if 3 + k == *cursor {
                        keys_here(p, gl, r, side * 0.5);
                    }
                    let button = &mut buttons[3 + k];
                    button.focused = lit[k];
                    button.draw_icon(p, gl, r, icons[k], "");
                    button.focused = false;
                }
            }
            None => {}
        }
    }

    /// The marks over the pages `placed` on the window `all`, and what goes with the tools: the
    /// chosen note's handles, the crop being drawn, the words being typed, the brush's tip.
    pub fn paint(&mut self, p: &mut Painter, gl: &Gl, all: Rect, placed: &[Placed]) {
        self.paint_marks(p, gl, all, placed);
        self.paint_chosen(p, gl, placed);
        self.paint_crop(p, gl, placed);
        self.paint_typing(p, gl, placed);
        self.paint_tip(p, gl, placed);
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
        match live.as_ref().map(|n| &n.shape) {
            Some(Shape::Stroke { points, .. }) => (points.len(), points.last().map(|p| (p.0.to_bits(), p.1.to_bits()))).hash(&mut hasher),
            other => format!("{other:?}").hash(&mut hasher),
        }
        let key = hasher.finish();
        // The highlighters' tint: a picture of its own, laid first, multiplying.
        let tinting = live.as_ref().is_some_and(|n| n.brush() == Some(Brush::Highlighter)) || marks.notes.iter().any(|n| n.brush() == Some(Brush::Highlighter) && shown.iter().any(|pl| pl.page == n.page));
        let fits = self.overlay.texture.is_some_and(|t| t.width == w && t.height == h) && (!tinting || self.overlay.tint.is_some_and(|t| t.width == w && t.height == h));
        if key != self.overlay.key || !fits || tinting != self.overlay.tinting {
            self.overlay.key = key;
            self.overlay.tinting = tinting;
            let Overlay { pixels, tinted, .. } = &mut self.overlay;
            pixels.clear();
            pixels.resize((w * h * 4) as usize, 0);
            tinted.clear();
            if tinting {
                tinted.resize((w * h * 4) as usize, 255);
            }
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
                        let tint = if tinting { Some(&mut tinted[..]) } else { None };
                        ink::paint(&mut pixmap, tint, &notes, map, Some(clip), &mut fonts);
                    }
                }
            }
            ink::unpremultiply(pixels);
            let Some(texture) = upload(p, gl, self.overlay.texture.take(), &self.overlay.pixels, w, h) else { return };
            self.overlay.texture = Some(texture);
            if tinting {
                self.overlay.tint = upload(p, gl, self.overlay.tint.take(), &self.overlay.tinted, w, h);
            }
        }
        if let Some(tint) = self.overlay.tint.filter(|_| self.overlay.tinting) {
            p.image_multiplied(gl, tint, all, [0.0, 0.0, 1.0, 1.0]);
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
        let (x, y) = pl.to_window(typing.anchor);
        let em = (typing.size * pl.scale()).max(12.0);
        typing.field.em = Some(em);
        let share = typing.face.alignment.share();
        typing.field.align = share;
        // Typed in the face they will have.
        let face = spatiand_render::TextFace {
            family: match typing.face.typeface {
                Typeface::SansSerif => spatiand_render::FaceFamily::SansSerif,
                Typeface::Serif => spatiand_render::FaceFamily::Serif,
                Typeface::Monospace => spatiand_render::FaceFamily::Monospace,
            },
            bold: typing.face.bold,
            italic: typing.face.italic,
        };
        typing.field.face = face;
        let wide = 8192.0 / p.scale();
        let before = p.set_face(face);
        let text_w = p.measure(gl, &format!("{}|", typing.field.text), em, wide).0;
        p.set_face(before);
        let w = (text_w + theme::ROW_INSET * 2.0 + em).max(em * 6.0).max(240.0);
        let h = em * 1.7;
        // Placed so that the words in it are where they will be drawn: their edge or their
        // middle, as their alignment says, at the place pressed.
        let inner = w - theme::ROW_INSET * 2.0;
        let rect = Rect::new(x - theme::ROW_INSET - inner * share, y + (em * ink::LEADING - h) * 0.5, w, h);
        // On glass, to be read over whatever is under it: a white page as well as a dark picture.
        look::glass(p, gl, rect, theme::ROW_RADIUS, 1.0);
        typing.field.draw(p, gl, rect);
    }

    /// Where a pen, a highlighter or the eraser would touch, under the pointer on a page: the
    /// pen's dot, the highlighter's square, the eraser's reach -- what the room's pointer alone
    /// does not show.
    fn paint_tip(&self, p: &mut Painter, gl: &Gl, placed: &[Placed]) {
        if self.popup.is_some() || self.bar.contains(self.pointer.0, self.pointer.1) || !matches!(self.tool, Tool::Pen | Tool::Highlighter | Tool::Eraser) {
            return;
        }
        let Some(pl) = self.under(placed) else { return };
        let (x, y) = self.pointer;
        let short = self.short(pl.page);
        let around = |p: &mut Painter, r: Rect, radius: f32| {
            p.fill(gl, r.inset(-3.0), radius + 3.0, [0.0, 0.0, 0.0, 0.55]);
            p.fill(gl, r.inset(-1.5), radius + 1.5, [1.0, 1.0, 1.0, 0.9]);
        };
        match self.tool {
            Tool::Pen => {
                let d = (on_picture(self.pen.size, short) * pl.scale()).max(3.0);
                let r = Rect::new(x - d * 0.5, y - d * 0.5, d, d);
                around(p, r, d * 0.5);
                p.fill(gl, r, d * 0.5, self.pen.colour.unit());
            }
            Tool::Highlighter => {
                let d = (on_picture(self.highlighter.size, short) * pl.scale()).max(3.0);
                let r = Rect::new(x - d * 0.5, y - d * 0.5, d, d);
                around(p, r, 0.0);
                p.fill(gl, r, 0.0, self.highlighter.colour.on_white().unit());
            }
            _ => {
                let d = self.eraser_reach(pl) * 2.0 * pl.scale();
                let r = Rect::new(x - d * 0.5, y - d * 0.5, d, d);
                frame(p, gl, r.inset(-1.5), 1.5, [0.0, 0.0, 0.0, 0.55]);
                frame(p, gl, r, 1.5, [1.0, 1.0, 1.0, 0.95]);
            }
        }
    }

    /// The GPU's copies of the marks let go of.
    pub fn forget(&mut self, gl: &Gl) {
        for texture in [self.overlay.texture.take(), self.overlay.tint.take()].into_iter().flatten() {
            unsafe { gl.api().delete_texture(texture.raw()) };
        }
    }
}

/// `pixels` (alpha not multiplied in) on the GPU, `w` by `h`: into `old` if it is that size, or
/// a new texture, `old` let go of.
fn upload(p: &mut Painter, gl: &Gl, old: Option<Texture>, pixels: &[u8], w: u32, h: u32) -> Option<Texture> {
    match old {
        Some(t) if t.width == w && t.height == h => {
            unsafe {
                let g = gl.api();
                g.bind_texture(glow::TEXTURE_2D, Some(t.raw()));
                g.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                g.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, w as i32, h as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(pixels)));
            }
            Some(t)
        }
        old => {
            if let Some(old) = old {
                p.forget(gl, old);
            }
            match p.texture(gl, pixels, w, h) {
                Ok(t) => Some(t),
                Err(e) => {
                    log::warn!("the marks would not go on the GPU: {e}");
                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLACED: [Placed; 1] = [Placed { page: 0, rect: Rect::new(100.0, 200.0, 1190.0, 595.0), region: Area { x0: 0.0, y0: 0.0, x1: 2380.0, y1: 1190.0 } }];

    fn picture() -> Desk {
        let mut desk = Desk::new(0, true);
        desk.regions = vec![PLACED[0].region];
        desk
    }

    fn at(desk: &mut Desk, x: f32, y: f32) {
        desk.input(&Input::Pointer { x, y }, &PLACED);
    }

    fn press(desk: &mut Desk, pressed: bool) {
        desk.input(&Input::Press { pressed, modifiers: Modifiers::default() }, &PLACED);
    }

    fn drag(desk: &mut Desk, path: &[(f32, f32)]) {
        at(desk, path[0].0, path[0].1);
        press(desk, true);
        for p in &path[1..] {
            at(desk, p.0, p.1);
        }
        press(desk, false);
    }

    #[test]
    fn a_picture_has_chromium_s_tools_and_a_pdf_keeps_its_own() {
        let desk = picture();
        assert_eq!(desk.offered(), &Tool::PICTURE);
        assert_eq!(desk.offered().iter().map(|t| t.name()).collect::<Vec<_>>(), ["Select", "Pen", "Highlighter", "Eraser", "Text", "Box", "Circle", "Arrow", "Crop"]);
        let mut pdf = Desk::new(0, false);
        assert_eq!(pdf.offered(), &Tool::PDF);
        pdf.choose(Tool::Pen);
        assert_eq!(pdf.tool, Tool::Select, "no pen for a PDF, for now");
        // A PDF's colours are its seven, a picture's pen Chromium's twenty, its highlighter ten.
        assert_eq!(pdf.colours().len(), 7);
        let mut desk = picture();
        desk.choose(Tool::Pen);
        assert_eq!((desk.colours().len(), desk.sizes(), desk.colour_now()), (20, vec![1.0, 2.0, 3.0, 6.0, 8.0], Some(Colour::BLACK)));
        desk.choose(Tool::Highlighter);
        assert_eq!((desk.colours().len(), desk.size_now()), (10, Some(Pick::Chromium(8.0))));
        assert!(desk.colours().iter().all(|c| c.1), "a highlighter's colours are shown as they lie");
        desk.choose(Tool::Text);
        assert_eq!((desk.colours().len(), desk.sizes().len(), desk.size_now()), (20, 16, Some(Pick::Chromium(12.0))));
        desk.choose(Tool::Eraser);
        assert!(desk.colours().is_empty() && desk.colour_now().is_none(), "the eraser has no colour");
    }

    #[test]
    fn the_pen_draws_lines_in_chromium_s_values_and_stays_in_hand() {
        let mut desk = picture();
        desk.choose(Tool::Pen);
        desk.set_colour(PEN_COLOURS[13].colour);
        desk.set_size(6.0);
        drag(&mut desk, &[(300.0, 300.0), (310.0, 305.0), (340.0, 310.0), (400.0, 320.0)]);
        let note = desk.markup.marks().notes[0].clone();
        let Shape::Stroke { brush: Brush::Pen, points } = &note.shape else { panic!("a pen's line: {:?}", note.shape) };
        // The page is shown at half its size: (300, 300) on the window is (400, 200) on it.
        assert_eq!(points[0], (400.0, 200.0));
        assert_eq!(*points.last().unwrap(), (600.0, 240.0));
        assert_eq!(note.colour.hex(), "#4285f4");
        assert_eq!(note.pick, Pick::Chromium(6.0));
        // 6 of Chromium's on a picture 1190 short: 6 * 0.75 * 1190 / 595 = 9 of its pixels.
        assert!((note.size - 9.0).abs() < 1e-4, "{}", note.size);
        assert_eq!((desk.tool, desk.markup.chosen), (Tool::Pen, None), "the pen stays, and the line is not chosen");
        // A press that does not move: a dot.
        drag(&mut desk, &[(500.0, 500.0)]);
        assert!(matches!(&desk.markup.marks().notes[1].shape, Shape::Stroke { points, .. } if points.len() == 1));
        // Two steps, undone one at a time.
        desk.undo();
        assert_eq!(desk.markup.marks().notes.len(), 1);
        assert!(desk.words_for_speech().starts_with("Markup is on, with the pen tool, in blue 2, thick."), "{}", desk.words_for_speech());
    }

    #[test]
    fn the_eraser_takes_a_drag_s_strokes_away_in_one_step_but_not_words() {
        let mut desk = picture();
        desk.choose(Tool::Pen);
        drag(&mut desk, &[(300.0, 300.0), (500.0, 300.0)]);
        desk.choose(Tool::Highlighter);
        drag(&mut desk, &[(300.0, 400.0), (500.0, 400.0)]);
        desk.choose(Tool::Text);
        at(&mut desk, 300.0, 500.0);
        press(&mut desk, true);
        press(&mut desk, false);
        for c in "kept".chars() {
            desk.input(&Input::Key { key: Key::Char(c), text: Some(c.to_string()), modifiers: Modifiers::default(), letter: None }, &PLACED);
        }
        desk.input(&Input::Key { key: Key::Enter, text: None, modifiers: Modifiers::default(), letter: None }, &PLACED);
        assert_eq!(desk.markup.marks().notes.len(), 3);
        // One sweep down across all three, quickly: few moves, far apart.
        desk.choose(Tool::Eraser);
        drag(&mut desk, &[(400.0, 250.0), (400.0, 420.0), (310.0, 515.0)]);
        let left: Vec<bool> = desk.markup.marks().notes.iter().map(|n| matches!(n.shape, Shape::Text { .. })).collect();
        assert_eq!(left, [true], "the words stay");
        desk.undo();
        assert_eq!(desk.markup.marks().notes.len(), 3, "the sweep was one step");
    }

    #[test]
    fn words_take_chromium_s_face_and_keep_their_place_by_their_alignment() {
        let mut desk = picture();
        desk.choose(Tool::Text);
        let face = Face { typeface: Typeface::Serif, bold: true, italic: false, alignment: Alignment::Center };
        desk.set_face(face);
        desk.set_size(24.0);
        desk.set_colour(TEXT_COLOURS[18].colour);
        at(&mut desk, 600.0, 400.0);
        press(&mut desk, true);
        press(&mut desk, false);
        for c in "Look".chars() {
            desk.input(&Input::Key { key: Key::Char(c), text: Some(c.to_string()), modifiers: Modifiers::default(), letter: None }, &PLACED);
        }
        desk.finish_typing();
        let note = desk.markup.marks().notes[0].clone();
        let Shape::Text { at: top_left, extent, face: kept, words } = &note.shape else { panic!("words") };
        assert_eq!((words.as_str(), *kept, note.colour.hex(), note.pick), ("Look", face, "#12a4af".to_string(), Pick::Chromium(24.0)));
        // Centred on the place pressed, (1000, 400) on the page.
        assert!((top_left.0 + extent.0 * 0.5 - 1000.0).abs() < 0.01 && (top_left.1 - 400.0).abs() < 0.01, "{top_left:?} {extent:?}");
        // Chosen, larger: still centred there. Then from the right: its middle is where its right
        // edge is now -- the point it is held by stays.
        desk.set_size(48.0);
        let Shape::Text { at: a, extent: e, .. } = desk.markup.marks().notes[0].shape.clone() else { unreachable!() };
        assert!((a.0 + e.0 * 0.5 - 1000.0).abs() < 0.01 && e.0 > extent.0 * 1.9);
        desk.set_face(Face { alignment: Alignment::Right, ..face });
        let Shape::Text { at: r, extent: re, .. } = desk.markup.marks().notes[0].shape.clone() else { unreachable!() };
        assert!((r.0 + re.0 - 1000.0).abs() < 0.01, "{r:?} {re:?}");
        // Each a step of its own.
        desk.undo();
        desk.undo();
        let Shape::Text { extent: back, .. } = desk.markup.marks().notes[0].shape.clone() else { unreachable!() };
        assert_eq!(back, *extent);
        assert!(desk.words_for_speech().contains("in cyan 3, 24 serif, bold, center aligned"), "{}", desk.words_for_speech());
    }

    #[test]
    fn the_bar_s_popups_choose_with_the_keys() {
        let mut desk = picture();
        desk.choose(Tool::Pen);
        let key = |key: Key| Input::Key { key, text: None, modifiers: Modifiers::default(), letter: None };
        // The colours, five across: down a row and right one from black is Yellow 1.
        desk.popup = Some(Popup::Colours { cursor: 0, buttons: (0..20).map(|_| Button::default()).collect(), area: Rect::new(0.0, 0.0, 400.0, 300.0), columns: 5 });
        for k in [Key::Down, Key::Right, Key::Enter] {
            desk.input(&key(k), &PLACED);
        }
        assert_eq!(desk.pen.colour, PEN_COLOURS[6].colour);
        assert!(desk.popup.is_none());
        // The sizes, in a row: up and down go along it.
        desk.popup = Some(Popup::Sizes { cursor: 2, buttons: (0..5).map(|_| Button::default()).collect(), area: Rect::new(0.0, 0.0, 400.0, 100.0), columns: 5 });
        for k in [Key::Down, Key::Down, Key::Enter] {
            desk.input(&key(k), &PLACED);
        }
        assert_eq!(desk.pen.size, 8.0);
        // Words' face: Serif, then down to its row's italic and left to bold, toggled; the panel
        // kept open all the while; Escape closes it.
        desk.choose(Tool::Text);
        desk.popup = Some(Popup::Face { cursor: 0, buttons: (0..FACE_BUTTONS).map(|_| Button::default()).collect(), area: Rect::new(0.0, 0.0, 400.0, 300.0) });
        for k in [Key::Right, Key::Enter, Key::Down, Key::Left, Key::Space] {
            desk.input(&key(k), &PLACED);
        }
        assert_eq!(desk.lettering.face, Face { typeface: Typeface::Serif, bold: true, ..Face::default() });
        assert!(desk.popup.is_some());
        desk.input(&key(Key::Escape), &PLACED);
        assert!(desk.popup.is_none());
    }
}
