//! A window's frame, title bar and buttons, and what a point on them means.
//!
//! The Deck's own `pointer.rs` geometry, moved here so that what is drawn and what can be pressed are one
//! piece of arithmetic on every platform: the Deck and the Beam Pro aim at it with a ray, the Mac's room
//! does the same, and none of them has a copy.

use spatiand_render::ray::Quad;


/// How tall the title bar is, as a fraction of the window's height.
///
/// Generous, and it has to be. This is a target hit with a head-anchored ray at 2.2 m, where
/// the whole window is only about 17° tall — so a desktop-proportioned bar works out at a
/// degree or so, which is roughly the tremor in holding your head still. 11% gives a little
/// over 2°, which is comfortably aimable. The test below is what caught 7.5% being too thin.
pub const TITLE_BAR_FRACTION: f64 = 0.11;

/// How thick the window's frame is, as a fraction of the content's height.
///
/// Sized by the same argument as the title bar, and for the same reason it had to be
/// generous. The frame used to be 0.012 m — about a third of a degree at 2.2 m — which is
/// fine for something whose only job is to make the window's edge visible against a dark sky,
/// and hopeless as a target. At 10% of the content's height it comes out near 1.8°, which is
/// a little under the bar and still comfortably aimable. `the_frame_is_a_reachable_target`
/// is what holds that.
pub const BORDER_FRACTION: f64 = 0.10;

/// How much of the bar's height the icon and the close button occupy.
///
/// Comfortably under the whole bar, so both sit *in* it with glass showing around them rather
/// than filling it edge to edge. At the bar's ~1.5° this leaves a target of about 1°, which is
/// the smallest thing on the window anyone is asked to hit — and the reason the close button is
/// at the end of the bar, where overshooting lands on the bar rather than on the surface.
const FURNITURE_FRACTION: f64 = 0.66;

/// The smallest the title bar may be, in radians as the wearer sees it.
///
/// The bar and everything on it are sized as a share of the *content's* height, which is
/// right for an ordinary window and falls apart for a short one. A client that turns itself
/// into a transport bar -- same width, a fifth of the height -- got a bar of 0.7 degrees with
/// a close button of half a degree in it, which is not a target, it is a dare.
///
/// So the share becomes a floor instead. 1.5 degrees is a little under what a default window
/// already gets (2.2), so nothing ordinary changes and only windows that had shrunk past
/// usefulness are affected. What grows is the chrome, not the content: the surface keeps
/// exactly the size the client asked for.
const MIN_BAR_RADIANS: f64 = 0.0262;

/// The smallest the frame may be, in radians. Same argument as the bar.
///
/// The frame is the resize target, so a short window losing its bar to unclickability was
/// losing its handles at the same time and for the same reason. 1.2 degrees, again under the
/// 1.8 a default window has.
const MIN_BORDER_RADIANS: f64 = 0.0209;

/// How much taller than its content a window's chrome may ever be.
///
/// A guard rather than a design: at some point a buffer is short enough that a floored bar and
/// two floored borders would dwarf it, and a window that is mostly frame is worse than one
/// with a small bar. Nothing real reaches this -- a transport bar lands at about 0.66.
const MAX_CHROME_OVERHEAD: f64 = 1.5;

/// How thick the frame of a pinned window is, as a fraction of the picture's height.
///
/// A hairline: the picture is in the corner of the view to be looked past, and a frame as thick
/// as an ordinary window's would be a third of what it shows. It is a *border*, not a target --
/// nothing about a pinned window is grabbed by its edge, because where it sits is a setting.
pub const PIP_BORDER_FRACTION: f64 = 0.035;

/// How big the buttons on a pinned window are, as a fraction of the picture's height.
///
/// They have no bar to sit in, so they float inside the top right corner of the picture and
/// show only while it is aimed at. Big enough to hit at a metre and a half, which is nearer
/// than the room's windows and so wants less -- and no bigger, because four of them side by side
/// are a share of the width of a small picture, and a fifth of its height made them over half.
const PIP_BUTTON_FRACTION: f64 = 0.17;

/// How far in from the picture's top and right edges the buttons float, as a fraction of its
/// height.
const PIP_BUTTON_INSET: f64 = 0.04;

/// How far apart two pieces of furniture sit, as a multiple of their own width.
///
/// A little over one, so they are neighbours with a gap rather than a single wide control.
/// Aiming at these is done down a ray from a couple of metres away, where two touching
/// buttons are one button.
const FURNITURE_SPACING: f64 = 1.25;

/// How far along the bottom edge counts as a corner rather than a side, in border widths.
///
/// Two, so a corner is roughly a 3.6° square. Smaller and it is a target you hit by luck;
/// much larger and the bottom edge stops existing on a narrow window.
pub const CORNER_REACH: f64 = 2.0;

