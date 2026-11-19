//! Marks painted: one painting for the window, at the size a page is shown, and for a picture
//! saved, at its own size -- so that what is saved is what was seen, on the CPU and so with no
//! GPU's limit on a picture's size. Lines and shapes by tiny-skia (resvg's); words by
//! cosmic-text with the room's own fonts and their fallbacks for every script
//! (`spatiand_render::text::font_system`), whose glyph outlines are also a PDF's words
//! (`annotate`).

use std::sync::{Mutex, MutexGuard, OnceLock};

use cosmic_text::{Attrs, Buffer, CacheKeyFlags, Command, Family, FontSystem, Metrics, Shaping, SwashCache, SwashContent};
use resvg::tiny_skia::{FillRule, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, PixmapMut, Rect as SkRect, Stroke, Transform};

use crate::markup::{arrow_head, Area, Note, Point, Shape};

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

fn attrs() -> Attrs<'static> {
    Attrs::new().family(Family::SansSerif).cache_key_flags(CacheKeyFlags::DISABLE_HINTING)
}

/// `words` set `px` large, as one line each (no wrapping): the room's sans serif, shaped, with
/// whatever font has the letters a script needs.
fn set(fonts: &mut Fonts, words: &str, px: f32) -> Buffer {
    let px = px.max(0.5);
    let mut buffer = Buffer::new(&mut fonts.system, Metrics::new(px, px * LEADING));
    {
        let mut b = buffer.borrow_with(&mut fonts.system);
        b.set_size(None, None);
        b.set_text(words, &attrs(), Shaping::Advanced, None);
        b.shape_until_scroll(false);
    }
    buffer
}

/// How wide and tall `words` are set `size` large, in the units of `size`.
pub fn measure(fonts: &mut Fonts, words: &str, size: f32) -> (f32, f32) {
    let buffer = set(fonts, words, REFERENCE);
    let (mut width, mut lines) = (0.0f32, 0usize);
    for run in buffer.layout_runs() {
        width = width.max(run.line_w);
        lines += 1;
    }
    let scale = size / REFERENCE;
    (width * scale, lines.max(1) as f32 * REFERENCE * LEADING * scale)
}

/// One step of a glyph's outline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    Move(Point),
    Line(Point),
    Quad(Point, Point),
    Cubic(Point, Point, Point),
    Close,
}

/// The outlines of `words` set `size` large, their box's top left at (0, 0), down the page as
/// a page's units are: what a PDF's words are drawn with, which then need no font in the PDF.
/// A glyph with no outline (a colour emoji's picture) is left out.
pub fn outlines(fonts: &mut Fonts, words: &str, size: f32) -> Vec<Segment> {
    let buffer = set(fonts, words, REFERENCE);
    let scale = size / REFERENCE;
    let mut out = Vec::new();
    for run in buffer.layout_runs() {
        for glyph in run.glyphs.iter() {
            // Where the glyph's origin is, as `LayoutGlyph::physical` puts it, but not rounded.
            let (gx, gy) = (glyph.x + glyph.font_size * glyph.x_offset, run.line_y + glyph.y - glyph.font_size * glyph.y_offset);
            let key = glyph.physical((0.0, 0.0), 1.0).cache_key;
            let Some(commands) = fonts.cache.get_outline_commands_uncached(&mut fonts.system, key) else { continue };
            // The outline's y is up from the baseline; the page's is down.
            let p = |x: f32, y: f32| ((gx + x) * scale, (gy - y) * scale);
            for command in commands.iter() {
                out.push(match *command {
                    Command::MoveTo(v) => Segment::Move(p(v.x, v.y)),
                    Command::LineTo(v) => Segment::Line(p(v.x, v.y)),
                    Command::QuadTo(c, v) => Segment::Quad(p(c.x, c.y), p(v.x, v.y)),
                    Command::CurveTo(a, b, v) => Segment::Cubic(p(a.x, a.y), p(b.x, b.y), p(v.x, v.y)),
                    Command::Close => Segment::Close,
                });
            }
        }
    }
    out
}

/// A note's line, as tiny-skia draws it.
fn ink_paint(note: &Note) -> Paint<'static> {
    let [r, g, b] = note.ink.rgb();
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

/// Paint `notes` into `pixmap` (its pixels multiplied by their alpha, as tiny-skia keeps them)
/// through `map`, nothing outside `clip` (in its pixels).
pub fn paint(pixmap: &mut PixmapMut, notes: &[&Note], map: Map, clip: Option<Area>, fonts: &mut Fonts) {
    let (width, height) = (pixmap.width(), pixmap.height());
    let mask = clip.and_then(|c| {
        let mut mask = Mask::new(width, height)?;
        let rect = SkRect::from_ltrb(c.x0, c.y0, c.x1.max(c.x0 + 0.01), c.y1.max(c.y0 + 0.01))?;
        mask.fill_path(&PathBuilder::from_rect(rect), FillRule::Winding, false, Transform::identity());
        Some(mask)
    });
    for note in notes {
        let paint = ink_paint(note);
        let width_px = (note.size * map.scale).max(0.1);
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
            Shape::Text { at, words, .. } => {
                let origin = map.point(*at);
                let bounds = clip.unwrap_or(Area::new(0.0, 0.0, width as f32, height as f32));
                paint_words(pixmap, words, origin, note.size * map.scale, note.ink.rgb(), bounds, fonts);
            }
        }
    }
}

