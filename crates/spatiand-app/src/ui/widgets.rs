//! The first widgets: a list, a grid of posters, tabs, a text field and a button.
//!
//! Each is a little state and two calls. `draw` is given a rectangle every frame and
//! remembers where it put things; `input` is given what the wearer did and answers with what
//! came of it -- a row chosen, a tab changed -- using those remembered places for the pointer.
//! All of them work from the keys alone, because a controller's pad arrives as arrow keys and
//! a Beam Pro has no pointer that hovers.
//!
//! Which widget the keys are going to is the application's to decide: it sets `focused` on
//! the one, and gives key input to that one only.

use spatiand_render::panel::keep_in_view;

use super::{theme, Input, Painter, Rect, Texture};
use crate::{Gl, Key};

// --- list ---

/// One line of a list: the room's own row (`crates/spatiand/src/menu.rs`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub label: String,
    /// Set to the right, in blue: a status rather than a name.
    pub trailing: Option<String>,
    /// The name of the group the rows under it belong to: small, dim, never chosen.
    pub heading: bool,
    /// Drawn before the label: what kind of place, or thing, the row is.
    pub icon: Option<super::Icon>,
    /// Drawn before the label in place of an icon: a picture of its own (an application's).
    pub picture: Option<super::Texture>,
}

impl Row {
    /// The same row with a picture before its label, where an icon would be.
    pub fn picture(mut self, picture: super::Texture) -> Row {
        self.picture = Some(picture);
        self
    }

    pub fn new(label: impl Into<String>) -> Row {
        Row { label: label.into(), ..Row::default() }
    }

    pub fn with(label: impl Into<String>, trailing: impl Into<String>) -> Row {
        Row { label: label.into(), trailing: Some(trailing.into()), ..Row::default() }
    }

    pub fn heading(label: impl Into<String>) -> Row {
        Row { label: label.into(), heading: true, ..Row::default() }
    }

    /// The same row with an icon before its label.
    pub fn icon(mut self, icon: super::Icon) -> Row {
        self.icon = Some(icon);
        self
    }
}

/// A column of rows, one of them selected, scrolling when there are more than fit.
#[derive(Debug, Default)]
pub struct List {
    pub cursor: usize,
    pub focused: bool,
    first: usize,
    area: Rect,
    /// Where each row drawn last frame is, by its index.
    placed: Vec<(usize, Rect)>,
    headings: Vec<bool>,
    hover: Option<usize>,
    pressed: Option<usize>,
    /// Wheel movement not yet enough to move a row.
    wheel: f32,
}

impl List {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.area.contains(x, y)
    }

    /// The row under the pointer, as last told.
    pub fn hovered(&self) -> Option<usize> {
        self.hover
    }

    /// Where a row was drawn last, if it was on show: for what is drawn beside it (a menu's
    /// submenu) or over its end (a button of its own).
    pub fn row_rect(&self, index: usize) -> Option<Rect> {
        self.placed.iter().find(|(i, _)| *i == index).map(|(_, r)| *r)
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, rect: Rect, rows: &[Row]) {
        self.area = rect;
        self.headings = rows.iter().map(|r| r.heading).collect();
        self.cursor = self.cursor.min(rows.len().saturating_sub(1));
        self.settle(1);
        let per_row = theme::ROW_HEIGHT + theme::ROW_GAP;
        let visible = (((rect.h + theme::ROW_GAP) / per_row).floor() as usize).min(rows.len());
        self.first = keep_in_view(self.first, self.cursor, visible, rows.len());
        self.placed.clear();
        let scrolls = rows.len() > visible;
        let width = rect.w - if scrolls { theme::SCROLL_WIDTH + 12.0 } else { 0.0 };
        for (slot, index) in (self.first..self.first + visible).enumerate() {
            let row = &rows[index];
            let at = Rect::new(rect.x, rect.y + slot as f32 * per_row, width, theme::ROW_HEIGHT);
            self.placed.push((index, at));
            let text = Rect { x: at.x + theme::ROW_INSET, w: at.w - theme::ROW_INSET * 2.0, ..at };
            if row.heading {
                // Sitting low in its row, so it reads as belonging to what is under it.
                let low = Rect { y: at.y + at.h * 0.3, h: at.h * 0.7, ..text };
                p.label(gl, &row.label, low, theme::FOOTER_EM, theme::ink_hint(), 0.0);
                continue;
            }
            let selected = index == self.cursor;
            if selected {
                let wash = if self.focused { theme::selected() } else { theme::selected_away() };
                p.fill(gl, at, theme::ROW_RADIUS, wash);
            } else if self.hover == Some(index) {
                p.fill(gl, at, theme::ROW_RADIUS, theme::hover());
            }
            let mut label = text;
            if let Some(trailing) = row.trailing.as_deref() {
                let taken = p.label(gl, trailing, text, theme::ROW_EM * 0.8, theme::ink_trailing(), 1.0);
                label.w = (label.w - taken - 24.0).max(0.0);
            }
            let ink = if selected { theme::ink() } else { theme::ink_row() };
            if let Some(picture) = row.picture {
                // A picture is its own colours, a little larger than an icon's mark.
                let side = (theme::ROW_EM * 1.15).min(at.h - 6.0);
                let mark = Rect::new(label.x - 2.0, at.y + (at.h - side) * 0.5, side, side);
                p.image(gl, picture, mark, [0.0, 0.0, 1.0, 1.0], 0.0, [1.0, 1.0, 1.0, 1.0], false);
                label.x += side + 14.0;
                label.w = (label.w - side - 14.0).max(0.0);
            } else if let Some(icon) = row.icon {
                let side = theme::ROW_EM * 0.95;
                let mark = Rect::new(label.x, at.y + (at.h - side) * 0.5, side, side);
                super::icons::draw(p, gl, icon, mark, if selected { theme::mark() } else { theme::ink_detail() });
                label.x += side + 18.0;
                label.w = (label.w - side - 18.0).max(0.0);
            }
            p.label(gl, &row.label, label, theme::ROW_EM, ink, 0.0);
        }
        if scrolls && visible > 0 {
            let track = Rect::new(rect.right() - theme::SCROLL_WIDTH, rect.y, theme::SCROLL_WIDTH, visible as f32 * per_row - theme::ROW_GAP);
            super::look::scrollbar(p, gl, track, visible as f32, rows.len() as f32, self.first as f32);
        }
    }

    /// Move the cursor off a heading, onwards in `direction` and back if there is nothing there.
    fn settle(&mut self, direction: isize) {
        let total = self.headings.len();
        let heading = |i: usize| self.headings.get(i).copied().unwrap_or(false);
        let mut at = self.cursor as isize;
        while at >= 0 && (at as usize) < total && heading(at as usize) {
            at += direction;
        }
        if at < 0 || at as usize >= total {
            at = self.cursor as isize;
            while at >= 0 && (at as usize) < total && heading(at as usize) {
                at -= direction;
            }
        }
        if at >= 0 && (at as usize) < total {
            self.cursor = at as usize;
        }
    }

    fn step(&mut self, by: isize) {
        let total = self.headings.len() as isize;
        if total == 0 {
            return;
        }
        self.cursor = (self.cursor as isize + by).clamp(0, total - 1) as usize;
        self.settle(by.signum());
    }

    fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        let (index, _) = self.placed.iter().find(|(_, rect)| rect.contains(x, y))?;
        (!self.headings.get(*index).copied().unwrap_or(false)).then_some(*index)
    }

    /// Returns the row chosen, if this chose one.
    pub fn input(&mut self, input: &Input) -> Option<usize> {
        match input {
            Input::Pointer { x, y } => self.hover = self.row_at(*x, *y),
            Input::PointerLeft => {
                self.hover = None;
                self.pressed = None;
            }
            Input::Press { pressed: true, .. } => {
                self.pressed = self.hover;
                if let Some(row) = self.hover {
                    self.cursor = row;
                }
            }
            // Chosen on letting go over the row that was pressed, like a button anywhere.
            Input::Press { pressed: false, .. } => {
                let pressed = self.pressed.take();
                if pressed.is_some() && pressed == self.hover {
                    return pressed;
                }
            }
            Input::Scroll { dy, .. } if self.hover.is_some() || self.focused => {
                self.wheel += dy;
                let per_row = theme::ROW_HEIGHT + theme::ROW_GAP;
                let rows = (self.wheel / per_row).trunc();
                if rows != 0.0 {
                    self.wheel -= rows * per_row;
                    let last = self.headings.len().saturating_sub(self.placed.len());
                    self.first = (self.first as isize + rows as isize).clamp(0, last as isize) as usize;
                    // The cursor comes with the list rather than dragging it back.
                    self.cursor = self.cursor.clamp(self.first, self.first + self.placed.len().saturating_sub(1));
                    self.settle(rows.signum() as isize);
                }
            }
            Input::Key { key, .. } if self.focused => match key {
                Key::Up => self.step(-1),
                Key::Down => self.step(1),
                Key::PageUp => self.step(-(self.placed.len().max(1) as isize)),
                Key::PageDown => self.step(self.placed.len().max(1) as isize),
                Key::Home => {
                    self.cursor = 0;
                    self.settle(1);
                }
                Key::End => {
                    self.cursor = self.headings.len().saturating_sub(1);
                    self.settle(-1);
                }
                Key::Enter if !self.headings.is_empty() => return Some(self.cursor),
                _ => {}
            },
            _ => {}
        }
        None
    }
}

