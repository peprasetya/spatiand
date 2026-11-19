//! Markup: what the wearer draws over a picture or a PDF's pages -- boxes, ovals, arrows and
//! words -- and what is cropped away, kept as shapes while the file is open, every change one
//! that can be undone. Nothing here draws or writes: `ink` paints these, `save` writes a
//! picture with them and `annotate` a PDF.
//!
//! Each shape is in its page's own units: a picture's own pixels, the picture as it is shown
//! (turned as it is turned); a PDF page's points from its top left, as PDFium shows the page. A
//! picture is page 0.

/// A point on a page, in its units, down from its top left.
pub type Point = (f32, f32);

/// A rectangle on a page, its corners in order: `x0 <= x1`, `y0 <= y1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Area {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Area {
        Area::between((x0, y0), (x1, y1))
    }

    /// The rectangle with these two corners, whichever they are.
    pub fn between(a: Point, b: Point) -> Area {
        Area { x0: a.0.min(b.0), y0: a.1.min(b.1), x1: a.0.max(b.0), y1: a.1.max(b.1) }
    }

    pub fn w(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn h(&self) -> f32 {
        self.y1 - self.y0
    }

    pub fn centre(&self) -> Point {
        ((self.x0 + self.x1) * 0.5, (self.y0 + self.y1) * 0.5)
    }

    pub fn contains(&self, p: Point) -> bool {
        p.0 >= self.x0 && p.0 <= self.x1 && p.1 >= self.y0 && p.1 <= self.y1
    }

    /// Larger by `by` all round (smaller, for less than nought).
    pub fn grown(&self, by: f32) -> Area {
        Area { x0: self.x0 - by, y0: self.y0 - by, x1: self.x1 + by, y1: self.y1 + by }
    }

    /// What two areas have in common: `None` if they do not meet.
    pub fn meet(&self, other: &Area) -> Option<Area> {
        let a = Area { x0: self.x0.max(other.x0), y0: self.y0.max(other.y0), x1: self.x1.min(other.x1), y1: self.y1.min(other.y1) };
        (a.x0 < a.x1 && a.y0 < a.y1).then_some(a)
    }

    /// The corners clockwise from the top left: the order of [`Handle::Corner`].
    pub fn corners(&self) -> [Point; 4] {
        [(self.x0, self.y0), (self.x1, self.y0), (self.x1, self.y1), (self.x0, self.y1)]
    }
}

/// A colour to draw with: Preview's few, red to begin with. A drawing's own colours, the same in
/// the dark and the light look, not the room's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Ink {
    #[default]
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Black,
    White,
}

impl Ink {
    pub const ALL: [Ink; 7] = [Ink::Red, Ink::Orange, Ink::Yellow, Ink::Green, Ink::Blue, Ink::Black, Ink::White];

    pub fn rgb(self) -> [u8; 3] {
        match self {
            Ink::Red => [235, 51, 36],
            Ink::Orange => [245, 140, 30],
            Ink::Yellow => [250, 210, 25],
            Ink::Green => [50, 180, 75],
            Ink::Blue => [35, 115, 235],
            Ink::Black => [20, 20, 22],
            Ink::White => [255, 255, 255],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Ink::Red => "Red",
            Ink::Orange => "Orange",
            Ink::Yellow => "Yellow",
            Ink::Green => "Green",
            Ink::Blue => "Blue",
            Ink::Black => "Black",
            Ink::White => "White",
        }
    }
}

/// How heavy a line is drawn, or how large words are set: three steps, each a share of the
/// page's shorter side, so that "thin" looks thin on a phone's photograph and on a page alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Weight {
    Light,
    #[default]
    Medium,
    Heavy,
}

impl Weight {
    pub const ALL: [Weight; 3] = [Weight::Light, Weight::Medium, Weight::Heavy];

    fn step(self) -> usize {
        match self {
            Weight::Light => 0,
            Weight::Medium => 1,
            Weight::Heavy => 2,
        }
    }

    /// A line's width on a page whose shorter side is `short`: an A4 page's lines are 1.5, 3 and
    /// 6 points, a 12-megapixel photograph's 7.5, 15 and 30 pixels.
    pub fn line(self, short: f32) -> f32 {
        (short * [0.0025, 0.005, 0.01][self.step()]).max([1.0, 2.0, 3.0][self.step()])
    }

