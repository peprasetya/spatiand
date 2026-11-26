//! Markup: what the wearer draws over a picture -- lines drawn by hand with a pen or a
//! highlighter, boxes, ovals, arrows and words -- and what is cropped away, kept as shapes while
//! the file is open, every change one that can be undone. Nothing here draws or writes: `ink`
//! paints these, and `save` writes a picture with them.
//!
//! Each shape is in the picture's own pixels, the picture as it is shown (turned as it is
//! turned): its page, page 0, the only one a picture has.
//!
//! A picture is drawn on with the tools of Chromium's PDF viewer (the owner, 2026-11-27:
//! "chromium tools. spatiloupe tools match chromium, if not exists box circle arrow crop, add
//! for pictures"): its pen, highlighter and eraser and its words, in its own colours and sizes,
//! kept as its own values -- a colour's hex, a size's number, a typeface's, a style's and an
//! alignment's names -- from Chromium 157's `chrome/browser/resources/pdf` and `pdf/`. A PDF is
//! marked up with those tools themselves, in SpatiMesh's viewer, which shows it.

/// A point on a page, in its units, down from its top left.
pub type Point = (f32, f32);

/// A rectangle on a page, its corners in order: `x0 <= x1`, `y0 <= y1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Area {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Area {
        Area::between((x0, y0), (x1, y1))
    }

    /// The rectangle with these two corners, whichever they are.
    pub fn between(a: Point, b: Point) -> Area {
        Area { x0: a.0.min(b.0), y0: a.1.min(b.1), x1: a.0.max(b.0), y1: a.1.max(b.1) }
    }

    pub fn w(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn h(&self) -> f32 {
        self.y1 - self.y0
    }

    pub fn centre(&self) -> Point {
        ((self.x0 + self.x1) * 0.5, (self.y0 + self.y1) * 0.5)
    }

    pub fn contains(&self, p: Point) -> bool {
        p.0 >= self.x0 && p.0 <= self.x1 && p.1 >= self.y0 && p.1 <= self.y1
    }

    /// Larger by `by` all round (smaller, for less than nought).
    pub fn grown(&self, by: f32) -> Area {
        Area { x0: self.x0 - by, y0: self.y0 - by, x1: self.x1 + by, y1: self.y1 + by }
    }

    /// What two areas have in common: `None` if they do not meet.
    pub fn meet(&self, other: &Area) -> Option<Area> {
        let a = Area { x0: self.x0.max(other.x0), y0: self.y0.max(other.y0), x1: self.x1.min(other.x1), y1: self.y1.min(other.y1) };
        (a.x0 < a.x1 && a.y0 < a.y1).then_some(a)
    }

    /// The corners clockwise from the top left: the order of [`Handle::Corner`].
    pub fn corners(&self) -> [Point; 4] {
        [(self.x0, self.y0), (self.x1, self.y0), (self.x1, self.y1), (self.x0, self.y1)]
    }
}

/// A colour exactly as Chromium's PDF viewer keeps one -- `#rrggbb` (its `hexToColor` and
/// `colorToHex`, pdf_viewer_utils.ts) -- and as a mark is painted in: a drawing's own colour, the
/// same in the dark and the light look, not the room's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Colour(pub [u8; 3]);

/// How much of a highlighter's colour lies over what is under it, in Chromium (`GetOpacity`,
/// pdf_ink_brush.cc).
pub const HIGHLIGHTER_OPACITY: f32 = 0.4;

impl Colour {
    pub const BLACK: Colour = Colour::of(0x000000);

    /// From its hex as a number: `Colour::of(0xf28b82)`.
    pub const fn of(hex: u32) -> Colour {
        Colour([(hex >> 16) as u8, (hex >> 8) as u8, hex as u8])
    }

    pub fn rgb(self) -> [u8; 3] {
        self.0
    }

    /// As Chromium writes it: "#f28b82". What the palettes are held to against Chromium's source.
    #[cfg(test)]
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0[0], self.0[1], self.0[2])
    }

    /// As the painter takes one: each 0 to 1, opaque.
    pub fn unit(self) -> [f32; 4] {
        let [r, g, b] = self.0;
        [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
    }

    /// As a highlighter of it lies on white paper in Chromium: its colour at 40 per cent over
    /// white (`blendHighlighterColorValue`, pdf_viewer_utils.ts), which is how Chromium shows a
    /// highlighter's colours to choose from. What a highlighter multiplies what is under it by
    /// here, so that on white it is Chromium's to the pixel.
    pub fn on_white(self) -> Colour {
        Colour(self.0.map(|v| (v as f32 * HIGHLIGHTER_OPACITY + 255.0 * (1.0 - HIGHLIGHTER_OPACITY)).round() as u8))
    }
}

/// One of Chromium's colours to choose from, with the name its PDF viewer gives it (its strings,
/// components/pdf_strings.grdp).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Swatch {
    pub colour: Colour,
    pub name: &'static str,
}

const fn swatch(hex: u32, name: &'static str) -> Swatch {
    Swatch { colour: Colour::of(hex), name }
}

/// The pen's colours -- and so a box's, an oval's and an arrow's -- as Chromium has them
/// (`PEN_COLORS`, ink_annotation_brush_mixin.ts), in its order: four rows of five.
pub const PEN_COLOURS: [Swatch; 20] = [
    swatch(0x000000, "Black"),
    swatch(0x5f6368, "Dark Grey 2"),
    swatch(0x9aa0a6, "Dark Grey 1"),
    swatch(0xdadce0, "Light Grey"),
    swatch(0xffffff, "White"),
    swatch(0xf28b82, "Red 1"),
    swatch(0xfdd663, "Yellow 1"),
    swatch(0x81c995, "Green 1"),
    swatch(0x8ab4f8, "Blue 1"),
    swatch(0xeec9ae, "Tan 1"),
    swatch(0xea4335, "Red 2"),
    swatch(0xfbbc04, "Yellow 2"),
    swatch(0x34a853, "Green 2"),
    swatch(0x4285f4, "Blue 2"),
    swatch(0xe2a185, "Tan 2"),
    swatch(0xc5221f, "Red 3"),
    swatch(0xf29900, "Yellow 3"),
    swatch(0x188038, "Green 3"),
    swatch(0x1967d2, "Blue 3"),
    swatch(0x885945, "Tan 3"),
];

/// The highlighter's (`HIGHLIGHTER_COLORS`): two rows of five, each "blended" -- shown, and laid
/// over what is under it, see-through.
pub const HIGHLIGHTER_COLOURS: [Swatch; 10] = [
    swatch(0xf28b82, "Light Red"),
    swatch(0xfdd663, "Light Yellow"),
    swatch(0x34a853, "Light Green"),
    swatch(0x4285f4, "Light Blue"),
    swatch(0xffae80, "Light Orange"),
    swatch(0xd93025, "Red"),
    swatch(0xddf300, "Yellow"),
    swatch(0x25e387, "Green"),
    swatch(0x5379ff, "Blue"),
    swatch(0xff630c, "Orange"),
];