// --- grid ---

/// The shape of a grid's pictures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shape {
    /// A film's poster, two wide to three tall.
    #[default]
    Tall,
    /// A frame of the thing itself, sixteen to nine.
    Wide,
    Square,
}

impl Shape {
    /// Width over height.
    pub fn aspect(self) -> f32 {
        match self {
            Shape::Tall => 2.0 / 3.0,
            Shape::Wide => 16.0 / 9.0,
            Shape::Square => 1.0,
        }
    }

    /// The shape most of these pictures are, by their own width over height: a library of
    /// posters is shown as posters and a folder of episodes' frames as frames, whichever
    /// server either came from. With nothing to go on, `fallback`.
    pub fn of(aspects: impl IntoIterator<Item = f32>, fallback: Shape) -> Shape {
        let (mut tall, mut wide, mut square) = (0usize, 0usize, 0usize);
        for aspect in aspects {
            if aspect < 0.85 {
                tall += 1;
            } else if aspect > 1.2 {
                wide += 1;
            } else {
                square += 1;
            }
        }
        if tall + wide + square == 0 {
            fallback
        } else if tall >= wide && tall >= square {
            Shape::Tall
        } else if wide >= square {
            Shape::Wide
        } else {
            Shape::Square
        }
    }

    /// How wide a tile of this shape would like to be.
    fn width(self) -> f32 {
        match self {
            Shape::Tall => 200.0,
            Shape::Wide => 330.0,
            Shape::Square => 220.0,
        }
    }
}

/// One thing in a grid.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tile {
    pub title: String,
    /// A second, dimmer line: a year, a count, how long.
    pub detail: Option<String>,
    /// Its picture, once there is one.
    pub picture: Option<Texture>,
    /// How far through it the wearer is, 0 to 1, drawn as a bar along the picture's foot.
    pub progress: Option<f32>,
    /// What stands where its picture would be, for a tile that will never have one: a
    /// mark, in place of the first letter of its name.
    pub glyph: Option<String>,
    /// The colours of that place, top and foot, for a tile that is a kind of thing rather
    /// than a thing: the room's dim blue otherwise.
    pub tint: Option<([f32; 4], [f32; 4])>,
    /// An icon in place of the picture, for a tile that is a kind of thing -- a folder.
    pub icon: Option<super::Icon>,
}

/// A grid of pictures with their names under them, scrolling by the row.
#[derive(Debug, Default)]
pub struct Grid {
    pub cursor: usize,
    pub focused: bool,
    pub shape: Shape,
    /// How far down the content the top of the view is, in logical pixels.
    scroll: f32,
    area: Rect,
    columns: usize,
    row_height: f32,
    total: usize,
    placed: Vec<(usize, Rect)>,
    hover: Option<usize>,
    pressed: Option<usize>,
}

const CAPTION: f32 = theme::ROW_EM * 0.72;
const CAPTION_DETAIL: f32 = theme::FOOTER_EM * 0.92;
/// The room a tile's two lines of words take under its picture.
const CAPTION_ROOM: f32 = CAPTION * 1.4 + CAPTION_DETAIL * 1.4 + 10.0;

/// One tile: its picture, `picture` tall, and its words under it. The one the cursor is on
/// stands in a ring of the room's blue with light spilling from it -- found at a glance from
/// across a room, which a faint wash is not.
#[allow(clippy::too_many_arguments)]
fn draw_tile(p: &mut Painter, gl: &Gl, cell: Rect, picture: f32, tile: &Tile, selected: bool, focused: bool, hover: bool) {
    let frame = Rect { h: picture, ..cell };
    if selected && focused {
        p.glow(gl, frame, theme::ROW_RADIUS, theme::glow(0.42), 22.0);
        p.fill(gl, frame.inset(-3.0), theme::ROW_RADIUS + 3.0, theme::mark());
    } else if selected {
        p.fill(gl, frame.inset(-2.0), theme::ROW_RADIUS + 2.0, theme::wash(0.28));
    } else if hover {
        p.fill(gl, frame.inset(-2.0), theme::ROW_RADIUS + 2.0, theme::wash(0.16));
    }
    match tile.picture {
        Some(texture) => p.cover(gl, texture, frame, theme::ROW_RADIUS, theme::ink()),
        None => {
            // Its name, large, where its picture will be: a tile that is still loading, or
            // has no picture, says what it is rather than being a grey box among grey boxes.
            p.fill(gl, frame, theme::ROW_RADIUS, theme::ground());
            let (top, foot) = tile.tint.unwrap_or(([0.36, 0.48, 0.78, 0.26], [0.16, 0.20, 0.36, 0.14]));
            p.gradient(gl, frame, theme::ROW_RADIUS, top, foot);
            let initial: String = match tile.glyph.as_deref() {
                Some(glyph) => glyph.to_string(),
                None => tile.title.chars().find(|c| c.is_alphanumeric()).into_iter().collect::<String>().to_uppercase(),
            };
            let ink = if tile.tint.is_some() { [1.0, 1.0, 1.0, 0.82] } else { [1.0, 1.0, 1.0, 0.16] };
            match tile.icon {
                Some(icon) => {
                    let side = picture.min(cell.w) * 0.34;
                    super::icons::draw(p, gl, icon, Rect::new(frame.x + (frame.w - side) * 0.5, frame.y + (frame.h - side) * 0.5, side, side), ink);
                }
                None => {
                    p.label(gl, &initial, frame, picture.min(cell.w) * 0.36, ink, 0.5);
                }
            }
        }
    }
    if let Some(progress) = tile.progress.filter(|p| *p > 0.0) {
        // A shade for the bar to be read against, whatever the picture is.
        p.gradient(gl, Rect { y: frame.bottom() - 46.0, h: 46.0, ..frame }, theme::ROW_RADIUS, [0.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 0.6]);
        let bar = Rect::new(frame.x + 12.0, frame.bottom() - 15.0, frame.w - 24.0, 5.0);
        p.fill(gl, bar, 2.5, [1.0, 1.0, 1.0, 0.25]);
        p.fill(gl, Rect { w: bar.w * progress.min(1.0), ..bar }, 2.5, theme::mark());
    }
    let name = Rect::new(cell.x + 4.0, frame.bottom() + 10.0, cell.w - 8.0, CAPTION * 1.4);
    p.label(gl, &tile.title, name, CAPTION, if selected { theme::ink() } else { theme::ink_row() }, 0.0);
    if let Some(detail) = tile.detail.as_deref() {
        let under = Rect { y: name.bottom(), h: CAPTION_DETAIL * 1.4, ..name };
        p.label(gl, detail, under, CAPTION_DETAIL, theme::ink_hint(), 0.0);
    }
}

