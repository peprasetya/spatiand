//! A window's frame, title bar and buttons, as the Deck has them.
//!
//! The geometry is the Deck's `pointer.rs` (`Frame`, `Zone`, `resize`) and the drawing is its
//! `scene.rs`, ported rather than shared because that crate is a Wayland compositor: the same numbers
//! and the same pictures, so a window in the Mac's room is framed, grabbed, resized and closed in the
//! way a window on the Deck is.
//!
//! Everything about a window's chrome is in units of the *content's height*, so one set of numbers
//! serves the drawing and the aim: a close button drawn from one set of numbers and aimed at from
//! another is the bug this shape exists to make impossible.
//!
//! ```text
//!   ┌─────────────────────────┐  ─┐ border
//!   │  icon   title   ▭ ◉ ✕   │   ├ bar
//!   ├──┬───────────────────┬──┤  ─┘
//!   │  │                   │  │
//!   │  │      content      │  │   1.0
//!   │  │                   │  │
//!   ├──┴───────────────────┴──┤  ─┐ border
//!   └─────────────────────────┘  ─┘
//! ```

use std::sync::{Mutex, OnceLock};

use spatiand_render::TextRenderer;

use crate::look;
use crate::room::Placement;

/// How tall the title bar is, as a fraction of the window's height. See the Deck's `pointer.rs` for why
/// it is as generous as it is: it is aimed at down a ray at two metres.
pub const TITLE_BAR_FRACTION: f64 = 0.11;
/// How thick the frame is, as a fraction of the content's height.
pub const BORDER_FRACTION: f64 = 0.10;
/// How much of the bar's height the icon and the buttons occupy.
const FURNITURE_FRACTION: f64 = 0.66;
/// The smallest the bar and frame may be, in radians as the wearer sees them: a short window's chrome
/// must stay something that can be aimed at.
const MIN_BAR_RADIANS: f64 = 0.0262;
const MIN_BORDER_RADIANS: f64 = 0.0209;
/// How much taller than its content a window's chrome may ever be.
const MAX_CHROME_OVERHEAD: f64 = 1.5;
/// A pinned window: a hairline frame, and buttons laid over the picture.
pub const PIP_BORDER_FRACTION: f64 = 0.035;
const PIP_BUTTON_FRACTION: f64 = 0.17;
const PIP_BUTTON_INSET: f64 = 0.04;
/// How far apart two buttons sit, as a multiple of their own width.
const FURNITURE_SPACING: f64 = 1.25;
/// How far along the bottom edge counts as a corner rather than a side, in border widths.
const CORNER_REACH: f64 = 2.0;
/// The smallest and largest a window may be dragged, in degrees across.
pub const MINIMUM_ANGLE_DEG: f64 = 8.0;
pub const MAXIMUM_ANGLE_DEG: f64 = 90.0;

/// Which edge of a window is being dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Bottom,
    BottomLeft,
    BottomRight,
}

/// What a point on a window's quad means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    /// The bar along the top. Grab to move.
    Title,
    Close,
    /// Put the window away; the window list brings it back.
    Hide,
    /// Pin it to the glass, or let a pinned one go.
    Pin,
    /// Silence this window's application.
    Mute,
    /// The application's own surface.
    Content,
    /// The frame. Grab to resize.
    Resize(Edge),
}

impl Zone {
    /// For the C interface: 0 none, 1 content, 2 title, 3 close, 4 hide, 5 pin, 6 mute, 7.. an edge.
    pub fn code(self) -> i32 {
        match self {
            Zone::Content => 1,
            Zone::Title => 2,
            Zone::Close => 3,
            Zone::Hide => 4,
            Zone::Pin => 5,
            Zone::Mute => 6,
            Zone::Resize(Edge::Left) => 7,
            Zone::Resize(Edge::Right) => 8,
            Zone::Resize(Edge::Bottom) => 9,
            Zone::Resize(Edge::BottomLeft) => 10,
            Zone::Resize(Edge::BottomRight) => 11,
        }
    }