    /// Words' size (their em) on such a page: an A4 page's 15, 24 and 39 points.
    pub fn text(self, short: f32) -> f32 {
        (short * [0.025, 0.04, 0.065][self.step()]).max([10.0, 14.0, 20.0][self.step()])
    }

    pub fn line_name(self) -> &'static str {
        ["Thin", "Medium", "Thick"][self.step()]
    }

    pub fn text_name(self) -> &'static str {
        ["Small", "Medium", "Large"][self.step()]
    }
}

/// What is drawn.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// A rectangle's outline.
    Box(Area),
    /// An ellipse's outline, inside its area.
    Oval(Area),
    /// A line with an arrowhead at `to`.
    Arrow { from: Point, to: Point },
    /// Words: their box's top left, and the box they take as the text engine sets them at the
    /// note's size ([`crate::ink::measure`]).
    Text { at: Point, words: String, extent: (f32, f32) },
}

/// One thing drawn on a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub page: usize,
    pub shape: Shape,
    pub ink: Ink,
    pub weight: Weight,
    /// A line's width, or the words' size, in the page's units.
    pub size: f32,
}

/// A place to take hold of a chosen note and change its shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    /// A box's or an oval's corner, clockwise from the top left.
    Corner(u8),
    /// An arrow's ends.
    From,
    To,
}

/// How near a point is to the segment from `a` to `b`.
pub fn distance_to_segment(p: Point, a: Point, b: Point) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length <= f32::EPSILON { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length).clamp(0.0, 1.0) };
    let (x, y) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - x).powi(2) + (p.1 - y).powi(2)).sqrt()
}

/// An arrow's head, for a line of `width` from `from` to `to`: where the line stops under it,
/// and its three corners (the tip first). Shared by every way an arrow is drawn, so that it is
/// the same on the window, in a saved picture and in a PDF.
pub fn arrow_head(from: Point, to: Point, width: f32) -> (Point, [Point; 3]) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = (dx * dx + dy * dy).sqrt().max(1e-3);
    let (ux, uy) = (dx / length, dy / length);
    let long = (width * 4.5 + 2.0).min(length * 0.6);
    let half = long * 0.45;
    let base = (to.0 - ux * long, to.1 - uy * long);
    let stop = (to.0 - ux * long * 0.8, to.1 - uy * long * 0.8);
    (stop, [to, (base.0 - uy * half, base.1 + ux * half), (base.0 + uy * half, base.1 - ux * half)])
}

impl Shape {
    /// The rectangle it takes, not counting the width of its line.
    pub fn bounds(&self) -> Area {
        match self {
            Shape::Box(a) | Shape::Oval(a) => *a,
            Shape::Arrow { from, to } => Area::between(*from, *to),
            Shape::Text { at, extent, .. } => Area::new(at.0, at.1, at.0 + extent.0, at.1 + extent.1),
        }
    }

    pub fn moved(&self, dx: f32, dy: f32) -> Shape {
        let m = |p: Point| (p.0 + dx, p.1 + dy);
        match self {
            Shape::Box(a) => Shape::Box(Area::between(m((a.x0, a.y0)), m((a.x1, a.y1)))),
            Shape::Oval(a) => Shape::Oval(Area::between(m((a.x0, a.y0)), m((a.x1, a.y1)))),
            Shape::Arrow { from, to } => Shape::Arrow { from: m(*from), to: m(*to) },
            Shape::Text { at, words, extent } => Shape::Text { at: m(*at), words: words.clone(), extent: *extent },
        }
    }

    /// The places it can be taken hold of to change it: a box's and an oval's corners, an
    /// arrow's ends. Words have none; they are moved, and their size is chosen.
    pub fn handles(&self) -> Vec<(Handle, Point)> {
        match self {
            Shape::Box(a) | Shape::Oval(a) => a.corners().iter().enumerate().map(|(i, p)| (Handle::Corner(i as u8), *p)).collect(),
            Shape::Arrow { from, to } => vec![(Handle::From, *from), (Handle::To, *to)],
            Shape::Text { .. } => Vec::new(),
        }
    }