impl Grid {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.area.contains(x, y)
    }

    /// Which tiles are on show, as last drawn and no more: the only ones whose pictures are
    /// worth fetching. Nothing before the first drawing.
    pub fn in_view(&self) -> std::ops::Range<usize> {
        let (first, last) = self.placed.iter().fold((usize::MAX, 0), |(a, b), (i, _)| (a.min(*i), b.max(*i + 1)));
        if first == usize::MAX {
            0..0
        } else {
            first..last
        }
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, rect: Rect, tiles: &[Tile]) {
        self.area = rect;
        self.total = tiles.len();
        self.cursor = self.cursor.min(tiles.len().saturating_sub(1));
        let gap = theme::TILE_GAP;
        // The selection's wash stands a little proud of the tile, so the tiles stand in from
        // the edge by as much.
        let edge = 12.0;
        let inner = rect.w - edge * 2.0;
        self.columns = (((inner + gap) / (self.shape.width() + gap)).floor() as usize).max(1);
        let width = (inner - gap * (self.columns - 1) as f32) / self.columns as f32;
        let picture = width / self.shape.aspect();
        let caption = CAPTION_ROOM;
        self.row_height = picture + caption + gap;
        let rows = tiles.len().div_ceil(self.columns);
        let content = rows as f32 * self.row_height - gap + edge * 2.0;
        // Only as far as it takes to bring the cursor's row wholly into view.
        let top = (self.cursor / self.columns) as f32 * self.row_height;
        let bottom = top + self.row_height - gap + edge * 2.0;
        if top < self.scroll {
            self.scroll = top;
        } else if bottom > self.scroll + rect.h {
            self.scroll = bottom - rect.h;
        }
        self.scroll = self.scroll.clamp(0.0, (content - rect.h).max(0.0));

        self.placed.clear();
        p.clip(gl, rect);
        for (index, tile) in tiles.iter().enumerate() {
            let (column, row) = (index % self.columns, index / self.columns);
            let cell = Rect::new(
                rect.x + edge + column as f32 * (width + gap),
                rect.y + edge + row as f32 * self.row_height - self.scroll,
                width,
                picture + caption,
            );
            if cell.bottom() < rect.y || cell.y > rect.bottom() {
                continue;
            }
            self.placed.push((index, cell));
            draw_tile(p, gl, cell, picture, tile, index == self.cursor, self.focused, self.hover == Some(index));
        }
        p.unclip(gl);
        if content > rect.h {
            let track = Rect::new(rect.right() - theme::SCROLL_WIDTH, rect.y, theme::SCROLL_WIDTH, rect.h);
            super::look::scrollbar(p, gl, track, rect.h, content, self.scroll);
        }
    }

    fn tile_at(&self, x: f32, y: f32) -> Option<usize> {
        if !self.area.contains(x, y) {
            return None;
        }
        self.placed.iter().find(|(_, rect)| rect.contains(x, y)).map(|(index, _)| *index)
    }

    fn step(&mut self, by: isize) {
        if self.total == 0 {
            return;
        }
        self.cursor = (self.cursor as isize + by).clamp(0, self.total as isize - 1) as usize;
    }

    /// Whether a key would take the cursor off the top of the grid, for an application with
    /// something above it to give the keys to.
    /// Back to the first row.
    pub fn top(&mut self) {
        self.scroll = 0.0;
    }

    pub fn at_top(&self) -> bool {
        self.cursor < self.columns.max(1)
    }

    /// Returns the tile chosen, if this chose one.
    pub fn input(&mut self, input: &Input) -> Option<usize> {
        let columns = self.columns.max(1) as isize;
        match input {
            Input::Pointer { x, y } => self.hover = self.tile_at(*x, *y),
            Input::PointerLeft => {
                self.hover = None;
                self.pressed = None;
            }
            Input::Press { pressed: true, .. } => {
                self.pressed = self.hover;
                if let Some(tile) = self.hover {
                    self.cursor = tile;
                }
            }
            Input::Press { pressed: false, .. } => {
                let pressed = self.pressed.take();
                if pressed.is_some() && pressed == self.hover {
                    return pressed;
                }
            }
            Input::Scroll { dy, .. } if self.hover.is_some() || self.focused => {
                let rows = self.total.div_ceil(self.columns.max(1)) as f32;
                let most = (rows * self.row_height - theme::TILE_GAP + 20.0 - self.area.h).max(0.0);
                self.scroll = (self.scroll + dy).clamp(0.0, most);
                // The cursor comes with the view, or the next frame would drag the view back.
                if self.row_height > 0.0 {
                    let first = (self.scroll / self.row_height).ceil() as usize;
                    let last = (((self.scroll + self.area.h) / self.row_height).floor() as usize).max(first + 1) - 1;
                    let row = (self.cursor / self.columns.max(1)).clamp(first, last);
                    self.cursor = (row * self.columns.max(1) + self.cursor % self.columns.max(1)).min(self.total.saturating_sub(1));
                }
            }
            Input::Key { key, .. } if self.focused => match key {
                Key::Left => self.step(-1),
                Key::Right => self.step(1),
                Key::Up => {
                    if self.cursor as isize >= columns {
                        self.step(-columns)
                    }
                }
                Key::Down => {
                    // Onto a short last row from a column it has not got, too.
                    let last_row = (self.total.saturating_sub(1)) as isize / columns;
                    if (self.cursor as isize) / columns < last_row {
                        self.step(columns)
                    }
                }
                Key::PageUp => self.step(-columns * 3),
                Key::PageDown => self.step(columns * 3),
                Key::Home => self.cursor = 0,
                Key::End => self.cursor = self.total.saturating_sub(1),
                Key::Enter if self.total > 0 => return Some(self.cursor),
                _ => {}
            },
            _ => {}
        }
        None
    }
}

// --- strip ---

/// One row of tiles, going off to the right: a shelf.
#[derive(Debug, Default)]
pub struct Strip {
    pub cursor: usize,
    pub focused: bool,
    pub shape: Shape,
    /// How far along the content the left of the view is.
    scroll: f32,
    area: Rect,
    total: usize,
    placed: Vec<(usize, Rect)>,
    hover: Option<usize>,
    pressed: Option<usize>,
}

impl Strip {
    /// How tall a strip of tiles `width` wide is.
    pub fn height(shape: Shape, width: f32) -> f32 {
        width / shape.aspect() + CAPTION_ROOM + 28.0
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.area.contains(x, y)
    }

    /// Which tiles are on show, as last drawn and no more. Nothing before the first drawing.
    pub fn in_view(&self) -> std::ops::Range<usize> {
        let (first, last) = self.placed.iter().fold((usize::MAX, 0), |(a, b), (i, _)| (a.min(*i), b.max(*i + 1)));
        if first == usize::MAX {
            0..0
        } else {
            first..last
        }
    }

    /// Draw `tiles`, each `width` wide, in a row across `rect`.
    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, rect: Rect, tiles: &[Tile], width: f32) {
        self.area = rect;
        self.total = tiles.len();
        self.cursor = self.cursor.min(tiles.len().saturating_sub(1));
        let (gap, edge) = (theme::TILE_GAP, 14.0);
        let picture = width / self.shape.aspect();
        let step = width + gap;
        let content = tiles.len() as f32 * step - gap + edge * 2.0;
        let left = self.cursor as f32 * step;
        if left < self.scroll {
            self.scroll = left;
        } else if left + width + edge * 2.0 > self.scroll + rect.w {
            self.scroll = left + width + edge * 2.0 - rect.w;
        }
        self.scroll = self.scroll.clamp(0.0, (content - rect.w).max(0.0));
        self.placed.clear();
        p.clip(gl, rect);
        for (index, tile) in tiles.iter().enumerate() {
            let cell = Rect::new(rect.x + edge + index as f32 * step - self.scroll, rect.y + edge, width, picture + CAPTION_ROOM);
            if cell.right() < rect.x || cell.x > rect.right() {
                continue;
            }
            self.placed.push((index, cell));
            draw_tile(p, gl, cell, picture, tile, index == self.cursor, self.focused, self.hover == Some(index));
        }
        p.unclip(gl);
    }

    /// Returns the tile chosen, if this chose one.
    pub fn input(&mut self, input: &Input) -> Option<usize> {
        match input {
            Input::Pointer { x, y } => {
                self.hover = self.area.contains(*x, *y).then(|| self.placed.iter().find(|(_, r)| r.contains(*x, *y)).map(|(i, _)| *i)).flatten()
            }
            Input::PointerLeft => {
                self.hover = None;
                self.pressed = None;
            }
            Input::Press { pressed: true, .. } => {
                self.pressed = self.hover;
                if let Some(tile) = self.hover {
                    self.cursor = tile;
                }
            }
            Input::Press { pressed: false, .. } => {
                let pressed = self.pressed.take();
                if pressed.is_some() && pressed == self.hover {
                    return pressed;
                }
            }
            Input::Scroll { dx, dy } if self.hover.is_some() => {
                // A wheel has one direction and a shelf has one too.
                self.scroll = (self.scroll + dx + dy).max(0.0);
                self.cursor = self.placed.iter().map(|(i, _)| *i).min().unwrap_or(self.cursor).max(self.cursor.min(self.placed.iter().map(|(i, _)| *i).max().unwrap_or(0)));
            }
            Input::Key { key, .. } if self.focused => match key {
                Key::Left => self.cursor = self.cursor.saturating_sub(1),
                Key::Right => self.cursor = (self.cursor + 1).min(self.total.saturating_sub(1)),
                Key::Home => self.cursor = 0,
                Key::End => self.cursor = self.total.saturating_sub(1),
                Key::Enter if self.total > 0 => return Some(self.cursor),
                _ => {}
            },
            _ => {}
        }
        None
    }
}