    pub fn from_code(code: i32) -> Option<Zone> {
        Some(match code {
            1 => Zone::Content,
            2 => Zone::Title,
            3 => Zone::Close,
            4 => Zone::Hide,
            5 => Zone::Pin,
            6 => Zone::Mute,
            7 => Zone::Resize(Edge::Left),
            8 => Zone::Resize(Edge::Right),
            9 => Zone::Resize(Edge::Bottom),
            10 => Zone::Resize(Edge::BottomLeft),
            11 => Zone::Resize(Edge::BottomRight),
            _ => return None,
        })
    }
}

/// A box on a window's quad, in the quad's own 0..1 coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Box2 {
    pub u: f64,
    pub v: f64,
    pub half_u: f64,
    pub half_v: f64,
}

impl Box2 {
    fn contains(&self, u: f64, v: f64) -> bool {
        (u - self.u).abs() <= self.half_u && (v - self.v).abs() <= self.half_v
    }
}

/// A window's quad broken into its parts, in units of the content's height.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    /// The content's width: its aspect ratio, in units of its own height.
    pub content_width: f64,
    pub border: f64,
    pub bar: f64,
    /// A pinned window: no bar, a hairline frame, and buttons laid over the picture.
    pub overlay: bool,
}

impl Frame {
    /// The frame for a window of this shape, this wide (metres) at this distance, pinned or not.
    pub fn of(pixels: (u32, u32), width_m: f64, radius_m: f64, pinned: bool) -> Self {
        let content_width = pixels.0 as f64 / pixels.1.max(1) as f64;
        if pinned {
            return Self { content_width, border: PIP_BORDER_FRACTION, bar: 0.0, overlay: true };
        }
        let content_height = width_m / content_width.max(0.01);
        let per_radian = if content_height > 1e-6 { radius_m / content_height } else { 0.0 };
        let bar = (TITLE_BAR_FRACTION / (1.0 - TITLE_BAR_FRACTION))
            .max(MIN_BAR_RADIANS * per_radian)
            .min(MAX_CHROME_OVERHEAD * 0.6);
        let border = BORDER_FRACTION.max(MIN_BORDER_RADIANS * per_radian).min(MAX_CHROME_OVERHEAD * 0.2);
        Self { content_width, border, bar, overlay: false }
    }

    pub fn width(&self) -> f64 {
        self.content_width + self.border * 2.0
    }

    pub fn height(&self) -> f64 {
        1.0 + self.bar + self.border * 2.0
    }

    /// Where a quad hit falls, in content coordinates: 0..1 inside, outside that beyond.
    pub fn content_at(&self, u: f64, v: f64) -> (f64, f64) {
        ((u * self.width() - self.border) / self.content_width.max(1e-6), v * self.height() - self.border - self.bar)
    }

    fn furniture(&self, from_right: bool) -> Box2 {
        self.furniture_at(from_right, 0)
    }

    fn furniture_at(&self, from_right: bool, slot: usize) -> Box2 {
        let side = if self.overlay { PIP_BUTTON_FRACTION } else { self.bar * FURNITURE_FRACTION };
        let (w, h) = (self.width(), self.height());
        let v = if self.overlay { (self.border + PIP_BUTTON_INSET + side * 0.5) / h } else { 0.5 - 0.5 / h };
        let step = side * FURNITURE_SPACING / w;
        let inset = if self.overlay {
            (self.border + PIP_BUTTON_INSET + side * 0.5) / w
        } else {
            (self.border + side * 0.5) / w
        } + step * slot as f64;
        Box2 { u: if from_right { 1.0 - inset } else { inset }, v, half_u: side * 0.5 / w, half_v: side * 0.5 / h }
    }

