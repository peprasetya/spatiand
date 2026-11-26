//! Marks painted: one painting for the window, at the size a picture is shown, and for a picture
//! saved, at its own size -- so that what is saved is what was seen, on the CPU and so with no
//! GPU's limit on a picture's size. Lines and shapes by tiny-skia (resvg's); words by
//! cosmic-text with the room's own fonts and their fallbacks for every script
//! (`spatiand_render::text::font_system`), in the typeface, weight and slant chosen.
//!
//! A pen's line is drawn with a round tip, round at its ends and where it turns, as Chromium's
//! pen is (`GetCornerRounding` 1, pdf_ink_brush.cc). A highlighter's tip is an upright square
//! (its rounding 0), and it is laid on see-through: what is under it is multiplied by its colour
//! as Chromium shows that colour on white ([`Colour::on_white`]) -- so that on white it is
//! Chromium's highlighter to the pixel, and on a photograph what is under it shows through, as
//! through a highlighter's ink. When the marks are painted as a layer of their own, to be laid
//! over the picture afterwards (the window's, and a saved picture's), what the highlighters
//! multiply the picture by is a second layer, [`Layers::tint`], and what is under a highlighter in
//! the first is multiplied there too: the picture multiplied by the one and the other laid over
//! it come to the marks painted on the picture in their order.

use std::sync::{Mutex, MutexGuard, OnceLock};

use cosmic_text::{Attrs, Buffer, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, Style, SwashCache, SwashContent, Weight};
use resvg::tiny_skia::{FillRule, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, PixmapMut, Rect as SkRect, Stroke, Transform};

use crate::markup::{arrow_head, highlighter_shapes, stroke_line, Area, Brush, Colour, Face, Note, Point, Shape, Typeface};

/// The space between lines of words, as a share of their size.
pub const LEADING: f32 = 1.25;

/// The size words are measured and outlined at, then scaled from: set without hinting, they
/// scale in proportion.
const REFERENCE: f32 = 100.0;

/// The room's fonts and a glyph rasteriser, for setting words.
pub struct Fonts {
    system: FontSystem,
    cache: SwashCache,
}

/// The fonts, made the first time words are set (reading what fonts the machine has takes a
/// moment), and shared by the window's thread and a save's.
pub fn fonts() -> MutexGuard<'static, Fonts> {
    static FONTS: OnceLock<Mutex<Fonts>> = OnceLock::new();
    FONTS.get_or_init(|| Mutex::new(Fonts { system: spatiand_render::text::font_system(), cache: SwashCache::new() })).lock().unwrap_or_else(|e| e.into_inner())
}

/// Where a page's units land in a picture's pixels: `x * scale + dx`, `y * scale + dy`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Map {
    pub scale: f32,
    pub dx: f32,
    pub dy: f32,
}

impl Map {
    pub fn point(&self, p: Point) -> Point {
        (p.0 * self.scale + self.dx, p.1 * self.scale + self.dy)
    }

    pub fn area(&self, a: &Area) -> Area {
        Area::between(self.point((a.x0, a.y0)), self.point((a.x1, a.y1)))
    }
}

/// Words' attributes in `face`: the machine's family for its typeface -- whatever fonts it has
/// for sans serif, serif and monospace, with the fallbacks for every script -- bold and italic
/// where asked (a font with no italic is slanted by the text engine).
fn attrs(face: &Face) -> Attrs<'static> {
    let family = match face.typeface {
        Typeface::SansSerif => Family::SansSerif,
        Typeface::Serif => Family::Serif,
        Typeface::Monospace => Family::Monospace,
    };
    Attrs::new()
        .family(family)
        .weight(if face.bold { Weight::BOLD } else { Weight::NORMAL })
        .style(if face.italic { Style::Italic } else { Style::Normal })
        .cache_key_flags(CacheKeyFlags::DISABLE_HINTING)
}

/// `words` set `px` large in `face`, as one line each (no wrapping): shaped, with whatever font
/// has the letters a script needs.
fn set(fonts: &mut Fonts, words: &str, px: f32, face: &Face) -> Buffer {
    let px = px.max(0.5);
    let mut buffer = Buffer::new(&mut fonts.system, Metrics::new(px, px * LEADING));
    {
        let mut b = buffer.borrow_with(&mut fonts.system);
        b.set_size(None, None);
        b.set_text(words, &attrs(face), Shaping::Advanced, None);
        b.shape_until_scroll(false);
    }
    buffer
}