// --- tabs ---

/// A line of names, one of them the one being shown.
#[derive(Debug, Default)]
pub struct Tabs {
    pub active: usize,
    pub focused: bool,
    count: usize,
    placed: Vec<Rect>,
    hover: Option<usize>,
}

impl Tabs {
    /// The height a line of tabs is drawn at.
    pub const HEIGHT: f32 = theme::ROW_HEIGHT;

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.placed.iter().any(|rect| rect.contains(x, y))
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, rect: Rect, labels: &[&str]) {
        self.count = labels.len();
        self.active = self.active.min(labels.len().saturating_sub(1));
        self.placed.clear();
        let em = theme::ROW_EM * 0.86;
        let wide = 8192.0 / p.scale();
        let mut x = rect.x;
        for (index, label) in labels.iter().enumerate() {
            let width = p.measure(gl, label, em, wide).0 + theme::ROW_INSET * 2.0;
            let at = Rect::new(x, rect.y, width.min(rect.right() - x), rect.h.min(Self::HEIGHT));
            if at.w <= theme::ROW_INSET {
                break;
            }
            self.placed.push(at);
            let active = index == self.active;
            if active {
                let wash = if self.focused { theme::selected() } else { theme::selected_away() };
                p.fill(gl, at, at.h * 0.5, wash);
            } else if self.hover == Some(index) {
                p.fill(gl, at, at.h * 0.5, theme::hover());
            }
            let ink = if active { theme::ink() } else { theme::ink_row() };
            p.label(gl, label, at, em, ink, 0.5);
            x += width + 8.0;
        }
    }

    /// Returns the tab changed to, if this changed it.
    pub fn input(&mut self, input: &Input) -> Option<usize> {
        let before = self.active;
        match input {
            Input::Pointer { x, y } => self.hover = self.placed.iter().position(|rect| rect.contains(*x, *y)),
            Input::PointerLeft => self.hover = None,
            Input::Press { pressed: true, .. } => {
                if let Some(tab) = self.hover {
                    self.active = tab;
                }
            }
            Input::Key { key: Key::Left, .. } if self.focused => self.active = self.active.saturating_sub(1),
            Input::Key { key: Key::Right, .. } if self.focused => {
                self.active = (self.active + 1).min(self.count.saturating_sub(1))
            }
            _ => {}
        }
        (self.active != before).then_some(self.active)
    }
}

// --- text field ---

/// One line of text to type into.
#[derive(Debug, Default)]
pub struct Field {
    pub text: String,
    pub focused: bool,
    /// Shown, dim, while there is no text: what the field is for.
    pub hint: String,
    /// What is typed is not to be read over a shoulder: drawn as dots.
    pub secret: bool,
    /// The text's size, when not the rows' own ([`theme::ROW_EM`]): a browser's address
    /// is long and wants more of it in view.
    pub em: Option<f32>,
    /// Where the caret is, in characters from the start.
    caret: usize,
    /// The other end of the selection, when there is one: what is between it and the caret.
    anchor: Option<usize>,
    area: Rect,
    /// The pointer, as last told.
    pointer: (f32, f32),
    /// The pointer's doing, waiting for the next draw to be told which character it is at:
    /// only the painter can measure the text.
    aim: Option<Aim>,
    /// A press in the field not yet let go of: a move drags out a selection from it.
    dragging: Option<Grab>,
    /// The last press, for a double (a word) or a triple (everything) click.
    last_press: Option<(std::time::Instant, (f32, f32), u8)>,
    /// The pointer's button, pressed in the field, is held: a press and its letting go can both
    /// come before the press is drawn, and then there is nothing to drag.
    held: bool,
}

/// What a press or a drag in a field was, to be put at a character when the field is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Aim {
    /// A press at `x`: `clicks` in a row (1, 2 a word, 3 everything), `extend` with Shift held.
    Press { x: f32, clicks: u8, extend: bool },
    /// The pointer moved to `x` with the button held.
    Drag { x: f32 },
}

/// What a drag from a press holds on to: the character pressed at, or the word (or all) a
/// double (triple) click chose, which a drag then grows from.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Grab {
    from: usize,
    to: usize,
    clicks: u8,
}

/// How near in time and place two presses are to be one double click: in every app, a list's,
/// a field's, a file's or a picture's.
pub const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(450);
pub const DOUBLE_CLICK_SLOP: f32 = 6.0;
/// How far, in logical pixels, a press goes before it is a drag rather than a press: a band,
/// a file carried, a picture swiped.
pub const DRAG_AFTER: f32 = 8.0;

/// Which kind of character a word is made of: a double click chooses a run of one kind.
fn word_class(c: char) -> u8 {
    if c.is_alphanumeric() || c == '_' {
        0
    } else if c.is_whitespace() {
        1
    } else {
        2
    }
}

/// The word around the character boundary `at` of `text`: (from, to), in characters. The
/// character after the boundary is the one pressed on (the last, at the end).
pub fn word_at(text: &str, at: usize) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return (0, 0);
    }
    let at = at.min(chars.len());
    let on = if at == chars.len() { at - 1 } else { at };
    let class = word_class(chars[on]);
    let mut from = on;
    while from > 0 && word_class(chars[from - 1]) == class {
        from -= 1;
    }
    let mut to = on + 1;
    while to < chars.len() && word_class(chars[to]) == class {
        to += 1;
    }
    (from, to)
}