/// Words' (`TEXT_COLORS`, ink_annotation_text_mixin.ts): four rows of five, with cyans where the
/// pen has tans.
pub const TEXT_COLOURS: [Swatch; 20] = [
    swatch(0x000000, "Black"),
    swatch(0x5f6368, "Dark Grey 2"),
    swatch(0x9aa0a6, "Dark Grey 1"),
    swatch(0xdadce0, "Light Grey"),
    swatch(0xffffff, "White"),
    swatch(0xf28b82, "Red 1"),
    swatch(0xfdd663, "Yellow 1"),
    swatch(0x81c995, "Green 1"),
    swatch(0x78d9ec, "Cyan 1"),
    swatch(0x8ab4f8, "Blue 1"),
    swatch(0xe94235, "Red 2"),
    swatch(0xfbbc04, "Yellow 2"),
    swatch(0x34a853, "Green 2"),
    swatch(0x24c1e0, "Cyan 2"),
    swatch(0x4285f4, "Blue 2"),
    swatch(0xc5221f, "Red 3"),
    swatch(0xd56e0c, "Yellow 3"),
    swatch(0x188038, "Green 3"),
    swatch(0x12a4af, "Cyan 3"),
    swatch(0x1967d2, "Blue 3"),
];

/// One of Chromium's sizes for a brush, with its name.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub value: f32,
    pub name: &'static str,
}

/// The pen's sizes (`PEN_SIZES`, ink_size_selector.ts) -- and so a box's, an oval's and an
/// arrow's.
pub const PEN_SIZES: [Size; 5] = [
    Size { value: 1.0, name: "Extra thin" },
    Size { value: 2.0, name: "Thin" },
    Size { value: 3.0, name: "Medium" },
    Size { value: 6.0, name: "Thick" },
    Size { value: 8.0, name: "Extra thick" },
];

/// The highlighter's (`HIGHLIGHTER_SIZES`).
pub const HIGHLIGHTER_SIZES: [Size; 5] = [
    Size { value: 4.0, name: "Extra thin" },
    Size { value: 6.0, name: "Thin" },
    Size { value: 8.0, name: "Medium" },
    Size { value: 12.0, name: "Thick" },
    Size { value: 16.0, name: "Extra thick" },
];

/// Words' sizes (`TEXT_SIZES`, ink_annotation_text_mixin.ts).
pub const TEXT_SIZES: [f32; 16] = [6.0, 8.0, 10.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 32.0, 40.0, 48.0, 64.0, 72.0, 100.0];

/// Chromium's own first choices: the pen black at 3 (`CreateDefaultPenBrush`, pdf_ink_module.cc),
/// the highlighter light red at 8 (`CreateDefaultHighlighterBrush`), and words black at 12 --
/// sans serif, from the left, neither bold nor italic (ink2_manager.ts's `attributes_`).
pub const PEN: (Colour, f32) = (Colour::BLACK, 3.0);
pub const HIGHLIGHTER: (Colour, f32) = (Colour::of(0xf28b82), 8.0);
pub const WORDS: (Colour, f32) = (Colour::BLACK, 12.0);

/// Chromium's sizes are in CSS pixels of a page shown at 100 per cent -- its "canonical"
/// coordinates (pdf_ink_transform.h), where its strokes are drawn -- and a CSS pixel is three
/// quarters of a point (`kUnitConversionFactorPixelsToPoints`, printing/units.h): its pen of 3
/// is 2.25 points wide on the page, its words of 12 are set 9 points high
/// (`CSSFontSizeToPdfFontSize`).
pub const CSS_PIXEL: f32 = 0.75;

/// An A4 page's shorter side, in points: a picture's shorter side is taken as one, so that a
/// size looks on a photograph as it does on a page.
pub const A4_SHORT: f32 = 595.0;

/// How far the eraser reaches either way of where it is, in Chromium's units: a square
/// (`kEraserSize`, pdf_ink_module.cc).
pub const ERASER: f32 = 3.0;

/// One of Chromium's numbers -- a brush's size, words' size, the eraser's reach -- on a picture
/// whose shorter side is `short` of its pixels: on a 12-megapixel photograph's 3024, the pen's 3
/// is 11.4 pixels, as it is 2.25 points on an A4 page's 595.
pub fn on_picture(value: f32, short: f32) -> f32 {
    value * CSS_PIXEL * short / A4_SHORT
}

/// What draws a line by hand: Chromium's brushes, "pen" and "highlighter"
/// (`AnnotationBrushType`, constants.ts), but for the eraser, which draws nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Brush {
    /// A round tip, the colour laid on as it is.
    Pen,
    /// A square tip, upright (`GetCornerRounding` 0), see-through.
    Highlighter,
}

/// Words' typeface, by Chromium's names (`TextTypeface`, constants.ts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Typeface {
    #[default]
    SansSerif,
    Serif,
    Monospace,
}

impl Typeface {
    pub const ALL: [Typeface; 3] = [Typeface::SansSerif, Typeface::Serif, Typeface::Monospace];

    /// Chromium's own name for it, which the tests hold it to.
    #[cfg(test)]
    pub fn value(self) -> &'static str {
        match self {
            Typeface::SansSerif => "sans-serif",
            Typeface::Serif => "serif",
            Typeface::Monospace => "monospace",
        }
    }

    /// As Chromium's PDF viewer says it.
    pub fn name(self) -> &'static str {
        match self {
            Typeface::SansSerif => "Sans Serif",
            Typeface::Serif => "Serif",
            Typeface::Monospace => "Monospace",
        }
    }
}

/// How words' lines lie in their box (`TextAlignment`, constants.ts). One line, as SpatiLoupe's
/// words are, lies against the edge its alignment says: that edge stays where it is as the words
/// change, and a new line of words is set from the place pressed by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Alignment {
    #[default]
    Left,
    Center,
    Right,
}

impl Alignment {
    pub const ALL: [Alignment; 3] = [Alignment::Left, Alignment::Center, Alignment::Right];

    /// Chromium's own name for it, which the tests hold it to.
    #[cfg(test)]
    pub fn value(self) -> &'static str {
        match self {
            Alignment::Left => "left",
            Alignment::Center => "center",
            Alignment::Right => "right",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Alignment::Left => "Left",
            Alignment::Center => "Center",
            Alignment::Right => "Right",
        }
    }

    /// How far across a line the place that holds it is: its left, its middle, its right.
    pub fn share(self) -> f32 {
        match self {
            Alignment::Left => 0.0,
            Alignment::Center => 0.5,
            Alignment::Right => 1.0,
        }
    }
}

/// How words are set: Chromium's `TextAttributes` but for their size and colour, which every mark
/// has -- the styles its viewer offers (bold and italic: underline and strikethrough are behind a
/// flag that is off in Chromium 157, text_styles_selector.ts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Face {
    pub typeface: Typeface,
    pub bold: bool,
    pub italic: bool,
    pub alignment: Alignment,
}

/// Where words `width` wide begin, held by their `alignment` at `x`.
pub fn left_of(x: f32, width: f32, alignment: Alignment) -> f32 {
    x - width * alignment.share()
}

/// Where words whose box is `width` wide from `x0` are held by their `alignment`.
pub fn held_at(x0: f32, width: f32, alignment: Alignment) -> f32 {
    x0 + width * alignment.share()
}

/// What is drawn.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// A rectangle's outline.
    Box(Area),
    /// An ellipse's outline, inside its area.
    Oval(Area),
    /// A line with an arrowhead at `to`.
    Arrow { from: Point, to: Point },
    /// Words: their box's top left, and the box they take as the text engine sets them at the
    /// note's size in their face ([`crate::ink::measure`]).
    Text { at: Point, words: String, extent: (f32, f32), face: Face },
    /// A line drawn by hand: the points the pointer went through, in order, which it is drawn
    /// smoothly through ([`stroke_line`]) -- one, for a press that did not move: a dot.
    Stroke { brush: Brush, points: Vec<Point> },
}

/// What a mark's size was chosen as: one of Chromium's sizes, as its own number ([`PEN_SIZES`],
/// [`HIGHLIGHTER_SIZES`], [`TEXT_SIZES`]), which the size painted is made from on the picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pick {
    Chromium(f32),
}