    pub fn close(&self) -> Box2 {
        self.furniture(true)
    }
    pub fn hide(&self) -> Box2 {
        self.furniture_at(true, 1)
    }
    pub fn pin(&self) -> Box2 {
        self.furniture_at(true, 2)
    }
    pub fn mute(&self) -> Box2 {
        self.furniture_at(true, 3)
    }
    pub fn icon(&self) -> Box2 {
        self.furniture(false)
    }

    /// What the wearer is pointing at. `sounding` says whether this window has a speaker at all.
    pub fn zone(&self, u: f64, v: f64, sounding: bool) -> Zone {
        let button = |u: f64, v: f64| {
            if self.close().contains(u, v) {
                Some(Zone::Close)
            } else if self.hide().contains(u, v) {
                Some(Zone::Hide)
            } else if self.pin().contains(u, v) {
                Some(Zone::Pin)
            } else if sounding && self.mute().contains(u, v) {
                Some(Zone::Mute)
            } else {
                None
            }
        };
        if self.overlay {
            return button(u, v).unwrap_or(Zone::Content);
        }
        let (x, y) = self.content_at(u, v);
        if y < 0.0 {
            return button(u, v).unwrap_or(Zone::Title);
        }
        let left = x < 0.0;
        let right = x > 1.0;
        let bottom = y > 1.0;
        if !left && !right && !bottom {
            return Zone::Content;
        }
        let reach_x = self.border * CORNER_REACH / self.content_width.max(1e-6);
        let reach_y = self.border * CORNER_REACH;
        let near_bottom = y > 1.0 - reach_y;
        let near_left = x < reach_x;
        let near_right = x > 1.0 - reach_x;
        Zone::Resize(match (left, right, bottom) {
            (true, _, _) if near_bottom => Edge::BottomLeft,
            (_, true, _) if near_bottom => Edge::BottomRight,
            (true, _, _) => Edge::Left,
            (_, true, _) => Edge::Right,
            (_, _, _) if near_left => Edge::BottomLeft,
            (_, _, _) if near_right => Edge::BottomRight,
            _ => Edge::Bottom,
        })
    }
}

/// A window's new size and place, part-way through a resize drag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resized {
    pub placement: Placement,
    pub pixels: (u32, u32),
}

fn clamp_pitch(pitch: f64) -> f64 {
    let limit = 89.0f64.to_radians();
    pitch.clamp(-limit, limit)
}

/// Work out a window's new size and position, part-way through a resize drag. `dx` and `dy` are how far
/// the ray has travelled since the grab, in metres across and down the window's own plane.
///
/// The opposite edge stays put, which is what makes this resizing rather than scaling. Pixels follow the
/// world size at a **constant density**, so the application is asked for more buffer rather than a bigger
/// picture of the same one: text keeps its angular size and more of it fits.
pub fn resize(edge: Edge, start: &Placement, start_pixels: (u32, u32), dx: f64, dy: f64) -> Resized {
    let aspect = start_pixels.0 as f64 / start_pixels.1.max(1) as f64;
    let start_height = start.width / aspect.max(0.01);
    let density = start_pixels.0 as f64 / start.width.max(1e-6);
    let (grow_x, grow_y) = match edge {
        Edge::Left => (-dx, 0.0),
        Edge::Right => (dx, 0.0),
        Edge::Bottom => (0.0, dy),
        Edge::BottomLeft => (-dx, dy),
        Edge::BottomRight => (dx, dy),
    };
    let limit = |degrees: f64| 2.0 * start.radius * (degrees.to_radians() / 2.0).tan();
    let (min_w, max_w) = (limit(MINIMUM_ANGLE_DEG), limit(MAXIMUM_ANGLE_DEG));
    let width = (start.width + grow_x).clamp(min_w, max_w);
    let height = (start_height + grow_y).clamp(min_w / aspect.max(0.01), max_w);
    let moved_x = width - start.width;
    let moved_y = height - start_height;
    let shift_right = match edge {
        Edge::Left | Edge::BottomLeft => -moved_x * 0.5,
        Edge::Right | Edge::BottomRight => moved_x * 0.5,
        Edge::Bottom => 0.0,
    };
    let shift_down = match edge {
        Edge::Bottom | Edge::BottomLeft | Edge::BottomRight => moved_y * 0.5,
        _ => 0.0,
    };
    Resized {
        placement: Placement {
            yaw: start.yaw - shift_right / start.radius,
            pitch: clamp_pitch(start.pitch - shift_down / start.radius),
            radius: start.radius,
            width,
            ..*start
        },
        pixels: (
            (width * density).round().clamp(160.0, 4096.0) as u32,
            (height * density).round().clamp(120.0, 4096.0) as u32,
        ),
    }
}