    /// The shape with `handle` taken to `to`; `even` keeps a box square and an oval round, the
    /// corner across from the handle staying where it is.
    pub fn with_handle(&self, handle: Handle, to: Point, even: bool) -> Shape {
        match (self, handle) {
            (Shape::Box(a) | Shape::Oval(a), Handle::Corner(i)) => {
                let fixed = a.corners()[(i as usize + 2) % 4];
                let area = corner_to(fixed, to, even);
                if matches!(self, Shape::Box(_)) { Shape::Box(area) } else { Shape::Oval(area) }
            }
            (Shape::Arrow { to: end, .. }, Handle::From) => Shape::Arrow { from: to, to: *end },
            (Shape::Arrow { from, .. }, Handle::To) => Shape::Arrow { from: *from, to },
            _ => self.clone(),
        }
    }

    /// Whether it is so small that it is nothing: a press that was not a drag.
    pub fn is_speck(&self, least: f32) -> bool {
        match self {
            Shape::Box(a) | Shape::Oval(a) => a.w() < least && a.h() < least,
            Shape::Arrow { from, to } => (to.0 - from.0).hypot(to.1 - from.1) < least,
            Shape::Text { words, .. } => words.trim().is_empty(),
        }
    }
}

/// The rectangle from `fixed` to `to`, square if `even`: as wide as it is tall, the larger of
/// the two, towards `to`.
pub fn corner_to(fixed: Point, to: Point, even: bool) -> Area {
    if !even {
        return Area::between(fixed, to);
    }
    let (dx, dy) = (to.0 - fixed.0, to.1 - fixed.1);
    let side = dx.abs().max(dy.abs());
    Area::between(fixed, (fixed.0 + side * if dx < 0.0 { -1.0 } else { 1.0 }, fixed.1 + side * if dy < 0.0 { -1.0 } else { 1.0 }))
}

/// A point of a picture `size` across and down, as it is once the picture is turned a quarter,
/// clockwise or not.
pub fn turn_point(p: Point, size: (f32, f32), clockwise: bool) -> Point {
    if clockwise { (size.1 - p.1, p.0) } else { (p.1, size.0 - p.0) }
}

pub fn turn_area(a: &Area, size: (f32, f32), clockwise: bool) -> Area {
    Area::between(turn_point((a.x0, a.y0), size, clockwise), turn_point((a.x1, a.y1), size, clockwise))
}

impl Note {
    /// The note on a picture turned a quarter. Words stay upright, wherever they go: their middle
    /// goes where that part of the picture goes.
    pub fn turned(&self, size: (f32, f32), clockwise: bool) -> Note {
        let shape = match &self.shape {
            Shape::Box(a) => Shape::Box(turn_area(a, size, clockwise)),
            Shape::Oval(a) => Shape::Oval(turn_area(a, size, clockwise)),
            Shape::Arrow { from, to } => Shape::Arrow { from: turn_point(*from, size, clockwise), to: turn_point(*to, size, clockwise) },
            Shape::Text { at, words, extent } => {
                let middle = turn_point((at.0 + extent.0 * 0.5, at.1 + extent.1 * 0.5), size, clockwise);
                Shape::Text { at: (middle.0 - extent.0 * 0.5, middle.1 - extent.1 * 0.5), words: words.clone(), extent: *extent }
            }
        };
        Note { shape, ..self.clone() }
    }

    /// Whether `p` is on it, give or take `slack`: on a line, in a box or an oval or its words.
    fn holds(&self, p: Point, slack: f32) -> bool {
        match &self.shape {
            Shape::Arrow { from, to } => distance_to_segment(p, *from, *to) <= slack + self.size * 0.5,
            shape => shape.bounds().grown(slack + self.size * 0.5).contains(p),
        }
    }

    /// Whether `p` is on its line itself -- a box's or an oval's edge -- rather than only inside.
    fn on_line(&self, p: Point, slack: f32) -> bool {
        let reach = slack + self.size * 0.5;
        match &self.shape {
            Shape::Box(a) => {
                let c = a.corners();
                (0..4).any(|i| distance_to_segment(p, c[i], c[(i + 1) % 4]) <= reach)
            }
            Shape::Oval(a) => {
                // Near enough the ellipse: its equation's value, as a distance along the gradient.
                let (cx, cy) = a.centre();
                let (rx, ry) = ((a.w() * 0.5).max(1e-3), (a.h() * 0.5).max(1e-3));
                let (nx, ny) = ((p.0 - cx) / rx, (p.1 - cy) / ry);
                let f = nx * nx + ny * ny - 1.0;
                let gradient = 2.0 * ((nx / rx).powi(2) + (ny / ry).powi(2)).sqrt();
                (f / gradient.max(1e-6)).abs() <= reach
            }
            _ => self.holds(p, slack),
        }
    }
}