/// The character boundary nearest `x` along a line of `count` characters whose boundaries are
/// at `width(i)` (the width of the first `i` characters, which grows with `i`): found by
/// halving, so a long line costs a few measurements, not one a character.
pub fn boundary_at(x: f32, count: usize, mut width: impl FnMut(usize) -> f32) -> usize {
    if x <= 0.0 || count == 0 {
        return 0;
    }
    if width(count) <= x {
        return count;
    }
    // The first boundary at or past x...
    let (mut low, mut high) = (0usize, count);
    while low < high {
        let mid = (low + high) / 2;
        if width(mid) < x {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    // ...or the one before it, whichever is nearer.
    if low > 0 && x - width(low - 1) < width(low) - x {
        low - 1
    } else {
        low
    }
}

/// What typing into a field came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Typed {
    Changed,
    /// Enter: the wearer has finished.
    Entered,
}

impl Field {
    /// An empty field whose text is `em` high rather than a row's.
    pub fn sized(em: f32) -> Self {
        Field { em: Some(em), ..Field::default() }
    }

    pub const HEIGHT: f32 = theme::ROW_HEIGHT;

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.area.contains(x, y)
    }

    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.caret = self.text.chars().count();
        self.anchor = None;
    }

    /// The text from character `from` to the end chosen, the caret at the end: what a browser's
    /// address bar does with the rest of an address it finished for the wearer, so that what
    /// they type next replaces it.
    pub fn select_from(&mut self, from: usize) {
        self.caret = self.text.chars().count();
        self.anchor = Some(from.min(self.caret));
    }

    /// Characters `from` to `to` chosen, the caret at `to`: a file's name but its ending, as
    /// a file manager chooses it to be renamed.
    pub fn select(&mut self, from: usize, to: usize) {
        let length = self.text.chars().count();
        self.caret = to.min(length);
        self.anchor = Some(from.min(self.caret));
    }

    /// Whether the caret is at the end with nothing chosen: where typing adds to the text.
    pub fn caret_at_end(&self) -> bool {
        self.selection().is_none() && self.caret >= self.text.chars().count()
    }

    /// Everything chosen, as Control-A (Command-A) does: what is typed next replaces it.
    pub fn select_all(&mut self) {
        self.caret = self.text.chars().count();
        self.anchor = Some(0);
    }

    /// The chosen part, from and to, in characters; none when nothing is.
    fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?.min(self.text.chars().count());
        let caret = self.caret.min(self.text.chars().count());
        (anchor != caret).then(|| (anchor.min(caret), anchor.max(caret)))
    }

    /// What is chosen, if anything is.
    pub fn selected_text(&self) -> Option<String> {
        let (from, to) = self.selection()?;
        Some(self.text.chars().skip(from).take(to - from).collect())
    }

    /// Take out what is chosen. Whether there was anything.
    fn delete_selection(&mut self) -> bool {
        let Some((from, to)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        let (a, b) = (self.byte_at(from), self.byte_at(to));
        self.text.replace_range(a..b, "");
        self.caret = from;
        self.anchor = None;
        true
    }

    /// Put text in at the caret, in place of what is chosen: a paste. One line: what would
    /// have broken it is a space.
    pub fn insert(&mut self, text: &str) {
        self.delete_selection();
        let text: String = text.chars().map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c }).collect();
        let at = self.byte_at(self.caret);
        self.text.insert_str(at, &text);
        self.caret += text.chars().count();
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, rect: Rect) {
        self.area = rect;
        let em = self.em.unwrap_or(theme::ROW_EM);
        self.caret = self.caret.min(self.text.chars().count());
        p.fill(gl, rect, theme::ROW_RADIUS, theme::raised());
        if self.focused {
            p.fill(gl, Rect { y: rect.bottom() - 3.0, h: 3.0, ..rect.inset(theme::ROW_RADIUS * 0.5) }, 1.5, theme::mark());
        }
        let text = Rect { x: rect.x + theme::ROW_INSET, w: rect.w - theme::ROW_INSET * 2.0, ..rect };
        let wide = 8192.0 / p.scale();
        if self.text.is_empty() {
            p.label(gl, &self.hint, text, em, theme::ink_hint(), 0.0);
        }
        // What is before the caret decides where the caret is; a field too full to show it
        // all slides left so that the caret is never out of sight.
        let shown: String = if self.secret { "\u{2022}".repeat(self.text.chars().count()) } else { self.text.clone() };
        let before: String = shown.chars().take(self.caret).collect();
        // A trailing space has no ink and so no width as far as the text setter is concerned:
        // measured with something after it, and that something taken off again.
        let stop = p.measure(gl, "|", em, wide).0;
        let mut caret = if before.is_empty() { 0.0 } else { p.measure(gl, &format!("{before}|"), em, wide).0 - stop };
        let mut slide = (caret - text.w + 6.0).max(0.0);
        // The pointer's doing, put at the character it was at, as the text lay when it was
        // pressed (slid by where the caret was).
        if let Some(aim) = self.aim.take() {
            let count = shown.chars().count();
            let mut width = |chars: usize| -> f32 {
                let part: String = shown.chars().take(chars).collect();
                if part.is_empty() { 0.0 } else { p.measure(gl, &format!("{part}|"), em, wide).0 - stop }
            };
            let x = match aim {
                Aim::Press { x, .. } | Aim::Drag { x } => x - (text.x - slide),
            };
            let at = boundary_at(x, count, &mut width);
            self.point(aim, at);
            if !self.held {
                self.dragging = None;
            }
            caret = width(self.caret);
            slide = (caret - text.w + 6.0).max(0.0);
        }
        p.clip(gl, text);
        if let Some((from, to)) = self.selection() {
            let width = |p: &mut Painter, chars: usize| {
                let part: String = shown.chars().take(chars).collect();
                if part.is_empty() { 0.0 } else { p.measure(gl, &format!("{part}|"), em, wide).0 - stop }
            };
            let (a, b) = (width(p, from), width(p, to));
            let wash = Rect::new(text.x + a - slide, rect.y + rect.h * 0.18, (b - a).max(2.0), rect.h * 0.64);
            p.fill(gl, wash, 2.0, if self.focused { let mut c = theme::selected(); c[3] = 0.42; c } else { theme::selected_away() });
        }
        if !self.text.is_empty() {
            let (_, h) = p.measure(gl, &shown, em, wide);
            p.text(gl, &shown, text.x - slide, text.y + (text.h - h) * 0.5, em, wide, theme::ink());
        }
        p.unclip(gl);
        if self.focused {
            let bar = Rect::new(text.x + caret - slide + 1.0, rect.y + rect.h * 0.22, 2.0, rect.h * 0.56);
            p.fill(gl, bar, 1.0, theme::mark());
        }
    }

    fn byte_at(&self, chars: usize) -> usize {
        self.text.char_indices().nth(chars).map_or(self.text.len(), |(at, _)| at)
    }

    /// A press or a drag put at the character boundary `at`.
    fn point(&mut self, aim: Aim, at: usize) {
        let length = self.text.chars().count();
        let at = at.min(length);
        match aim {
            Aim::Press { extend: true, .. } => {
                // Shift: from the far end of what was chosen (or from the caret) to here.
                let anchor = match self.selection() {
                    Some((from, to)) if at <= from => to,
                    Some((from, to)) if at >= to => from,
                    Some((from, _)) => self.anchor.unwrap_or(from),
                    None => self.caret,
                };
                self.anchor = (anchor != at).then_some(anchor);
                self.caret = at;
                self.dragging = self.held.then_some(Grab { from: anchor, to: anchor, clicks: 1 });
            }
            Aim::Press { clicks, .. } => {
                let (from, to) = match clicks {
                    1 => (at, at),
                    2 => word_at(&self.text, at),
                    _ => (0, length),
                };
                self.anchor = (from != to).then_some(from);
                self.caret = to;
                self.dragging = self.held.then_some(Grab { from, to, clicks });
            }
            Aim::Drag { .. } => {
                let Some(grab) = self.dragging else { return };
                // Grown from what the press chose -- by characters, or by words after a double
                // click -- to wherever the pointer is.
                let (from, to) = match grab.clicks {
                    1 => (grab.from, at),
                    2 => {
                        let (a, b) = word_at(&self.text, at);
                        if a < grab.from { (grab.to, a) } else { (grab.from, b.max(grab.to)) }
                    }
                    _ => (0, length),
                };
                self.anchor = (from != to).then_some(from);
                self.caret = to;
            }
        }
    }

    /// The pointer in the field. A press puts the caret where it is (two in a row choose a
    /// word, three everything; with Shift, from what was chosen to there); a drag chooses.
    /// Only a focused field heeds it: whether a press elsewhere ends the typing is for the
    /// application to say.
    fn pointer_input(&mut self, input: &Input) {
        match input {
            Input::Pointer { x, y } => {
                self.pointer = (*x, *y);
                if self.dragging.is_some() && self.focused {
                    // The press itself, waiting to be drawn, stays: the drag goes on from it.
                    if !matches!(self.aim, Some(Aim::Press { .. })) {
                        self.aim = Some(Aim::Drag { x: *x });
                    }
                }
            }
            Input::Press { pressed: true, modifiers } if self.focused && self.area.contains(self.pointer.0, self.pointer.1) => {
                let now = std::time::Instant::now();
                let (x, y) = self.pointer;
                let clicks = match self.last_press {
                    Some((when, at, n)) if !modifiers.shift && now.duration_since(when) < DOUBLE_CLICK && (at.0 - x).abs() <= DOUBLE_CLICK_SLOP && (at.1 - y).abs() <= DOUBLE_CLICK_SLOP => n % 3 + 1,
                    _ => 1,
                };
                self.last_press = Some((now, (x, y), clicks));
                self.aim = Some(Aim::Press { x, clicks, extend: modifiers.shift });
                // Held from now on: a move before the next draw is a drag from it.
                self.dragging = Some(Grab { from: self.caret, to: self.caret, clicks });
                self.held = true;
            }
            Input::Press { pressed: false, .. } | Input::PointerLeft => {
                // A drag's last move not yet drawn is drawn first, and that is its end.
                if !matches!(self.aim, Some(Aim::Drag { .. })) {
                    self.dragging = None;
                }
                self.held = false;
            }
            _ => {}
        }
    }

    /// Whether the pointer's button, pressed in this field, is still held: a drag choosing.
    pub fn is_dragging(&self) -> bool {
        self.dragging.is_some()
    }

    /// Hand the field input: keys type, and the pointer puts the caret and chooses (see
    /// [`Field::pointer_input`]). What typing came to, if it came to anything.
    pub fn input(&mut self, input: &Input) -> Option<Typed> {
        let Input::Key { key, text, modifiers, .. } = input else {
            self.pointer_input(input);
            return None;
        };
        if !self.focused {
            return None;
        }
        let length = self.text.chars().count();
        // Control, or Command on a Mac: the clipboard's keys, and choosing everything.
        let command = modifiers.control || modifiers.logo;
        if command {
            match key {
                Key::Char('a') => {
                    self.select_all();
                    return None;
                }
                Key::Char('c') => {
                    if let Some(chosen) = self.selected_text().filter(|_| !self.secret) {
                        crate::clipboard::copy(chosen);
                    }
                    return None;
                }
                Key::Char('x') => {
                    if let Some(chosen) = self.selected_text().filter(|_| !self.secret) {
                        crate::clipboard::copy(chosen);
                        self.delete_selection();
                        return Some(Typed::Changed);
                    }
                    return None;
                }
                Key::Char('v') => {
                    if let Some(pasted) = crate::clipboard::text() {
                        self.insert(&pasted);
                        return Some(Typed::Changed);
                    }
                    return None;
                }
                _ => {}
            }
        }
        // Moving the caret with Shift held chooses; without, lets go of what was chosen (to
        // its near end, for Left and Right).
        let moving = matches!(key, Key::Left | Key::Right | Key::Home | Key::End);
        if moving {
            if modifiers.shift {
                self.anchor.get_or_insert(self.caret);
            } else if let Some((from, to)) = self.selection() {
                self.anchor = None;
                match key {
                    Key::Left => {
                        self.caret = from;
                        return None;
                    }
                    Key::Right => {
                        self.caret = to;
                        return None;
                    }
                    _ => {}
                }
            } else {
                self.anchor = None;
            }
        } else if matches!(key, Key::Backspace | Key::Delete) && self.delete_selection() {
            return Some(Typed::Changed);
        }
        match key {
            Key::Enter => return Some(Typed::Entered),
            Key::Left => self.caret = self.caret.saturating_sub(1),
            Key::Right => self.caret = (self.caret + 1).min(length),
            Key::Home => self.caret = 0,
            Key::End => self.caret = length,
            Key::Backspace if self.caret > 0 => {
                let (from, to) = (self.byte_at(self.caret - 1), self.byte_at(self.caret));
                self.text.replace_range(from..to, "");
                self.caret -= 1;
                return Some(Typed::Changed);
            }
            Key::Delete if self.caret < length => {
                let (from, to) = (self.byte_at(self.caret), self.byte_at(self.caret + 1));
                self.text.replace_range(from..to, "");
                return Some(Typed::Changed);
            }
            // Control-U, as in a terminal: start again. Not Command-U on a Mac (Command comes
            // as Control with `command`), which is not "clear" there.
            Key::Char('u') if modifiers.control && !modifiers.command => {
                self.text.clear();
                self.caret = 0;
                return Some(Typed::Changed);
            }
            _ => {
                // What a key with Control or Command types is a command, not text.
                if let Some(typed) = text.as_deref().filter(|_| !command) {
                    self.insert(typed);
                    return Some(Typed::Changed);
                }
            }
        }
        None
    }
}