// MARK: drawing

/// What a window's chrome shows. Anything that changes it is in here, so a hash of it says whether the
/// picture is stale.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    pub title: String,
    pub focused: bool,
    pub hot: Option<Zone>,
    pub pinned: bool,
    /// `Some(muted)` for a window whose application is making a sound.
    pub sound: Option<bool>,
    /// RGBA, 128 by 128 at most, straight alpha.
    pub icon: Option<(u32, u32, std::sync::Arc<Vec<u8>>)>,
    pub pixels: (u32, u32),
    pub width_m: f64,
    pub radius_m: f64,
}

/// A number that changes when the picture would.
pub fn look_key(look: &Look) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    look.title.hash(&mut h);
    look.focused.hash(&mut h);
    look.hot.map(|z| z.code()).hash(&mut h);
    look.pinned.hash(&mut h);
    look.sound.hash(&mut h);
    look.icon.as_ref().map(|(w, hh, p)| (*w, *hh, std::sync::Arc::as_ptr(p) as usize)).hash(&mut h);
    look.pixels.hash(&mut h);
    ((look.width_m * 500.0).round() as i64).hash(&mut h);
    ((look.radius_m * 200.0).round() as i64).hash(&mut h);
    h.finish().max(1)
}

/// The text renderer, which takes a second or two to find the machine's fonts: made once, on a thread
/// started early, and waited for only by whoever needs a title first.
fn text() -> &'static Mutex<TextRenderer> {
    static TEXT: OnceLock<Mutex<TextRenderer>> = OnceLock::new();
    TEXT.get_or_init(|| Mutex::new(TextRenderer::new()))
}

/// Start finding fonts now.
pub fn warm_up() {
    std::thread::spawn(|| {
        drop(text().lock());
    });
}

/// Text in a size and colour, straight RGBA, through the shared renderer.
pub fn text_image(s: &str, em_px: f32, max_width: u32, colour: [u8; 4]) -> spatiand_render::TextImage {
    text().lock().unwrap().render(s, em_px.max(4.0), max_width.max(8), colour)
}

/// A block of text wrapped to a width and set left, keeping the whole width.
pub fn text_block(s: &str, em_px: f32, width: u32, colour: [u8; 4]) -> spatiand_render::TextImage {
    text().lock().unwrap().render_aligned(s, em_px.max(4.0), width.max(8), colour, spatiand_render::TextAlign::Left)
}