/// One thing drawn on a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub page: usize,
    pub shape: Shape,
    /// Its colour, exactly as it was chosen: one of Chromium's.
    pub colour: Colour,
    /// A line's width, or the words' size, in the page's units: what is painted.
    pub size: f32,
    /// What that size was chosen as.
    pub pick: Pick,
}

impl Note {
    /// A note `size` large, its size as chosen not said: Chromium's pen's medium.
    #[cfg(test)]
    pub fn new(page: usize, shape: Shape, colour: Colour, size: f32) -> Note {
        Note { page, shape, colour, size, pick: Pick::Chromium(PEN.1) }
    }

    /// The brush that drew it, if it was drawn by hand.
    pub fn brush(&self) -> Option<Brush> {
        match &self.shape {
            Shape::Stroke { brush, .. } => Some(*brush),
            _ => None,
        }
    }
}

/// A place to take hold of a chosen note and change its shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    /// A box's or an oval's corner, clockwise from the top left.
    Corner(u8),
    /// An arrow's ends.
    From,
    To,
}

/// How near a point is to the segment from `a` to `b`.
pub fn distance_to_segment(p: Point, a: Point, b: Point) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length <= f32::EPSILON { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length).clamp(0.0, 1.0) };
    let (x, y) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - x).powi(2) + (p.1 - y).powi(2)).sqrt()
}

/// Whether the segment from `a` to `b` meets the upright square `half` either way of `centre`.
pub fn square_meets_segment(centre: Point, half: f32, a: Point, b: Point) -> bool {
    // The segment clipped to the square's slabs, across and down (Liang and Barsky's).
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (start, delta, low, high) in [(a.0, b.0 - a.0, centre.0 - half, centre.0 + half), (a.1, b.1 - a.1, centre.1 - half, centre.1 + half)] {
        if delta.abs() <= f32::EPSILON {
            if start < low || start > high {
                return false;
            }
            continue;
        }
        let (mut near, mut far) = ((low - start) / delta, (high - start) / delta);
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        t0 = t0.max(near);
        t1 = t1.min(far);
        if t0 > t1 {
            return false;
        }
    }
    true
}

/// How far the segment from `a` to `b` is from the upright square `half` either way of
/// `centre`: nought where they meet.
pub fn square_to_segment(centre: Point, half: f32, a: Point, b: Point) -> f32 {
    if square_meets_segment(centre, half, a, b) {
        return 0.0;
    }
    // Apart, the nearest two points are an end of one and a side of the other.
    let corners = [(centre.0 - half, centre.1 - half), (centre.0 + half, centre.1 - half), (centre.0 + half, centre.1 + half), (centre.0 - half, centre.1 + half)];
    let mut least = f32::MAX;
    for i in 0..4 {
        let (c, d) = (corners[i], corners[(i + 1) % 4]);
        least = least.min(distance_to_segment(a, c, d)).min(distance_to_segment(b, c, d)).min(distance_to_segment(c, a, b));
    }
    least
}

/// The line a stroke is drawn along: from its first point to its last, through the middles
/// between its points, each point pulling the line its way as a quadratic curve's control --
/// a hand's tremble smoothed a little -- in short straight pieces.
pub fn stroke_line(points: &[Point]) -> Vec<Point> {
    const PIECES: usize = 6;
    if points.len() < 3 {
        return points.to_vec();
    }
    let mid = |a: Point, b: Point| ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5);
    let mut line = vec![points[0]];
    let mut from = points[0];
    for i in 1..points.len() - 1 {
        let (control, to) = (points[i], mid(points[i], points[i + 1]));
        for k in 1..=PIECES {
            let t = k as f32 / PIECES as f32;
            let u = 1.0 - t;
            line.push((u * u * from.0 + 2.0 * u * t * control.0 + t * t * to.0, u * u * from.1 + 2.0 * u * t * control.1 + t * t * to.1));
        }
        from = to;
    }
    line.push(points[points.len() - 1]);
    line
}

/// The shapes a highlighter's square tip covers along `line`, `half` either way of it: for each
/// piece of the line, the six-sided shape the square makes moving along it (the two squares at
/// its ends and what is between them), each wound the same way, so that filled together they are
/// the stroke. A stroke of one point is its square.
pub fn highlighter_shapes(line: &[Point], half: f32) -> Vec<Vec<Point>> {
    let square = |p: Point| [(p.0 - half, p.1 - half), (p.0 + half, p.1 - half), (p.0 + half, p.1 + half), (p.0 - half, p.1 + half)];
    match line {
        [] => Vec::new(),
        [only] => vec![square(*only).to_vec()],
        _ => line
            .windows(2)
            .map(|pair| {
                let mut corners: Vec<Point> = square(pair[0]).into_iter().chain(square(pair[1])).collect();
                hull(&mut corners)
            })
            .collect(),
    }
}

/// The convex hull of `points`, anticlockwise as y goes up (Andrew's monotone chain).
fn hull(points: &mut [Point]) -> Vec<Point> {
    points.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let cross = |o: Point, a: Point, b: Point| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    let mut lower: Vec<Point> = Vec::new();
    for p in points.iter() {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], *p) <= 0.0 {
            lower.pop();
        }
        lower.push(*p);
    }
    let mut upper: Vec<Point> = Vec::new();
    for p in points.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], *p) <= 0.0 {
            upper.pop();
        }
        upper.push(*p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// The points of an ellipse inside `a`, round it in `n` steps: what an oval's line is tested
/// against.
fn ellipse_points(a: &Area, n: usize) -> Vec<Point> {
    let (cx, cy) = a.centre();
    let (rx, ry) = (a.w() * 0.5, a.h() * 0.5);
    (0..=n).map(|i| {
        let t = i as f32 / n as f32 * std::f32::consts::TAU;
        (cx + rx * t.cos(), cy + ry * t.sin())
    }).collect()
}

/// An arrow's head, for a line of `width` from `from` to `to`: where the line stops under it,
/// and its three corners (the tip first). Shared by every way an arrow is drawn, so that it is
/// the same on the window and in a saved picture.
pub fn arrow_head(from: Point, to: Point, width: f32) -> (Point, [Point; 3]) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = (dx * dx + dy * dy).sqrt().max(1e-3);
    let (ux, uy) = (dx / length, dy / length);
    let long = (width * 4.5 + 2.0).min(length * 0.6);
    let half = long * 0.45;
    let base = (to.0 - ux * long, to.1 - uy * long);
    let stop = (to.0 - ux * long * 0.8, to.1 - uy * long * 0.8);
    (stop, [to, (base.0 - uy * half, base.1 + ux * half), (base.0 + uy * half, base.1 - ux * half)])
}

impl Shape {
    /// The rectangle it takes, not counting the width of its line.
    pub fn bounds(&self) -> Area {
        match self {
            Shape::Box(a) | Shape::Oval(a) => *a,
            Shape::Arrow { from, to } => Area::between(*from, *to),
            Shape::Text { at, extent, .. } => Area::new(at.0, at.1, at.0 + extent.0, at.1 + extent.1),
            Shape::Stroke { points, .. } => {
                let Some(first) = points.first() else { return Area::new(0.0, 0.0, 0.0, 0.0) };
                points.iter().fold(Area::between(*first, *first), |a, p| Area { x0: a.x0.min(p.0), y0: a.y0.min(p.1), x1: a.x1.max(p.0), y1: a.y1.max(p.1) })
            }
        }
    }

