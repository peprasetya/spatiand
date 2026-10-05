//! The pictures the Deck draws a window's frame and the pointer with, copied from `crates/spatiand/src/scene.rs`
//! so the Mac's room looks like the Deck's rather than like another program's.
//!
//! They are CPU-only -- a gradient and a few signed distance fields -- and so could be taken whole. Keep
//! them in step by hand: the Deck's copy is the one that changes.

/// A pane of glass: rounded, with a lit top edge and a soft rim.
///
/// Used for the window frame and title bar, drawn as one piece so they read as a single object
/// rather than a bar sitting on a border. A flat quad cannot do that -- it has hard corners
/// and a uniform fill, and next to the refracting bubbles it looks like a different program.
///
/// Generated rather than shipped, like everything else here: it is a gradient and a rounded
/// rectangle, and an asset would be one more thing to install and license.
pub fn glass_panel_image(width: u32, height: u32, corner: f32) -> Vec<u8> {
    let mut out = vec![0u8; (width * height * 4) as usize];
    let (w, h) = (width as f32, height as f32);
    for y in 0..height {
        for x in 0..width {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Distance outside a rounded rectangle, in pixels. Negative inside.
            let dx = (corner - fx).max(fx - (w - corner)).max(0.0);
            let dy = (corner - fy).max(fy - (h - corner)).max(0.0);
            let outside = (dx * dx + dy * dy).sqrt() - corner;
            // One pixel of feathering: enough to kill the jaggies, little enough that the edge
            // still reads as an edge at this angular size.
            let coverage = (1.0 - (outside + corner.min(1.5))).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }

            // Vertical gradient, lighter at the top, the way a sheet of glass catches a room.
            let t = fy / h;
            let body = 0.24 - 0.14 * t;
            // A bright line just inside the top edge, and a dimmer one at the bottom.
            let from_top = fy / h.max(1.0);
            let lip = (1.0 - (from_top * 26.0).min(1.0)).powf(1.6) * 0.55;
            let base = (1.0 - ((1.0 - from_top) * 34.0).min(1.0)).powf(2.0) * 0.16;
            // Rim: brighter within a couple of pixels of the outline, all the way round.
            let rim = (1.0 - ((-outside) / 2.5).clamp(0.0, 1.0)).powf(1.5) * 0.45;

            let light = (body + lip + base + rim).clamp(0.0, 1.0);
            let i = ((y * width + x) * 4) as usize;
            out[i] = (light * 255.0) as u8;
            out[i + 1] = (light * 255.0) as u8;
            out[i + 2] = (light * 255.0) as u8;
            out[i + 3] = ((0.30 + light * 0.72).min(1.0) * coverage * 255.0) as u8;
        }
    }
    out
}

