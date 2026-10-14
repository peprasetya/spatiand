//! The room's own look, as widgets.
//!
//! The launcher, the settings and every card in the room are drawn one way: a dark ground with
//! a faint rim, rows a degree tall, a blue wash behind what is selected, text set by
//! `spatiand-render`. This is that, for applications -- the same measurements
//! (`spatiand_render::panel`), the same colours, the same text -- so an application's list is
//! the room's list and not a likeness of it.
//!
//! Everything is laid out in *logical pixels*, which a [`Painter`] scales to the picture: at a
//! scale of one, a row's label is the 36 pixels the room's own menus use, a little under a
//! degree in the glasses. Widgets keep their own small state (a cursor, how far they are
//! scrolled), are drawn into a rectangle each frame, and are handed what the wearer did as
//! [`Input`].

pub mod look;
mod paint;
mod widgets;

pub use paint::{Painter, Texture};
pub use widgets::{Button, Field, Grid, List, Row, Shape, Strip, Tabs, Tile, Typed};

use crate::{Event, Key, Modifiers};

/// A rectangle in logical pixels, origin at the window's top left, y downwards.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    /// Smaller by `by` on every side.
    pub fn inset(&self, by: f32) -> Rect {
        Rect { x: self.x + by, y: self.y + by, w: (self.w - by * 2.0).max(0.0), h: (self.h - by * 2.0).max(0.0) }
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    /// Cut `height` off the top, returning it and leaving the rest.
    pub fn take_top(&mut self, height: f32) -> Rect {
        let height = height.min(self.h);
        let top = Rect { h: height, ..*self };
        self.y += height;
        self.h -= height;
        top
    }

    pub fn take_bottom(&mut self, height: f32) -> Rect {
        let height = height.min(self.h);
        self.h -= height;
        Rect { y: self.y + self.h, h: height, ..*self }
    }

    pub fn take_left(&mut self, width: f32) -> Rect {
        let width = width.min(self.w);
        let left = Rect { w: width, ..*self };
        self.x += width;
        self.w -= width;
        left
    }
}

/// A colour: red, green, blue and alpha, each 0 to 1, not multiplied by alpha.
pub type Color = [f32; 4];

/// The room's colours and sizes. The colours are `crates/spatiand/src/scene.rs`'s own; the
/// sizes are `spatiand_render::panel`'s.
pub mod theme {
    use super::Color;
    pub use spatiand_render::panel::{CARD_RADIUS, DETAIL_EM, FOOTER_EM, ROW_EM, ROW_HEIGHT, ROW_INSET, ROW_RADIUS, TITLE_EM};

    /// What a window is cleared to: the card's ground, opaque, since a window has no room
    /// behind it to be seen through to.
    pub const GROUND: Color = [0.043, 0.05, 0.072, 1.0];
    /// A surface raised off the ground: a field, a button, a tile with no picture yet.
    pub const RAISED: Color = [1.0, 1.0, 1.0, 0.06];
    pub const RIM: Color = [0.55, 0.70, 1.0, 0.16];
    pub const SELECTED: Color = [0.42, 0.68, 1.0, 0.20];
    /// Selected, in a widget the keys are not going to.
    pub const SELECTED_AWAY: Color = [1.0, 1.0, 1.0, 0.08];
    pub const HOVER: Color = [1.0, 1.0, 1.0, 0.05];
    pub const MARK: Color = [0.45, 0.72, 1.0, 1.0];
    pub const INK: Color = [1.0, 1.0, 1.0, 1.0];
    pub const INK_ROW: Color = [0.78, 0.82, 0.90, 1.0];
    pub const INK_TRAILING: Color = [0.50, 0.70, 1.0, 1.0];
    pub const INK_DETAIL: Color = [0.70, 0.76, 0.87, 1.0];
    pub const INK_HINT: Color = [0.50, 0.55, 0.66, 1.0];
    pub const SEPARATOR: Color = [1.0, 1.0, 1.0, 0.10];
    pub const SCROLL_TRACK: Color = [1.0, 1.0, 1.0, 0.08];
    pub const SCROLL_THUMB: Color = [0.42, 0.68, 1.0, 0.70];

    /// The gap between rows, and between tiles.
    pub const ROW_GAP: f32 = 4.0;
    pub const TILE_GAP: f32 = 22.0;
    /// The margin between a window's edge and what is in it.
    pub const PAD: f32 = 42.0;
    pub const SCROLL_WIDTH: f32 = 5.0;
}

/// What the wearer did, as a widget wants to hear it: positions in logical pixels.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Pointer { x: f32, y: f32 },
    PointerLeft,
    /// The pointer's main button.
    Press { pressed: bool },
    Scroll { dx: f32, dy: f32 },
    Key { key: Key, text: Option<String>, modifiers: Modifiers },
}

impl Input {
    /// An event as input for widgets drawn at `scale`, if it is input at all. Key releases are
    /// not: nothing here acts on one.
    pub fn of(event: &Event, scale: f32) -> Option<Input> {
        match event {
            Event::PointerMoved { x, y, .. } => Some(Input::Pointer { x: x / scale, y: y / scale }),
            Event::PointerLeft { .. } => Some(Input::PointerLeft),
            Event::Button { button: crate::Button::Left, pressed, .. } => Some(Input::Press { pressed: *pressed }),
            Event::Scroll { dx, dy, .. } => Some(Input::Scroll { dx: dx / scale, dy: dy / scale }),
            Event::Key { key, pressed: true, text, modifiers, .. } => {
                Some(Input::Key { key: *key, text: text.clone(), modifiers: *modifiers })
            }
            _ => None,
        }
    }
}