/// Straight-alpha source-over of one pixel onto another.
pub fn over(dst: &mut [u8], src: [f32; 4]) {
    let sa = src[3].clamp(0.0, 1.0);
    if sa <= 0.0 {
        return;
    }
    let da = dst[3] as f32 / 255.0;
    let oa = sa + da * (1.0 - sa);
    for c in 0..3 {
        let d = dst[c] as f32 / 255.0;
        let o = (src[c] * sa + d * da * (1.0 - sa)) / oa.max(1e-6);
        dst[c] = (o.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    dst[3] = (oa.clamp(0.0, 1.0) * 255.0).round() as u8;
}

/// Draw a picture, stretched into a rectangle of the canvas, each pixel multiplied by `tint`.
#[allow(clippy::too_many_arguments)]
pub fn blit(canvas: &mut [u8], cw: u32, ch: u32, src: &[u8], sw: u32, sh: u32, x: f32, y: f32, w: f32, h: f32, tint: [f32; 4]) {
    if w < 1.0 || h < 1.0 || sw == 0 || sh == 0 {
        return;
    }
    let (x0, y0) = (x.floor().max(0.0) as i32, y.floor().max(0.0) as i32);
    let (x1, y1) = ((x + w).ceil().min(cw as f32) as i32, (y + h).ceil().min(ch as f32) as i32);
    for py in y0..y1 {
        for px in x0..x1 {
            // Bilinear from the source, which may be a good deal smaller or bigger than its place.
            let fx = ((px as f32 + 0.5 - x) / w * sw as f32 - 0.5).clamp(0.0, sw as f32 - 1.0);
            let fy = ((py as f32 + 0.5 - y) / h * sh as f32 - 0.5).clamp(0.0, sh as f32 - 1.0);
            let (ix, iy) = (fx.floor() as u32, fy.floor() as u32);
            let (tx, ty) = (fx - ix as f32, fy - iy as f32);
            let at = |ax: u32, ay: u32| {
                let i = ((ay.min(sh - 1) * sw + ax.min(sw - 1)) * 4) as usize;
                [src[i] as f32, src[i + 1] as f32, src[i + 2] as f32, src[i + 3] as f32]
            };
            let (a, b, c, d) = (at(ix, iy), at(ix + 1, iy), at(ix, iy + 1), at(ix + 1, iy + 1));
            let mut px_rgba = [0.0f32; 4];
            for k in 0..4 {
                px_rgba[k] = (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty;
                px_rgba[k] = px_rgba[k] / 255.0 * tint[k];
            }
            let i = ((py as u32 * cw + px as u32) * 4) as usize;
            over(&mut canvas[i..i + 4], px_rgba);
        }
    }
}

/// A disc, anti-aliased, over the canvas.
pub fn disc(canvas: &mut [u8], cw: u32, ch: u32, cx: f32, cy: f32, radius: f32, colour: [f32; 4]) {
    let (x0, y0) = ((cx - radius - 1.0).floor().max(0.0) as i32, (cy - radius - 1.0).floor().max(0.0) as i32);
    let (x1, y1) = ((cx + radius + 1.0).ceil().min(cw as f32) as i32, (cy + radius + 1.0).ceil().min(ch as f32) as i32);
    for py in y0..y1 {
        for px in x0..x1 {
            let d = ((px as f32 + 0.5 - cx).powi(2) + (py as f32 + 0.5 - cy).powi(2)).sqrt() - radius;
            let cover = (0.5 - d).clamp(0.0, 1.0);
            if cover > 0.0 {
                let i = ((py as u32 * cw + px as u32) * 4) as usize;
                over(&mut canvas[i..i + 4], [colour[0], colour[1], colour[2], colour[3] * cover]);
            }
        }
    }
}

/// The glyphs, made once.
struct Glyphs {
    glass: Vec<u8>,
    close: Vec<u8>,
    hide: Vec<u8>,
    pin: Vec<u8>,
    speaker: Vec<u8>,
    speaker_off: Vec<u8>,
}

const GLYPH_PX: u32 = 96;

fn glyphs() -> &'static Glyphs {
    static G: OnceLock<Glyphs> = OnceLock::new();
    G.get_or_init(|| Glyphs {
        glass: look::glass_panel_image(256, 96, 14.0),
        close: look::close_glyph_image(GLYPH_PX),
        hide: look::hide_glyph_image(GLYPH_PX),
        pin: look::pin_glyph_image(GLYPH_PX),
        speaker: look::speaker_glyph_image(GLYPH_PX, false),
        speaker_off: look::speaker_glyph_image(GLYPH_PX, true),
    })
}

/// The window's chrome as one picture: the pane of glass, the application's icon, its title and the
/// buttons. Straight RGBA, `(width, height, pixels)`; the middle, where the application's own surface
/// goes, is left as glass because the surface is drawn over it.
pub fn compose(look: &Look, width_px: u32) -> (u32, u32, Vec<u8>) {
    let frame = Frame::of(look.pixels, look.width_m, look.radius_m, look.pinned);
    let aspect = frame.width() / frame.height();
    let mut cw = width_px.clamp(256, 2048);
    let mut ch = (cw as f64 / aspect).round().max(64.0) as u32;
    if ch > 2048 {
        ch = 2048;
        cw = (ch as f64 * aspect).round().max(64.0) as u32;
    }
    let mut canvas = vec![0u8; (cw * ch * 4) as usize];
    let g = glyphs();
    let (w, h) = (cw as f32, ch as f32);
    let unit = h / frame.height() as f32; // pixels in one content height

    // One pane of glass, tinted by whether the window has the keyboard.
    let tint = if look.focused { [0.62, 0.76, 1.0, 0.92] } else { [0.42, 0.47, 0.60, 0.60] };
    if !frame.overlay {
        blit(&mut canvas, cw, ch, &g.glass, 256, 96, 0.0, 0.0, w, h, tint);
    } else {
        // A pinned window is a picture in the corner: a hairline of glass round it.
        blit(&mut canvas, cw, ch, &g.glass, 256, 96, 0.0, 0.0, w, h, tint);
    }
    let dim = if look.focused { 1.0 } else { 0.7 };
    let box_px = |b: Box2| -> (f32, f32, f32, f32) {
        ((b.u - b.half_u) as f32 * w, (b.v - b.half_v) as f32 * h, (b.half_u * 2.0) as f32 * w, (b.half_v * 2.0) as f32 * h)
    };

    // The application's icon, at the left of the bar.
    if let (Some((iw, ih, pixels)), false) = (&look.icon, frame.overlay) {
        let mut b = frame.icon();
        b.half_u *= 0.82;
        b.half_v *= 0.82;
        let (x, y, bw, bh) = box_px(b);
        blit(&mut canvas, cw, ch, pixels, *iw, *ih, x, y, bw, bh, [1.0, 1.0, 1.0, if look.focused { 1.0 } else { 0.72 }]);
    }

    // The title, between the icon and the nearest button; a long one is cut off there, not shrunk.
    if !look.title.is_empty() && !frame.overlay {
        let bar_px = frame.bar as f32 * unit;
        let label_h = bar_px * 0.62;
        let image = text_image(&look.title, label_h / 1.4, cw, [226, 234, 250, 255]);
        if !image.is_empty() {
            let label_w = label_h * image.width as f32 / image.height.max(1) as f32;
            let icon = frame.icon();
            let left_edge = (icon.u + icon.half_u) as f32 * w;
            let outer = if look.sound.is_some() { frame.mute() } else { frame.pin() };
            let right_edge = (outer.u - outer.half_u) as f32 * w;
            let gap = icon.half_u as f32 * 0.5 * w;
            let room = (right_edge - left_edge - gap * 2.0).max(0.0);
            let bar_centre_y = (frame.border as f32 + frame.bar as f32 * 0.5) * unit;
            let y = bar_centre_y - label_h * 0.5;
            if label_w <= room {
                let x = w * 0.5 - label_w * 0.5;
                blit(&mut canvas, cw, ch, &image.rgba, image.width, image.height, x, y, label_w, label_h, [1.0, 1.0, 1.0, dim]);
            } else if room > 4.0 {
                // Left-aligned in the space there is, the rest cut off.
                let keep = ((room / label_w) * image.width as f32).floor() as u32;
                let mut cropped = vec![0u8; (keep * image.height * 4) as usize];
                for row in 0..image.height {
                    let a = (row * image.width * 4) as usize;
                    let b = (row * keep * 4) as usize;
                    cropped[b..b + (keep * 4) as usize].copy_from_slice(&image.rgba[a..a + (keep * 4) as usize]);
                }
                blit(&mut canvas, cw, ch, &cropped, keep, image.height, left_edge + gap, y, room, label_h, [1.0, 1.0, 1.0, dim]);
            }
        }
    }

    // The buttons: a disc of brighter glass with a glyph on it.
    let hot = |z: Zone| look.hot == Some(z);
    let faint = |focused: bool| if focused { [0.86, 0.92, 1.0, 0.28] } else { [0.80, 0.86, 1.0, 0.15] };
    let rest_glyph = [0.92, 0.95, 1.0, if look.focused { 0.90 } else { 0.55 }];
    let mut button = |b: Box2, disc_colour: [f32; 4], glyph: &[u8], scale: f64, glyph_tint: [f32; 4]| {
        let (x, y, bw, bh) = box_px(b);
        disc(&mut canvas, cw, ch, x + bw * 0.5, y + bh * 0.5, bw.min(bh) * 0.5, disc_colour);
        let (gw, gh) = (bw * scale as f32, bh * scale as f32);
        blit(&mut canvas, cw, ch, glyph, GLYPH_PX, GLYPH_PX, x + (bw - gw) * 0.5, y + (bh - gh) * 0.5, gw, gh, glyph_tint);
    };
    let dark = [0.04, 0.06, 0.09, 0.62];
    let draw_buttons = !frame.overlay || look.hot.is_some();
    if draw_buttons {
        let close_hot = hot(Zone::Close);
        let disc_colour = if close_hot {
            [0.98, 0.42, 0.40, 0.92]
        } else if frame.overlay {
            dark
        } else {
            faint(look.focused)
        };
        button(frame.close(), disc_colour, &g.close, 0.92, if close_hot { [1.0; 4] } else { rest_glyph });

        let hide_hot = hot(Zone::Hide);
        let disc_colour = if hide_hot {
            [0.86, 0.94, 1.0, 0.60]
        } else if frame.overlay {
            dark
        } else {
            faint(look.focused)
        };
        button(frame.hide(), disc_colour, &g.hide, 0.92, if hide_hot { [1.0; 4] } else { rest_glyph });

        let pin_hot = hot(Zone::Pin);
        let disc_colour = if pin_hot {
            [0.86, 0.94, 1.0, 0.60]
        } else if look.pinned {
            [0.45, 0.68, 1.0, 0.80]
        } else if frame.overlay {
            dark
        } else {
            faint(look.focused)
        };
        button(frame.pin(), disc_colour, &g.pin, 1.04, if pin_hot || look.pinned { [1.0; 4] } else { rest_glyph });

        if let Some(muted) = look.sound {
            let mute_hot = hot(Zone::Mute);
            let disc_colour = if muted {
                [1.0, 0.55, 0.45, if mute_hot { 0.85 } else { 0.55 }]
            } else if mute_hot {
                [0.86, 0.94, 1.0, 0.60]
            } else if frame.overlay {
                dark
            } else {
                [0.80, 0.90, 1.0, 0.30]
            };
            let glyph = if muted { &g.speaker_off } else { &g.speaker };
            button(frame.mute(), disc_colour, glyph, 0.92, if mute_hot || muted { [1.0; 4] } else { rest_glyph });
        }
    }
    (cw, ch, canvas)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placement(width: f64) -> Placement {
        Placement { width, ..Placement::default() }
    }

    const PIXELS: (u32, u32) = (1280, 800);

    fn frame() -> Frame {
        let p = placement(1.1);
        Frame::of(PIXELS, p.width, p.radius, false)
    }

    #[test]
    fn the_middle_is_content_the_top_is_the_bar_and_the_edges_resize() {
        let f = frame();
        assert_eq!(f.zone(0.5, 0.5, false), Zone::Content);
        assert_eq!(f.zone(0.5, 0.5 - 0.5 / f.height() + 0.01, false), Zone::Title);
        let beside = f.border * 0.5 / f.width();
        let up_the_side = (f.border + f.bar + 0.5) / f.height();
        assert_eq!(f.zone(beside, up_the_side, false), Zone::Resize(Edge::Left));
        assert_eq!(f.zone(1.0 - beside, up_the_side, false), Zone::Resize(Edge::Right));
        let below = 1.0 - f.border * 0.5 / f.height();
        assert_eq!(f.zone(0.5, below, false), Zone::Resize(Edge::Bottom));
        assert_eq!(f.zone(beside, below, false), Zone::Resize(Edge::BottomLeft));
        assert_eq!(f.zone(1.0 - beside, below, false), Zone::Resize(Edge::BottomRight));
    }

    #[test]
    fn the_buttons_are_where_they_are_drawn() {
        let f = frame();
        for (b, zone) in [(f.close(), Zone::Close), (f.hide(), Zone::Hide), (f.pin(), Zone::Pin), (f.mute(), Zone::Mute)] {
            assert_eq!(f.zone(b.u, b.v, true), zone);
        }
        // A window that makes no sound has no speaker to press.
        assert_eq!(f.zone(f.mute().u, f.mute().v, false), Zone::Title);
    }

    #[test]
    fn content_coordinates_span_the_whole_surface() {
        let f = frame();
        let (x, y) = f.content_at((f.border + 0.001 * f.content_width) / f.width(), (f.border + f.bar) / f.height() + 0.001);
        assert!(x < 0.01 && y < 0.01);
        let (x, y) = f.content_at((f.border + 0.999 * f.content_width) / f.width(), (f.border + f.bar + 0.999) / f.height());
        assert!(x > 0.99 && y > 0.99);
    }

    #[test]
    fn dragging_the_right_edge_out_widens_the_window_at_constant_density_and_the_left_edge_stays() {
        let start = placement(1.1);
        let out = resize(Edge::Right, &start, PIXELS, 0.2, 0.0);
        assert!(out.placement.width > start.width && out.pixels.0 > PIXELS.0);
        let before = PIXELS.0 as f64 / start.width;
        let after = out.pixels.0 as f64 / out.placement.width;
        assert!((before - after).abs() / before < 0.01);
        let left_before = start.yaw + (start.width * 0.5) / start.radius;
        let left_after = out.placement.yaw + (out.placement.width * 0.5) / out.placement.radius;
        assert!((left_before - left_after).abs() < 1e-9);
    }

    #[test]
    fn a_window_cannot_be_dragged_to_nothing_or_over_the_sky() {
        let start = placement(1.1);
        let small = resize(Edge::Right, &start, PIXELS, -100.0, 0.0);
        let angle = 2.0 * (small.placement.width / 2.0 / small.placement.radius).atan().to_degrees();
        assert!(angle >= MINIMUM_ANGLE_DEG - 0.01);
        let big = resize(Edge::Right, &start, PIXELS, 100.0, 0.0);
        let angle = 2.0 * (big.placement.width / 2.0 / big.placement.radius).atan().to_degrees();
        assert!(angle <= MAXIMUM_ANGLE_DEG + 0.01);
    }

    #[test]
    fn the_chrome_picture_has_glass_round_the_edge_and_a_title_in_the_bar() {
        let look = Look {
            title: "Settings".into(),
            focused: true,
            hot: None,
            pinned: false,
            sound: Some(false),
            icon: None,
            pixels: PIXELS,
            width_m: 1.1,
            radius_m: 2.2,
        };
        let (w, h, rgba) = compose(&look, 1024);
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        let opaque = |x: u32, y: u32| rgba[((y * w + x) * 4 + 3) as usize] > 40;
        assert!(opaque(w / 2, h / 40), "the bar is glass");
        let f = frame();
        let bar_y = ((f.border + f.bar * 0.5) / f.height() * h as f64) as u32;
        let ink = (0..w).filter(|x| rgba[((bar_y * w + x) * 4) as usize] > 200).count();
        assert!(ink > 20, "something is written in the bar: {ink}");
    }
}
