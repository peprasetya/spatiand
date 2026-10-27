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

mod icons;
pub mod look;
mod paint;
mod widgets;

pub use icons::{coverage as icon_coverage, draw as draw_icon, Icon};
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

    pub fn take_right(&mut self, width: f32) -> Rect {
        let width = width.min(self.w);
        self.w -= width;
        Rect { x: self.x + self.w, w: width, ..*self }
    }
}

/// A colour: red, green, blue and alpha, each 0 to 1, not multiplied by alpha.
pub type Color = [f32; 4];

/// The room's colours and sizes. The colours are `crates/spatiand/src/scene.rs`'s own; the
/// sizes are `spatiand_render::panel`'s.
pub mod theme {
    use super::Color;
    pub use spatiand_render::panel::{CARD_RADIUS, DETAIL_EM, FOOTER_EM, ROW_EM, ROW_HEIGHT, ROW_INSET, ROW_RADIUS, TITLE_EM};

    /// Whether the platform is set to dark (the default) or light: told by the application's
    /// loop (`crate::App::wait`), which looks each second, as a page's look follows it too.
    static LIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    pub fn dark() -> bool {
        !LIGHT.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Says whether it changed.
    pub fn set_dark(dark: bool) -> bool {
        LIGHT.swap(!dark, std::sync::atomic::Ordering::Relaxed) == dark
    }

    /// Draws with `draw` in the dark colours, whatever the platform is set to: for what is drawn
    /// over a film or a picture, on a dark scrim, as a player's controls.
    pub fn always_dark<R>(draw: impl FnOnce() -> R) -> R {
        let was = LIGHT.swap(false, std::sync::atomic::Ordering::Relaxed);
        let drawn = draw();
        LIGHT.store(was, std::sync::atomic::Ordering::Relaxed);
        drawn
    }

    fn pick(dark: Color, light: Color) -> Color {
        if self::dark() { dark } else { light }
    }

    /// What lies on the ground at `alpha`: white on dark, black on light. For washes, rims and
    /// faces that are only a little off the ground.
    pub fn wash(alpha: f32) -> Color {
        if dark() { [1.0, 1.0, 1.0, alpha] } else { [0.0, 0.0, 0.0, alpha * 0.75] }
    }

    /// What a window is cleared to: the card's ground, opaque, since a window has no room
    /// behind it to be seen through to.
    pub fn ground() -> Color { pick([0.043, 0.05, 0.072, 1.0], [0.955, 0.961, 0.973, 1.0]) }
    /// A surface raised off the ground: a field, a button, a tile with no picture yet.
    pub fn raised() -> Color { pick([1.0, 1.0, 1.0, 0.06], [0.0, 0.0, 0.0, 0.05]) }
    pub fn rim() -> Color { pick([0.55, 0.70, 1.0, 0.16], [0.18, 0.32, 0.65, 0.20]) }
    pub fn selected() -> Color { pick([0.42, 0.68, 1.0, 0.20], [0.16, 0.44, 0.95, 0.16]) }
    /// Selected, in a widget the keys are not going to.
    pub fn selected_away() -> Color { pick([1.0, 1.0, 1.0, 0.08], [0.0, 0.0, 0.0, 0.07]) }
    pub fn hover() -> Color { pick([1.0, 1.0, 1.0, 0.05], [0.0, 0.0, 0.0, 0.04]) }
    pub fn mark() -> Color { pick([0.45, 0.72, 1.0, 1.0], [0.10, 0.40, 0.88, 1.0]) }
    pub fn ink() -> Color { pick([1.0, 1.0, 1.0, 1.0], [0.06, 0.07, 0.10, 1.0]) }
    pub fn ink_row() -> Color { pick([0.78, 0.82, 0.90, 1.0], [0.16, 0.18, 0.24, 1.0]) }
    pub fn ink_trailing() -> Color { pick([0.50, 0.70, 1.0, 1.0], [0.10, 0.36, 0.82, 1.0]) }
    pub fn ink_detail() -> Color { pick([0.70, 0.76, 0.87, 1.0], [0.28, 0.32, 0.40, 1.0]) }
    pub fn ink_hint() -> Color { pick([0.50, 0.55, 0.66, 1.0], [0.44, 0.48, 0.56, 1.0]) }
    pub fn separator() -> Color { wash(0.10) }
    pub fn scroll_track() -> Color { wash(0.08) }
    pub fn scroll_thumb() -> Color { pick([0.42, 0.68, 1.0, 0.70], [0.16, 0.44, 0.95, 0.70]) }

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
    /// Two fingers pinching, as a factor since the last: see [`crate::App::set_pinch_taken`].
    Pinch { scale: f32 },
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
            Event::Pinch { scale, .. } => Some(Input::Pinch { scale: *scale }),
            // Escape held is one Escape: repeated, it closed what was under what it closed.
            Event::Key { key: Key::Escape, repeat: true, .. } => None,
            Event::Key { key, pressed: true, text, modifiers, .. } => {
                Some(Input::Key { key: *key, text: text.clone(), modifiers: *modifiers })
            }
            _ => None,
        }
    }
}
