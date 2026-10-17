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
}

impl Row {
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
                p.label(gl, &row.label, low, theme::FOOTER_EM, theme::INK_HINT, 0.0);
                continue;
            }
            let selected = index == self.cursor;
            if selected {
                let wash = if self.focused { theme::SELECTED } else { theme::SELECTED_AWAY };
                p.fill(gl, at, theme::ROW_RADIUS, wash);
            } else if self.hover == Some(index) {
                p.fill(gl, at, theme::ROW_RADIUS, theme::HOVER);
            }
            let mut label = text;
            if let Some(trailing) = row.trailing.as_deref() {
                let taken = p.label(gl, trailing, text, theme::ROW_EM * 0.8, theme::INK_TRAILING, 1.0);
                label.w = (label.w - taken - 24.0).max(0.0);
            }
            let ink = if selected { theme::INK } else { theme::INK_ROW };
            if let Some(icon) = row.icon {
                let side = theme::ROW_EM * 0.95;
                let mark = Rect::new(label.x, at.y + (at.h - side) * 0.5, side, side);
                super::icons::draw(p, gl, icon, mark, if selected { theme::MARK } else { theme::INK_DETAIL });
                label.x += side + 18.0;
                label.w = (label.w - side - 18.0).max(0.0);
            }
            p.label(gl, &row.label, label, theme::ROW_EM, ink, 0.0);
        }
        if scrolls && visible > 0 {
            let track = Rect::new(rect.right() - theme::SCROLL_WIDTH, rect.y, theme::SCROLL_WIDTH, visible as f32 * per_row - theme::ROW_GAP);
            p.fill(gl, track, theme::SCROLL_WIDTH * 0.5, theme::SCROLL_TRACK);
            let thumb = (track.h * visible as f32 / rows.len() as f32).max(theme::SCROLL_WIDTH * 4.0).min(track.h);
            let progress = self.first as f32 / (rows.len() - visible) as f32;
            let thumb = Rect { y: track.y + (track.h - thumb) * progress, h: thumb, ..track };
            p.fill(gl, thumb, theme::SCROLL_WIDTH * 0.5, theme::SCROLL_THUMB);
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
            Input::Press { pressed: true } => {
                self.pressed = self.hover;
                if let Some(row) = self.hover {
                    self.cursor = row;
                }
            }
            // Chosen on letting go over the row that was pressed, like a button anywhere.
            Input::Press { pressed: false } => {
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
        p.glow(gl, frame, theme::ROW_RADIUS, [0.30, 0.62, 1.0, 0.42], 22.0);
        p.fill(gl, frame.inset(-3.0), theme::ROW_RADIUS + 3.0, theme::MARK);
    } else if selected {
        p.fill(gl, frame.inset(-2.0), theme::ROW_RADIUS + 2.0, [1.0, 1.0, 1.0, 0.28]);
    } else if hover {
        p.fill(gl, frame.inset(-2.0), theme::ROW_RADIUS + 2.0, [1.0, 1.0, 1.0, 0.16]);
    }
    match tile.picture {
        Some(texture) => p.cover(gl, texture, frame, theme::ROW_RADIUS, theme::INK),
        None => {
            // Its name, large, where its picture will be: a tile that is still loading, or
            // has no picture, says what it is rather than being a grey box among grey boxes.
            p.fill(gl, frame, theme::ROW_RADIUS, theme::GROUND);
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
        p.fill(gl, Rect { w: bar.w * progress.min(1.0), ..bar }, 2.5, theme::MARK);
    }
    let name = Rect::new(cell.x + 4.0, frame.bottom() + 10.0, cell.w - 8.0, CAPTION * 1.4);
    p.label(gl, &tile.title, name, CAPTION, if selected { theme::INK } else { theme::INK_ROW }, 0.0);
    if let Some(detail) = tile.detail.as_deref() {
        let under = Rect { y: name.bottom(), h: CAPTION_DETAIL * 1.4, ..name };
        p.label(gl, detail, under, CAPTION_DETAIL, theme::INK_HINT, 0.0);
    }
}

impl Grid {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.area.contains(x, y)
    }

    /// Which tiles are on show, or nearly: the ones whose pictures are worth fetching.
    pub fn in_view(&self) -> std::ops::Range<usize> {
        if self.columns == 0 || self.row_height <= 0.0 {
            return 0..self.total.min(24);
        }
        let first = (self.scroll / self.row_height).floor().max(0.0) as usize;
        let rows = (self.area.h / self.row_height).ceil() as usize + 2;
        (first * self.columns).min(self.total)..((first + rows) * self.columns).min(self.total)
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
            p.fill(gl, track, theme::SCROLL_WIDTH * 0.5, theme::SCROLL_TRACK);
            let thumb = (track.h * rect.h / content).max(theme::SCROLL_WIDTH * 4.0);
            let progress = self.scroll / (content - rect.h);
            p.fill(gl, Rect { y: track.y + (track.h - thumb) * progress, h: thumb, ..track }, theme::SCROLL_WIDTH * 0.5, theme::SCROLL_THUMB);
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
            Input::Press { pressed: true } => {
                self.pressed = self.hover;
                if let Some(tile) = self.hover {
                    self.cursor = tile;
                }
            }
            Input::Press { pressed: false } => {
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

    /// Which tiles are on show, or nearly.
    pub fn in_view(&self) -> std::ops::Range<usize> {
        let (first, last) = self.placed.iter().fold((usize::MAX, 0), |(a, b), (i, _)| (a.min(*i), b.max(*i + 1)));
        if first == usize::MAX {
            0..self.total.min(8)
        } else {
            first..(last + 2).min(self.total)
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
            Input::Press { pressed: true } => {
                self.pressed = self.hover;
                if let Some(tile) = self.hover {
                    self.cursor = tile;
                }
            }
            Input::Press { pressed: false } => {
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
                let wash = if self.focused { theme::SELECTED } else { theme::SELECTED_AWAY };
                p.fill(gl, at, at.h * 0.5, wash);
            } else if self.hover == Some(index) {
                p.fill(gl, at, at.h * 0.5, theme::HOVER);
            }
            let ink = if active { theme::INK } else { theme::INK_ROW };
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
            Input::Press { pressed: true } => {
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
    /// Where the caret is, in characters from the start.
    caret: usize,
    /// The other end of the selection, when there is one: what is between it and the caret.
    anchor: Option<usize>,
    area: Rect,
}

/// What typing into a field came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Typed {
    Changed,
    /// Enter: the wearer has finished.
    Entered,
}

impl Field {
    pub const HEIGHT: f32 = theme::ROW_HEIGHT;

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.area.contains(x, y)
    }

    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.caret = self.text.chars().count();
        self.anchor = None;
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
        self.caret = self.caret.min(self.text.chars().count());
        p.fill(gl, rect, theme::ROW_RADIUS, theme::RAISED);
        if self.focused {
            p.fill(gl, Rect { y: rect.bottom() - 3.0, h: 3.0, ..rect.inset(theme::ROW_RADIUS * 0.5) }, 1.5, theme::MARK);
        }
        let text = Rect { x: rect.x + theme::ROW_INSET, w: rect.w - theme::ROW_INSET * 2.0, ..rect };
        let wide = 8192.0 / p.scale();
        if self.text.is_empty() {
            p.label(gl, &self.hint, text, theme::ROW_EM, theme::INK_HINT, 0.0);
        }
        // What is before the caret decides where the caret is; a field too full to show it
        // all slides left so that the caret is never out of sight.
        let shown: String = if self.secret { "\u{2022}".repeat(self.text.chars().count()) } else { self.text.clone() };
        let before: String = shown.chars().take(self.caret).collect();
        // A trailing space has no ink and so no width as far as the text setter is concerned:
        // measured with something after it, and that something taken off again.
        let stop = p.measure(gl, "|", theme::ROW_EM, wide).0;
        let caret = if before.is_empty() { 0.0 } else { p.measure(gl, &format!("{before}|"), theme::ROW_EM, wide).0 - stop };
        let slide = (caret - text.w + 6.0).max(0.0);
        p.clip(gl, text);
        if let Some((from, to)) = self.selection() {
            let width = |p: &mut Painter, chars: usize| {
                let part: String = shown.chars().take(chars).collect();
                if part.is_empty() { 0.0 } else { p.measure(gl, &format!("{part}|"), theme::ROW_EM, wide).0 - stop }
            };
            let (a, b) = (width(p, from), width(p, to));
            let wash = Rect::new(text.x + a - slide, rect.y + rect.h * 0.18, (b - a).max(2.0), rect.h * 0.64);
            p.fill(gl, wash, 2.0, if self.focused { [0.42, 0.68, 1.0, 0.42] } else { theme::SELECTED_AWAY });
        }
        if !self.text.is_empty() {
            let (_, h) = p.measure(gl, &shown, theme::ROW_EM, wide);
            p.text(gl, &shown, text.x - slide, text.y + (text.h - h) * 0.5, theme::ROW_EM, wide, theme::INK);
        }
        p.unclip(gl);
        if self.focused {
            let bar = Rect::new(text.x + caret - slide + 1.0, rect.y + rect.h * 0.22, 2.0, rect.h * 0.56);
            p.fill(gl, bar, 1.0, theme::MARK);
        }
    }

    fn byte_at(&self, chars: usize) -> usize {
        self.text.char_indices().nth(chars).map_or(self.text.len(), |(at, _)| at)
    }

    pub fn input(&mut self, input: &Input) -> Option<Typed> {
        let Input::Key { key, text, modifiers } = input else { return None };
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
            // Control-U, as in a terminal: start again.
            Key::Char('u') if modifiers.control => {
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
            theme::SELECTED
        } else if self.focused {
            theme::SELECTED
        } else if self.hover {
            [1.0, 1.0, 1.0, 0.11]
        } else {
            theme::RAISED
        };
        p.fill(gl, rect, theme::ROW_RADIUS, ground);
        let ink = if self.focused || self.hover { theme::INK } else { theme::INK_ROW };
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
            theme::SELECTED
        } else if self.hover {
            [1.0, 1.0, 1.0, 0.11]
        } else {
            theme::RAISED
        };
        let radius = if label.is_empty() { rect.h.min(rect.w) * 0.5 } else { theme::ROW_RADIUS };
        if self.hover || lit {
            p.glow(gl, rect, radius, [0.30, 0.62, 1.0, 0.18], 12.0);
        }
        p.fill(gl, rect, radius, ground);
        let ink = if self.focused || self.hover { theme::INK } else { theme::INK_ROW };
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
            Input::Press { pressed: true } => self.pressed = self.hover,
            Input::Press { pressed: false } => return std::mem::take(&mut self.pressed) && self.hover,
            Input::Key { key: Key::Enter | Key::Space, .. } if self.focused => return true,
            _ => {}
        }
        false
    }
}