/// The smallest a window may be dragged, in degrees across.
///
/// Not zero, and not a pixel count. A window below about 8° is a few hundred pixels of
/// squinting, and one dragged to nothing cannot be grabbed again to undo it — the frame it
/// would be grabbed by has gone with it.
pub const MINIMUM_ANGLE_DEG: f64 = 8.0;

/// And the largest. 90° is already wider than either eye can see at once; past that a window
/// is simply hiding the world with no way to tell how much of it is left.
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
    /// The button at the right of the bar. Press to ask the window to close.
    Close,
    /// The button left of close. Press to put the window away; the window list brings it back.
    Hide,
    /// The pin, left of hide. Press to pin the window to the glass, or to let a pinned one go.
    Pin,
    /// The speaker, to the left of the pin. Press to silence this window alone.
    ///
    /// Only ever reached when the window is actually making a sound: a button that is not
    /// drawn must not be pressable, or the bar has an invisible dead spot in it.
    Mute,
    /// The client's own surface. Everything here is forwarded.
    Content,
    /// The frame. Grab to resize.
    Resize(Edge),
}

impl Zone {
    /// For a C interface: 0 none, 1 content, 2 title, 3 close, 4 hide, 5 pin, 6 mute, 7.. an edge.
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
///
/// Expressed in quad fractions rather than in metres so that one definition serves both the
/// hit test, which has `(u, v)` from the ray, and the drawing, which has the quad's size. A
/// close button drawn from one set of numbers and aimed at from another is the specific bug
/// this shape exists to make impossible — and it is not a bug anyone finds by reading, only by
/// pressing a thing and having nothing happen.
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
///
/// Proportional rather than metric, so the same numbers describe a window at any size or
/// distance and the whole thing is testable without a placement. Multiplying by the content
/// height gives metres, which is all [`quad_of`] does.
///
/// ```text
///   ┌─────────────────────────┐  ─┐ border
///   │        title bar        │   ├ bar
///   ├──┬───────────────────┬──┤  ─┘
///   │  │                   │  │
///   │  │      content      │  │   1.0
///   │  │                   │  │
///   ├──┴───────────────────┴──┤  ─┐ border
///   └─────────────────────────┘  ─┘
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    /// The content's width — which, in units of its own height, is its aspect ratio.
    pub content_width: f64,
    pub border: f64,
    pub bar: f64,
    /// A pinned window: no bar, a hairline frame, and buttons laid over the picture. Nothing
    /// about it can be dragged or resized, because where it is is a setting.
    pub overlay: bool,
}

impl Frame {
    /// The frame for a window of this shape, in this place.
    ///
    /// The placement is needed for the angular floors and nothing else: the fractions are
    /// still fractions, but a share of a short window's height can be too small to aim at, and
    /// how small "too small" is depends on how far away the window is. Everything else here
    /// stays proportional, which is what keeps one set of numbers serving both the drawing and
    /// the hit test.
    pub fn of(pixels: (u32, u32), placement: &crate::placement::Placement) -> Self {
        let content_width = pixels.0 as f64 / pixels.1.max(1) as f64;
        if placement.pip {
            return Self {
                content_width,
                border: PIP_BORDER_FRACTION,
                bar: 0.0,
                overlay: true,
            };
        }
        let content_height = placement.width / content_width.max(0.01);
        // One radian of arc at this window's distance, in content-height units -- which is
        // what turns an angular floor into a fraction this struct can hold.
        let per_radian = if content_height > 1e-6 {
            placement.radius / content_height
        } else {
            0.0
        };
        // TITLE_BAR_FRACTION is a share of the bar-plus-content height, which is how the
        // drawing has always expressed it; here everything is relative to the content
        // alone, so it has to be rebased.
        let bar = (TITLE_BAR_FRACTION / (1.0 - TITLE_BAR_FRACTION))
            .max(MIN_BAR_RADIANS * per_radian)
            .min(MAX_CHROME_OVERHEAD * 0.6);
        let border = BORDER_FRACTION
            .max(MIN_BORDER_RADIANS * per_radian)
            .min(MAX_CHROME_OVERHEAD * 0.2);
        Self {
            content_width,
            border,
            bar,
            overlay: false,
        }
    }

    pub fn width(&self) -> f64 {
        self.content_width + self.border * 2.0
    }

    pub fn height(&self) -> f64 {
        1.0 + self.bar + self.border * 2.0
    }

    /// Where a quad hit falls, in content coordinates — 0..1 inside, outside that beyond.
    pub fn content_at(&self, u: f64, v: f64) -> (f64, f64) {
        (
            (u * self.width() - self.border) / self.content_width.max(1e-6),
            v * self.height() - self.border - self.bar,
        )
    }