/// How far in each line of set words begins: a line shorter than the longest lies in the words'
/// box as their alignment says.
fn line_starts(buffer: &Buffer, face: &Face) -> Vec<f32> {
    let widest = buffer.layout_runs().fold(0.0f32, |w, run| w.max(run.line_w));
    buffer.layout_runs().map(|run| (widest - run.line_w) * face.alignment.share()).collect()
}

/// How wide and tall `words` are set `size` large in `face`, in the units of `size`.
pub fn measure(fonts: &mut Fonts, words: &str, size: f32, face: &Face) -> (f32, f32) {
    let buffer = set(fonts, words, REFERENCE, face);
    let (mut width, mut lines) = (0.0f32, 0usize);
    for run in buffer.layout_runs() {
        width = width.max(run.line_w);
        lines += 1;
    }
    let scale = size / REFERENCE;
    (width * scale, lines.max(1) as f32 * REFERENCE * LEADING * scale)
}

/// A colour as tiny-skia draws it.
fn ink_paint(colour: Colour) -> Paint<'static> {
    let [r, g, b] = colour.rgb();
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, 255);
    paint.anti_alias = true;
    paint
}

fn outline_of(corners: &[Point]) -> Option<Path> {
    let mut path = PathBuilder::new();
    path.move_to(corners[0].0, corners[0].1);
    for c in &corners[1..] {
        path.line_to(c.0, c.1);
    }
    path.close();
    path.finish()
}

/// A stroke's points through `map`, each one not where the one before it is.
fn mapped(points: &[Point], map: Map) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for p in points.iter().map(|p| map.point(*p)) {
        if out.last() != Some(&p) {
            out.push(p);
        }
    }
    out
}

/// A pen's line through `points`, by the middles between them as [`stroke_line`] has it, to be
/// stroked: none for a dot.
fn pen_path(points: &[Point]) -> Option<Path> {
    if points.len() < 2 {
        return None;
    }
    let mut path = PathBuilder::new();
    path.move_to(points[0].0, points[0].1);
    for i in 1..points.len() - 1 {
        let (control, next) = (points[i], points[i + 1]);
        path.quad_to(control.0, control.1, (control.0 + next.0) * 0.5, (control.1 + next.1) * 0.5);
    }
    let last = points[points.len() - 1];
    path.line_to(last.0, last.1);
    path.finish()
}

/// What a highlighter's square tip, `half` either way, covers along `points`: one path of the
/// shapes it sweeps, each wound alike, to be filled.
fn highlighter_path(points: &[Point], half: f32) -> Option<Path> {
    let mut path = PathBuilder::new();
    for shape in highlighter_shapes(&stroke_line(points), half) {
        path.move_to(shape[0].0, shape[0].1);
        for p in &shape[1..] {
            path.line_to(p.0, p.1);
        }
        path.close();
    }
    path.finish()
}

/// A highlighter laid where `path` covers: what is painted under it already (`under`,
/// tiny-skia's, its colour multiplied by its alpha) multiplied by `colour` as Chromium shows it on
/// white, by as much as the path covers each pixel -- and, when the marks are a layer of their
/// own, `tint` too (three bytes a pixel and a fourth, white where nothing is): what the picture
/// under the layer is to be multiplied by. Nothing outside `clip`.
fn highlight(under: &mut PixmapMut, mut tint: Option<&mut [u8]>, path: &Path, colour: Colour, clip: Option<&Mask>) {
    let (w, h) = (under.width() as i32, under.height() as i32);
    let b = path.bounds();
    let (x0, y0) = ((b.left().floor() as i32 - 1).clamp(0, w), (b.top().floor() as i32 - 1).clamp(0, h));
    let (x1, y1) = ((b.right().ceil() as i32 + 1).clamp(0, w), (b.bottom().ceil() as i32 + 1).clamp(0, h));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    // Where it covers, in a mask of its own bounds only: one the picture's size for every stroke
    // would be the picture's size again for each.
    let (bw, bh) = ((x1 - x0) as u32, (y1 - y0) as u32);
    let Some(mut cover) = Mask::new(bw, bh) else { return };
    let Some(moved) = path.clone().transform(Transform::from_translate(-x0 as f32, -y0 as f32)) else { return };
    cover.fill_path(&moved, FillRule::Winding, true, Transform::identity());
    let m = colour.on_white().rgb().map(|v| v as u32);
    let data = under.data_mut();
    for y in 0..bh {
        for x in 0..bw {
            let mut a = cover.data()[(y * bw + x) as usize] as u32;
            if a == 0 {
                continue;
            }
            let i = ((y0 as u32 + y) * w as u32 + x0 as u32 + x) as usize;
            if let Some(clip) = clip {
                a = a * clip.data()[i] as u32 / 255;
                if a == 0 {
                    continue;
                }
            }
            // What is there times 1 - a (1 - m): white where it does not cover, its colour where
            // it does. In 255ths of 255ths.
            for k in 0..3 {
                let f = 65025 - a * (255 - m[k]);
                let c = &mut data[i * 4 + k];
                *c = ((*c as u32 * f + 32512) / 65025) as u8;
                if let Some(tint) = tint.as_deref_mut() {
                    let t = &mut tint[i * 4 + k];
                    *t = ((*t as u32 * f + 32512) / 65025) as u8;
                }
            }
        }
    }
}