/// Everything drawn and cut on a file: what is saved.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Marks {
    pub notes: Vec<Note>,
    /// Each page's crop, in its units: what is kept of it.
    pub crops: Vec<(usize, Area)>,
    /// A picture's quarter turns clockwise from how it was read: it is saved as it is shown.
    pub turns: u8,
}

impl Marks {
    pub fn crop(&self, page: usize) -> Option<Area> {
        self.crops.iter().find(|(p, _)| *p == page).map(|(_, a)| *a)
    }

    /// The pages anything is drawn on or cut from, in order.
    pub fn pages(&self) -> Vec<usize> {
        let mut pages: Vec<usize> = self.notes.iter().map(|n| n.page).chain(self.crops.iter().map(|(p, _)| *p)).collect();
        pages.sort_unstable();
        pages.dedup();
        pages
    }

    /// The marks on a picture `size` across and down, turned a quarter with it: every note, the
    /// crop, and the count of turns.
    pub fn turned(&self, size: (f32, f32), clockwise: bool) -> Marks {
        Marks {
            notes: self.notes.iter().map(|n| n.turned(size, clockwise)).collect(),
            crops: self.crops.iter().map(|(p, a)| (*p, turn_area(a, size, clockwise))).collect(),
            turns: (self.turns + if clockwise { 1 } else { 3 }) % 4,
        }
    }
}

/// How many changes can be taken back.
const HISTORY: usize = 200;

/// The marks on one file as the wearer works on them: what there is now, what there was when the
/// file was opened (or last saved), and every step between, back and forth.
#[derive(Debug, Clone)]
pub struct Markup {
    now: Marks,
    origin: Marks,
    undo: Vec<Marks>,
    redo: Vec<Marks>,
    /// The note chosen, which the colour and weight chosen next apply to, and which Delete takes
    /// away.
    pub chosen: Option<usize>,
    /// Counts every change of the marks, for what draws them to know it is to draw them again.
    pub generation: u64,
}

impl Markup {
    /// Nothing drawn yet on a file shown turned `turns` quarters from how it was read.
    pub fn new(turns: u8) -> Markup {
        let origin = Marks { turns, ..Marks::default() };
        Markup { now: origin.clone(), origin, undo: Vec::new(), redo: Vec::new(), chosen: None, generation: 0 }
    }

    pub fn marks(&self) -> &Marks {
        &self.now
    }

    /// Whether there is anything to save: the marks are not what the file had.
    pub fn edited(&self) -> bool {
        self.now != self.origin
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// A change, as one step that can be undone: nothing is kept if it changed nothing.
    pub fn change<R>(&mut self, change: impl FnOnce(&mut Marks) -> R) -> R {
        let before = self.now.clone();
        let answer = change(&mut self.now);
        self.finish(before);
        answer
    }

    /// The marks as they are, kept by a drag as it begins: its every move is [`Markup::adjust`],
    /// and [`Markup::finish`] with this makes the whole drag one step.
    pub fn begin(&self) -> Marks {
        self.now.clone()
    }

    /// A change on the way -- a drag's -- not yet a step of its own.
    pub fn adjust(&mut self, change: impl FnOnce(&mut Marks)) {
        change(&mut self.now);
        self.generation += 1;
    }

    /// The step from `before` to now, if there is one.
    pub fn finish(&mut self, before: Marks) {
        self.generation += 1;
        if before == self.now {
            return;
        }
        self.undo.push(before);
        if self.undo.len() > HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.keep_chosen();
    }

    /// Back a step: whether there was one.
    pub fn undo(&mut self) -> bool {
        let Some(before) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(&mut self.now, before));
        self.generation += 1;
        self.keep_chosen();
        true
    }