    pub fn moved(&self, dx: f32, dy: f32) -> Shape {
        let m = |p: Point| (p.0 + dx, p.1 + dy);
        match self {
            Shape::Box(a) => Shape::Box(Area::between(m((a.x0, a.y0)), m((a.x1, a.y1)))),
            Shape::Oval(a) => Shape::Oval(Area::between(m((a.x0, a.y0)), m((a.x1, a.y1)))),
            Shape::Arrow { from, to } => Shape::Arrow { from: m(*from), to: m(*to) },
            Shape::Text { at, words, extent, face } => Shape::Text { at: m(*at), words: words.clone(), extent: *extent, face: *face },
            Shape::Stroke { brush, points } => Shape::Stroke { brush: *brush, points: points.iter().map(|p| m(*p)).collect() },
        }
    }

    /// The places it can be taken hold of to change it: a box's and an oval's corners, an
    /// arrow's ends. Words and lines drawn by hand have none; they are moved, and their size is
    /// chosen.
    pub fn handles(&self) -> Vec<(Handle, Point)> {
        match self {
            Shape::Box(a) | Shape::Oval(a) => a.corners().iter().enumerate().map(|(i, p)| (Handle::Corner(i as u8), *p)).collect(),
            Shape::Arrow { from, to } => vec![(Handle::From, *from), (Handle::To, *to)],
            Shape::Text { .. } | Shape::Stroke { .. } => Vec::new(),
        }
    }

    /// The shape with `handle` taken to `to`; `even` keeps a box square and an oval round, the
    /// corner across from the handle staying where it is.
    pub fn with_handle(&self, handle: Handle, to: Point, even: bool) -> Shape {
        match (self, handle) {
            (Shape::Box(a) | Shape::Oval(a), Handle::Corner(i)) => {
                let fixed = a.corners()[(i as usize + 2) % 4];
                let area = corner_to(fixed, to, even);
                if matches!(self, Shape::Box(_)) { Shape::Box(area) } else { Shape::Oval(area) }
            }
            (Shape::Arrow { to: end, .. }, Handle::From) => Shape::Arrow { from: to, to: *end },
            (Shape::Arrow { from, .. }, Handle::To) => Shape::Arrow { from: *from, to },
            _ => self.clone(),
        }
    }

    /// Whether it is so small that it is nothing: a press that was not a drag. A press with a
    /// pen or a highlighter is a dot, as it is in Chromium.
    pub fn is_speck(&self, least: f32) -> bool {
        match self {
            Shape::Box(a) | Shape::Oval(a) => a.w() < least && a.h() < least,
            Shape::Arrow { from, to } => (to.0 - from.0).hypot(to.1 - from.1) < least,
            Shape::Text { words, .. } => words.trim().is_empty(),
            Shape::Stroke { points, .. } => points.is_empty(),
        }
    }
}

/// The rectangle from `fixed` to `to`, square if `even`: as wide as it is tall, the larger of
/// the two, towards `to`.
pub fn corner_to(fixed: Point, to: Point, even: bool) -> Area {
    if !even {
        return Area::between(fixed, to);
    }
    let (dx, dy) = (to.0 - fixed.0, to.1 - fixed.1);
    let side = dx.abs().max(dy.abs());
    Area::between(fixed, (fixed.0 + side * if dx < 0.0 { -1.0 } else { 1.0 }, fixed.1 + side * if dy < 0.0 { -1.0 } else { 1.0 }))
}

/// A point of a picture `size` across and down, as it is once the picture is turned a quarter,
/// clockwise or not.
pub fn turn_point(p: Point, size: (f32, f32), clockwise: bool) -> Point {
    if clockwise { (size.1 - p.1, p.0) } else { (p.1, size.0 - p.0) }
}

pub fn turn_area(a: &Area, size: (f32, f32), clockwise: bool) -> Area {
    Area::between(turn_point((a.x0, a.y0), size, clockwise), turn_point((a.x1, a.y1), size, clockwise))
}

impl Note {
    /// The note on a picture turned a quarter. Words stay upright, wherever they go: their middle
    /// goes where that part of the picture goes. A highlighter's square tip is upright either way.
    pub fn turned(&self, size: (f32, f32), clockwise: bool) -> Note {
        let shape = match &self.shape {
            Shape::Box(a) => Shape::Box(turn_area(a, size, clockwise)),
            Shape::Oval(a) => Shape::Oval(turn_area(a, size, clockwise)),
            Shape::Arrow { from, to } => Shape::Arrow { from: turn_point(*from, size, clockwise), to: turn_point(*to, size, clockwise) },
            Shape::Text { at, words, extent, face } => {
                let middle = turn_point((at.0 + extent.0 * 0.5, at.1 + extent.1 * 0.5), size, clockwise);
                Shape::Text { at: (middle.0 - extent.0 * 0.5, middle.1 - extent.1 * 0.5), words: words.clone(), extent: *extent, face: *face }
            }
            Shape::Stroke { brush, points } => Shape::Stroke { brush: *brush, points: points.iter().map(|p| turn_point(*p, size, clockwise)).collect() },
        };
        Note { shape, ..self.clone() }
    }

    /// Whether `p` is on it, give or take `slack`: on a line, in a box or an oval or its words.
    fn holds(&self, p: Point, slack: f32) -> bool {
        match &self.shape {
            Shape::Arrow { from, to } => distance_to_segment(p, *from, *to) <= slack + self.size * 0.5,
            Shape::Stroke { .. } => self.near_stroke(p, slack),
            shape => shape.bounds().grown(slack + self.size * 0.5).contains(p),
        }
    }

    /// Whether the upright square `half` either way of `centre` meets a line drawn by hand, as it
    /// is painted: a pen's round tip, a highlighter's square one, swept along its line.
    fn stroke_meets(&self, centre: Point, half: f32) -> bool {
        let Shape::Stroke { brush, points } = &self.shape else { return false };
        let line = stroke_line(points);
        let tip = self.size * 0.5;
        match (brush, line.as_slice()) {
            (_, []) => false,
            // Two upright squares meet where one grown by the other holds the other's middle.
            (Brush::Highlighter, [only]) => (only.0 - centre.0).abs() <= half + tip && (only.1 - centre.1).abs() <= half + tip,
            (Brush::Highlighter, _) => line.windows(2).any(|w| square_meets_segment(centre, half + tip, w[0], w[1])),
            (Brush::Pen, [only]) => square_to_segment(centre, half, *only, *only) <= tip,
            (Brush::Pen, _) => line.windows(2).any(|w| square_to_segment(centre, half, w[0], w[1]) <= tip),
        }
    }

    /// Whether `p` is on a line drawn by hand, give or take `slack`.
    fn near_stroke(&self, p: Point, slack: f32) -> bool {
        self.stroke_meets(p, slack)
    }

    /// Whether the eraser, the upright square `half` either way of `centre`, touches it: a line
    /// drawn by hand, a box's, an oval's or an arrow's line -- not what is only inside them -- but
    /// never words, which Chromium's eraser leaves too (`EraseHelper`, pdf_ink_module.cc, erases
    /// strokes only).
    pub fn erased_by(&self, centre: Point, half: f32) -> bool {
        let reach = self.size * 0.5;
        let near = |a: Point, b: Point| square_to_segment(centre, half, a, b) <= reach;
        match &self.shape {
            Shape::Stroke { .. } => self.stroke_meets(centre, half),
            Shape::Box(a) => {
                let c = a.corners();
                (0..4).any(|i| near(c[i], c[(i + 1) % 4]))
            }
            Shape::Oval(a) => ellipse_points(a, 48).windows(2).any(|w| near(w[0], w[1])),
            Shape::Arrow { from, to } => {
                let (stop, head) = arrow_head(*from, *to, self.size);
                // Its line, its head's edges, or inside its head.
                let inside = {
                    let side = |a: Point, b: Point| (b.0 - a.0) * (centre.1 - a.1) - (b.1 - a.1) * (centre.0 - a.0);
                    let s = [side(head[0], head[1]), side(head[1], head[2]), side(head[2], head[0])];
                    s.iter().all(|v| *v >= 0.0) || s.iter().all(|v| *v <= 0.0)
                };
                near(*from, stop) || inside || (0..3).any(|i| square_to_segment(centre, half, head[i], head[(i + 1) % 3]) <= 0.0)
            }
            Shape::Text { .. } => false,
        }
    }

