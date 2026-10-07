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

pub use spatiand_room::placement::Placement;
pub use spatiand_room::frame::{
    resize, Box2, Edge, Frame, Resized, Zone, BORDER_FRACTION, MAXIMUM_ANGLE_DEG, MINIMUM_ANGLE_DEG, PIP_BORDER_FRACTION,
    TITLE_BAR_FRACTION,
};

// MARK: drawing

/// What a window's chrome shows. Anything that changes it is in here, so a hash of it says whether the
/// picture is stale.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    pub title: String,
    pub focused: bool,
    pub hot: Option<Zone>,
    /// `Some(muted)` for a window whose application is making a sound.
    pub sound: Option<bool>,
    /// RGBA, 128 by 128 at most, straight alpha.
    pub icon: Option<(u32, u32, std::sync::Arc<Vec<u8>>)>,
    pub pixels: (u32, u32),
    /// Where it is: its width, distance, and whether it is pinned to the glass.
    pub place: Placement,
}

/// A number that changes when the picture would.
pub fn look_key(look: &Look) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    look.title.hash(&mut h);
    look.focused.hash(&mut h);
    look.hot.map(|z| z.code()).hash(&mut h);
    look.place.pip.hash(&mut h);
    look.sound.hash(&mut h);
    look.icon.as_ref().map(|(w, hh, p)| (*w, *hh, std::sync::Arc::as_ptr(p) as usize)).hash(&mut h);
    look.pixels.hash(&mut h);
    ((look.place.width * 500.0).round() as i64).hash(&mut h);
    ((look.place.radius * 200.0).round() as i64).hash(&mut h);
    h.finish().max(1)
}

/// The text renderer, which takes a second or two to find the machine's fonts: made once, on a thread
/// started early, and waited for only by whoever needs a title first.
fn text() -> &'static Mutex<TextRenderer> {
    static TEXT: OnceLock<Mutex<TextRenderer>> = OnceLock::new();
    TEXT.get_or_init(|| Mutex::new(TextRenderer::new()))
}

/// Something drawn with the shared text renderer, which only one thing uses at a time.
pub fn with_text<R>(f: impl FnOnce(&mut TextRenderer) -> R) -> R {
    f(&mut text().lock().unwrap())
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
    let frame = Frame::of(look.pixels, &look.place);
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
        button(frame.close(), disc_colour, &g.close, 0.58, if close_hot { [1.0; 4] } else { rest_glyph });

        let hide_hot = hot(Zone::Hide);
        let disc_colour = if hide_hot {
            [0.86, 0.94, 1.0, 0.60]
        } else if frame.overlay {
            dark
        } else {
            faint(look.focused)
        };
        button(frame.hide(), disc_colour, &g.hide, 0.58, if hide_hot { [1.0; 4] } else { rest_glyph });

        let pin_hot = hot(Zone::Pin);
        let disc_colour = if pin_hot {
            [0.86, 0.94, 1.0, 0.60]
        } else if look.place.pip {
            [0.45, 0.68, 1.0, 0.80]
        } else if frame.overlay {
            dark
        } else {
            faint(look.focused)
        };
        button(frame.pin(), disc_colour, &g.pin, 0.70, if pin_hot || look.place.pip { [1.0; 4] } else { rest_glyph });

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
            button(frame.mute(), disc_colour, glyph, 0.62, if mute_hot || muted { [1.0; 4] } else { rest_glyph });
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
        Frame::of(PIXELS, &placement(1.1))
    }

    #[test]
    fn the_chrome_picture_has_glass_round_the_edge_and_a_title_in_the_bar() {
        let look = Look {
            title: "Settings".into(),
            focused: true,
            hot: None,
            sound: Some(false),
            icon: None,
            pixels: PIXELS,
            place: Placement::default(),
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