    /// Where the title bar's furniture sits, in quad fractions.
    ///
    /// Both are square, sized against the bar's height, and inset from the chrome's outer edge
    /// by the border — so they sit over the glass rather than over the surface, whatever the
    /// window's aspect ratio.
    fn furniture(&self, from_right: bool) -> Box2 {
        self.furniture_at(from_right, 0)
    }

    /// Furniture `slot` places in from one end of the bar, counting from zero.
    fn furniture_at(&self, from_right: bool, slot: usize) -> Box2 {
        let side = if self.overlay {
            PIP_BUTTON_FRACTION
        } else {
            self.bar * FURNITURE_FRACTION
        };
        let (w, h) = (self.width(), self.height());
        // The bar's centre is half a content-height above the quad's centre — the chrome
        // reaches further above the content than below it, so the two centres do not coincide.
        // A pinned window has no bar: its buttons sit inside the picture, just under its top.
        let v = if self.overlay {
            (self.border + PIP_BUTTON_INSET + side * 0.5) / h
        } else {
            0.5 - 0.5 / h
        };
        // Spaced by a little more than their own width, so two buttons read as two things
        // rather than as one wide one — which matters more here than on a desktop, because
        // they are aimed at down a ray from across the room.
        let step = side * FURNITURE_SPACING / w;
        let inset = if self.overlay {
            (self.border + PIP_BUTTON_INSET + side * 0.5) / w
        } else {
            (self.border + side * 0.5) / w
        } + step * slot as f64;
        Box2 {
            u: if from_right { 1.0 - inset } else { inset },
            v,
            half_u: side * 0.5 / w,
            half_v: side * 0.5 / h,
        }
    }

    /// The hide button, immediately left of the close button.
    ///
    /// Next to close because the two are the same kind of thing — something you do *to* the
    /// window rather than with it — and because the right end of the bar is where a hand
    /// already goes. It is the lesser of the two, so it is the one further in: overshooting
    /// the end of the bar lands on close, which asks politely, rather than the reverse.
    pub fn hide(&self) -> Box2 {
        self.furniture_at(true, 1)
    }

    /// The pin, left of hide: the same kind of thing, a state the window is put in.
    pub fn pin(&self) -> Box2 {
        self.furniture_at(true, 2)
    }

    /// The mute button, left of the pin.
    pub fn mute(&self) -> Box2 {
        self.furniture_at(true, 3)
    }

    /// The application's icon, at the left of the bar. Decoration only — nothing to press.
    pub fn icon(&self) -> Box2 {
        self.furniture(false)
    }

    /// The close button, at the right of the bar.
    ///
    /// There is no minimise and no maximise, and that is a statement rather than an omission:
    /// neither means anything in a room. A window that is in the way is moved or pushed
    /// further off, and one you are finished with is closed.
    pub fn close(&self) -> Box2 {
        self.furniture(true)
    }