    /// Whether `p` is on its line itself -- a box's or an oval's edge -- rather than only inside.
    fn on_line(&self, p: Point, slack: f32) -> bool {
        let reach = slack + self.size * 0.5;
        match &self.shape {
            Shape::Box(a) => {
                let c = a.corners();
                (0..4).any(|i| distance_to_segment(p, c[i], c[(i + 1) % 4]) <= reach)
            }
            Shape::Oval(a) => {
                // Near enough the ellipse: its equation's value, as a distance along the gradient.
                let (cx, cy) = a.centre();
                let (rx, ry) = ((a.w() * 0.5).max(1e-3), (a.h() * 0.5).max(1e-3));
                let (nx, ny) = ((p.0 - cx) / rx, (p.1 - cy) / ry);
                let f = nx * nx + ny * ny - 1.0;
                let gradient = 2.0 * ((nx / rx).powi(2) + (ny / ry).powi(2)).sqrt();
                (f / gradient.max(1e-6)).abs() <= reach
            }
            _ => self.holds(p, slack),
        }
    }
}

/// Everything drawn and cut on a file: what is saved.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Marks {
    pub notes: Vec<Note>,
    /// Each page's crop, in its units: what is kept of it.
    pub crops: Vec<(usize, Area)>,
    /// A picture's quarter turns clockwise from how it was read: it is saved as it is shown.
    pub turns: u8,
}

impl Marks {
    pub fn crop(&self, page: usize) -> Option<Area> {
        self.crops.iter().find(|(p, _)| *p == page).map(|(_, a)| *a)
    }

    /// What the eraser, `half` either way of `centre` on `page`, touches there, taken away whole
    /// (as Chromium's does): how many marks.
    pub fn erase(&mut self, page: usize, centre: Point, half: f32) -> usize {
        let before = self.notes.len();
        self.notes.retain(|n| n.page != page || !n.erased_by(centre, half));
        before - self.notes.len()
    }

    /// The marks on a picture `size` across and down, turned a quarter with it: every note, the
    /// crop, and the count of turns.
    pub fn turned(&self, size: (f32, f32), clockwise: bool) -> Marks {
        Marks {
            notes: self.notes.iter().map(|n| n.turned(size, clockwise)).collect(),
            crops: self.crops.iter().map(|(p, a)| (*p, turn_area(a, size, clockwise))).collect(),
            turns: (self.turns + if clockwise { 1 } else { 3 }) % 4,
        }
    }
}

/// How many changes can be taken back.
const HISTORY: usize = 200;

/// The marks on one file as the wearer works on them: what there is now, what there was when the
/// file was opened (or last saved), and every step between, back and forth.
#[derive(Debug, Clone)]
pub struct Markup {
    now: Marks,
    origin: Marks,
    undo: Vec<Marks>,
    redo: Vec<Marks>,
    /// The note chosen, which the colour and weight chosen next apply to, and which Delete takes
    /// away.
    pub chosen: Option<usize>,
    /// Counts every change of the marks, for what draws them to know it is to draw them again.
    pub generation: u64,
}

impl Markup {
    /// Nothing drawn yet on a file shown turned `turns` quarters from how it was read.
    pub fn new(turns: u8) -> Markup {
        let origin = Marks { turns, ..Marks::default() };
        Markup { now: origin.clone(), origin, undo: Vec::new(), redo: Vec::new(), chosen: None, generation: 0 }
    }

    pub fn marks(&self) -> &Marks {
        &self.now
    }

    /// Whether there is anything to save: the marks are not what the file had.
    pub fn edited(&self) -> bool {
        self.now != self.origin
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// A change, as one step that can be undone: nothing is kept if it changed nothing.
    pub fn change<R>(&mut self, change: impl FnOnce(&mut Marks) -> R) -> R {
        let before = self.now.clone();
        let answer = change(&mut self.now);
        self.finish(before);
        answer
    }

    /// The marks as they are, kept by a drag as it begins: its every move is [`Markup::adjust`],
    /// and [`Markup::finish`] with this makes the whole drag one step.
    pub fn begin(&self) -> Marks {
        self.now.clone()
    }

    /// A change on the way -- a drag's -- not yet a step of its own.
    pub fn adjust(&mut self, change: impl FnOnce(&mut Marks)) {
        change(&mut self.now);
        self.generation += 1;
    }

    /// The step from `before` to now, if there is one.
    pub fn finish(&mut self, before: Marks) {
        self.generation += 1;
        if before == self.now {
            return;
        }
        self.undo.push(before);
        if self.undo.len() > HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.keep_chosen();
    }

    /// Back a step: whether there was one.
    pub fn undo(&mut self) -> bool {
        let Some(before) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(&mut self.now, before));
        self.generation += 1;
        self.keep_chosen();
        true
    }

