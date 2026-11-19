//! The text read in one picture: its lines in reading order, each with the side it is read along,
//! the side opposite, and where each of its graphemes lies between them.
//!
//! Places are in the picture's own pixels, as it was read. A line's two sides run the way it is
//! read -- left to right for Latin, right to left for Arabic, down a column for vertical Japanese
//! -- so that a grapheme is a fraction of the way along both, and the stretch of a line between
//! two graphemes is the quadrilateral between those fractions of its two sides: true for a photo
//! taken askew, whose two sides are not parallel, as for a screenshot.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use spatiand_voice::hemera::ocr;
pub use spatiand_voice::hemera::ocr::Direction;

/// A point: x, y.
pub type Point = [f32; 2];

/// How sure hemera must be of a word for it to be copied unmarked. Under this it is shown, and
/// told to the room, as unsure: an ID number copied wrong is worse than none (hemera's advice).
pub const SURE: f32 = 0.9;

/// A place between two graphemes: before grapheme `at` of line `line` (`at` may be the line's
/// length: its end). In reading order, line by line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Spot {
    pub line: usize,
    pub at: usize,
}

/// One extended grapheme cluster -- é as e and its accent, a flag, a Thai syllable's marks, an
/// emoji with its skin tone: what a reader takes for one character.
#[derive(Debug, Clone, PartialEq)]
pub struct Grapheme {
    /// Its bytes in the line's text.
    pub bytes: Range<usize>,
    /// Where it begins and ends, as fractions of the way along the line.
    pub along: (f32, f32),
}

/// A word: graphemes between spaces, and how sure the reading of it is (0 to 1).
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub graphemes: Range<usize>,
    pub sure: f32,
}

/// One line of text.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    /// Which block (paragraph, column, a form's row) it is in: lines of two blocks are copied with
    /// an empty line between them.
    pub block: usize,
    pub direction: Direction,
    /// The side it is read along, from where reading begins to where it ends: its top, or for a
    /// column read downwards its right side.
    pub top: (Point, Point),
    /// The side opposite, the same way round.
    pub bottom: (Point, Point),
    pub graphemes: Vec<Grapheme>,
    pub words: Vec<Word>,
    pub sure: f32,
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: Point, b: Point) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: Point, b: Point) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}

fn length(a: Point) -> f32 {
    a[0].hypot(a[1])
}