// --- button ---

/// Something to press.
#[derive(Debug, Default)]
pub struct Button {
    pub focused: bool,
    area: Rect,
    hover: bool,
    pressed: bool,
}

impl Button {
    pub const HEIGHT: f32 = theme::ROW_HEIGHT;

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.area.contains(x, y)
    }

    /// The width a button with this label would like.
    pub fn width(p: &mut Painter, gl: &Gl, label: &str) -> f32 {
        p.measure(gl, label, theme::ROW_EM * 0.86, 8192.0 / p.scale()).0 + theme::ROW_INSET * 2.0
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, rect: Rect, label: &str) {
        self.area = rect;
        let ground = if self.pressed && self.hover {
            theme::selected()
        } else if self.focused {
            theme::selected()
        } else if self.hover {
            theme::wash(0.11)
        } else {
            theme::raised()
        };
        p.fill(gl, rect, theme::ROW_RADIUS, ground);
        let ink = if self.focused || self.hover { theme::ink() } else { theme::ink_row() };
        p.label(gl, label, rect, theme::ROW_EM * 0.86, ink, 0.5);
    }

    /// The width a button with this icon and label would like; an icon alone is a square.
    pub fn icon_width(p: &mut Painter, gl: &Gl, label: &str) -> f32 {
        if label.is_empty() {
            Self::HEIGHT
        } else {
            Self::width(p, gl, label) + theme::ROW_EM * 0.9 + 12.0
        }
    }

    /// A button with an icon, and a label after it unless `label` is empty -- then it is round.
    pub fn draw_icon(&mut self, p: &mut Painter, gl: &Gl, rect: Rect, icon: super::Icon, label: &str) {
        self.area = rect;
        let lit = self.focused || (self.pressed && self.hover);
        let ground = if lit {
            theme::selected()
        } else if self.hover {
            theme::wash(0.11)
        } else {
            theme::raised()
        };
        let radius = if label.is_empty() { rect.h.min(rect.w) * 0.5 } else { theme::ROW_RADIUS };
        if self.hover || lit {
            p.glow(gl, rect, radius, theme::glow(0.18), 12.0);
        }
        p.fill(gl, rect, radius, ground);
        let ink = if self.focused || self.hover { theme::ink() } else { theme::ink_row() };
        let side = (theme::ROW_EM * 0.9).min(rect.h * 0.6);
        if label.is_empty() {
            super::icons::draw(p, gl, icon, Rect::new(rect.x + (rect.w - side) * 0.5, rect.y + (rect.h - side) * 0.5, side, side), ink);
        } else {
            let mark = Rect::new(rect.x + theme::ROW_INSET * 0.8, rect.y + (rect.h - side) * 0.5, side, side);
            super::icons::draw(p, gl, icon, mark, ink);
            let text = Rect { x: mark.right() + 12.0, w: rect.right() - mark.right() - 12.0 - theme::ROW_INSET * 0.8, ..rect };
            p.label(gl, label, text, theme::ROW_EM * 0.86, ink, 0.0);
        }
    }

    /// Returns whether it was pressed.
    pub fn input(&mut self, input: &Input) -> bool {
        match input {
            Input::Pointer { x, y } => self.hover = self.area.contains(*x, *y),
            Input::PointerLeft => {
                self.hover = false;
                self.pressed = false;
            }
            Input::Press { pressed: true, .. } => self.pressed = self.hover,
            Input::Press { pressed: false, .. } => return std::mem::take(&mut self.pressed) && self.hover,
            Input::Key { key: Key::Enter | Key::Space, .. } if self.focused => return true,
            _ => {}
        }
        false
    }
}

// --- a question ---

/// A question over a window, answered yes or no: "Close this tab? sleep is still running in
/// it." The pane floats over everything, the window dimmed behind it; while it is up the
/// application gives it all of the input. Return answers with the button lit (the yes, to
/// begin with, or the no for [`Confirm::careful`]), Escape is no, the arrows and Tab go between
/// the two, the pointer presses either.
#[derive(Debug)]
pub struct Confirm {
    pub text: String,
    yes_label: String,
    no_label: String,
    yes: Button,
    no: Button,
    on_yes: bool,
}

impl Confirm {
    pub fn new(text: impl Into<String>, yes: impl Into<String>, no: impl Into<String>) -> Confirm {
        Confirm { text: text.into(), yes_label: yes.into(), no_label: no.into(), yes: Button::default(), no: Button::default(), on_yes: true }
    }

    /// A question whose yes destroys or ends something -- delete, empty, forget, remove, quit or
    /// close with work going -- which starts on the no: a Return typed on from what came before
    /// it loses nothing.
    pub fn careful(text: impl Into<String>, yes: impl Into<String>, no: impl Into<String>) -> Confirm {
        Confirm { on_yes: false, ..Confirm::new(text, yes, no) }
    }