/// Paint `notes` into `pixmap` (its pixels multiplied by their alpha, as tiny-skia keeps them)
/// through `map`, nothing outside `clip` (in its pixels). `tint` is where the highlighters go
/// when `pixmap` is a layer of the marks alone ([`Layers::tint`], its size); without it they
/// multiply `pixmap` itself, a picture being painted on.
pub fn paint(pixmap: &mut PixmapMut, mut tint: Option<&mut [u8]>, notes: &[&Note], map: Map, clip: Option<Area>, fonts: &mut Fonts) {
    let (width, height) = (pixmap.width(), pixmap.height());
    let mask = clip.and_then(|c| {
        let mut mask = Mask::new(width, height)?;
        let rect = SkRect::from_ltrb(c.x0, c.y0, c.x1.max(c.x0 + 0.01), c.y1.max(c.y0 + 0.01))?;
        mask.fill_path(&PathBuilder::from_rect(rect), FillRule::Winding, false, Transform::identity());
        Some(mask)
    });
    for note in notes {
        let paint = ink_paint(note.colour);
        let width_px = (note.size * map.scale).max(0.1);
        // A box or an oval of no size yet -- the one being drawn, as its press goes down -- is
        // nothing to stroke.
        if let Shape::Box(a) | Shape::Oval(a) = &note.shape {
            let a = map.area(a);
            if a.w() < 0.05 && a.h() < 0.05 {
                continue;
            }
        }
        match &note.shape {
            Shape::Box(a) => {
                let a = map.area(a);
                if let Some(path) = outline_of(&a.corners()) {
                    let stroke = Stroke { width: width_px, line_join: LineJoin::Miter, ..Stroke::default() };
                    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), mask.as_ref());
                }
            }
            Shape::Oval(a) => {
                let a = map.area(a);
                let stroke = Stroke { width: width_px, line_cap: LineCap::Round, ..Stroke::default() };
                let path = SkRect::from_ltrb(a.x0, a.y0, a.x1, a.y1).filter(|_| a.w() > 0.5 && a.h() > 0.5).and_then(PathBuilder::from_oval).or_else(|| outline_of(&[(a.x0, a.y0), (a.x1, a.y1)]));
                if let Some(path) = path {
                    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), mask.as_ref());
                }
            }
            Shape::Arrow { from, to } => {
                let (stop, head) = arrow_head(*from, *to, note.size);
                let (a, b) = (map.point(*from), map.point(stop));
                let mut line = PathBuilder::new();
                line.move_to(a.0, a.1);
                line.line_to(b.0, b.1);
                if let Some(path) = line.finish() {
                    let stroke = Stroke { width: width_px, line_cap: LineCap::Round, ..Stroke::default() };
                    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), mask.as_ref());
                }
                let head: Vec<Point> = head.iter().map(|p| map.point(*p)).collect();
                if let Some(path) = outline_of(&head) {
                    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), mask.as_ref());
                }
            }
            Shape::Text { at, words, face, .. } => {
                let origin = map.point(*at);
                let bounds = clip.unwrap_or(Area::new(0.0, 0.0, width as f32, height as f32));
                paint_words(pixmap, words, origin, note.size * map.scale, note.colour.rgb(), face, bounds, fonts);
            }
            Shape::Stroke { brush: Brush::Pen, points } => {
                let points = mapped(points, map);
                match pen_path(&points) {
                    Some(path) => {
                        let stroke = Stroke { width: width_px, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
                        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), mask.as_ref());
                    }
                    // A press that did not move: a dot, the tip's own.
                    None => {
                        if let Some(dot) = points.first().and_then(|p| PathBuilder::from_circle(p.0, p.1, width_px * 0.5)) {
                            pixmap.fill_path(&dot, &paint, FillRule::Winding, Transform::identity(), mask.as_ref());
                        }
                    }
                }
            }
            Shape::Stroke { brush: Brush::Highlighter, points } => {
                if let Some(path) = highlighter_path(&mapped(points, map), width_px * 0.5) {
                    highlight(pixmap, tint.as_deref_mut(), &path, note.colour, mask.as_ref());
                }
            }
        }
    }
}