pub(crate) fn lerp(a: Point, b: Point, t: f32) -> Point {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// Words where a line was given none: runs of graphemes between white space, as sure as the line.
fn words_of(text: &str, graphemes: &[Grapheme], sure: f32) -> Vec<Word> {
    let mut words = Vec::new();
    let mut start = None;
    for (i, g) in graphemes.iter().enumerate() {
        let space = text[g.bytes.clone()].chars().all(char::is_whitespace);
        match (space, start) {
            (false, None) => start = Some(i),
            (true, Some(s)) => {
                words.push(Word { graphemes: s..i, sure });
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        words.push(Word { graphemes: s..graphemes.len(), sure });
    }
    words
}

impl Line {
    /// A line as hemera read it, in block `block`. Its graphemes are `text`'s own, as hemera counts
    /// them (extended grapheme clusters), placed where it says; where it gave no places, or a
    /// different count of them, spread evenly along the line.
    pub fn of(read: &ocr::Line, block: usize) -> Option<Line> {
        if read.polygon.len() != 4 || read.text.is_empty() {
            return None;
        }
        let p = &read.polygon;
        // hemera's corners go clockwise from the line's own top left as it is read: the top is read
        // from 0 to 1 across (left to right) and down a column, from 0 to 3 right to left.
        let (top, bottom) = match read.direction {
            Direction::Rtl => ((p[0], p[3]), (p[1], p[2])),
            Direction::Ltr | Direction::Ttb => ((p[0], p[1]), (p[3], p[2])),
        };
        let long = length(sub(top.1, top.0)).max(1e-3);
        let clusters: Vec<(usize, &str)> = read.text.grapheme_indices(true).collect();
        let n = clusters.len();
        let placed = read.chars.len() == n;
        let graphemes: Vec<Grapheme> = clusters
            .iter()
            .enumerate()
            .map(|(i, (at, g))| {
                let along = if placed {
                    let [a, b] = read.chars[i];
                    ((a / long).clamp(0.0, 1.0), (b / long).clamp(0.0, 1.0))
                } else {
                    (i as f32 / n as f32, (i + 1) as f32 / n as f32)
                };
                Grapheme { bytes: *at..*at + g.len(), along: (along.0.min(along.1), along.0.max(along.1)) }
            })
            .collect();
        let mut words: Vec<Word> = read.words.iter().filter(|w| w.start < w.end && w.end <= n).map(|w| Word { graphemes: w.start..w.end, sure: w.confidence }).collect();
        if words.is_empty() {
            words = words_of(&read.text, &graphemes, read.confidence);
        }
        Some(Line { text: read.text.clone(), block, direction: read.direction, top, bottom, graphemes, words, sure: read.confidence })
    }

    /// A line of `text` set upright across `rect` (left, top, right, bottom in the picture's
    /// pixels), each glyph where `places` says (its bytes, its left and right): a PDF page's own
    /// characters, a subtitle as it is drawn. Read right to left if its first letter is Arabic or
    /// Hebrew. Certain: nothing was guessed at.
    pub fn across(text: &str, block: usize, rect: [f32; 4], places: &[(Range<usize>, f32, f32)]) -> Line {
        let [x0, y0, x1, y1] = rect;
        let rtl = text.chars().find(|c| c.is_alphabetic()).is_some_and(right_to_left);
        let wide = (x1 - x0).max(1e-3);
        let mut last = if rtl { x1 } else { x0 };
        let graphemes: Vec<Grapheme> = text
            .grapheme_indices(true)
            .map(|(at, g)| {
                let bytes = at..at + g.len();
                let mine = places.iter().filter(|(b, _, _)| bytes.contains(&b.start));
                let (left, right) = mine.fold((f32::MAX, f32::MIN), |(l, r), (_, a, b)| (l.min(*a), r.max(*b)));
                // A grapheme with no glyph of its own (a joiner) sits where the last one ended.
                let (left, right) = if left <= right { (left, right) } else { (last, last) };
                last = if rtl { left } else { right };
                let along = if rtl { ((x1 - right) / wide, (x1 - left) / wide) } else { ((left - x0) / wide, (right - x0) / wide) };
                Grapheme { bytes, along: (along.0.clamp(0.0, 1.0), along.1.clamp(0.0, 1.0)) }
            })
            .collect();
        let (top, bottom) = if rtl { (([x1, y0], [x0, y0]), ([x1, y1], [x0, y1])) } else { (([x0, y0], [x1, y0]), ([x0, y1], [x1, y1])) };
        let words = words_of(text, &graphemes, 1.0);
        Line { text: text.to_string(), block, direction: if rtl { Direction::Rtl } else { Direction::Ltr }, top, bottom, graphemes, words, sure: 1.0 }
    }

    pub fn len(&self) -> usize {
        self.graphemes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.graphemes.is_empty()
    }

    /// Its four corners: the top's start and end, then the bottom's end and start (clockwise for a
    /// line read across).
    pub fn corners(&self) -> [Point; 4] {
        [self.top.0, self.top.1, self.bottom.1, self.bottom.0]
    }

    /// How tall it is, about: from its top to its bottom, half way along.
    pub fn height(&self) -> f32 {
        length(sub(lerp(self.bottom.0, self.bottom.1, 0.5), lerp(self.top.0, self.top.1, 0.5)))
    }

    /// How long it is, along its top.
    pub fn length(&self) -> f32 {
        length(sub(self.top.1, self.top.0))
    }

    /// How far along the line the place before grapheme `at` is.
    fn edge(&self, at: usize) -> f32 {
        match (at.checked_sub(1).and_then(|i| self.graphemes.get(i)), self.graphemes.get(at)) {
            // Where the one before it ends.
            (Some(before), _) => before.along.1,
            (None, Some(first)) => first.along.0,
            (None, None) => 0.0,
        }
    }

    /// The stretch of the line from grapheme `from` to before `to`: its corners as
    /// [`Line::corners`] gives them.
    pub fn quad(&self, from: usize, to: usize) -> [Point; 4] {
        let (a, b) = (self.graphemes.get(from).map_or(self.edge(from), |g| g.along.0), self.edge(to));
        let (a, b) = (a.min(b), a.max(b));
        [lerp(self.top.0, self.top.1, a), lerp(self.top.0, self.top.1, b), lerp(self.bottom.0, self.bottom.1, b), lerp(self.bottom.0, self.bottom.1, a)]
    }

    /// Where `p` is in the line's own terms: how far along it (0 where reading begins, 1 where it
    /// ends) and how far across (0 its top, 1 its bottom); outside it, less than 0 or more than 1.
    /// The quadrilateral's own coordinates, its two sides not taken to be parallel.
    pub fn locate(&self, p: Point) -> (f32, f32) {
        let (a, b, c, d) = (self.top.0, self.top.1, self.bottom.0, self.bottom.1);
        // p = a + s e + t f + s t g, for s along and t across.
        let (e, f, h) = (sub(b, a), sub(c, a), sub(p, a));
        let g = [a[0] - b[0] - c[0] + d[0], a[1] - b[1] - c[1] + d[1]];
        let (qa, qb, qc) = (cross(f, g), cross(f, e) - cross(h, g), -cross(h, e));
        let t = if qa.abs() <= 1e-6 * (length(e) * length(f)).max(1.0) {
            if qb.abs() < 1e-9 { 0.0 } else { -qc / qb }
        } else {
            let root = (qb * qb - 4.0 * qa * qc).max(0.0).sqrt();
            let (t1, t2) = ((-qb - root) / (2.0 * qa), (-qb + root) / (2.0 * qa));
            // The one that is this quadrilateral's, not the folded one beyond it.
            if (t1 - 0.5).abs() <= (t2 - 0.5).abs() { t1 } else { t2 }
        };
        let way = [e[0] + t * g[0], e[1] + t * g[1]];
        let off = [h[0] - t * f[0], h[1] - t * f[1]];
        (dot(off, way) / dot(way, way).max(1e-9), t)
    }

    /// How far `p` is outside the line, in pixels: nothing if it is on it.
    pub fn distance(&self, p: Point) -> f32 {
        let (s, t) = self.locate(p);
        let out_along = (-s).max(s - 1.0).max(0.0) * self.length();
        let out_across = (-t).max(t - 1.0).max(0.0) * self.height();
        out_along.hypot(out_across)
    }

    /// The place between graphemes nearest `along` (a fraction of the line): 0 to its length.
    pub fn boundary(&self, along: f32) -> usize {
        self.graphemes.iter().position(|g| along < (g.along.0 + g.along.1) * 0.5).unwrap_or(self.graphemes.len())
    }

    /// The grapheme `along` is in, or the nearest one.
    pub fn grapheme_at(&self, along: f32) -> Option<usize> {
        if self.graphemes.is_empty() {
            return None;
        }
        let inside = self.graphemes.iter().position(|g| along >= g.along.0 && along <= g.along.1);
        inside.or_else(|| {
            let gap = |g: &Grapheme| if along < g.along.0 { g.along.0 - along } else { along - g.along.1 };
            (0..self.graphemes.len()).min_by(|&x, &y| gap(&self.graphemes[x]).total_cmp(&gap(&self.graphemes[y])))
        })
    }

    /// The graphemes of the word grapheme `at` is in -- or the grapheme alone, a space between two.
    pub fn word_around(&self, at: usize) -> Range<usize> {
        self.words.iter().find(|w| w.graphemes.contains(&at)).map_or(at..(at + 1).min(self.len()), |w| w.graphemes.clone())
    }

    /// Its text from grapheme `from` to before `to`.
    pub fn slice(&self, from: usize, to: usize) -> &str {
        let (from, to) = (from.min(self.len()), to.min(self.len()));
        if from >= to {
            return "";
        }
        &self.text[self.graphemes[from].bytes.start..self.graphemes[to - 1].bytes.end]
    }

    /// Its text from `from` to `to` with each word read unsurely marked `[?word?]`.
    fn marked(&self, from: usize, to: usize) -> String {
        let mut out = String::new();
        let mut at = from;
        let mut unsure: Vec<&Word> = self.words.iter().filter(|w| w.sure < SURE && w.graphemes.end > from && w.graphemes.start < to).collect();
        unsure.sort_by_key(|w| w.graphemes.start);
        for word in unsure {
            let (start, end) = (word.graphemes.start.max(from), word.graphemes.end.min(to));
            if start < at {
                continue;
            }
            out.push_str(self.slice(at, start));
            out.push_str("[?");
            out.push_str(self.slice(start, end));
            out.push_str("?]");
            at = end;
        }
        out.push_str(self.slice(at, to));
        out
    }
}

/// Whether a letter is of a script read right to left.
fn right_to_left(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF)
}

/// The text read in one picture.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReadText {
    /// The picture's size as it was read, in its pixels: what the places are in.
    pub size: (f32, f32),
    /// In reading order: blocks, and the lines of each.
    pub lines: Vec<Line>,
}

impl ReadText {
    /// What hemera read: its blocks' lines in its order.
    pub fn of(answer: &ocr::Answer) -> ReadText {
        let lines = answer.blocks.iter().enumerate().flat_map(|(b, block)| block.lines.iter().filter_map(move |line| Line::of(line, b))).collect();
        ReadText { size: (answer.image.width as f32, answer.image.height as f32), lines }
    }

    pub fn new(size: (f32, f32), lines: Vec<Line>) -> ReadText {
        ReadText { size, lines: lines.into_iter().filter(|l| !l.is_empty()).collect() }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The same text with every place moved by `map`, into a picture `size` across and down: read
    /// from a copy of a picture (smaller, turned, one eye of two), placed on the picture itself.
    /// A grapheme's fraction of the way along its line stays what it was, an affine map keeping
    /// fractions of a side.
    pub fn mapped(&self, map: &super::Mapping, size: (f32, f32)) -> ReadText {
        let lines = self
            .lines
            .iter()
            .map(|line| Line { top: (map.apply(line.top.0), map.apply(line.top.1)), bottom: (map.apply(line.bottom.0), map.apply(line.bottom.1)), ..line.clone() })
            .collect();
        ReadText { size, lines }
    }

    /// The very end of it.
    pub fn end(&self) -> Spot {
        self.lines.last().map_or(Spot::default(), |l| Spot { line: self.lines.len() - 1, at: l.len() })
    }

    /// `from` to `to` (either way round) as it is copied: what lies wholly before a line's start or
    /// after its end is not a line of it.
    pub fn span(&self, from: Spot, to: Spot) -> (Spot, Spot) {
        let (mut from, mut to) = (from.min(to), from.max(to));
        while from.line < to.line && self.lines.get(from.line).is_none_or(|l| from.at >= l.len()) {
            from = Spot { line: from.line + 1, at: 0 };
        }
        while to.line > from.line && to.at == 0 {
            to = Spot { line: to.line - 1, at: self.lines[to.line - 1].len() };
        }
        (from, to)
    }

    /// The text from `from` to `to`: lines one under another, blocks an empty line apart -- as
    /// hemera writes all of it.
    pub fn text(&self, from: Spot, to: Spot) -> String {
        self.joined(from, to, |line, a, b| line.slice(a, b).to_string())
    }

    /// All of it.
    pub fn all(&self) -> String {
        self.text(Spot::default(), self.end())
    }

    /// All of it, as the room's speech is told it: each word read unsurely marked `[?word?]`.
    pub fn described(&self) -> String {
        self.joined(Spot::default(), self.end(), |line, a, b| line.marked(a, b))
    }

    fn joined(&self, from: Spot, to: Spot, part: impl Fn(&Line, usize, usize) -> String) -> String {
        if self.lines.is_empty() {
            return String::new();
        }
        let (from, to) = self.span(from, to);
        let mut out = String::new();
        for index in from.line..=to.line.min(self.lines.len() - 1) {
            let line = &self.lines[index];
            if index > from.line {
                out.push_str(if line.block != self.lines[index - 1].block { "\n\n" } else { "\n" });
            }
            let a = if index == from.line { from.at } else { 0 };
            let b = if index == to.line { to.at } else { line.len() };
            out.push_str(&part(line, a, b));
        }
        out
    }

    /// The line `p` is on, give or take `slack` pixels round it; the nearest of two that are.
    pub fn line_at(&self, p: Point, slack: f32) -> Option<usize> {
        let near: Vec<(usize, f32)> = self.lines.iter().enumerate().map(|(i, l)| (i, l.distance(p))).filter(|(_, d)| *d <= slack).collect();
        near.into_iter().min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i)
    }

    /// The place between graphemes that `p` is at, on a line give or take `slack` pixels.
    pub fn hit(&self, p: Point, slack: f32) -> Option<Spot> {
        let line = self.line_at(p, slack)?;
        let (along, _) = self.lines[line].locate(p);
        Some(Spot { line, at: self.lines[line].boundary(along) })
    }

    /// The place nearest `p`, wherever it is: a drag that has left the text.
    pub fn nearest(&self, p: Point) -> Spot {
        let Some(line) = (0..self.lines.len()).min_by(|&a, &b| self.lines[a].distance(p).total_cmp(&self.lines[b].distance(p))) else { return Spot::default() };
        let (along, _) = self.lines[line].locate(p);
        Spot { line, at: self.lines[line].boundary(along.clamp(0.0, 1.0)) }
    }

    /// The word at `p`'s grapheme on `line`: its start and end.
    pub fn word(&self, line: usize, p: Point) -> (Spot, Spot) {
        let Some(l) = self.lines.get(line) else { return (Spot::default(), Spot::default()) };
        let (along, _) = l.locate(p);
        let at = l.grapheme_at(along.clamp(0.0, 1.0)).unwrap_or(0);
        let word = l.word_around(at);
        (Spot { line, at: word.start }, Spot { line, at: word.end })
    }

    /// All of `line`.
    pub fn whole(&self, line: usize) -> (Spot, Spot) {
        (Spot { line, at: 0 }, Spot { line, at: self.lines.get(line).map_or(0, Line::len) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line as hemera would send it, upright across `x0..x1` at `y0..y1`, its graphemes evenly
    /// placed, in block `block`, with the words' sureness given in order.
    fn upright(text: &str, x0: f32, y0: f32, x1: f32, y1: f32, block: usize, sure: &[f32]) -> Line {
        let n = text.graphemes(true).count();
        let w = (x1 - x0) / n as f32;
        let read = ocr::Line {
            text: text.into(),
            polygon: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
            direction: Direction::Ltr,
            confidence: 0.99,
            chars: (0..n).map(|i| [i as f32 * w, (i + 1) as f32 * w]).collect(),
            ..Default::default()
        };
        let mut line = Line::of(&read, block).unwrap();
        for (word, s) in line.words.iter_mut().zip(sure) {
            word.sure = *s;
        }
        line
    }

    #[test]
    fn graphemes_are_whole_characters_whatever_they_are_made_of() {
        // e and a combining acute; a flag; a thumb with its skin tone; Thai with its marks; a
        // family joined by ZWJ.
        let text = "e\u{301} \u{1F1EE}\u{1F1E9} \u{1F44D}\u{1F3FD} \u{0E01}\u{0E34}\u{0E48} \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        let line = Line::of(&ocr::Line { text: text.into(), polygon: vec![[0.0, 0.0], [90.0, 0.0], [90.0, 10.0], [0.0, 10.0]], confidence: 1.0, ..Default::default() }, 0).unwrap();
        let parts: Vec<&str> = (0..line.len()).map(|i| line.slice(i, i + 1)).collect();
        assert_eq!(parts, ["e\u{301}", " ", "\u{1F1EE}\u{1F1E9}", " ", "\u{1F44D}\u{1F3FD}", " ", "\u{0E01}\u{0E34}\u{0E48}", " ", "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"]);
        // Placed evenly where hemera said nothing of places; words between the spaces.
        assert_eq!(line.graphemes[1].along, (1.0 / 9.0, 2.0 / 9.0));
        assert_eq!(line.words.iter().map(|w| line.slice(w.graphemes.start, w.graphemes.end)).collect::<Vec<_>>(), ["e\u{301}", "\u{1F1EE}\u{1F1E9}", "\u{1F44D}\u{1F3FD}", "\u{0E01}\u{0E34}\u{0E48}", "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"]);
        // A grapheme chosen half is the whole of it: a boundary is never inside one.
        let mid = (line.graphemes[0].along.0 + line.graphemes[0].along.1) * 0.5;
        assert_eq!(line.boundary(mid - 0.01), 0);
        assert_eq!(line.boundary(mid + 0.01), 1);
    }

    #[test]
    fn a_choice_across_lines_and_blocks_is_copied_as_hemera_writes_it() {
        let read = ReadText::new(
            (1000.0, 1000.0),
            vec![
                upright("PROVINSI DKI JAKARTA", 100.0, 100.0, 500.0, 130.0, 0, &[]),
                upright("KOTA JAKARTA SELATAN", 100.0, 140.0, 500.0, 170.0, 0, &[]),
                upright("NIK", 100.0, 300.0, 160.0, 330.0, 1, &[]),
                upright(": 3171234567890001", 200.0, 300.0, 560.0, 330.0, 1, &[]),
            ],
        );
        assert_eq!(read.all(), "PROVINSI DKI JAKARTA\nKOTA JAKARTA SELATAN\n\nNIK\n: 3171234567890001");
        // From "JAKARTA" on the first line to "KOTA" on the second.
        assert_eq!(read.text(Spot { line: 0, at: 13 }, Spot { line: 1, at: 4 }), "JAKARTA\nKOTA");
        // Either way round.
        assert_eq!(read.text(Spot { line: 1, at: 4 }, Spot { line: 0, at: 13 }), "JAKARTA\nKOTA");
        // Across the blocks: an empty line between.
        assert_eq!(read.text(Spot { line: 1, at: 13 }, Spot { line: 3, at: 6 }), "SELATAN\n\nNIK\n: 3171");
        // Ending at a line's very start, or starting at one's very end: not a line of it.
        assert_eq!(read.text(Spot { line: 0, at: 9 }, Spot { line: 1, at: 0 }), "DKI JAKARTA");
        assert_eq!(read.text(Spot { line: 0, at: 20 }, Spot { line: 1, at: 4 }), "KOTA");
    }

    #[test]
    fn unsure_words_are_marked_for_the_room() {
        let read = ReadText::new((500.0, 100.0), vec![upright("NIK : 3171234567890001", 0.0, 0.0, 440.0, 30.0, 0, &[0.99, 0.99, 0.62])]);
        assert_eq!(read.described(), "NIK : [?3171234567890001?]");
        assert_eq!(read.all(), "NIK : 3171234567890001", "copied as read, unmarked");
    }

    #[test]
    fn a_press_finds_its_line_and_the_place_between_graphemes() {
        let read = ReadText::new((1000.0, 1000.0), vec![upright("abcd", 100.0, 100.0, 140.0, 120.0, 0, &[]), upright("efgh", 100.0, 130.0, 140.0, 150.0, 0, &[])]);
        // Inside the first line, a little past half of "b": after it.
        assert_eq!(read.hit([116.0, 110.0], 0.0), Some(Spot { line: 0, at: 2 }));
        assert_eq!(read.hit([116.0, 140.0], 0.0), Some(Spot { line: 1, at: 2 }));
        // Between the lines, given slack: the nearer.
        assert_eq!(read.hit([116.0, 123.0], 5.0).map(|s| s.line), Some(0));
        assert_eq!(read.hit([116.0, 127.0], 5.0).map(|s| s.line), Some(1));
        assert_eq!(read.hit([300.0, 110.0], 5.0), None);
        // Off everything, the nearest place: past a line's end is its end.
        assert_eq!(read.nearest([300.0, 112.0]), Spot { line: 0, at: 4 });
        assert_eq!(read.nearest([0.0, 900.0]), Spot { line: 1, at: 0 });
        assert_eq!(read.word(0, [116.0, 110.0]), (Spot { line: 0, at: 0 }, Spot { line: 0, at: 4 }));
    }

    #[test]
    fn a_line_read_from_glyph_places_is_its_glyphs_and_right_to_left_for_arabic() {
        let line = Line::across("ab c", 0, [10.0, 0.0, 50.0, 10.0], &[(0..1, 10.0, 20.0), (1..2, 20.0, 30.0), (2..3, 30.0, 35.0), (3..4, 35.0, 50.0)]);
        assert_eq!(line.direction, Direction::Ltr);
        assert_eq!(line.graphemes[1].along, (0.25, 0.5));
        assert_eq!(line.words.len(), 2);
        // Arabic, set from the right: its first letter is at the right, read first.
        let arabic = "\u{0645}\u{0631}";
        let line = Line::across(arabic, 0, [0.0, 0.0, 20.0, 10.0], &[(2..4, 0.0, 10.0), (0..2, 10.0, 20.0)]);
        assert_eq!(line.direction, Direction::Rtl);
        assert_eq!(line.top, ([20.0, 0.0], [0.0, 0.0]));
        assert_eq!(line.graphemes[0].along, (0.0, 0.5));
        assert_eq!(line.quad(0, 1), [[20.0, 0.0], [10.0, 0.0], [10.0, 10.0], [20.0, 10.0]]);
    }
}