    /// On again, after an undo.
    pub fn redo(&mut self) -> bool {
        let Some(after) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(&mut self.now, after));
        self.generation += 1;
        self.keep_chosen();
        true
    }

    /// The choice let go of if what was chosen has gone.
    fn keep_chosen(&mut self) {
        if self.chosen.is_some_and(|i| i >= self.now.notes.len()) {
            self.chosen = None;
        }
    }

    /// A note drawn: chosen, as Preview chooses what was just drawn.
    pub fn add(&mut self, note: Note) -> usize {
        let index = self.change(|marks| {
            marks.notes.push(note);
            marks.notes.len() - 1
        });
        self.chosen = Some(index);
        index
    }

    /// The chosen note taken away: whether there was one.
    pub fn delete_chosen(&mut self) -> bool {
        let Some(index) = self.chosen.filter(|&i| i < self.now.notes.len()) else { return false };
        self.change(|marks| {
            marks.notes.remove(index);
        });
        self.chosen = None;
        true
    }

    /// The note at `p` on `page`, `slack` either side: one whose line is there before one only
    /// round it, then the smallest -- a box drawn inside another is reached inside it -- then the
    /// one drawn last.
    pub fn hit(&self, page: usize, p: Point, slack: f32) -> Option<usize> {
        let under: Vec<usize> = (0..self.now.notes.len()).rev().filter(|&i| self.now.notes[i].page == page && self.now.notes[i].holds(p, slack)).collect();
        let area = |i: usize| {
            let b = self.now.notes[i].shape.bounds();
            b.w() * b.h()
        };
        under.iter().copied().find(|&i| self.now.notes[i].on_line(p, slack)).or_else(|| under.iter().copied().min_by(|&a, &b| area(a).total_cmp(&area(b))))
    }

    /// The handle of note `index` at `p`, `slack` either side.
    pub fn handle_at(&self, index: usize, p: Point, slack: f32) -> Option<Handle> {
        let note = self.now.notes.get(index)?;
        note.shape.handles().into_iter().filter(|(_, at)| (at.0 - p.0).abs() <= slack && (at.1 - p.1).abs() <= slack).min_by(|a, b| {
            let d = |q: Point| (q.0 - p.0).hypot(q.1 - p.1);
            d(a.1).total_cmp(&d(b.1))
        }).map(|(h, _)| h)
    }

    /// The marks turned a quarter with a picture `size` across and down, as one step.
    pub fn turn(&mut self, size: (f32, f32), clockwise: bool) {
        self.change(|marks| *marks = marks.turned(size, clockwise));
    }

    /// The file has been written as the marks are: nothing is left to save.
    pub fn saved(&mut self) {
        self.origin = self.now.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(shape: Shape) -> Note {
        Note { page: 0, shape, ink: Ink::Red, weight: Weight::Medium, size: 4.0 }
    }

    fn boxed(x0: f32, y0: f32, x1: f32, y1: f32) -> Note {
        note(Shape::Box(Area::new(x0, y0, x1, y1)))
    }

    #[test]
    fn notes_are_added_chosen_and_taken_away_and_every_step_comes_back() {
        let mut m = Markup::new(0);
        assert!(!m.edited() && !m.can_undo());
        let a = m.add(boxed(10.0, 10.0, 100.0, 60.0));
        assert_eq!((a, m.chosen), (0, Some(0)));
        m.add(note(Shape::Arrow { from: (0.0, 0.0), to: (50.0, 50.0) }));
        m.add(note(Shape::Text { at: (5.0, 200.0), words: "Grüße 你好".into(), extent: (120.0, 30.0) }));
        assert_eq!(m.marks().notes.len(), 3);
        assert!(m.edited());
        m.chosen = Some(1);
        assert!(m.delete_chosen());
        assert_eq!(m.chosen, None);
        assert_eq!(m.marks().notes.len(), 2);
        assert!(!m.delete_chosen(), "nothing chosen, nothing taken");
        // Back through all four steps, and on again through them.
        let mut seen = vec![m.marks().notes.len()];
        while m.undo() {
            seen.push(m.marks().notes.len());
        }
        assert_eq!(seen, [2, 3, 2, 1, 0]);
        assert!(!m.edited(), "all undone is as the file was");
        while m.redo() {}
        assert_eq!(m.marks().notes.len(), 2);
        assert!(matches!(m.marks().notes[1].shape, Shape::Text { .. }));
        // A new change after an undo: nothing left to redo.
        m.undo();
        m.add(boxed(0.0, 0.0, 5.0, 5.0));
        assert!(!m.can_redo());
    }

    #[test]
    fn a_drag_is_one_step_and_a_change_that_changes_nothing_is_none() {
        let mut m = Markup::new(0);
        m.add(boxed(10.0, 10.0, 100.0, 60.0));
        let before = m.begin();
        for step in 1..=20 {
            m.adjust(|marks| marks.notes[0].shape = Shape::Box(Area::new(10.0 + step as f32, 10.0, 100.0 + step as f32, 60.0)));
        }
        m.finish(before);
        assert_eq!(m.marks().notes[0].shape.bounds().x0, 30.0);
        assert!(m.undo());
        assert_eq!(m.marks().notes[0].shape.bounds().x0, 10.0, "the whole drag taken back at once");
        assert!(m.undo() && !m.undo());
        // Nothing moved: no step.
        m.redo();
        let before = m.begin();
        m.finish(before);
        m.change(|_| {});
        assert!(m.undo() && !m.can_undo());
    }

    #[test]
    fn crops_and_turns_are_steps_too() {
        let mut m = Markup::new(0);
        m.change(|marks| marks.crops = vec![(0, Area::new(10.0, 20.0, 300.0, 200.0))]);
        m.turn((400.0, 300.0), true);
        assert_eq!(m.marks().turns, 1);
        // Turned clockwise, a 400 by 300 picture is 300 by 400: what was 10 from the left is 10
        // from the top, and what was 20 from the top is 20 from the right.
        assert_eq!(m.marks().crop(0), Some(Area::new(100.0, 10.0, 280.0, 300.0)));
        m.undo();
        assert_eq!((m.marks().turns, m.marks().crop(0)), (0, Some(Area::new(10.0, 20.0, 300.0, 200.0))));
        m.undo();
        assert_eq!(m.marks().crop(0), None);
        assert!(!m.edited());
        // A picture turned before anything was drawn starts there, and is not an edit by that.
        let m = Markup::new(3);
        assert!(!m.edited() && m.marks().turns == 3);
    }

    #[test]
    fn four_quarter_turns_bring_every_note_back_and_words_stay_upright() {
        let marks = Marks {
            notes: vec![
                boxed(10.0, 20.0, 110.0, 70.0),
                note(Shape::Arrow { from: (5.0, 5.0), to: (395.0, 295.0) }),
                note(Shape::Text { at: (50.0, 100.0), words: "label".into(), extent: (80.0, 20.0) }),
            ],
            crops: vec![(0, Area::new(1.0, 2.0, 300.0, 250.0))],
            turns: 0,
        };
        let mut turned = marks.clone();
        let mut size = (400.0, 300.0);
        for _ in 0..4 {
            turned = turned.turned(size, true);
            size = (size.1, size.0);
        }
        assert_eq!(turned, marks);
        // Once clockwise: the arrow's start, 5 from the left and the top, is 5 from the top and
        // the right; the words keep their shape, and their middle is where the picture's went.
        let once = marks.turned((400.0, 300.0), true);
        assert_eq!(once.notes[1].shape, Shape::Arrow { from: (295.0, 5.0), to: (5.0, 395.0) });
        let Shape::Text { at, extent, .. } = &once.notes[2].shape else { unreachable!() };
        assert_eq!(*extent, (80.0, 20.0));
        assert_eq!((at.0 + 40.0, at.1 + 10.0), (300.0 - 110.0, 90.0));
        // And back the other way.
        assert_eq!(once.turned((300.0, 400.0), false), Marks { turns: 0, ..marks });
    }

    #[test]
    fn what_is_pressed_on_is_found() {
        let mut m = Markup::new(0);
        m.add(boxed(0.0, 0.0, 400.0, 400.0));
        m.add(boxed(100.0, 100.0, 150.0, 150.0));
        m.add(note(Shape::Oval(Area::new(200.0, 200.0, 300.0, 260.0))));
        m.add(note(Shape::Arrow { from: (10.0, 390.0), to: (390.0, 390.0) }));
        m.add(Note { page: 1, ..boxed(0.0, 0.0, 400.0, 400.0) });
        // Inside the small box, which is inside the large one: the small one.
        assert_eq!(m.hit(0, (120.0, 120.0), 5.0), Some(1));
        // On the large box's edge: the large one, though the oval is drawn later.
        assert_eq!(m.hit(0, (2.0, 200.0), 5.0), Some(0));
        // In the oval's corner, outside the ellipse but in its area: the oval still, as the
        // smallest thing there; on its rim, the oval.
        assert_eq!(m.hit(0, (205.0, 205.0), 5.0), Some(2));
        assert_eq!(m.hit(0, (299.0, 230.0), 5.0), Some(2));
        // On the arrow, and just off it: the arrow; then the box behind it.
        assert_eq!(m.hit(0, (200.0, 393.0), 5.0), Some(3));
        assert_eq!(m.hit(0, (200.0, 380.0), 5.0), Some(0));
        // Off everything, and another page's.
        assert_eq!(m.hit(0, (500.0, 500.0), 5.0), None);
        assert_eq!(m.hit(1, (2.0, 200.0), 5.0), Some(4));
        // A box's corners and an arrow's ends are its handles.
        assert_eq!(m.handle_at(1, (149.0, 151.0), 8.0), Some(Handle::Corner(2)));
        assert_eq!(m.handle_at(3, (12.0, 388.0), 8.0), Some(Handle::From));
        assert_eq!(m.handle_at(1, (125.0, 125.0), 8.0), None);
    }

    #[test]
    fn a_handle_reshapes_and_a_move_moves() {
        let b = Shape::Box(Area::new(10.0, 10.0, 50.0, 30.0));
        // The bottom right corner dragged past the top left: the box turns over, its corner
        // across staying where it was.
        assert_eq!(b.with_handle(Handle::Corner(2), (0.0, 5.0), false), Shape::Box(Area::new(0.0, 5.0, 10.0, 10.0)));
        // Even: square, as large as the larger way.
        assert_eq!(b.with_handle(Handle::Corner(2), (70.0, 20.0), true), Shape::Box(Area::new(10.0, 10.0, 70.0, 70.0)));
        let o = Shape::Oval(Area::new(10.0, 10.0, 50.0, 30.0));
        assert_eq!(o.with_handle(Handle::Corner(0), (0.0, 0.0), false), Shape::Oval(Area::new(0.0, 0.0, 50.0, 30.0)));
        let a = Shape::Arrow { from: (0.0, 0.0), to: (10.0, 10.0) };
        assert_eq!(a.with_handle(Handle::To, (20.0, 5.0), false), Shape::Arrow { from: (0.0, 0.0), to: (20.0, 5.0) });
        assert_eq!(a.moved(5.0, -5.0), Shape::Arrow { from: (5.0, -5.0), to: (15.0, 5.0) });
        let t = Shape::Text { at: (1.0, 2.0), words: "x".into(), extent: (10.0, 5.0) };
        assert!(t.handles().is_empty());
        assert_eq!(t.moved(1.0, 1.0).bounds(), Area::new(2.0, 3.0, 12.0, 8.0));
        assert!(Shape::Box(Area::new(0.0, 0.0, 1.0, 2.0)).is_speck(3.0));
        assert!(!Shape::Arrow { from: (0.0, 0.0), to: (0.0, 9.0) }.is_speck(3.0));
    }

    #[test]
    fn an_arrow_s_head_sits_on_its_end() {
        let (stop, head) = arrow_head((0.0, 0.0), (100.0, 0.0), 4.0);
        assert_eq!(head[0], (100.0, 0.0));
        // 4.5 times the width and 2: 20 long, and 9 either side of the line at its base.
        assert!((head[1].0 - 80.0).abs() < 1e-4 && (head[1].1 - 9.0).abs() < 1e-4, "{head:?}");
        assert!((head[2].1 + 9.0).abs() < 1e-4);
        assert!(stop.0 > 80.0 && stop.0 < 100.0, "the line ends under the head: {stop:?}");
        // A short arrow's head is never longer than most of it.
        let (_, head) = arrow_head((0.0, 0.0), (10.0, 0.0), 4.0);
        assert!((head[1].0 - 4.0).abs() < 1e-4);
    }

    #[test]
    fn weights_scale_with_the_page_and_never_vanish() {
        assert_eq!(Weight::Medium.line(595.0), 2.975);
        assert_eq!(Weight::Heavy.line(3024.0), 30.24);
        assert_eq!(Weight::Light.line(100.0), 1.0, "a small picture's thin line is still a pixel");
        assert!(Weight::Light.text(595.0) < Weight::Medium.text(595.0) && Weight::Medium.text(595.0) < Weight::Heavy.text(595.0));
        assert_eq!(Weight::Light.text(40.0), 10.0);
        assert_eq!((Weight::Light.line_name(), Weight::Heavy.text_name()), ("Thin", "Large"));
    }

    #[test]
    fn an_even_rectangle_goes_the_way_it_was_dragged() {
        assert_eq!(corner_to((10.0, 10.0), (0.0, 30.0), true), Area::new(-10.0, 10.0, 10.0, 30.0));
        assert_eq!(corner_to((10.0, 10.0), (12.0, 13.0), false), Area::new(10.0, 10.0, 12.0, 13.0));
    }
}