/// Words set `px` large in `face` with their box's top left at `origin`, in `rgb`, blended into
/// `pixmap` within `bounds`.
#[allow(clippy::too_many_arguments)]
fn paint_words(pixmap: &mut PixmapMut, words: &str, origin: Point, px: f32, rgb: [u8; 3], face: &Face, bounds: Area, fonts: &mut Fonts) {
    let (width, height) = (pixmap.width() as i32, pixmap.height() as i32);
    let (x0, y0) = (bounds.x0.floor().max(0.0) as i32, bounds.y0.floor().max(0.0) as i32);
    let (x1, y1) = ((bounds.x1.ceil() as i32).min(width), (bounds.y1.ceil() as i32).min(height));
    let buffer = set(fonts, words, px, face);
    let starts = line_starts(&buffer, face);
    let data = pixmap.data_mut();
    for (run, start) in buffer.layout_runs().zip(starts) {
        for glyph in run.glyphs.iter() {
            let physical = glyph.physical((origin.0 + start, origin.1 + run.line_y), 1.0);
            let Some(image) = fonts.cache.get_image_uncached(&mut fonts.system, physical.cache_key) else { continue };
            let (left, top) = (physical.x + image.placement.left, physical.y - image.placement.top);
            let (w, h) = (image.placement.width as i32, image.placement.height as i32);
            for gy in 0..h {
                let y = top + gy;
                if y < y0 || y >= y1 {
                    continue;
                }
                for gx in 0..w {
                    let x = left + gx;
                    if x < x0 || x >= x1 {
                        continue;
                    }
                    let i = (gy * w + gx) as usize;
                    let colour = match image.content {
                        SwashContent::Mask => [rgb[0], rgb[1], rgb[2], image.data[i]],
                        SwashContent::SubpixelMask => [rgb[0], rgb[1], rgb[2], image.data[i * 4 + 3]],
                        SwashContent::Color => [image.data[i * 4], image.data[i * 4 + 1], image.data[i * 4 + 2], image.data[i * 4 + 3]],
                    };
                    blend(&mut data[((y * width + x) * 4) as usize..][..4], colour);
                }
            }
        }
    }
}

/// One pixel, its alpha not multiplied in, over one of tiny-skia's, whose is.
fn blend(under: &mut [u8], over: [u8; 4]) {
    let a = over[3] as u32;
    if a == 0 {
        return;
    }
    for k in 0..3 {
        under[k] = ((over[k] as u32 * a + under[k] as u32 * (255 - a) + 127) / 255) as u8;
    }
    under[3] = (a + under[3] as u32 * (255 - a) / 255).min(255) as u8;
}

/// Pixels with their alpha multiplied in, as tiny-skia keeps them, and back: a picture's and
/// the GPU's are not.
#[cfg(test)]
pub fn premultiply(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
}

pub fn unpremultiply(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

/// Whether any of `notes` is a highlighter's, and so needs a tint to be laid with.
pub fn tints(notes: &[&Note]) -> bool {
    notes.iter().any(|n| n.brush() == Some(Brush::Highlighter))
}

/// The marks painted as layers of their own, to be laid over a picture: `over`, the marks
/// themselves (four bytes a pixel, alpha multiplied in, as tiny-skia keeps them), which is laid
/// over it; and `tint`, what the highlighters multiply it by first (three bytes a pixel and a
/// fourth, 255; white where there is none), none if there are no highlighters.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layers {
    pub over: Vec<u8>,
    pub tint: Option<Vec<u8>>,
}