/// Words set `px` large with their box's top left at `origin`, in `rgb`, blended into `pixmap`
/// within `bounds`.
fn paint_words(pixmap: &mut PixmapMut, words: &str, origin: Point, px: f32, rgb: [u8; 3], bounds: Area, fonts: &mut Fonts) {
    let (width, height) = (pixmap.width() as i32, pixmap.height() as i32);
    let (x0, y0) = (bounds.x0.floor().max(0.0) as i32, bounds.y0.floor().max(0.0) as i32);
    let (x1, y1) = ((bounds.x1.ceil() as i32).min(width), (bounds.y1.ceil() as i32).min(height));
    let buffer = set(fonts, words, px);
    let data = pixmap.data_mut();
    for run in buffer.layout_runs() {
        for glyph in run.glyphs.iter() {
            let physical = glyph.physical((origin.0, origin.1 + run.line_y), 1.0);
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

/// Paint `notes` over a picture `width` by `height` (four bytes a pixel, alpha not multiplied
/// in) through `map`: a picture being saved, at its own size.
pub fn paint_over(rgba: &mut [u8], width: u32, height: u32, notes: &[&Note], map: Map) -> Result<(), String> {
    premultiply(rgba);
    {
        let mut pixmap = PixmapMut::from_bytes(rgba, width, height).ok_or("a picture of no size")?;
        paint(&mut pixmap, notes, map, None, &mut fonts());
    }
    unpremultiply(rgba);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::{Ink, Weight};

    fn note(shape: Shape, ink: Ink, size: f32) -> Note {
        Note { page: 0, shape, ink, weight: Weight::Medium, size }
    }

    fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * width + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    #[test]
    fn a_red_box_s_edge_is_where_it_should_be_and_its_inside_is_untouched() {
        let (w, h) = (200u32, 100u32);
        let mut rgba = [255u8, 255, 255, 255].repeat((w * h) as usize);
        let boxed = note(Shape::Box(Area::new(20.0, 10.0, 120.0, 60.0)), Ink::Red, 4.0);
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
            note(Shape::Oval(Area::new(10.0, 10.0, 110.0, 60.0)), Ink::Green, 3.0),
            note(Shape::Arrow { from: (150.0, 150.0), to: (290.0, 150.0) }, Ink::Blue, 4.0),
            note(Shape::Text { at: (10.0, 100.0), words: "Grüße 你好".into(), extent: (0.0, 0.0) }, Ink::White, 30.0),
        ];
        let refs: Vec<&Note> = notes.iter().collect();
        paint_over(&mut rgba, w, h, &refs, Map { scale: 1.0, dx: 0.0, dy: 0.0 }).unwrap();
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
        let (width, _) = measure(&mut fonts(), "Grüße ", 30.0);
        assert!(inked(10 + width as u32 + 2, 300) > 40, "the Chinese drawn too ({width})");
    }

    #[test]
    fn words_measure_in_proportion_to_their_size_and_their_outlines_fill_that_box() {
        let mut fonts = fonts();
        let small = measure(&mut fonts, "Arrow here", 20.0);
        let large = measure(&mut fonts, "Arrow here", 40.0);
        assert!((large.0 - small.0 * 2.0).abs() < 0.01 && (large.1 - small.1 * 2.0).abs() < 0.01, "{small:?} {large:?}");
        assert!(small.0 > 60.0 && small.0 < 200.0 && (small.1 - 25.0).abs() < 0.01, "{small:?}");
        // Two lines are twice as tall.
        assert!((measure(&mut fonts, "a\nb", 20.0).1 - 50.0).abs() < 0.01);
        let outline = outlines(&mut fonts, "Arrow here", 20.0);
        assert!(outline.len() > 40, "{}", outline.len());
        let points: Vec<Point> = outline
            .iter()
            .filter_map(|s| match s {
                Segment::Move(p) | Segment::Line(p) | Segment::Quad(_, p) | Segment::Cubic(_, _, p) => Some(*p),
                Segment::Close => None,
            })
            .collect();
        let (x0, x1) = points.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
        let (y0, y1) = points.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
        // Within the box, and down the page: the capital's top above its baseline.
        assert!(x0 >= -0.5 && x1 <= small.0 + 0.5, "{x0} {x1} {small:?}");
        assert!(y0 >= 0.0 && y1 <= small.1 && y1 - y0 > 10.0, "{y0} {y1}");
    }

    #[test]
    fn nothing_is_drawn_outside_the_clip() {
        let (w, h) = (100u32, 100u32);
        let mut data = vec![0u8; (w * h * 4) as usize];
        let boxed = note(Shape::Box(Area::new(10.0, 10.0, 90.0, 90.0)), Ink::Red, 6.0);
        let words = note(Shape::Text { at: (0.0, 40.0), words: "WWWWWWWW".into(), extent: (0.0, 0.0) }, Ink::Red, 30.0);
        {
            let mut pixmap = PixmapMut::from_bytes(&mut data, w, h).unwrap();
            paint(&mut pixmap, &[&boxed, &words], Map { scale: 1.0, dx: 0.0, dy: 0.0 }, Some(Area::new(0.0, 0.0, 50.0, 100.0)), &mut fonts());
        }
        assert!(pixel(&data, w, 10, 50)[3] > 0, "the left edge, inside");
        for y in 0..h {
            for x in 51..w {
                assert_eq!(pixel(&data, w, x, y)[3], 0, "({x}, {y}) is outside");
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