/// The cross on a window's close button.
///
/// Generated like every other glyph here rather than shipped or shaped from a font: it is two
/// lines, and a font would make the one mark on a window that everybody recognises depend on
/// which fonts happen to be installed.
///
/// The strokes are drawn as a distance to the diagonal rather than by walking pixels, so the
/// edges are antialiased. A hard-edged cross a degree across, seen through optics, reads as a
/// smudge — the softness is what makes it look like a drawn mark at this size.
/// A speaker, with or without a line through it.
///
/// Drawn rather than shaped from a font, for the same reason the close cross is: the one
/// character that means this is an emoji, whose colour and metrics vary by whichever font
/// happens to be installed, and a control has to look the same on every machine.
///
/// Supersampled rather than distance-fielded. The shape is a handful of half-plane and
/// circle tests, and counting how many of a grid of samples land inside is both shorter than
/// the equivalent distance field and exactly as smooth at this size.
pub fn speaker_glyph_image(size: u32, muted: bool) -> Vec<u8> {
    /// How far out the cone flares, and where the body ends.
    const BODY: (f32, f32) = (-0.62, -0.28);
    const CONE_END: f32 = 0.10;
    const BODY_HALF: f32 = 0.26;
    const CONE_HALF: f32 = 0.62;

    let inside = |x: f32, y: f32| -> bool {
        // The box the diaphragm sits in.
        if x >= BODY.0 && x <= BODY.1 && y.abs() <= BODY_HALF {
            return true;
        }
        // The cone, widening linearly to its mouth.
        if x > BODY.1 && x <= CONE_END {
            let t = (x - BODY.1) / (CONE_END - BODY.1);
            if y.abs() <= BODY_HALF + t * (CONE_HALF - BODY_HALF) {
                return true;
            }
        }
        let (dx, dy) = (x - CONE_END, y);
        if muted {
            // A cross where the waves would be. A single slash was tried first and read as
            // one more wave at the size this is actually drawn -- the two strokes are what
            // make it unmistakably "not sounding" rather than "sounding a bit".
            let (ax, ay) = (x - 0.52, y);
            let arm = |across: f32, along: f32| {
                across.abs() * std::f32::consts::FRAC_1_SQRT_2 <= 0.075
                    && along.abs() * std::f32::consts::FRAC_1_SQRT_2 <= 0.30
            };
            return arm(ax - ay, ax + ay) || arm(ax + ay, ax - ay);
        }
        // Two arcs in front of it. Bounded by angle as well as radius, so they are arcs
        // rather than rings drawn round the back of the speaker.
        if dx <= 0.0 {
            return false;
        }
        let r = (dx * dx + dy * dy).sqrt();
        if dy.abs() > dx * 1.30 {
            return false;
        }
        [0.36f32, 0.60].iter().any(|ring| (r - ring).abs() <= 0.065)
    };

    let mut out = vec![0u8; (size * size * 4) as usize];
    let centre = (size as f32 - 1.0) * 0.5;
    let radius = centre;
    // A three-by-three grid inside each pixel, which is enough at this size and costs nothing
    // for a texture built once per session.
    const GRID: i32 = 3;
    for y in 0..size {
        for x in 0..size {
            let mut hits = 0;
            for sy in 0..GRID {
                for sx in 0..GRID {
                    let ox = (sx as f32 + 0.5) / GRID as f32 - 0.5;
                    let oy = (sy as f32 + 0.5) / GRID as f32 - 0.5;
                    let px = (x as f32 + ox - centre) / radius;
                    let py = (y as f32 + oy - centre) / radius;
                    if inside(px, py) {
                        hits += 1;
                    }
                }
            }
            if hits == 0 {
                continue;
            }
            let a = (hits as f32 / (GRID * GRID) as f32 * 255.0) as u8;
            let i = ((y * size + x) * 4) as usize;
            // White, with the coverage in alpha: the drawing tints it.
            out[i] = 255;
            out[i + 1] = 255;
            out[i + 2] = 255;
            out[i + 3] = a;
        }
    }
    out
}

/// A short horizontal bar, a little below the middle: the hide button's "put it away".
pub fn hide_glyph_image(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    let centre = (size as f32 - 1.0) * 0.5;
    let radius = centre;
    let (half_stroke, reach, drop) = (0.10f32, 0.80f32, 0.28f32);
    let feather = 1.5 / radius;
    for y in 0..size {
        for x in 0..size {
            let dx = (x as f32 - centre) / radius;
            let dy = (y as f32 - centre) / radius - drop;
            // Distance to a segment with round ends.
            let beyond = (dx.abs() - reach).max(0.0);
            let d = (beyond * beyond + dy * dy).sqrt();
            let a = (1.0 - (d - half_stroke) / feather).clamp(0.0, 1.0);
            if a <= 0.0 {
                continue;
            }
            let i = ((y * size + x) * 4) as usize;
            out[i] = 255;
            out[i + 1] = 255;
            out[i + 2] = 255;
            out[i + 3] = (a * 255.0) as u8;
        }
    }
    out
}