/// `notes` painted on nothing, `width` by `height`, through `map`: the marks' layers, to be laid
/// over a picture being saved at the picture's own depth (`depth::lay`).
pub fn layer(width: u32, height: u32, notes: &[&Note], map: Map) -> Result<Layers, String> {
    let size = width as usize * height as usize * 4;
    let mut over = vec![0u8; size];
    let mut tint = tints(notes).then(|| vec![255u8; size]);
    if !notes.is_empty() {
        let mut pixmap = PixmapMut::from_bytes(&mut over, width, height).ok_or("a picture of no size")?;
        paint(&mut pixmap, tint.as_deref_mut(), notes, map, None, &mut fonts());
    }
    Ok(Layers { over, tint })
}

/// Paint `notes` over a picture `width` by `height` (four bytes a pixel, alpha not multiplied
/// in) through `map`: a picture being saved, at its own size.
#[cfg(test)]
pub fn paint_over(rgba: &mut [u8], width: u32, height: u32, notes: &[&Note], map: Map) -> Result<(), String> {
    premultiply(rgba);
    {
        let mut pixmap = PixmapMut::from_bytes(rgba, width, height).ok_or("a picture of no size")?;
        paint(&mut pixmap, None, notes, map, None, &mut fonts());
    }
    unpremultiply(rgba);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::{Alignment, Typeface, HIGHLIGHTER_COLOURS, PEN_COLOURS};

    const RED: Colour = Colour([235, 51, 36]);
    const GREEN: Colour = Colour([50, 180, 75]);
    const BLUE: Colour = Colour([35, 115, 235]);
    const WHITE: Colour = Colour([255, 255, 255]);
    const INK: Colour = Colour([20, 20, 22]);

    fn note(shape: Shape, colour: Colour, size: f32) -> Note {
        Note::new(0, shape, colour, size)
    }

    fn words(at: Point, words: &str) -> Shape {
        Shape::Text { at, words: words.into(), extent: (0.0, 0.0), face: Face::default() }
    }

    fn stroke(brush: Brush, points: &[Point], colour: Colour, size: f32) -> Note {
        Note::new(0, Shape::Stroke { brush, points: points.to_vec() }, colour, size)
    }

    fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * width + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    const ONE: Map = Map { scale: 1.0, dx: 0.0, dy: 0.0 };

    #[test]
    fn a_red_box_s_edge_is_where_it_should_be_and_its_inside_is_untouched() {
        let (w, h) = (200u32, 100u32);
        let mut rgba = [255u8, 255, 255, 255].repeat((w * h) as usize);
        let boxed = note(Shape::Box(Area::new(20.0, 10.0, 120.0, 60.0)), RED, 4.0);
        // Two pixels to a unit, moved: the units (20, 10)..(120, 60) are the pixels
        // (20, 10)..(220, 110), and the line 8 wide. The left edge is at x 20, from 16 to 24;
        // the top at y 10; the right and the foot are off the picture.
        paint_over(&mut rgba, w, h, &[&boxed], Map { scale: 2.0, dx: -20.0, dy: -10.0 }).unwrap();
        assert_eq!(pixel(&rgba, w, 20, 50), [235, 51, 36, 255], "on the left edge");
        assert_eq!(pixel(&rgba, w, 17, 50)[1], 51, "the edge is 8 wide, centred on it");
        assert_eq!(pixel(&rgba, w, 12, 50), [255, 255, 255, 255], "outside it");
        assert_eq!(pixel(&rgba, w, 100, 10), [235, 51, 36, 255], "on the top edge");
        assert_eq!(pixel(&rgba, w, 100, 50), [255, 255, 255, 255], "inside, untouched");
    }

    #[test]
    fn an_oval_an_arrow_and_words_are_drawn() {
        let (w, h) = (300u32, 200u32);
        let mut rgba = [0u8, 0, 0, 255].repeat((w * h) as usize);
        let notes = [
            note(Shape::Oval(Area::new(10.0, 10.0, 110.0, 60.0)), GREEN, 3.0),
            note(Shape::Arrow { from: (150.0, 150.0), to: (290.0, 150.0) }, BLUE, 4.0),
            note(words((10.0, 100.0), "Grüße 你好"), WHITE, 30.0),
        ];
        let refs: Vec<&Note> = notes.iter().collect();
        paint_over(&mut rgba, w, h, &refs, ONE).unwrap();
        // The oval's leftmost point, at its middle height; not its corner.
        assert_eq!(pixel(&rgba, w, 10, 35), [50, 180, 75, 255]);
        assert_eq!(pixel(&rgba, w, 12, 12), [0, 0, 0, 255]);
        // The arrow's line and its head, wider than the line, and its tip.
        assert_eq!(pixel(&rgba, w, 200, 150), [35, 115, 235, 255]);
        assert_eq!(pixel(&rgba, w, 275, 155)[2], 235, "the head is wider than the line");
        assert_eq!(pixel(&rgba, w, 200, 156), [0, 0, 0, 255], "the line is not");
        // The words have ink in their box, white on black, and none past it.
        let inked = |x0: u32, x1: u32| (100..140).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| pixel(&rgba, w, x, y)[0] > 128).count();
        assert!(inked(10, 300) > 150, "words drawn: {}", inked(10, 300));
        // Both scripts: something in the right part, where the Chinese is set.
        let (width, _) = measure(&mut fonts(), "Grüße ", 30.0, &Face::default());
        assert!(inked(10 + width as u32 + 2, 300) > 40, "the Chinese drawn too ({width})");
    }

    /// Words painted in black on white, `w` by `h`, from the top left: where they inked, as the
    /// leftmost, topmost, rightmost and lowest pixels with ink, those below `below` only.
    fn inked(text: &str, size: f32, face: Face, (w, h): (u32, u32), below: u32) -> (Vec<u8>, [u32; 4]) {
        let mut rgba = [255u8, 255, 255, 255].repeat((w * h) as usize);
        let written = Note::new(0, Shape::Text { at: (0.0, 0.0), words: text.into(), extent: (0.0, 0.0), face }, INK, size);
        paint_over(&mut rgba, w, h, &[&written], ONE).unwrap();
        let mut bounds = [u32::MAX, u32::MAX, 0, 0];
        for y in below..h {
            for x in 0..w {
                if pixel(&rgba, w, x, y)[0] < 160 {
                    bounds = [bounds[0].min(x), bounds[1].min(y), bounds[2].max(x), bounds[3].max(y)];
                }
            }
        }
        (rgba, bounds)
    }

    #[test]
    fn words_measure_in_proportion_to_their_size_and_are_painted_within_that_box() {
        let plain = Face::default();
        let (small, large, two) = {
            let mut fonts = fonts();
            (measure(&mut fonts, "Arrow here", 20.0, &plain), measure(&mut fonts, "Arrow here", 40.0, &plain), measure(&mut fonts, "a\nb", 20.0, &plain))
        };
        assert!((large.0 - small.0 * 2.0).abs() < 0.01 && (large.1 - small.1 * 2.0).abs() < 0.01, "{small:?} {large:?}");
        assert!(small.0 > 60.0 && small.0 < 200.0 && (small.1 - 25.0).abs() < 0.01, "{small:?}");
        // Two lines are twice as tall.
        assert!((two.1 - 50.0).abs() < 0.01);
        // Painted, the ink is within the box measured -- give or take its edge's pixel -- and fills
        // most of its width and the capital's height of it.
        let (_, [x0, y0, x1, y1]) = inked("Arrow here", 20.0, plain, (240, 60), 0);
        assert!(x0 <= 3 && (x1 as f32) <= small.0 + 1.0 && x1 as f32 > small.0 * 0.8, "{x0} {x1} {small:?}");
        assert!((y1 as f32) <= small.1 + 1.0 && y1 - y0 > 10, "{y0} {y1} {small:?}");
    }

    #[test]
    fn a_typeface_bold_and_italic_set_words_otherwise() {
        let face = |typeface, bold, italic| Face { typeface, bold, italic, alignment: Alignment::Left };
        let (sans, mono, bold) = {
            let mut fonts = fonts();
            let mut width = |f: Face| measure(&mut fonts, "Illuminated minimum", 20.0, &f).0;
            (width(face(Typeface::SansSerif, false, false)), width(face(Typeface::Monospace, false, false)), width(face(Typeface::SansSerif, true, false)))
        };
        // A monospaced font gives the narrow letters as much room as the wide; bold is wider.
        assert!(mono > sans * 1.1, "{mono} {sans}");
        assert!(bold > sans * 1.02, "{bold} {sans}");
        // Italic is slanted: painted, it is not the upright letter.
        let (upright, _) = inked("l", 40.0, face(Typeface::SansSerif, false, false), (60, 60), 0);
        let (italic, _) = inked("l", 40.0, face(Typeface::SansSerif, false, true), (60, 60), 0);
        assert_ne!(upright, italic);
        // Lines of a centred note lie about its middle: the short one in from the left.
        let centred = Face { alignment: Alignment::Center, ..Face::default() };
        let (_, shifted) = inked("WWWWWW\ni", 20.0, centred, (300, 60), 26);
        let (_, plain) = inked("WWWWWW\ni", 20.0, Face::default(), (300, 60), 26);
        assert!(shifted[0] > plain[0] + 20, "{shifted:?} {plain:?}");
    }

    #[test]
    fn a_pen_draws_round_ended_lines_and_dots() {
        let (w, h) = (200u32, 100u32);
        let mut rgba = [255u8, 255, 255, 255].repeat((w * h) as usize);
        let red = PEN_COLOURS[10].colour;
        let notes = [stroke(Brush::Pen, &[(20.0, 50.0), (60.0, 50.0), (100.0, 50.0)], red, 10.0), stroke(Brush::Pen, &[(150.0, 50.0)], red, 10.0)];
        let refs: Vec<&Note> = notes.iter().collect();
        paint_over(&mut rgba, w, h, &refs, ONE).unwrap();
        assert_eq!(pixel(&rgba, w, 60, 50), [0xea, 0x43, 0x35, 255], "along the line");
        assert_eq!(pixel(&rgba, w, 60, 57), [255, 255, 255, 255], "5 either way of it and no more");
        // Its ends are round: beyond its last point by half its width straight on, not at the
        // corner a square end would have.
        assert!(pixel(&rgba, w, 103, 50)[1] < 120, "{:?}", pixel(&rgba, w, 103, 50));
        assert!(pixel(&rgba, w, 104, 54)[1] > 200, "{:?}", pixel(&rgba, w, 104, 54));
        // A press that did not move: a dot as wide as the pen.
        assert_eq!(pixel(&rgba, w, 150, 50), [0xea, 0x43, 0x35, 255]);
        assert!(pixel(&rgba, w, 150, 53)[1] < 120 && pixel(&rgba, w, 154, 54)[1] > 200);
    }

    #[test]
    fn a_highlighter_on_white_is_chromium_s_and_lets_what_is_under_it_show() {
        let (w, h) = (200u32, 60u32);
        // White on the left, black on the right, and a highlighter across both, 16 tall.
        let mut rgba: Vec<u8> = (0..w * h).flat_map(|i| if i % w < 100 { [255, 255, 255, 255] } else { [0, 0, 0, 255] }).collect();
        let yellow = HIGHLIGHTER_COLOURS[1].colour;
        let marker = stroke(Brush::Highlighter, &[(10.0, 30.0), (190.0, 30.0)], yellow, 16.0);
        paint_over(&mut rgba, w, h, &[&marker], ONE).unwrap();
        // On white: Chromium's yellow at 40 per cent over white, (fdd663) to (fef3c1).
        assert_eq!(pixel(&rgba, w, 50, 30), [0xfe, 0xef, 0xc1, 255]);
        assert_eq!(yellow.on_white(), Colour([0xfe, 0xef, 0xc1]));
        // On black: still black, as under a highlighter's ink; and nothing past its tip.
        assert_eq!(pixel(&rgba, w, 150, 30), [0, 0, 0, 255]);
        assert_eq!(pixel(&rgba, w, 50, 39), [255, 255, 255, 255]);
        // Its tip is square and upright: its end is square, out to the tip's corner.
        assert_eq!(pixel(&rgba, w, 3, 23), [0xfe, 0xef, 0xc1, 255]);
        // Twice over the same place: twice as deep, as two highlighters are.
        let mut twice = [255u8, 255, 255, 255].repeat((w * h) as usize);
        paint_over(&mut twice, w, h, &[&marker, &marker], ONE).unwrap();
        assert!(pixel(&twice, w, 50, 30)[2] < 0xc1 - 30, "{:?}", pixel(&twice, w, 50, 30));
        // A stroke over itself is laid once.
        let mut doubled = [255u8, 255, 255, 255].repeat((w * h) as usize);
        let back = stroke(Brush::Highlighter, &[(10.0, 30.0), (190.0, 30.0), (20.0, 30.0)], yellow, 16.0);
        paint_over(&mut doubled, w, h, &[&back], ONE).unwrap();
        assert_eq!(pixel(&doubled, w, 50, 30), [0xfe, 0xef, 0xc1, 255]);
    }

    #[test]
    fn the_layers_laid_over_a_picture_are_the_marks_painted_on_it() {
        // A picture: a ramp, with a white band. Marks in an order a layer must keep: a pen line,
        // a highlighter over it, words over that.
        let (w, h) = (120u32, 80u32);
        let picture: Vec<u8> = (0..w * h).flat_map(|i| {
            let (x, y) = (i % w, i / w);
            if y > 60 { [255, 255, 255, 255] } else { [(x * 2) as u8, 120, (255 - x * 2) as u8, 255] }
        }).collect();
        let notes = [
            stroke(Brush::Pen, &[(10.0, 20.0), (110.0, 20.0)], PEN_COLOURS[13].colour, 6.0),
            stroke(Brush::Highlighter, &[(60.0, 5.0), (60.0, 75.0)], HIGHLIGHTER_COLOURS[0].colour, 16.0),
            note(words((5.0, 40.0), "Hi"), INK, 20.0),
        ];
        let refs: Vec<&Note> = notes.iter().collect();
        let mut direct = picture.clone();
        paint_over(&mut direct, w, h, &refs, ONE).unwrap();
        let layers = layer(w, h, &refs, ONE).unwrap();
        let tint = layers.tint.as_ref().expect("a highlighter's tint");
        // Laid: the picture multiplied by the tint, then the marks over it.
        let mut laid = picture.clone();
        for (i, px) in laid.chunks_exact_mut(4).enumerate() {
            let (t, m) = (&tint[i * 4..i * 4 + 3], &layers.over[i * 4..i * 4 + 4]);
            for k in 0..3 {
                let under = px[k] as f32 * t[k] as f32 / 255.0;
                px[k] = (m[k] as f32 + under * (1.0 - m[3] as f32 / 255.0)).round() as u8;
            }
        }
        let worst = laid.iter().zip(&direct).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(worst <= 2, "the layers come to the painting, but for rounding: {worst}");
        // The pen's line under the highlighter is tinted by it, as painted in that order.
        assert_ne!(pixel(&laid, w, 60, 20), pixel(&laid, w, 20, 20));
        // No highlighter, no tint.
        assert!(layer(w, h, &refs[..1], ONE).unwrap().tint.is_none());
    }

    #[test]
    fn nothing_is_drawn_outside_the_clip() {
        let (w, h) = (100u32, 100u32);
        let mut data = vec![0u8; (w * h * 4) as usize];
        let mut tint = vec![255u8; (w * h * 4) as usize];
        let boxed = note(Shape::Box(Area::new(10.0, 10.0, 90.0, 90.0)), RED, 6.0);
        let written = note(words((0.0, 40.0), "WWWWWWWW"), RED, 30.0);
        let marker = stroke(Brush::Highlighter, &[(0.0, 70.0), (100.0, 70.0)], HIGHLIGHTER_COLOURS[3].colour, 12.0);
        {
            let mut pixmap = PixmapMut::from_bytes(&mut data, w, h).unwrap();
            paint(&mut pixmap, Some(&mut tint), &[&boxed, &written, &marker], ONE, Some(Area::new(0.0, 0.0, 50.0, 100.0)), &mut fonts());
        }
        assert!(pixel(&data, w, 10, 50)[3] > 0, "the left edge, inside");
        assert!(pixel(&tint, w, 20, 70)[0] < 255, "the highlighter, inside");
        for y in 0..h {
            for x in 51..w {
                assert_eq!(pixel(&data, w, x, y)[3], 0, "({x}, {y}) is outside");
                assert_eq!(pixel(&tint, w, x, y), [255, 255, 255, 255], "({x}, {y}) is outside");
            }
        }
    }

    #[test]
    fn alpha_goes_in_and_out_again() {
        let mut px = vec![200u8, 100, 50, 128, 10, 20, 30, 255, 1, 2, 3, 0];
        let before = px.clone();
        premultiply(&mut px);
        assert_eq!(&px[..4], &[100, 50, 25, 128]);
        unpremultiply(&mut px);
        // Within a step of what it was; a clear pixel stays clear, its colour nothing.
        for (a, b) in px[..8].iter().zip(&before) {
            assert!((*a as i32 - *b as i32).abs() <= 2, "{px:?} {before:?}");
        }
        assert_eq!(px[11], 0);
    }
}