    /// Drawn over `all`, the whole window.
    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, all: Rect) {
        let em = theme::ROW_EM * 0.8;
        let w = (all.w - 48.0).min(820.0);
        let (_, text_h) = p.measure_left(gl, &self.text, em, w - 56.0);
        let h = 28.0 + text_h + 28.0 + Button::HEIGHT + 28.0;
        let pane = Rect::new(all.x + (all.w - w) * 0.5, all.y + (all.h * 0.36 - h * 0.5).max(24.0), w, h);
        super::look::scrim(p, gl, all);
        p.fill(gl, pane, 24.0, theme::ground());
        super::look::glass(p, gl, pane, 24.0, 1.0);
        let mut inner = pane.inset(28.0);
        p.text_left(gl, &self.text, inner.x, inner.y, em, inner.w, theme::ink());
        let mut buttons = inner.take_bottom(Button::HEIGHT);
        let bw = |p: &mut Painter, label: &str| Button::width(p, gl, label).max(180.0);
        let (yes_w, no_w) = (bw(p, &self.yes_label), bw(p, &self.no_label));
        self.yes.focused = self.on_yes;
        self.no.focused = !self.on_yes;
        self.yes.draw(p, gl, Rect { x: buttons.right() - yes_w, w: yes_w, ..buttons }, &self.yes_label);
        buttons.w -= yes_w + 12.0;
        self.no.draw(p, gl, Rect { x: buttons.right() - no_w, w: no_w, ..buttons }, &self.no_label);
    }

    /// `Some(true)` for yes, `Some(false)` for no, `None` while it is still being asked.
    pub fn input(&mut self, input: &Input) -> Option<bool> {
        if let Input::Key { key, .. } = input {
            match key {
                Key::Escape => return Some(false),
                // Return only: a space typed on into a terminal as the question came up is no answer.
                Key::Enter => return Some(self.on_yes),
                // Two buttons: either way, and Tab or Shift-Tab, is the other one.
                Key::Left | Key::Right | Key::Up | Key::Down | Key::Tab => self.on_yes = !self.on_yes,
                _ => {}
            }
            return None;
        }
        if self.yes.input(input) {
            return Some(true);
        }
        if self.no.input(input) {
            return Some(false);
        }
        None
    }
}

// --- a menu ---

/// What a [`Menu`] said of an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuSaid {
    /// This row was chosen: the menu has done its work.
    Chose(usize),
    /// Escape, or a press outside it: closed, and nothing else is to be done.
    Closed,
}

/// A menu at a point of the window: a [`List`] on a pane of glass, as wide as its widest row
/// (320 to 620), kept 12 inside the window and no taller than it (a long one scrolls). While it
/// is open the application gives it the input first; arrows and Enter work it, Escape or a
/// press outside close it.
#[derive(Debug, Default)]
pub struct Menu {
    /// Where it opens: its top left, moved in to stay in the window.
    pub at: (f32, f32),
    pub list: List,
    /// Where it was last drawn, for a press to be inside or out.
    area: Rect,
}

impl Menu {
    pub const NARROWEST: f32 = 320.0;
    pub const WIDEST: f32 = 620.0;

    pub fn new(at: (f32, f32)) -> Menu {
        let mut list = List::default();
        list.focused = true;
        Menu { at, list, area: Rect::default() }
    }

    /// Where it is drawn with `rows`, in `within` (the window).
    pub fn rect(&self, p: &mut Painter, gl: &Gl, rows: &[Row], within: Rect) -> Rect {
        let max = 8192.0 / p.scale();
        let per_row = theme::ROW_HEIGHT + theme::ROW_GAP;
        let h = (rows.len() as f32 * per_row + 16.0).min(within.h - 24.0).max(0.0);
        let scrolls = rows.len() as f32 * per_row - theme::ROW_GAP > h - 20.0;
        let mut widest: f32 = 0.0;
        for row in rows {
            let mut w = if row.heading { p.measure(gl, &row.label, theme::FOOTER_EM, max).0 } else { p.measure(gl, &row.label, theme::ROW_EM, max).0 };
            if !row.heading {
                if let Some(trailing) = row.trailing.as_deref().filter(|t| !t.is_empty()) {
                    w += p.measure(gl, trailing, theme::ROW_EM * 0.8, max).0 + 24.0;
                }
                if row.picture.is_some() {
                    w += theme::ROW_EM * 1.15 + 12.0;
                } else if row.icon.is_some() {
                    w += theme::ROW_EM * 0.95 + 18.0;
                }
            }
            widest = widest.max(w);
        }
        // The row's insets, the list's inset in the pane, and the scrollbar's room.
        let w = (widest + theme::ROW_INSET * 2.0 + 20.0 + if scrolls { theme::SCROLL_WIDTH + 12.0 } else { 0.0 }).ceil().clamp(Self::NARROWEST, Self::WIDEST).min(within.w - 24.0);
        let x = self.at.0.min(within.right() - w - 12.0).max(within.x + 12.0);
        let y = self.at.1.min(within.bottom() - h - 12.0).max(within.y + 12.0);
        Rect::new(x, y, w, h)
    }

    /// Drawn over everything, in `within` (the window).
    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, rows: &[Row], within: Rect) {
        let rect = self.rect(p, gl, rows, within);
        self.area = rect;
        // On the ground's own colour under the glass: a menu over pictures is read, not seen
        // through.
        p.glow(gl, rect, 20.0, [0.0, 0.0, 0.0, 0.45], 24.0);
        p.fill(gl, rect, 20.0, theme::ground());
        super::look::glass(p, gl, rect, 20.0, 1.0);
        self.list.draw(p, gl, rect.inset(10.0), rows);
    }

    /// Whether `(x, y)` is on it, as last drawn.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.area.contains(x, y)
    }

    /// The pointer is at `pointer` (the application's own idea of it, as presses carry none).
    pub fn input(&mut self, input: &Input, pointer: (f32, f32)) -> Option<MenuSaid> {
        match input {
            Input::Key { key: Key::Escape, .. } => return Some(MenuSaid::Closed),
            Input::Press { pressed: true, .. } if !self.area.contains(pointer.0, pointer.1) => return Some(MenuSaid::Closed),
            _ => {}
        }
        self.list.input(input).map(MenuSaid::Chose)
    }
}

#[cfg(test)]
mod menu_tests {
    use super::*;
    use crate::Modifiers;

    #[test]
    fn escape_or_a_press_outside_closes_and_enter_chooses() {
        let key = |key: Key| Input::Key { key, text: None, modifiers: Modifiers::default(), letter: None };
        let press = Input::Press { pressed: true, modifiers: Modifiers::default() };
        let mut menu = Menu::new((100.0, 100.0));
        menu.area = Rect::new(100.0, 100.0, 400.0, 200.0);
        assert_eq!(menu.input(&key(Key::Escape), (0.0, 0.0)), Some(MenuSaid::Closed));
        assert_eq!(menu.input(&press, (50.0, 50.0)), Some(MenuSaid::Closed));
        assert_eq!(menu.input(&press, (150.0, 150.0)), None);
        assert!(menu.list.focused);
    }
}

#[cfg(test)]
mod confirm_tests {
    use super::*;
    use crate::Modifiers;

    fn key(key: Key) -> Input {
        Input::Key { key, text: None, modifiers: Modifiers::default(), letter: None }
    }

    #[test]
    fn a_careful_question_starts_on_the_no() {
        let mut c = Confirm::careful("Delete it? It cannot be had back.", "Delete", "Cancel");
        assert_eq!(c.input(&key(Key::Enter)), Some(false));
        let mut c = Confirm::careful("Delete it?", "Delete", "Cancel");
        assert_eq!(c.input(&key(Key::Left)), None);
        assert_eq!(c.input(&key(Key::Enter)), Some(true));
        assert_eq!(Confirm::careful("Delete it?", "Delete", "Cancel").input(&key(Key::Escape)), Some(false));
    }

    #[test]
    fn return_is_yes_escape_is_no() {
        let mut c = Confirm::new("Close this tab?", "Close", "Cancel");
        assert_eq!(c.input(&key(Key::Char('x'))), None);
        assert_eq!(c.input(&key(Key::Space)), None);
        assert_eq!(c.input(&key(Key::Enter)), Some(true));
        assert_eq!(c.input(&key(Key::Escape)), Some(false));
        // Over to the no: Return is no.
        assert_eq!(c.input(&key(Key::Left)), None);
        assert_eq!(c.input(&key(Key::Enter)), Some(false));
        assert_eq!(c.input(&key(Key::Tab)), None);
        assert_eq!(c.input(&key(Key::Enter)), Some(true));
    }
}