    /// What the wearer is pointing at.
    ///
    /// `sounding` says whether this window has a mute button at all. A window that makes no
    /// sound has no speaker drawn on it, and passing that through here is what stops the bar
    /// having an invisible dead spot where the button would have been.
    pub fn zone(&self, u: f64, v: f64, sounding: bool) -> Zone {
        if self.overlay {
            // Buttons, and otherwise the picture: no bar to grab, no edge to drag.
            return if self.close().contains(u, v) {
                Zone::Close
            } else if self.hide().contains(u, v) {
                Zone::Hide
            } else if self.pin().contains(u, v) {
                Zone::Pin
            } else if sounding && self.mute().contains(u, v) {
                Zone::Mute
            } else {
                Zone::Content
            };
        }
        let (x, y) = self.content_at(u, v);
        // Everything above the content is the bar, including the frame above it: a strip of
        // border one degree tall that behaves differently from the bar it touches would be
        // impossible to aim at and pointless if you could.
        if y < 0.0 {
            // Except the close button, which is inside the bar and has to be tested first or
            // it is simply a piece of the bar that happens to have a cross drawn on it.
            return if self.close().contains(u, v) {
                Zone::Close
            } else if self.hide().contains(u, v) {
                Zone::Hide
            } else if self.pin().contains(u, v) {
                Zone::Pin
            } else if sounding && self.mute().contains(u, v) {
                Zone::Mute
            } else {
                Zone::Title
            };
        }
        let left = x < 0.0;
        let right = x > 1.0;
        let bottom = y > 1.0;
        if !left && !right && !bottom {
            return Zone::Content;
        }
        // An L-shaped corner region wrapping the bottom of each side, as every 2D desktop
        // does it — the corner has to be reachable along both the side and the bottom, or it
        // is only ever found by accident.
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

/// What a resize drag has arrived at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resized {
    pub placement: crate::placement::Placement,
    pub pixels: (u32, u32),
}

/// Work out a window's new size and position, part-way through a resize drag.
///
/// `dx` and `dy` are how far the ray has travelled since the grab, in metres across and down
/// the window's own plane.
///
/// The opposite edge stays put, which is what makes this feel like resizing rather than
/// scaling: drag the right edge and the left one does not move. That is entirely a matter of
/// shifting the centre by half of whatever the size changed by, in the right direction.
///
/// Pixels follow the world size at a **constant density**, so the client is asked for more
/// buffer rather than a bigger picture of the same buffer. Text keeps its angular size and
/// more of it fits — which is the difference between resizing a window and zooming it, and
/// the thing the two-thumb gesture deliberately does not do.
pub fn resize(
    edge: Edge,
    start: &crate::placement::Placement,
    start_pixels: (u32, u32),
    dx: f64,
    dy: f64,
) -> Resized {
    let aspect = start_pixels.0 as f64 / start_pixels.1.max(1) as f64;
    let start_height = start.width / aspect.max(0.01);
    // Pixels per metre, held fixed for the whole drag.
    let density = start_pixels.0 as f64 / start.width.max(1e-6);

    let (grow_x, grow_y) = match edge {
        Edge::Left => (-dx, 0.0),
        Edge::Right => (dx, 0.0),
        Edge::Bottom => (0.0, dy),
        Edge::BottomLeft => (-dx, dy),
        Edge::BottomRight => (dx, dy),
    };

    // Limits in metres, from angles at this radius, so a window pushed far away is still
    // allowed to be as big on screen as a near one.
    let limit = |degrees: f64| 2.0 * start.radius * (degrees.to_radians() / 2.0).tan();
    let (min_w, max_w) = (limit(MINIMUM_ANGLE_DEG), limit(MAXIMUM_ANGLE_DEG));
    let width = (start.width + grow_x).clamp(min_w, max_w);
    let height = (start_height + grow_y).clamp(min_w / aspect.max(0.01), max_w);

    // Clamping changes how much the window actually grew, and the centre has to follow *that*
    // rather than the drag — otherwise a window held at its minimum keeps sliding sideways.
    let moved_x = width - start.width;
    let moved_y = height - start_height;

    // Half the growth, towards the edge being dragged. `+u` is rightwards and `+v` downwards,
    // and both are a negative rotation: yaw is positive to the left, pitch positive upwards.
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
        placement: crate::placement::Placement {
            yaw: start.yaw - shift_right / start.radius,
            pitch: crate::placement::clamp_pitch(start.pitch - shift_down / start.radius),
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

/// The quad a window occupies, including its title bar.
///
/// Takes the geometry rather than the whole [`WindowQuad`] so it can be tested: a `WindowQuad`
/// carries a live Wayland window, which cannot be conjured up without a compositor.
pub fn quad_of(pixels: (u32, u32), placement: &crate::placement::Placement) -> Quad {
    let frame = Frame::of(pixels, placement);
    let aspect = pixels.0 as f64 / pixels.1.max(1) as f64;
    let content_height = placement.width / aspect.max(0.01);
    Quad {
        centre: centre_of(pixels, placement),
        orientation: placement.orientation(),
        // The bar sits above the content and the frame surrounds it, so the quad the ray hits
        // is bigger than the surface on every side. A quad the size of the content alone
        // leaves the chrome visible and unclickable, which is how the title bar behaved
        // before it was included here.
        width: content_height * frame.width(),
        height: content_height * frame.height(),
        // Curved round the wearer by the window's own distance: see `Bend`. Drawn the same way
        // in `Scene::draw_windows`, which is what keeps the pointer on what it is aimed at. Not
        // for a window pinned to the glass, which is flat.
        bend: placement.bend_radius().map(|radius| spatiand_render::ray::Bend {
            radius,
            offset: 0.0,
        }),
    }
}

/// The centre of the whole quad, which is not the centre of the content.
///
/// The bar hangs above the content, so the chrome is taller upwards than downwards and its
/// midpoint sits above the surface's. Missing this offsets every hit by half a title bar —
/// about a degree — which is small enough to look like poor aim rather than a bug.
fn centre_of(pixels: (u32, u32), placement: &crate::placement::Placement) -> glam::DVec3 {
    let frame = Frame::of(pixels, placement);
    let aspect = pixels.0 as f64 / pixels.1.max(1) as f64;
    let content_height = placement.width / aspect.max(0.01);
    let up = placement.orientation() * glam::DVec3::Z;
    placement.position() + up * (content_height * frame.bar * 0.5)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement::Placement;

    fn placement(width: f64) -> Placement {
        Placement { width, ..Placement::default() }
    }

    const PIXELS: (u32, u32) = (1280, 800);

    fn frame() -> Frame {
        Frame::of(PIXELS, &placement(1.1))
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
}