/// A small picture sitting in the corner of a larger one: the pin button's "keep this on the
/// glass".
pub fn pin_glyph_image(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    let centre = (size as f32 - 1.0) * 0.5;
    let radius = centre;
    let feather = 1.5 / radius;
    // Distance to a rectangle's outline, and to its inside, both from the signed distance of a
    // box: `half` is the half extent, `at` its centre.
    let sd_box = |x: f32, y: f32, at: (f32, f32), half: (f32, f32)| {
        let (dx, dy) = ((x - at.0).abs() - half.0, (y - at.1).abs() - half.1);
        (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() + dx.max(dy).min(0.0)
    };
    for y in 0..size {
        for x in 0..size {
            let fx = (x as f32 - centre) / radius;
            let fy = (y as f32 - centre) / radius;
            // The outer picture: an outline. The inner: a filled block in its lower right.
            let outer = sd_box(fx, fy, (0.0, 0.0), (0.86, 0.62)).abs() - 0.075;
            let inner = sd_box(fx, fy, (0.30, 0.20), (0.36, 0.26));
            let a = [outer, inner]
                .into_iter()
                .map(|d| (1.0 - d / feather).clamp(0.0, 1.0))
                .fold(0.0f32, f32::max);
            if a <= 0.0 {
                continue;
            }
            let i = ((y * size + x) * 4) as usize;
            out[i] = 255;
            out[i + 1] = 255;
            out[i + 2] = 255;
            out[i + 3] = (a * 255.0) as u8;
        }
    }
    out
}

pub fn close_glyph_image(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    let centre = (size as f32 - 1.0) * 0.5;
    let radius = centre;
    // Half the stroke width, and how far each arm reaches, both as a fraction of the radius.
    let half_stroke = 0.085f32;
    let reach = 0.90f32;
    let feather = 1.5 / radius;
    for y in 0..size {
        for x in 0..size {
            let dx = (x as f32 - centre) / radius;
            let dy = (y as f32 - centre) / radius;
            // Distance to each of the two diagonals, and how far along it the point lies.
            let arm = |across: f32, along: f32| {
                if along.abs() > reach {
                    return 0.0;
                }
                let d = across.abs() * std::f32::consts::FRAC_1_SQRT_2;
                (1.0 - (d - half_stroke) / feather).clamp(0.0, 1.0)
            };
            let a = arm(dx - dy, dx + dy).max(arm(dx + dy, dx - dy));
            if a <= 0.0 {
                continue;
            }
            let i = ((y * size + x) * 4) as usize;
            out[i] = 255;
            out[i + 1] = 255;
            out[i + 2] = 255;
            out[i + 3] = (a * 255.0) as u8;
        }
    }
    out
}

/// Build the pointer reticle: a bright dot inside a thin ring.
///
/// A ring rather than a filled disc, so that a small target underneath stays visible through
/// the middle of the cursor. Generated rather than shipped as an asset for the same reason the
/// environment is — nothing to install, nothing to license.
pub fn reticle_image(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    let centre = (size as f32 - 1.0) * 0.5;
    let radius = centre;
    for y in 0..size {
        for x in 0..size {
            let dx = (x as f32 - centre) / radius;
            let dy = (y as f32 - centre) / radius;
            let r = (dx * dx + dy * dy).sqrt();
            // Ring at ~0.8 of the radius, dot inside 0.22. Both edges are feathered, because
            // a hard edge on something this small crawls badly as the head moves.
            let ring = 1.0 - ((r - 0.78).abs() / 0.12).min(1.0);
            let dot = 1.0 - ((r - 0.0) / 0.24).min(1.0);
            let a = (ring.max(dot)).clamp(0.0, 1.0).powf(0.8);
            let i = ((y * size + x) * 4) as usize;
            out[i] = 255;
            out[i + 1] = 255;
            out[i + 2] = 255;
            out[i + 3] = (a * 255.0) as u8;
        }
    }
    out
}