#[cfg(test)]
mod field_tests {
    use super::*;
    use crate::Modifiers;

    /// A field of `text` at the left of the window, each character 10 wide.
    fn field(text: &str) -> Field {
        let mut f = Field::default();
        f.set(text);
        f.focused = true;
        f.area = Rect::new(0.0, 0.0, 600.0, 40.0);
        f
    }

    /// The pointer's doing put at its character, as a draw does.
    fn drawn(f: &mut Field) {
        if let Some(aim) = f.aim.take() {
            let x = match aim {
                Aim::Press { x, .. } | Aim::Drag { x } => x,
            };
            let count = f.text.chars().count();
            let at = boundary_at(x, count, |i| i as f32 * 10.0);
            f.point(aim, at);
            if !f.held {
                f.dragging = None;
            }
        }
    }

    #[test]
    fn a_click_or_a_drag_that_comes_whole_before_a_draw() {
        // Pressed and let go of before the field was drawn: a click, and a move after it is
        // no drag.
        let mut f = field("holiday.jpeg");
        f.input(&Input::Pointer { x: 31.0, y: 20.0 });
        f.input(&Input::Press { pressed: true, modifiers: Modifiers::default() });
        release(&mut f);
        drawn(&mut f);
        assert_eq!((f.caret, f.selection()), (3, None));
        moved(&mut f, 90.0);
        assert_eq!(f.selection(), None, "nothing held, nothing chosen");
        // Pressed, moved and let go of, all before a draw after the press's: the drag's end
        // is drawn.
        press(&mut f, 0.0, false);
        f.input(&Input::Pointer { x: 70.0, y: 20.0 });
        release(&mut f);
        drawn(&mut f);
        assert_eq!(f.selected_text().as_deref(), Some("holiday"));
        assert!(!f.is_dragging());
    }

    fn press(f: &mut Field, x: f32, shift: bool) {
        f.input(&Input::Pointer { x, y: 20.0 });
        f.input(&Input::Press { pressed: true, modifiers: Modifiers { shift, ..Modifiers::default() } });
        drawn(f);
    }

    fn release(f: &mut Field) {
        f.input(&Input::Press { pressed: false, modifiers: Modifiers::default() });
    }

    fn moved(f: &mut Field, x: f32) {
        f.input(&Input::Pointer { x, y: 20.0 });
        drawn(f);
    }

    #[test]
    fn the_nearest_boundary_is_found_by_halving() {
        let mono = |i: usize| i as f32 * 10.0;
        assert_eq!(boundary_at(-5.0, 12, mono), 0);
        assert_eq!(boundary_at(14.0, 12, mono), 1);
        assert_eq!(boundary_at(16.0, 12, mono), 2);
        assert_eq!(boundary_at(119.0, 12, mono), 12);
        assert_eq!(boundary_at(500.0, 12, mono), 12);
        assert_eq!(boundary_at(5.0, 0, mono), 0);
        let mut asked = 0;
        boundary_at(333.0, 1000, |i| {
            asked += 1;
            i as f32 * 10.0
        });
        assert!(asked < 16, "a long line is measured a few times, not a thousand ({asked})");
    }

    #[test]
    fn words_are_runs_of_one_kind() {
        assert_eq!(word_at("holiday.jpeg", 3), (0, 7));
        assert_eq!(word_at("holiday.jpeg", 7), (7, 8), "the dot alone");
        assert_eq!(word_at("holiday.jpeg", 9), (8, 12));
        assert_eq!(word_at("holiday.jpeg", 12), (8, 12), "at the end, the last word");
        assert_eq!(word_at("my  film_2.mkv", 3), (2, 4), "the spaces between");
        assert_eq!(word_at("my  film_2.mkv", 6), (4, 10), "an underscore is part of a word");
        assert_eq!(word_at("", 0), (0, 0));
        assert_eq!(word_at("café crème", 2), (0, 4), "in characters");
    }

    #[test]
    fn a_click_puts_the_caret_and_clears_the_choice() {
        let mut f = field("holiday.jpeg");
        f.select(0, 7);
        press(&mut f, 31.0, false);
        release(&mut f);
        assert_eq!((f.caret, f.selection()), (3, None));
        f.input(&Input::Key { key: Key::Char('x'), text: Some("X".into()), modifiers: Modifiers::default(), letter: Some('x') });
        assert_eq!(f.text, "holXiday.jpeg", "typed where it was put");
    }

    #[test]
    fn a_field_s_chords_on_any_layout_and_command_u_is_not_clear() {
        let chord = |c: char, code: u32, modifiers: Modifiers| {
            let event = crate::Event::Key { window: crate::WindowId(1), key: Key::Char(c), code, pressed: true, repeat: false, text: Some(c.to_string()), modifiers };
            Input::of(&event, 1.0).unwrap()
        };
        let control = Modifiers { control: true, ..Default::default() };
        let command = Modifiers { control: true, command: true, ..Default::default() };
        // Russian: Control-Ф (on A's key, 30) chooses everything, as Control-A does.
        let mut f = field("holiday.jpeg");
        f.input(&chord('\u{444}', 30, control));
        assert_eq!(f.selection(), Some((0, 12)));
        // Command-U on a Mac leaves the text; Control-U clears it.
        let mut f = field("holiday.jpeg");
        f.input(&chord('u', 22, command));
        assert_eq!(f.text, "holiday.jpeg");
        f.input(&chord('u', 22, control));
        assert_eq!(f.text, "");
        // Typed without a chord, the layout's letter goes as it is.
        let mut f = field("");
        f.input(&chord('\u{444}', 30, Modifiers::default()));
        assert_eq!(f.text, "\u{444}");
    }

    #[test]
    fn a_drag_chooses_either_way() {
        let mut f = field("holiday.jpeg");
        press(&mut f, 20.0, false);
        moved(&mut f, 70.0);
        assert_eq!(f.selected_text().as_deref(), Some("liday"));
        moved(&mut f, 0.0);
        assert_eq!(f.selected_text().as_deref(), Some("ho"), "back past where it began");
        release(&mut f);
        moved(&mut f, 110.0);
        assert_eq!(f.selected_text().as_deref(), Some("ho"), "let go of, a move chooses nothing more");
        assert!(!f.is_dragging());
    }

    #[test]
    fn two_clicks_a_word_three_everything() {
        let mut f = field("holiday.jpeg");
        press(&mut f, 92.0, false);
        release(&mut f);
        press(&mut f, 92.0, false);
        assert_eq!(f.selected_text().as_deref(), Some("jpeg"));
        // Dragged on from a double click: word by word.
        moved(&mut f, 22.0);
        assert_eq!(f.selected_text().as_deref(), Some("holiday.jpeg"));
        release(&mut f);
        press(&mut f, 92.0, false);
        assert_eq!(f.selected_text().as_deref(), Some("holiday.jpeg"), "the third: all of it");
        release(&mut f);
        // Far from the last: a first click again.
        press(&mut f, 30.0, false);
        assert_eq!((f.caret, f.selection()), (3, None));
    }

    #[test]
    fn shift_click_extends_from_the_caret_or_the_choice() {
        let mut f = field("holiday.jpeg");
        press(&mut f, 20.0, false);
        release(&mut f);
        press(&mut f, 70.0, true);
        assert_eq!(f.selected_text().as_deref(), Some("liday"));
        release(&mut f);
        press(&mut f, 100.0, true);
        assert_eq!(f.selected_text().as_deref(), Some("liday.jp"), "further, from the same end");
        release(&mut f);
        press(&mut f, 0.0, true);
        assert_eq!(f.selected_text().as_deref(), Some("holiday.jp"), "before it: from its other end");
    }

    #[test]
    fn an_unfocused_field_or_a_press_outside_does_nothing() {
        let mut f = field("holiday.jpeg");
        f.select(0, 7);
        f.focused = false;
        press(&mut f, 31.0, false);
        assert_eq!(f.selected_text().as_deref(), Some("holiday"));
        f.focused = true;
        f.input(&Input::Pointer { x: 800.0, y: 20.0 });
        f.input(&Input::Press { pressed: true, modifiers: Modifiers::default() });
        drawn(&mut f);
        assert_eq!(f.selected_text().as_deref(), Some("holiday"), "outside it: the application's to say");
    }
}