    /// On again, after an undo.
    pub fn redo(&mut self) -> bool {
        let Some(after) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(&mut self.now, after));
        self.generation += 1;
        self.keep_chosen();
        true
    }

    /// The choice let go of if what was chosen has gone.
    fn keep_chosen(&mut self) {
        if self.chosen.is_some_and(|i| i >= self.now.notes.len()) {
            self.chosen = None;
        }
    }

    /// A note drawn: chosen, as Preview chooses what was just drawn.
    pub fn add(&mut self, note: Note) -> usize {
        let index = self.change(|marks| {
            marks.notes.push(note);
            marks.notes.len() - 1
        });
        self.chosen = Some(index);
        index
    }

    /// The chosen note taken away: whether there was one.
    pub fn delete_chosen(&mut self) -> bool {
        let Some(index) = self.chosen.filter(|&i| i < self.now.notes.len()) else { return false };
        self.change(|marks| {
            marks.notes.remove(index);
        });
        self.chosen = None;
        true
    }

    /// The note at `p` on `page`, `slack` either side: one whose line is there before one only
    /// round it, then the smallest -- a box drawn inside another is reached inside it -- then the
    /// one drawn last.
    pub fn hit(&self, page: usize, p: Point, slack: f32) -> Option<usize> {
        let under: Vec<usize> = (0..self.now.notes.len()).rev().filter(|&i| self.now.notes[i].page == page && self.now.notes[i].holds(p, slack)).collect();
        let area = |i: usize| {
            let b = self.now.notes[i].shape.bounds();
            b.w() * b.h()
        };
        under.iter().copied().find(|&i| self.now.notes[i].on_line(p, slack)).or_else(|| under.iter().copied().min_by(|&a, &b| area(a).total_cmp(&area(b))))
    }

    /// The handle of note `index` at `p`, `slack` either side.
    pub fn handle_at(&self, index: usize, p: Point, slack: f32) -> Option<Handle> {
        let note = self.now.notes.get(index)?;
        note.shape.handles().into_iter().filter(|(_, at)| (at.0 - p.0).abs() <= slack && (at.1 - p.1).abs() <= slack).min_by(|a, b| {
            let d = |q: Point| (q.0 - p.0).hypot(q.1 - p.1);
            d(a.1).total_cmp(&d(b.1))
        }).map(|(h, _)| h)
    }

    /// The marks turned a quarter with a picture `size` across and down, as one step.
    pub fn turn(&mut self, size: (f32, f32), clockwise: bool) {
        self.change(|marks| *marks = marks.turned(size, clockwise));
    }

    /// The file has been written as the marks are: nothing is left to save.
    pub fn saved(&mut self) {
        self.origin = self.now.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(shape: Shape) -> Note {
        Note::new(0, shape, PEN_COLOURS[10].colour, 4.0)
    }

    fn boxed(x0: f32, y0: f32, x1: f32, y1: f32) -> Note {
        note(Shape::Box(Area::new(x0, y0, x1, y1)))
    }

    #[test]
    fn notes_are_added_chosen_and_taken_away_and_every_step_comes_back() {
        let mut m = Markup::new(0);
        assert!(!m.edited() && !m.can_undo());
        let a = m.add(boxed(10.0, 10.0, 100.0, 60.0));
        assert_eq!((a, m.chosen), (0, Some(0)));
        m.add(note(Shape::Arrow { from: (0.0, 0.0), to: (50.0, 50.0) }));
        m.add(note(Shape::Text { at: (5.0, 200.0), words: "Grüße 你好".into(), extent: (120.0, 30.0), face: Face::default() }));
        assert_eq!(m.marks().notes.len(), 3);
        assert!(m.edited());
        m.chosen = Some(1);
        assert!(m.delete_chosen());
        assert_eq!(m.chosen, None);
        assert_eq!(m.marks().notes.len(), 2);
        assert!(!m.delete_chosen(), "nothing chosen, nothing taken");
        // Back through all four steps, and on again through them.
        let mut seen = vec![m.marks().notes.len()];
        while m.undo() {
            seen.push(m.marks().notes.len());
        }
        assert_eq!(seen, [2, 3, 2, 1, 0]);
        assert!(!m.edited(), "all undone is as the file was");
        while m.redo() {}
        assert_eq!(m.marks().notes.len(), 2);
        assert!(matches!(m.marks().notes[1].shape, Shape::Text { .. }));
        // A new change after an undo: nothing left to redo.
        m.undo();
        m.add(boxed(0.0, 0.0, 5.0, 5.0));
        assert!(!m.can_redo());
    }

    #[test]
    fn a_drag_is_one_step_and_a_change_that_changes_nothing_is_none() {
        let mut m = Markup::new(0);
        m.add(boxed(10.0, 10.0, 100.0, 60.0));
        let before = m.begin();
        for step in 1..=20 {
            m.adjust(|marks| marks.notes[0].shape = Shape::Box(Area::new(10.0 + step as f32, 10.0, 100.0 + step as f32, 60.0)));
        }
        m.finish(before);
        assert_eq!(m.marks().notes[0].shape.bounds().x0, 30.0);
        assert!(m.undo());
        assert_eq!(m.marks().notes[0].shape.bounds().x0, 10.0, "the whole drag taken back at once");
        assert!(m.undo() && !m.undo());
        // Nothing moved: no step.
        m.redo();
        let before = m.begin();
        m.finish(before);
        m.change(|_| {});
        assert!(m.undo() && !m.can_undo());
    }

    #[test]
    fn crops_and_turns_are_steps_too() {
        let mut m = Markup::new(0);
        m.change(|marks| marks.crops = vec![(0, Area::new(10.0, 20.0, 300.0, 200.0))]);
        m.turn((400.0, 300.0), true);
        assert_eq!(m.marks().turns, 1);
        // Turned clockwise, a 400 by 300 picture is 300 by 400: what was 10 from the left is 10
        // from the top, and what was 20 from the top is 20 from the right.
        assert_eq!(m.marks().crop(0), Some(Area::new(100.0, 10.0, 280.0, 300.0)));
        m.undo();
        assert_eq!((m.marks().turns, m.marks().crop(0)), (0, Some(Area::new(10.0, 20.0, 300.0, 200.0))));
        m.undo();
        assert_eq!(m.marks().crop(0), None);
        assert!(!m.edited());
        // A picture turned before anything was drawn starts there, and is not an edit by that.
        let m = Markup::new(3);
        assert!(!m.edited() && m.marks().turns == 3);
    }

    #[test]
    fn four_quarter_turns_bring_every_note_back_and_words_stay_upright() {
        let marks = Marks {
            notes: vec![
                boxed(10.0, 20.0, 110.0, 70.0),
                note(Shape::Arrow { from: (5.0, 5.0), to: (395.0, 295.0) }),
                note(Shape::Text { at: (50.0, 100.0), words: "label".into(), extent: (80.0, 20.0), face: Face::default() }),
            ],
            crops: vec![(0, Area::new(1.0, 2.0, 300.0, 250.0))],
            turns: 0,
        };
        let mut turned = marks.clone();
        let mut size = (400.0, 300.0);
        for _ in 0..4 {
            turned = turned.turned(size, true);
            size = (size.1, size.0);
        }
        assert_eq!(turned, marks);
        // Once clockwise: the arrow's start, 5 from the left and the top, is 5 from the top and
        // the right; the words keep their shape, and their middle is where the picture's went.
        let once = marks.turned((400.0, 300.0), true);
        assert_eq!(once.notes[1].shape, Shape::Arrow { from: (295.0, 5.0), to: (5.0, 395.0) });
        let Shape::Text { at, extent, .. } = &once.notes[2].shape else { unreachable!() };
        assert_eq!(*extent, (80.0, 20.0));
        assert_eq!((at.0 + 40.0, at.1 + 10.0), (300.0 - 110.0, 90.0));
        // And back the other way.
        assert_eq!(once.turned((300.0, 400.0), false), Marks { turns: 0, ..marks });
    }

    #[test]
    fn what_is_pressed_on_is_found() {
        let mut m = Markup::new(0);
        m.add(boxed(0.0, 0.0, 400.0, 400.0));
        m.add(boxed(100.0, 100.0, 150.0, 150.0));
        m.add(note(Shape::Oval(Area::new(200.0, 200.0, 300.0, 260.0))));
        m.add(note(Shape::Arrow { from: (10.0, 390.0), to: (390.0, 390.0) }));
        m.add(Note { page: 1, ..boxed(0.0, 0.0, 400.0, 400.0) });
        // Inside the small box, which is inside the large one: the small one.
        assert_eq!(m.hit(0, (120.0, 120.0), 5.0), Some(1));
        // On the large box's edge: the large one, though the oval is drawn later.
        assert_eq!(m.hit(0, (2.0, 200.0), 5.0), Some(0));
        // In the oval's corner, outside the ellipse but in its area: the oval still, as the
        // smallest thing there; on its rim, the oval.
        assert_eq!(m.hit(0, (205.0, 205.0), 5.0), Some(2));
        assert_eq!(m.hit(0, (299.0, 230.0), 5.0), Some(2));
        // On the arrow, and just off it: the arrow; then the box behind it.
        assert_eq!(m.hit(0, (200.0, 393.0), 5.0), Some(3));
        assert_eq!(m.hit(0, (200.0, 380.0), 5.0), Some(0));
        // Off everything, and another page's.
        assert_eq!(m.hit(0, (500.0, 500.0), 5.0), None);
        assert_eq!(m.hit(1, (2.0, 200.0), 5.0), Some(4));
        // A box's corners and an arrow's ends are its handles.
        assert_eq!(m.handle_at(1, (149.0, 151.0), 8.0), Some(Handle::Corner(2)));
        assert_eq!(m.handle_at(3, (12.0, 388.0), 8.0), Some(Handle::From));
        assert_eq!(m.handle_at(1, (125.0, 125.0), 8.0), None);
    }

    #[test]
    fn a_handle_reshapes_and_a_move_moves() {
        let b = Shape::Box(Area::new(10.0, 10.0, 50.0, 30.0));
        // The bottom right corner dragged past the top left: the box turns over, its corner
        // across staying where it was.
        assert_eq!(b.with_handle(Handle::Corner(2), (0.0, 5.0), false), Shape::Box(Area::new(0.0, 5.0, 10.0, 10.0)));
        // Even: square, as large as the larger way.
        assert_eq!(b.with_handle(Handle::Corner(2), (70.0, 20.0), true), Shape::Box(Area::new(10.0, 10.0, 70.0, 70.0)));
        let o = Shape::Oval(Area::new(10.0, 10.0, 50.0, 30.0));
        assert_eq!(o.with_handle(Handle::Corner(0), (0.0, 0.0), false), Shape::Oval(Area::new(0.0, 0.0, 50.0, 30.0)));
        let a = Shape::Arrow { from: (0.0, 0.0), to: (10.0, 10.0) };
        assert_eq!(a.with_handle(Handle::To, (20.0, 5.0), false), Shape::Arrow { from: (0.0, 0.0), to: (20.0, 5.0) });
        assert_eq!(a.moved(5.0, -5.0), Shape::Arrow { from: (5.0, -5.0), to: (15.0, 5.0) });
        let t = Shape::Text { at: (1.0, 2.0), words: "x".into(), extent: (10.0, 5.0), face: Face::default() };
        assert!(t.handles().is_empty());
        assert_eq!(t.moved(1.0, 1.0).bounds(), Area::new(2.0, 3.0, 12.0, 8.0));
        assert!(Shape::Box(Area::new(0.0, 0.0, 1.0, 2.0)).is_speck(3.0));
        assert!(!Shape::Arrow { from: (0.0, 0.0), to: (0.0, 9.0) }.is_speck(3.0));
    }

    #[test]
    fn an_arrow_s_head_sits_on_its_end() {
        let (stop, head) = arrow_head((0.0, 0.0), (100.0, 0.0), 4.0);
        assert_eq!(head[0], (100.0, 0.0));
        // 4.5 times the width and 2: 20 long, and 9 either side of the line at its base.
        assert!((head[1].0 - 80.0).abs() < 1e-4 && (head[1].1 - 9.0).abs() < 1e-4, "{head:?}");
        assert!((head[2].1 + 9.0).abs() < 1e-4);
        assert!(stop.0 > 80.0 && stop.0 < 100.0, "the line ends under the head: {stop:?}");
        // A short arrow's head is never longer than most of it.
        let (_, head) = arrow_head((0.0, 0.0), (10.0, 0.0), 4.0);
        assert!((head[1].0 - 4.0).abs() < 1e-4);
    }

    #[test]
    fn an_even_rectangle_goes_the_way_it_was_dragged() {
        assert_eq!(corner_to((10.0, 10.0), (0.0, 30.0), true), Area::new(-10.0, 10.0, 10.0, 30.0));
        assert_eq!(corner_to((10.0, 10.0), (12.0, 13.0), false), Area::new(10.0, 10.0, 12.0, 13.0));
    }

    fn stroke(brush: Brush, points: &[Point], size: f32) -> Note {
        Note { pick: Pick::Chromium(size), ..Note::new(0, Shape::Stroke { brush, points: points.to_vec() }, PEN.0, size) }
    }

    #[test]
    fn chromium_s_colours_and_sizes_are_kept_as_its_own_values() {
        // As many, in Chromium's order, each as its hex says; and back again.
        assert_eq!((PEN_COLOURS.len(), HIGHLIGHTER_COLOURS.len(), TEXT_COLOURS.len()), (20, 10, 20));
        assert_eq!(PEN_COLOURS[0].colour, Colour::BLACK);
        assert_eq!(PEN_COLOURS[19], Swatch { colour: Colour([0x88, 0x59, 0x45]), name: "Tan 3" });
        assert_eq!(HIGHLIGHTER_COLOURS[0].colour.hex(), "#f28b82");
        assert_eq!(TEXT_COLOURS[16].colour.hex(), "#d56e0c", "words' Yellow 3 is their own, not the pen's");
        // Each list as Chromium 157's source writes it, hex for hex.
        let hex = |list: &[Swatch]| list.iter().map(|s| s.colour.hex()).collect::<Vec<_>>().join(" ");
        assert_eq!(hex(&PEN_COLOURS), "#000000 #5f6368 #9aa0a6 #dadce0 #ffffff #f28b82 #fdd663 #81c995 #8ab4f8 #eec9ae #ea4335 #fbbc04 #34a853 #4285f4 #e2a185 #c5221f #f29900 #188038 #1967d2 #885945");
        assert_eq!(hex(&HIGHLIGHTER_COLOURS), "#f28b82 #fdd663 #34a853 #4285f4 #ffae80 #d93025 #ddf300 #25e387 #5379ff #ff630c");
        assert_eq!(hex(&TEXT_COLOURS), "#000000 #5f6368 #9aa0a6 #dadce0 #ffffff #f28b82 #fdd663 #81c995 #78d9ec #8ab4f8 #e94235 #fbbc04 #34a853 #24c1e0 #4285f4 #c5221f #d56e0c #188038 #12a4af #1967d2");
        assert_eq!(PEN_SIZES.map(|s| s.value), [1.0, 2.0, 3.0, 6.0, 8.0]);
        assert_eq!(HIGHLIGHTER_SIZES.map(|s| s.value), [4.0, 6.0, 8.0, 12.0, 16.0]);
        assert_eq!(TEXT_SIZES[3], WORDS.1, "words begin at Chromium's fourth size, 12");
        assert_eq!((PEN, HIGHLIGHTER), ((Colour::BLACK, 3.0), (Colour::of(0xf28b82), 8.0)));
        // Chromium's names for its values, which are what a mark keeps.
        assert_eq!(Typeface::ALL.map(Typeface::value), ["sans-serif", "serif", "monospace"]);
        assert_eq!(Alignment::ALL.map(Alignment::value), ["left", "center", "right"]);
        assert_eq!(Face::default(), Face { typeface: Typeface::SansSerif, bold: false, italic: false, alignment: Alignment::Left });
        // A highlighter's colour as Chromium shows it on white: 40 per cent of it.
        assert_eq!(Colour::of(0xf28b82).on_white(), Colour([250, 209, 205]));
        assert_eq!(Colour::of(0x000000).on_white(), Colour([153, 153, 153]));
    }

    #[test]
    fn chromium_s_sizes_look_on_a_photograph_as_on_an_a4_page() {
        // CSS pixels of a page at 100 per cent: three quarters of a point each.
        assert_eq!(on_picture(3.0, A4_SHORT), 2.25);
        assert_eq!(on_picture(12.0, A4_SHORT), 9.0);
        // A 12-megapixel photograph, 3024 short: the pen's 3 is 11.4 of its pixels.
        assert!((on_picture(3.0, 3024.0) - 11.435).abs() < 0.001);
        assert!((on_picture(ERASER, 1190.0) - 4.5).abs() < 1e-4);
    }

    #[test]
    fn the_eraser_takes_away_lines_and_shapes_it_touches_but_never_words() {
        let mut marks = Marks {
            notes: vec![
                // A pen's line, 4 wide, along y = 100 from x = 0 to 200.
                stroke(Brush::Pen, &[(0.0, 100.0), (100.0, 100.0), (200.0, 100.0)], 4.0),
                // A highlighter's, 16 tall, along y = 300.
                stroke(Brush::Highlighter, &[(0.0, 300.0), (200.0, 300.0)], 16.0),
                note(Shape::Box(Area::new(300.0, 0.0, 400.0, 100.0))),
                note(Shape::Oval(Area::new(300.0, 200.0, 400.0, 300.0))),
                note(Shape::Arrow { from: (500.0, 0.0), to: (500.0, 200.0) }),
                note(Shape::Text { at: (600.0, 0.0), words: "keep".into(), extent: (80.0, 30.0), face: Face::default() }),
                // A dot: a press with the pen that did not move.
                stroke(Brush::Pen, &[(700.0, 700.0)], 6.0),
            ],
            ..Marks::default()
        };
        // Off everything; inside a box and an oval, away from their lines; on the words.
        for (centre, why) in [((100.0, 90.0), "above the pen's line"), ((350.0, 50.0), "inside the box"), ((350.0, 250.0), "inside the oval"), ((620.0, 10.0), "on the words")] {
            assert_eq!(marks.clone().erase(0, centre, 3.0), 0, "{why}");
        }
        // The pen's line, 2 either way of it and 3 of the eraser: touched 4.9 off it, not 5.1.
        assert_eq!(marks.clone().erase(0, (150.0, 104.9), 3.0), 1);
        assert_eq!(marks.clone().erase(0, (150.0, 105.1), 3.0), 0);
        // The highlighter's square tip, 8 either way: from 11 off.
        assert_eq!(marks.clone().erase(0, (100.0, 310.9), 3.0), 1);
        assert_eq!(marks.clone().erase(0, (100.0, 311.1), 3.0), 0);
        // A box's edge, an oval's rim, an arrow's line and its head, the dot.
        assert_eq!(marks.clone().erase(0, (300.0, 50.0), 3.0), 1);
        assert_eq!(marks.clone().erase(0, (400.0, 250.0), 3.0), 1);
        assert_eq!(marks.clone().erase(0, (502.0, 100.0), 3.0), 1);
        assert_eq!(marks.clone().erase(0, (500.0, 195.0), 3.0), 1);
        assert_eq!(marks.clone().erase(0, (704.0, 700.0), 3.0), 1);
        // Another page's are not this page's.
        assert_eq!(marks.clone().erase(1, (150.0, 100.0), 3.0), 0);
        // Whole lines go, and only those touched: the words stay always.
        assert_eq!(marks.erase(0, (150.0, 100.0), 3.0), 1);
        assert_eq!(marks.notes.len(), 6);
        assert!(matches!(marks.notes[0].shape, Shape::Stroke { brush: Brush::Highlighter, .. }));
    }

    #[test]
    fn an_erase_is_one_step_to_undo_and_redo() {
        let mut m = Markup::new(0);
        m.add(stroke(Brush::Pen, &[(0.0, 0.0), (100.0, 0.0)], 4.0));
        m.add(stroke(Brush::Highlighter, &[(0.0, 50.0), (100.0, 50.0)], 8.0));
        m.add(note(Shape::Box(Area::new(10.0, 10.0, 90.0, 90.0))));
        // One drag of the eraser across all three, down x = 50 and then along the box's foot.
        let before = m.begin();
        for at in [(50.0, 0.0), (50.0, 20.0), (50.0, 50.0), (70.0, 90.0)] {
            m.adjust(|marks| {
                marks.erase(0, at, 3.0);
            });
        }
        m.finish(before);
        assert!(m.marks().notes.is_empty());
        assert!(m.undo());
        assert_eq!(m.marks().notes.len(), 3, "all three back at once");
        assert!(m.redo() && m.marks().notes.is_empty());
        // An erase that touched nothing is no step.
        m.undo();
        let before = m.begin();
        m.adjust(|marks| {
            marks.erase(0, (500.0, 500.0), 3.0);
        });
        m.finish(before);
        assert!(m.can_redo(), "nothing was done, so the redo is still there");
    }

    #[test]
    fn a_stroke_moves_and_turns_with_the_picture_and_is_found_where_it_is_drawn() {
        let line = stroke(Brush::Pen, &[(10.0, 20.0), (60.0, 30.0), (110.0, 20.0)], 4.0);
        let marks = Marks { notes: vec![line.clone()], ..Marks::default() };
        let mut turned = marks.clone();
        let mut size = (400.0, 300.0);
        for _ in 0..4 {
            turned = turned.turned(size, true);
            size = (size.1, size.0);
        }
        assert_eq!(turned, marks, "four quarters round");
        // Once: what was 10 from the left and 20 from the top is 20 from the right and 10 down.
        let once = marks.turned((400.0, 300.0), true);
        let Shape::Stroke { points, .. } = &once.notes[0].shape else { unreachable!() };
        assert_eq!(points[0], (280.0, 10.0));
        assert_eq!(line.shape.moved(5.0, -5.0).bounds(), Area::new(15.0, 15.0, 115.0, 25.0));
        assert!(line.shape.handles().is_empty() && !line.shape.is_speck(1.0));
        // Chosen by a press on it, not by one inside its bounds but away from it.
        let mut m = Markup::new(0);
        m.add(line);
        assert_eq!(m.hit(0, (60.0, 26.0), 2.0), Some(0));
        assert_eq!(m.hit(0, (60.0, 10.0), 2.0), None);
    }

    #[test]
    fn a_stroke_is_smoothed_through_the_middles_of_its_points_from_end_to_end() {
        let points = [(0.0, 0.0), (10.0, 10.0), (20.0, 0.0)];
        let line = stroke_line(&points);
        assert_eq!((line[0], *line.last().unwrap()), ((0.0, 0.0), (20.0, 0.0)), "it ends where the hand did");
        // Through the middle between its last two points; rounding the corner, never out to it.
        assert!(line.contains(&(15.0, 5.0)), "{line:?}");
        assert!(line.iter().all(|p| p.1 < 7.0) && line.iter().any(|p| p.1 > 6.0), "{line:?}");
        assert_eq!(stroke_line(&points[..2]), points[..2].to_vec(), "two points are a straight line");
        // A highlighter's square tip along a slanting piece: six sides; a dot, its square.
        let shapes = highlighter_shapes(&[(0.0, 0.0), (10.0, 10.0)], 2.0);
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].len(), 6, "{:?}", shapes[0]);
        assert!(shapes[0].contains(&(-2.0, -2.0)) && shapes[0].contains(&(12.0, 12.0)) && !shapes[0].contains(&(12.0, -2.0)) && shapes[0].contains(&(2.0, -2.0)));
        assert_eq!(highlighter_shapes(&[(5.0, 5.0)], 1.0), vec![vec![(4.0, 4.0), (6.0, 4.0), (6.0, 6.0), (4.0, 6.0)]]);
        // Level: its rectangle.
        assert_eq!(highlighter_shapes(&[(0.0, 0.0), (10.0, 0.0)], 1.0)[0].len(), 4);
    }

    #[test]
    fn words_are_held_by_their_alignment() {
        assert_eq!(left_of(100.0, 40.0, Alignment::Left), 100.0);
        assert_eq!(left_of(100.0, 40.0, Alignment::Center), 80.0);
        assert_eq!(left_of(100.0, 40.0, Alignment::Right), 60.0);
        for alignment in Alignment::ALL {
            assert_eq!(held_at(left_of(100.0, 40.0, alignment), 40.0, alignment), 100.0);
        }
    }
}
