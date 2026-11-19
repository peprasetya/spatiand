//! A PDF marked up, written by PDFium as an incremental update -- the file as it was, and after
//! it what changed -- with every mark a real annotation, so that Preview or Acrobat can still
//! move, change or delete it: a box a Square annotation, an oval a Circle, words a FreeText, an
//! arrow a Line with an arrow at its end. Each carries its own appearance, drawn here as a
//! page's operators -- the words as their glyphs' outlines, set by the room's text engine
//! (`ink`), so that every script shows with no font in the PDF -- and looks in any reader as
//! it looked in SpatiLoupe. A crop sets the page's crop box.
//!
//! **A workaround, named:** PDFium makes no Line annotations (`FPDFAnnot_IsSupportedSubtype`
//! leaves Line out, and nothing sets a line's ends or endings), so an arrow is made as an Ink
//! annotation of the same look, and a second update of SpatiLoupe's own, after PDFium's, gives
//! that annotation's dictionary as a Line's (`/L`, `/LE [/None /ClosedArrow]`). The update is
//! read back by PDFium before the file is written; if it cannot be made (an encrypted PDF, whose
//! strings SpatiLoupe does not encrypt) or does not read back, the arrow stays an Ink
//! annotation, which looks the same and is moved and deleted as one. The proper way is PDFium
//! making Line annotations itself (`FPDFPage_CreateAnnot` with `FPDF_ANNOT_LINE`, and a setter
//! for `/L` and `/LE`), upstream or in a PDFium built by this project.

use pdfium_render::prelude::*;

use crate::ink::{self, Segment};
use crate::markup::{arrow_head, Area, Marks, Note, Point, Shape};

/// PDFium's numbers for what it makes, from `fpdf_annot.h` and `fpdf_save.h`.
const FREETEXT: i32 = 3;
const SQUARE: i32 = 5;
const CIRCLE: i32 = 6;
const INK: i32 = 15;
const FLAG_PRINT: i32 = 4;
const APPEARANCE_NORMAL: i32 = 0;
const COLOUR: u32 = 0;
const INCREMENTAL: FPDF_DWORD = 1;

/// Where a page's points, as it is shown (from its top left, down; turned and cropped as PDFium
/// shows it), are in the page's own space: `(a x + c y + e, b x + d y + f)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    /// From where the shown page's top left, top right and bottom left corners are, on a page
    /// shown `size` points across and down.
    pub fn from_corners(top_left: (f64, f64), top_right: (f64, f64), bottom_left: (f64, f64), size: (f64, f64)) -> Affine {
        let (w, h) = (size.0.max(1e-6), size.1.max(1e-6));
        Affine {
            a: (top_right.0 - top_left.0) / w,
            b: (top_right.1 - top_left.1) / w,
            c: (bottom_left.0 - top_left.0) / h,
            d: (bottom_left.1 - top_left.1) / h,
            e: top_left.0,
            f: top_left.1,
        }
    }

    pub fn apply(&self, p: Point) -> (f64, f64) {
        let (x, y) = (p.0 as f64, p.1 as f64);
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }
}

/// A number as a PDF writes it: no more places than it needs.
fn n(v: f64) -> String {
    let text = format!("{v:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-0" { "0".to_string() } else { text.to_string() }
}

fn colour(rgb: [u8; 3]) -> String {
    format!("{} {} {}", n(rgb[0] as f64 / 255.0), n(rgb[1] as f64 / 255.0), n(rgb[2] as f64 / 255.0))
}

/// An appearance as it is built: its operators, and the points it reaches.
struct Drawing {
    ops: String,
    reach: Vec<(f64, f64)>,
}

impl Drawing {
    fn new() -> Drawing {
        Drawing { ops: String::new(), reach: Vec::new() }
    }

    fn op(&mut self, points: &[(f64, f64)], operator: &str) {
        for p in points {
            self.ops.push_str(&format!("{} {} ", n(p.0), n(p.1)));
            self.reach.push(*p);
        }
        self.ops.push_str(operator);
        self.ops.push('\n');
    }

    fn raw(&mut self, text: &str) {
        self.ops.push_str(text);
        self.ops.push('\n');
    }

    /// What it reaches, `margin` further all round, in the page's own space (up the page).
    fn bounds(&self, margin: f64) -> Area {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in &self.reach {
            (x0, y0, x1, y1) = (x0.min(p.0), y0.min(p.1), x1.max(p.0), y1.max(p.1));
        }
        if x0 > x1 {
            (x0, y0, x1, y1) = (0.0, 0.0, 0.0, 0.0);
        }
        Area { x0: (x0 - margin) as f32, y0: (y0 - margin) as f32, x1: (x1 + margin) as f32, y1: (y1 + margin) as f32 }
    }
}

/// Bezier's constant for a quarter of an ellipse.
const KAPPA: f32 = 0.552_284_8;

/// A note's appearance, drawn in its page's own space through `affine`.
fn appearance(note: &Note, affine: &Affine, fonts: &mut ink::Fonts) -> Drawing {
    let mut d = Drawing::new();
    let rgb = colour(note.ink.rgb());
    let width = note.size as f64;
    let p = |q: Point| affine.apply(q);
    match &note.shape {
        Shape::Box(a) => {
            d.raw(&format!("{} w 0 J 0 j [] 0 d {rgb} RG", n(width)));
            let c = a.corners();
            d.op(&[p(c[0])], "m");
            for corner in &c[1..] {
                d.op(&[p(*corner)], "l");
            }
            d.raw("h S");
        }
        Shape::Oval(a) => {
            d.raw(&format!("{} w 1 J 1 j [] 0 d {rgb} RG", n(width)));
            let (cx, cy) = a.centre();
            let (rx, ry) = (a.w() * 0.5, a.h() * 0.5);
            let (kx, ky) = (rx * KAPPA, ry * KAPPA);
            d.op(&[p((cx + rx, cy))], "m");
            d.op(&[p((cx + rx, cy + ky)), p((cx + kx, cy + ry)), p((cx, cy + ry))], "c");
            d.op(&[p((cx - kx, cy + ry)), p((cx - rx, cy + ky)), p((cx - rx, cy))], "c");
            d.op(&[p((cx - rx, cy - ky)), p((cx - kx, cy - ry)), p((cx, cy - ry))], "c");
            d.op(&[p((cx + kx, cy - ry)), p((cx + rx, cy - ky)), p((cx + rx, cy))], "c");
            d.raw("h S");
        }
        Shape::Arrow { from, to } => {
            let (stop, head) = arrow_head(*from, *to, note.size);
            d.raw(&format!("{} w 1 J 1 j [] 0 d {rgb} RG {rgb} rg", n(width)));
            d.op(&[p(*from)], "m");
            d.op(&[p(stop)], "l");
            d.raw("S");
            d.op(&[p(head[0])], "m");
            d.op(&[p(head[1])], "l");
            d.op(&[p(head[2])], "l");
            d.raw("h f");
        }
        Shape::Text { at, words, .. } => {
            d.raw(&format!("{rgb} rg"));
            let mut current = (0.0f32, 0.0f32);
            let mut start = current;
            let shift = |q: Point| (q.0 + at.0, q.1 + at.1);
            for segment in ink::outlines(fonts, words, note.size) {
                match segment {
                    Segment::Move(q) => {
                        let q = shift(q);
                        d.op(&[p(q)], "m");
                        (current, start) = (q, q);
                    }
                    Segment::Line(q) => {
                        let q = shift(q);
                        d.op(&[p(q)], "l");
                        current = q;
                    }
                    // A PDF has no quadratic curves: each is the cubic it equals.
                    Segment::Quad(c, q) => {
                        let (c, q) = (shift(c), shift(q));
                        let c1 = (current.0 + (c.0 - current.0) * 2.0 / 3.0, current.1 + (c.1 - current.1) * 2.0 / 3.0);
                        let c2 = (q.0 + (c.0 - q.0) * 2.0 / 3.0, q.1 + (c.1 - q.1) * 2.0 / 3.0);
                        d.op(&[p(c1), p(c2), p(q)], "c");
                        current = q;
                    }
                    Segment::Cubic(a, b, q) => {
                        let (a, b, q) = (shift(a), shift(b), shift(q));
                        d.op(&[p(a), p(b), p(q)], "c");
                        current = q;
                    }
                    Segment::Close => {
                        d.raw("h");
                        current = start;
                    }
                }
            }
            d.raw("f");
        }
    }
    d
}

// --- the arrows made Line annotations: a little of a PDF's syntax ---

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | b'\0')
}

fn is_delimiter(c: u8) -> bool {
    is_space(c) || matches!(c, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// Past the literal string that starts at `at`: its brackets balanced, its escapes skipped.
fn past_string(text: &[u8], at: usize) -> usize {
    let mut depth = 0;
    let mut i = at;
    while i < text.len() {
        match text[i] {
            b'\\' => i += 1,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    text.len()
}

/// Past the value that starts at `at` (no space before it): a dictionary, an array, a string,
/// a name, or a number -- with its generation and `R`, if it is a reference.
fn past_value(text: &[u8], at: usize) -> usize {
    let Some(&first) = text.get(at) else { return at };
    match first {
        b'(' => past_string(text, at),
        b'<' if text.get(at + 1) == Some(&b'<') => past_brackets(text, at, b"<<", b">>"),
        b'<' => text[at..].iter().position(|&c| c == b'>').map_or(text.len(), |p| at + p + 1),
        b'[' => past_brackets(text, at, b"[", b"]"),
        b'/' => at + 1 + text[at + 1..].iter().position(|&c| is_delimiter(c)).unwrap_or(text.len() - at - 1),
        _ => {
            let word = |from: usize| from + text[from..].iter().position(|&c| is_delimiter(c)).unwrap_or(text.len() - from);
            let end = word(at);
            // `12 0 R`: a reference is three words.
            let skip = |from: usize| from + text[from..].iter().take_while(|&&c| is_space(c)).count();
            let numeric = |a: usize, b: usize| b > a && text[a..b].iter().all(|c| c.is_ascii_digit());
            if numeric(at, end) {
                let g0 = skip(end);
                let g1 = word(g0);
                let r0 = skip(g1);
                if numeric(g0, g1) && text.get(r0) == Some(&b'R') && text.get(r0 + 1).is_none_or(|&c| is_delimiter(c)) {
                    return r0 + 1;
                }
            }
            end.max(at + 1)
        }
    }
}

/// Past the bracketed thing at `at` (`<<`...`>>` or `[`...`]`), what is inside it skipped as
/// values are.
fn past_brackets(text: &[u8], at: usize, open: &[u8], close: &[u8]) -> usize {
    let mut i = at + open.len();
    while i < text.len() {
        if text[i..].starts_with(close) {
            return i + close.len();
        }
        if is_space(text[i]) {
            i += 1;
        } else {
            i = past_value(text, i).max(i + 1);
        }
    }
    text.len()
}

/// The dictionary that starts at `at`, as its keys and each key's value as written.
fn entries(text: &[u8], at: usize) -> Vec<(Vec<u8>, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    if !text[at..].starts_with(b"<<") {
        return out;
    }
    let mut i = at + 2;
    loop {
        while i < text.len() && is_space(text[i]) {
            i += 1;
        }
        if i >= text.len() || text[i..].starts_with(b">>") || text[i] != b'/' {
            return out;
        }
        let key_end = past_value(text, i);
        let key = text[i + 1..key_end].to_vec();
        let mut v = key_end;
        while v < text.len() && is_space(text[v]) {
            v += 1;
        }
        let end = past_value(text, v);
        out.push((key, v..end));
        i = end;
    }
}

/// Where each dictionary in `text` is, `<<` to past `>>`: strings skipped.
fn dictionaries(text: &[u8]) -> Vec<std::ops::Range<usize>> {
    let (mut out, mut open, mut i) = (Vec::new(), Vec::new(), 0);
    while i < text.len() {
        match text[i] {
            b'(' => i = past_string(text, i),
            b'<' if text.get(i + 1) == Some(&b'<') => {
                open.push(i);
                i += 2;
            }
            b'<' => i = past_value(text, i),
            b'>' if text.get(i + 1) == Some(&b'>') => {
                if let Some(start) = open.pop() {
                    out.push(start..i + 2);
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    out
}

fn find(text: &[u8], what: &[u8], from: usize) -> Option<usize> {
    text.get(from..)?.windows(what.len()).position(|w| w == what).map(|p| p + from)
}

fn rfind(text: &[u8], what: &[u8], before: usize) -> Option<usize> {
    text[..before.min(text.len())].windows(what.len()).rposition(|w| w == what)
}

/// An arrow, to be made a Line annotation.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrowLine {
    /// Its annotation's name (`/NM`), which finds it.
    pub name: String,
    /// Its ends, in the page's own space.
    pub line: [f64; 4],
    pub rgb: [u8; 3],
    pub width: f64,
}

/// The indirect object written at a line's start before `at`: its number, its generation, and
/// where its body begins and its `endobj` is.
fn object_round(text: &[u8], at: usize) -> Option<(u32, u16, usize, usize)> {
    let mut before = at;
    loop {
        let obj = rfind(text, b" obj", before)?;
        // `\n12 0 obj`, the number at a line's start.
        let head = &text[..obj];
        let generation_start = head.iter().rposition(|&c| c == b' ')? + 1;
        let number_end = generation_start - 1;
        let number_start = head[..number_end].iter().rposition(|&c| !c.is_ascii_digit()).map_or(0, |p| p + 1);
        let at_line_start = number_start == 0 || matches!(head[number_start - 1], b'\n' | b'\r');
        let number = std::str::from_utf8(&head[number_start..number_end]).ok().and_then(|s| s.parse().ok());
        let generation = std::str::from_utf8(&head[generation_start..]).ok().and_then(|s| s.parse().ok());
        if let (true, Some(number), Some(generation)) = (at_line_start, number, generation) {
            let body = obj + 4;
            let end = find(text, b"endobj", at)?;
            return Some((number, generation, body, end));
        }
        before = obj;
    }
}

/// PDFium's incremental save of a file whose first `original` bytes were the file before it,
/// with each of `arrows` -- an Ink annotation PDFium wrote -- given again as a Line annotation
/// with an arrow at its end, in an update of its own after PDFium's: its page (or itself, were it
/// written apart) written again, a cross-reference of the same kind as PDFium's, and a trailer.
pub fn arrows_as_lines(saved: &[u8], original: usize, arrows: &[ArrowLine]) -> Result<Vec<u8>, String> {
    // Which object each arrow is in -- its number, its generation, where its body begins and
    // ends -- and where in it each arrow's name is.
    type Holding<'a> = (u32, u16, usize, usize, Vec<(usize, &'a ArrowLine)>);
    let mut objects: Vec<Holding> = Vec::new();
    for arrow in arrows {
        let mark = format!("/NM({})", arrow.name).into_bytes();
        let at = find(saved, &mark, original).ok_or_else(|| format!("{} is not in what PDFium wrote", arrow.name))?;
        let (number, generation, body, end) = object_round(saved, at).ok_or("no object round an arrow")?;
        match objects.iter_mut().find(|o| o.0 == number) {
            Some(object) => object.4.push((at, arrow)),
            None => objects.push((number, generation, body, end, vec![(at, arrow)])),
        }
    }
    let mut out = saved.to_vec();
    if !out.ends_with(b"\n") {
        out.extend_from_slice(b"\r\n");
    }
    let mut written: Vec<(u32, u16, usize)> = Vec::new();
    for (number, generation, body, end, arrows) in &objects {
        let text = &saved[*body..*end];
        let spans = dictionaries(text);
        // Each arrow's own dictionary: the smallest that holds its name.
        let mut changes: Vec<(std::ops::Range<usize>, Vec<u8>)> = Vec::new();
        for (at, arrow) in arrows {
            let at = at - body;
            let span = spans.iter().filter(|s| s.contains(&at)).min_by_key(|s| s.len()).ok_or("an arrow outside any dictionary")?.clone();
            let mut dict = text[span.clone()].to_vec();
            let ink = find(&dict, b"/Subtype/Ink", 0).ok_or("an arrow that is not Ink")?;
            dict.splice(ink..ink + 12, b"/Subtype/Line".iter().copied());
            if let Some(list) = find(&dict, b"/InkList", 0) {
                let mut v = list + 8;
                while v < dict.len() && is_space(dict[v]) {
                    v += 1;
                }
                let end = past_value(&dict, v);
                dict.drain(list..end);
            }
            let [x1, y1, x2, y2] = arrow.line;
            let rgb = colour(arrow.rgb);
            let added = format!("/L[{} {} {} {}]/LE[/None/ClosedArrow]/IC[{rgb}]/BS<</W {}/S/S>>", n(x1), n(y1), n(x2), n(y2), n(arrow.width));
            let close = dict.len() - 2;
            dict.splice(close..close, added.into_bytes());
            changes.push((span, dict));
        }
        changes.sort_by_key(|c| std::cmp::Reverse(c.0.start));
        let mut text = text.to_vec();
        for (span, dict) in changes {
            text.splice(span, dict);
        }
        written.push((*number, *generation, out.len()));
        out.extend_from_slice(format!("{number} {generation} obj\r\n").as_bytes());
        out.extend_from_slice(text.trim_ascii());
        out.extend_from_slice(b"\r\nendobj\r\n");
    }
    // PDFium's cross-reference, and so its trailer's values.
    let start = rfind(saved, b"startxref", saved.len()).ok_or("no startxref")?;
    let xref: usize = std::str::from_utf8(&saved[start + 9..]).ok().and_then(|s| s.split_whitespace().next()).and_then(|s| s.parse().ok()).ok_or("no cross-reference")?;
    let classic = saved.get(xref..).is_some_and(|t| t.starts_with(b"xref"));
    let dict_at = if classic { find(saved, b"<<", find(saved, b"trailer", xref).ok_or("no trailer")?) } else { find(saved, b"<<", xref) }.ok_or("no trailer dictionary")?;
    let values = entries(saved, dict_at);
    let value = |key: &[u8]| values.iter().find(|(k, _)| k == key).map(|(_, r)| String::from_utf8_lossy(&saved[r.clone()]).into_owned());
    if value(b"Encrypt").is_some() {
        return Err("the PDF is encrypted".into());
    }
    let size: u32 = value(b"Size").and_then(|s| s.trim().parse().ok()).ok_or("a trailer with no size")?;
    let root = value(b"Root").ok_or("a trailer with no root")?;
    let mut trailer = format!("/Root {root}");
    for key in ["Info", "ID"] {
        if let Some(v) = value(key.as_bytes()) {
            trailer.push_str(&format!("/{key} {v}"));
        }
    }
    written.sort_by_key(|w| w.0);
    if classic {
        let at = out.len();
        out.extend_from_slice(b"xref\r\n");
        for (number, generation, offset) in &written {
            out.extend_from_slice(format!("{number} 1\r\n{offset:010} {generation:05} n\r\n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\r\n<</Size {size}{trailer}/Prev {xref}>>\r\nstartxref\r\n{at}\r\n%%EOF\r\n").as_bytes());
    } else {
        // A cross-reference stream, as PDFium's: one more object, given the next number.
        let own = size;
        let at = out.len();
        let mut rows = Vec::new();
        let mut index = String::new();
        for (number, generation, offset) in written.iter().copied().chain([(own, 0, at)]) {
            let offset = u32::try_from(offset).map_err(|_| "a PDF too long for this cross-reference")?;
            rows.push(1u8);
            rows.extend_from_slice(&offset.to_be_bytes());
            rows.push(generation.min(255) as u8);
            index.push_str(&format!("{number} 1 "));
        }
        out.extend_from_slice(format!("{own} 0 obj\r\n<</Type/XRef/Size {}{trailer}/Prev {xref}/W[1 4 1]/Index[{}]/Length {}>>stream\r\n", own + 1, index.trim_end(), rows.len()).as_bytes());
        out.extend_from_slice(&rows);
        out.extend_from_slice(format!("\r\nendstream\r\nendobj\r\nstartxref\r\n{at}\r\n%%EOF\r\n").as_bytes());
    }
    Ok(out)
}

// --- PDFium ---

/// A day, as the year, month and day it is in (Hinnant's civil from days).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// A moment as a PDF writes one, in UTC: `D:20261119143000Z`.
fn pdf_date(seconds: i64) -> String {
    let (year, month, day) = civil(seconds.div_euclid(86_400));
    let s = seconds.rem_euclid(86_400);
    format!("D:{year:04}{month:02}{day:02}{:02}{:02}{:02}Z", s / 3600, s / 60 % 60, s % 60)
}

/// What a mark is written as, to be found again in what was written.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Made {
    Square,
    Circle,
    Arrow,
    FreeText,
}

/// Where PDFium writes a save: into memory.
#[repr(C)]
struct Sink {
    raw: FPDF_FILEWRITE,
    bytes: Vec<u8>,
}

unsafe extern "C" fn write_block(this: *mut FPDF_FILEWRITE, data: *const std::ffi::c_void, size: std::os::raw::c_ulong) -> std::os::raw::c_int {
    let sink = unsafe { &mut *(this as *mut Sink) };
    sink.bytes.extend_from_slice(unsafe { std::slice::from_raw_parts(data as *const u8, size as usize) });
    1
}

/// Where the page's points as PDFium shows it are in its own space, from PDFium's own sum for
/// a page shown `w` by `h`: its rotation and its crop box both.
unsafe fn page_affine(b: &dyn PdfiumLibraryBindings, page: FPDF_PAGE, w: f64, h: f64) -> Affine {
    let (sx, sy) = ((w * 100.0).round().max(1.0) as i32, (h * 100.0).round().max(1.0) as i32);
    let at = |x: i32, y: i32| {
        let (mut px, mut py) = (0.0f64, 0.0f64);
        unsafe { b.FPDF_DeviceToPage(page, 0, 0, sx, sy, 0, x, y, &mut px, &mut py) };
        (px, py)
    };
    Affine::from_corners(at(0, 0), at(sx, 0), at(0, sy), (w, h))
}

fn rect(a: &Area) -> FS_RECTF {
    FS_RECTF { left: a.x0, bottom: a.y0, right: a.x1, top: a.y1 }
}

/// One annotation made on `page`: its kind, where it is, its colour (none for words, whose box
/// has no background), its line's width, its name and its appearance.
#[allow(clippy::too_many_arguments)]
unsafe fn make(b: &dyn PdfiumLibraryBindings, page: FPDF_PAGE, kind: i32, at: &Area, rgb: Option<[u8; 3]>, border: f32, name: &str, when: &str, ap: &str, more: impl FnOnce(FPDF_ANNOTATION)) -> Result<(), String> {
    let annot = unsafe { b.FPDFPage_CreateAnnot(page, kind) };
    if annot.is_null() {
        return Err("PDFium would not make an annotation".into());
    }
    let made = unsafe {
        let mut ok = b.is_true(b.FPDFAnnot_SetRect(annot, &rect(at)));
        if let Some([r, g, bl]) = rgb {
            ok &= b.is_true(b.FPDFAnnot_SetColor(annot, COLOUR, r as u32, g as u32, bl as u32, 255));
        }
        ok &= b.is_true(b.FPDFAnnot_SetBorder(annot, 0.0, 0.0, border));
        ok &= b.is_true(b.FPDFAnnot_SetFlags(annot, FLAG_PRINT));
        ok &= b.is_true(b.FPDFAnnot_SetStringValue_str(annot, "NM", name));
        ok &= b.is_true(b.FPDFAnnot_SetStringValue_str(annot, "M", when));
        more(annot);
        // Last: what else is set does not touch the appearance after it.
        ok &= b.is_true(b.FPDFAnnot_SetAP_str(annot, APPEARANCE_NORMAL, ap));
        b.FPDFPage_CloseAnnot(annot);
        ok
    };
    if made { Ok(()) } else { Err("PDFium would not write an annotation's parts".into()) }
}

/// The PDF of `bytes` with `marks` written into it as annotations and crop boxes, by PDFium as
/// an incremental update (and the arrows made Line annotations after it, as the module says):
/// the whole file to be written. What was written is opened again and its marks found before it
/// is given back. An encrypted PDF is opened with `password` (none for one that opens without
/// asking) and written encrypted as it was: PDFium's update encrypts what it adds with the
/// document's own key, under its own security dictionary and file identifier.
pub fn annotate(pdfium: &Pdfium, bytes: &[u8], marks: &Marks, password: Option<&str>) -> Result<Vec<u8>, String> {
    let b = pdfium.bindings();
    let doc = unsafe { b.FPDF_LoadMemDocument64(bytes, password) };
    if doc.is_null() {
        return Err("PDFium would not open it again".into());
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let when = pdf_date(stamp);
    let mut fonts = ink::fonts();
    let mut made: Vec<(usize, String, Made)> = Vec::new();
    let mut arrows: Vec<ArrowLine> = Vec::new();
    let written = (|| -> Result<(Vec<u8>, usize, bool), String> {
        let pages = unsafe { b.FPDF_GetPageCount(doc) }.max(0) as usize;
        for index in marks.pages() {
            if index >= pages {
                return Err(format!("it has no page {}", index + 1));
            }
            let page = unsafe { b.FPDF_LoadPage(doc, index as i32) };
            if page.is_null() {
                return Err(format!("page {} would not open", index + 1));
            }
            let done = (|| -> Result<(), String> {
                let (w, h) = unsafe { (b.FPDF_GetPageWidthF(page) as f64, b.FPDF_GetPageHeightF(page) as f64) };
                let affine = unsafe { page_affine(b, page, w, h) };
                for (k, note) in marks.notes.iter().filter(|n| n.page == index).enumerate() {
                    let name = format!("spatiloupe-{stamp}-{index}-{k}");
                    let drawing = appearance(note, &affine, &mut fonts);
                    let margin = note.size as f64 * 0.5 + 1.0;
                    let rgb = note.ink.rgb();
                    let kind = match &note.shape {
                        Shape::Box(_) => {
                            unsafe { make(b, page, SQUARE, &drawing.bounds(margin), Some(rgb), note.size, &name, &when, &drawing.ops, |_| {}) }?;
                            Made::Square
                        }
                        Shape::Oval(_) => {
                            unsafe { make(b, page, CIRCLE, &drawing.bounds(margin), Some(rgb), note.size, &name, &when, &drawing.ops, |_| {}) }?;
                            Made::Circle
                        }
                        Shape::Arrow { from, to } => {
                            let (stop, head) = arrow_head(*from, *to, note.size);
                            let point = |p: Point| {
                                let (x, y) = affine.apply(p);
                                FS_POINTF { x: x as f32, y: y as f32 }
                            };
                            let strokes = [vec![point(*from), point(stop)], vec![point(head[1]), point(head[0]), point(head[2])]];
                            unsafe {
                                make(b, page, INK, &drawing.bounds(margin), Some(rgb), note.size, &name, &when, &drawing.ops, |annot| {
                                    for stroke in &strokes {
                                        b.FPDFAnnot_AddInkStroke(annot, stroke.as_ptr(), stroke.len() as _);
                                    }
                                })
                            }?;
                            let (a, z) = (affine.apply(*from), affine.apply(*to));
                            arrows.push(ArrowLine { name: name.clone(), line: [a.0, a.1, z.0, z.1], rgb, width: note.size as f64 });
                            Made::Arrow
                        }
                        Shape::Text { at, words, extent } => {
                            // The words' box, and anything of theirs that reaches past it.
                            let mut reach = Drawing::new();
                            for corner in Area::new(at.0, at.1, at.0 + extent.0, at.1 + extent.1).corners() {
                                reach.reach.push(affine.apply(corner));
                            }
                            reach.reach.extend_from_slice(&drawing.reach);
                            let size = note.size;
                            let da = format!("/Helv {} Tf {} rg", n(size as f64), colour(rgb));
                            let ds = format!("font: Helvetica,sans-serif {}pt; color: #{:02X}{:02X}{:02X}", n(size as f64), rgb[0], rgb[1], rgb[2]);
                            unsafe {
                                make(b, page, FREETEXT, &reach.bounds(1.0), None, 0.0, &name, &when, &drawing.ops, |annot| {
                                    b.FPDFAnnot_SetStringValue_str(annot, "Contents", words);
                                    b.FPDFAnnot_SetStringValue_str(annot, "DA", &da);
                                    b.FPDFAnnot_SetStringValue_str(annot, "DS", &ds);
                                })
                            }?;
                            Made::FreeText
                        }
                    };
                    made.push((index, name, kind));
                }
                if let Some(crop) = marks.crop(index) {
                    let mut corners = Drawing::new();
                    corners.reach = crop.corners().iter().map(|c| affine.apply(*c)).collect();
                    let box_ = corners.bounds(0.0);
                    unsafe { b.FPDFPage_SetCropBox(page, box_.x0, box_.y0, box_.x1, box_.y1) };
                }
                Ok(())
            })();
            unsafe { b.FPDF_ClosePage(page) };
            done?;
        }
        let mut sink = Sink { raw: FPDF_FILEWRITE { version: 1, WriteBlock: Some(write_block) }, bytes: Vec::with_capacity(bytes.len() + (64 << 10)) };
        if !b.is_true(unsafe { b.FPDF_SaveAsCopy(doc, &mut sink.raw, INCREMENTAL) }) {
            return Err("PDFium would not write it".into());
        }
        let encrypted = unsafe { b.FPDF_GetSecurityHandlerRevision(doc) } != -1;
        Ok((sink.bytes, pages, encrypted))
    })();
    unsafe { b.FPDF_CloseDocument(doc) };
    drop(fonts);
    let (saved, pages, encrypted) = written?;
    if !arrows.is_empty() && !encrypted {
        match arrows_as_lines(&saved, bytes.len(), &arrows).and_then(|lined| found(pdfium, &lined, pages, &made, true, None).map(|_| lined)) {
            Ok(lined) => return Ok(lined),
            Err(e) => log::warn!("the arrows stay Ink annotations: {e}"),
        }
    }
    found(pdfium, &saved, pages, &made, false, password)?;
    Ok(saved)
}

/// Whether the PDF `bytes` opens -- with `password`, if it is locked -- with its `pages` and every
/// mark made in it, of the kind it was made: an arrow a Line annotation if `lines`, else Ink.
fn found(pdfium: &Pdfium, bytes: &[u8], pages: usize, made: &[(usize, String, Made)], lines: bool, password: Option<&str>) -> Result<(), String> {
    let document = pdfium.load_pdf_from_byte_slice(bytes, password).map_err(|e| format!("what was written does not open: {e}"))?;
    if document.pages().len() as usize != pages {
        return Err("what was written has lost pages".into());
    }
    for (index, name, kind) in made {
        let page = document.pages().get(*index as PdfPageIndex).map_err(|e| e.to_string())?;
        let annotation = page.annotations().iter().find(|a| a.name().as_deref() == Some(name.as_str())).ok_or_else(|| format!("{name} is not in what was written"))?;
        let want = match kind {
            Made::Square => PdfPageAnnotationType::Square,
            Made::Circle => PdfPageAnnotationType::Circle,
            Made::FreeText => PdfPageAnnotationType::FreeText,
            Made::Arrow if lines => PdfPageAnnotationType::Line,
            Made::Arrow => PdfPageAnnotationType::Ink,
        };
        if annotation.annotation_type() != want {
            return Err(format!("{name} was written as {:?}", annotation.annotation_type()));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::markup::{Ink, Weight};

    /// PDFium, where it is here (tools/fetch-pdfium.sh); without it, a test says so and passes.
    fn pdfium() -> Option<Pdfium> {
        crate::pdf::bind().map_err(|e| eprintln!("{e}: not tried")).ok()
    }

    /// The page drawn by PDFium as a reader sees it, annotations and all: a pixel's colour.
    fn drawn(pdfium: &Pdfium, bytes: &[u8], page: PdfPageIndex, width: i32) -> (u32, Vec<u8>) {
        let document = pdfium.load_pdf_from_byte_slice(bytes, None).unwrap();
        let page = document.pages().get(page).unwrap();
        let bitmap = page.render_with_config(&PdfRenderConfig::new().set_target_width(width).render_annotations(true)).unwrap();
        (bitmap.width() as u32, bitmap.as_rgba_bytes())
    }

    #[test]
    fn a_pdf_marked_up_is_written_with_real_annotations_and_opens_again() {
        let Some(pdfium) = pdfium() else { return };
        // A page as it is, and one turned a quarter (/Rotate 90): shown 800 across and 600 down.
        let path = crate::pdf::tests::made_turned("marked.pdf", &[(600.0, 800.0, "first", 0), (600.0, 800.0, "second", 90)]);
        let original = std::fs::read(&path).unwrap();
        let note = |page, shape, ink, size| Note { page, shape, ink, weight: Weight::Medium, size };
        let extent = ink::measure(&mut ink::fonts(), "Grüße 你好", 24.0);
        let marks = Marks {
            notes: vec![
                note(0, Shape::Box(Area::new(100.0, 100.0, 300.0, 200.0)), Ink::Red, 4.0),
                note(0, Shape::Oval(Area::new(100.0, 300.0, 300.0, 400.0)), Ink::Green, 4.0),
                note(0, Shape::Arrow { from: (100.0, 500.0), to: (400.0, 600.0) }, Ink::Blue, 4.0),
                note(0, Shape::Text { at: (100.0, 650.0), words: "Grüße 你好".into(), extent }, Ink::Black, 24.0),
                note(1, Shape::Box(Area::new(50.0, 50.0, 250.0, 150.0)), Ink::Red, 4.0),
            ],
            crops: vec![(1, Area::new(20.0, 20.0, 700.0, 500.0))],
            turns: 0,
        };
        let bytes = annotate(&pdfium, &original, &marks, None).unwrap();
        assert!(bytes.starts_with(&original), "an incremental update: the file as it was, then what changed");
        // Beside the PDF it was made from, for another reader to be shown it.
        std::fs::write(path.with_file_name("marked (edited).pdf"), &bytes).unwrap();
        let document = pdfium.load_pdf_from_byte_slice(&bytes, None).unwrap();
        let page = |i: PdfPageIndex| document.pages().get(i).unwrap();
        let kinds = |i: PdfPageIndex| page(i).annotations().iter().map(|a| a.annotation_type()).collect::<Vec<_>>();
        type Kind = PdfPageAnnotationType;
        assert_eq!(kinds(0), [Kind::Square, Kind::Circle, Kind::Line, Kind::FreeText]);
        assert_eq!(kinds(1), [Kind::Square]);
        let words = page(0).annotations().iter().find(|a| a.annotation_type() == Kind::FreeText).and_then(|a| a.contents());
        assert_eq!(words.as_deref(), Some("Grüße 你好"), "the words are the annotation's own, for a reader to change");
        // The turned page's crop box: what was shown from (20, 20) to (700, 500) is, in its own
        // space, x from 20 to 500 and y from 20 to 700.
        let crop = page(1).boundaries().crop().unwrap().bounds;
        assert_eq!((crop.left().value, crop.bottom().value, crop.right().value, crop.top().value), (20.0, 20.0, 500.0, 700.0));
        // The arrow's line, its ends in the page's space, up from its foot; and a Line's arrow.
        unsafe {
            let b = pdfium.bindings();
            let doc = b.FPDF_LoadMemDocument64(&bytes, None);
            let raw = b.FPDF_LoadPage(doc, 0);
            let annot = b.FPDFPage_GetAnnot(raw, 2);
            let (mut start, mut end) = (FS_POINTF { x: 0.0, y: 0.0 }, FS_POINTF { x: 0.0, y: 0.0 });
            assert!(b.is_true(b.FPDFAnnot_GetLine(annot, &mut start, &mut end)));
            assert_eq!((start.x, start.y, end.x, end.y), (100.0, 300.0, 400.0, 200.0));
            b.FPDFPage_CloseAnnot(annot);
            b.FPDF_ClosePage(raw);
            b.FPDF_CloseDocument(doc);
        }
        let tail = String::from_utf8_lossy(&bytes[original.len()..]).into_owned();
        assert!(tail.contains("/LE[/None/ClosedArrow]") && !tail.contains("/Subtype/Line/Type/Annot/InkList"));
        // As PDFium draws the pages: a point a pixel, the box's edge red where it was drawn; on the
        // turned page, cropped, where it was drawn less the crop's corner.
        let red = |(w, rgba): &(u32, Vec<u8>), x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            rgba[i] > 180 && rgba[i + 1] < 110 && rgba[i + 2] < 110
        };
        let first = drawn(&pdfium, &bytes, 0, 600);
        assert!(red(&first, 100, 150) && red(&first, 200, 100) && !red(&first, 200, 150), "the box's edges, and not its inside");
        let second = drawn(&pdfium, &bytes, 1, 680);
        assert!(red(&second, 30, 80) && red(&second, 130, 30) && !red(&second, 130, 80), "the turned page's box");
        // The words drawn: dark in their box.
        let (w, rgba) = &first;
        let dark = (650..680).flat_map(|y| (100..250).map(move |x| (x, y))).filter(|&(x, y)| rgba[((y * w + x) * 4) as usize] < 90).count();
        assert!(dark > 200, "{dark}");
        // And what was there before is there still.
        assert_eq!(document.pages().len(), 2);
    }

    /// The encrypted PDFs of testdata/encrypted (qpdf made them: make.sh there), each with the
    /// password that opens it (none: an owner's password only), its security handler's revision,
    /// and whether its owner allows notes and its pages changed.
    pub(crate) type Locked = (&'static str, &'static [u8], Option<&'static str>, i32, bool, bool);
    pub(crate) const LOCKED: [Locked; 6] = [
        ("rc4-128.pdf", include_bytes!("../testdata/encrypted/rc4-128.pdf"), None, 3, true, true),
        ("rc4-128-user.pdf", include_bytes!("../testdata/encrypted/rc4-128-user.pdf"), Some("user"), 3, true, true),
        ("aes-128-noannot.pdf", include_bytes!("../testdata/encrypted/aes-128-noannot.pdf"), None, 4, false, true),
        ("aes-128-user.pdf", include_bytes!("../testdata/encrypted/aes-128-user.pdf"), Some("user"), 4, true, true),
        ("aes-256-nomodify.pdf", include_bytes!("../testdata/encrypted/aes-256-nomodify.pdf"), None, 6, true, false),
        ("aes-256-user.pdf", include_bytes!("../testdata/encrypted/aes-256-user.pdf"), Some("p\u{e4}ssw\u{f6}rd"), 6, true, true),
    ];

    /// A page drawn as a reader sees it, opened with `password`.
    fn drawn_locked(pdfium: &Pdfium, bytes: &[u8], password: Option<&str>, width: i32) -> (u32, Vec<u8>) {
        let document = pdfium.load_pdf_from_byte_slice(bytes, password).unwrap();
        let page = document.pages().get(0).unwrap();
        let bitmap = page.render_with_config(&PdfRenderConfig::new().set_target_width(width).render_annotations(true)).unwrap();
        (bitmap.width() as u32, bitmap.as_rgba_bytes())
    }

    /// What PDFium says of a document's security, opened with `password`: its revision, and the
    /// permissions it gives (all, opened with the owner's password).
    fn security(pdfium: &Pdfium, bytes: &[u8], password: Option<&str>) -> Option<(i32, u32)> {
        let b = pdfium.bindings();
        unsafe {
            let doc = b.FPDF_LoadMemDocument64(bytes, password);
            if doc.is_null() {
                return None;
            }
            let said = (b.FPDF_GetSecurityHandlerRevision(doc), b.FPDF_GetDocPermissions(doc) as u32);
            b.FPDF_CloseDocument(doc);
            Some(said)
        }
    }

    #[test]
    fn an_encrypted_pdf_marked_up_stays_encrypted_as_it_was() {
        let Some(pdfium) = pdfium() else { return };
        let note = |shape, ink, size| Note { page: 0, shape, ink, weight: Weight::Medium, size };
        let extent = ink::measure(&mut ink::fonts(), "Grüße 你好", 24.0);
        for (name, original, password, revision, annotate_ok, modify_ok) in LOCKED {
            // What its owner allows, as SpatiLoupe reads it.
            let rights = crate::pdf::rights(&pdfium, std::path::Path::new(name), Some(original), password);
            assert_eq!(rights, crate::pdf::Rights { encrypted: true, annotate: annotate_ok, modify: modify_ok }, "{name}");
            if password.is_some() {
                assert!(pdfium.load_pdf_from_byte_slice(original, None).is_err(), "{name} opened without its password");
            }
            // Not marked up: markup is not offered for it (the view's test).
            if !annotate_ok {
                continue;
            }
            let marks = Marks {
                notes: vec![
                    note(Shape::Box(Area::new(100.0, 100.0, 300.0, 200.0)), Ink::Red, 4.0),
                    note(Shape::Oval(Area::new(100.0, 300.0, 300.0, 400.0)), Ink::Green, 4.0),
                    note(Shape::Arrow { from: (100.0, 500.0), to: (400.0, 600.0) }, Ink::Blue, 4.0),
                    note(Shape::Text { at: (100.0, 650.0), words: "Grüße 你好".into(), extent }, Ink::Black, 24.0),
                ],
                crops: if modify_ok { vec![(0, Area::new(20.0, 20.0, 580.0, 780.0))] } else { Vec::new() },
                turns: 0,
            };
            let bytes = annotate(&pdfium, original, &marks, password).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(bytes.starts_with(original), "{name}: an incremental update");
            // The same security dictionary, named again by the update's trailer, and the notes'
            // strings encrypted with the document's key: no name of one is in the clear.
            let tail = String::from_utf8_lossy(&bytes[original.len()..]).into_owned();
            let encrypt = |text: &str| text.rfind("/Encrypt").map(|at| text[at + 8..].split_whitespace().next().unwrap_or_default().to_string());
            assert_eq!(encrypt(&tail), encrypt(&String::from_utf8_lossy(original)), "{name}: {tail}");
            assert!(!tail.contains("spatiloupe-"), "{name}: a note's name (a string) in the clear");
            // Locked as it was: not opened without its password; opened with it, the same
            // revision and the same permissions.
            if password.is_some() {
                assert!(pdfium.load_pdf_from_byte_slice(&bytes, None).is_err(), "{name}: the edited file opened without its password");
            }
            let before = security(&pdfium, original, password).unwrap();
            assert_eq!(before.0, revision, "{name}");
            assert_eq!(security(&pdfium, &bytes, password), Some(before), "{name}: its security changed");
            // Its notes as they were made, the words read back through the encryption; an arrow an
            // Ink annotation (SpatiLoupe's own update does not encrypt, so it is not made a Line).
            let document = pdfium.load_pdf_from_byte_slice(&bytes, password).unwrap();
            let page = document.pages().get(0).unwrap();
            type Kind = PdfPageAnnotationType;
            let kinds: Vec<Kind> = page.annotations().iter().map(|a| a.annotation_type()).collect();
            assert_eq!(kinds, [Kind::Square, Kind::Circle, Kind::Ink, Kind::FreeText], "{name}");
            let words = page.annotations().iter().find(|a| a.annotation_type() == Kind::FreeText).and_then(|a| a.contents());
            assert_eq!(words.as_deref(), Some("Grüße 你好"), "{name}");
            if modify_ok {
                let crop = page.boundaries().crop().unwrap().bounds;
                assert_eq!((crop.left().value, crop.bottom().value, crop.right().value, crop.top().value), (20.0, 20.0, 580.0, 780.0), "{name}");
            }
            drop(page);
            drop(document);
            // Drawn: the box's edge red where it was put, its inside not (cropped 20 in, or not).
            let (w, rgba) = drawn_locked(&pdfium, &bytes, password, if modify_ok { 560 } else { 600 });
            let shift = if modify_ok { 20 } else { 0 };
            let red = |x: u32, y: u32| {
                let i = (((y - shift) * w + x - shift) * 4) as usize;
                rgba[i] > 180 && rgba[i + 1] < 110 && rgba[i + 2] < 110
            };
            assert!(red(100, 150) && red(200, 100) && !red(200, 150), "{name}: the box drawn");
            // Beside the test's other PDFs, for poppler or another reader to be shown.
            let dir = std::env::temp_dir().join(format!("spatiloupe-pdf-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(name.replace(".pdf", " (edited).pdf")), &bytes).unwrap();
        }
    }

    #[test]
    fn a_moment_is_written_as_a_pdf_writes_it() {
        assert_eq!(pdf_date(0), "D:19700101000000Z");
        assert_eq!(pdf_date(1_795_100_400), "D:20261119150000Z");
        assert_eq!(civil(-1), (1969, 12, 31));
    }

    #[test]
    fn numbers_are_written_short() {
        assert_eq!((n(1.0), n(0.5), n(-0.0001), n(12.3456), n(-3.1)), ("1".into(), "0.5".into(), "0".into(), "12.346".into(), "-3.1".into()));
    }

    #[test]
    fn a_page_shown_turned_maps_into_its_own_space() {
        // A page 600 wide and 800 tall, unturned: the top left is (0, 800), down is down.
        let plain = Affine::from_corners((0.0, 800.0), (600.0, 800.0), (0.0, 0.0), (600.0, 800.0));
        assert_eq!(plain.apply((10.0, 20.0)), (10.0, 780.0));
        // Turned a quarter clockwise as shown (/Rotate 90): shown 800 across and 600 down, its
        // top left the page's (0, 0), across the page's up, down the page's right.
        let turned = Affine::from_corners((0.0, 0.0), (0.0, 800.0), (600.0, 0.0), (800.0, 600.0));
        assert_eq!(turned.apply((10.0, 20.0)), (20.0, 10.0));
    }

    #[test]
    fn appearances_are_drawn_in_the_page_s_space() {
        let plain = Affine::from_corners((0.0, 800.0), (600.0, 800.0), (0.0, 0.0), (600.0, 800.0));
        let note = |shape| Note { page: 0, shape, ink: Ink::Red, weight: Weight::Medium, size: 3.0 };
        let mut fonts = ink::fonts();
        let boxed = appearance(&note(Shape::Box(Area::new(10.0, 20.0, 110.0, 70.0))), &plain, &mut fonts);
        assert_eq!(boxed.ops, "3 w 0 J 0 j [] 0 d 0.922 0.2 0.141 RG\n10 780 m\n110 780 l\n110 730 l\n10 730 l\nh S\n");
        assert_eq!(boxed.bounds(1.5), Area::new(8.5, 728.5, 111.5, 781.5));
        let oval = appearance(&note(Shape::Oval(Area::new(0.0, 0.0, 100.0, 50.0))), &plain, &mut fonts);
        assert_eq!(oval.ops.matches(" c\n").count(), 4);
        assert!(oval.ops.starts_with("3 w 1 J 1 j [] 0 d 0.922 0.2 0.141 RG\n100 775 m\n"), "{}", oval.ops);
        let arrow = appearance(&note(Shape::Arrow { from: (0.0, 0.0), to: (100.0, 0.0) }), &plain, &mut fonts);
        assert!(arrow.ops.contains("0 800 m\n87.6 800 l\nS\n100 800 m\n"), "{}", arrow.ops);
        assert!(arrow.ops.ends_with("h f\n"));
        let words = appearance(&note(Shape::Text { at: (50.0, 100.0), words: "Hi 你好".into(), extent: (0.0, 0.0) }), &plain, &mut fonts);
        assert!(words.ops.starts_with("0.922 0.2 0.141 rg\n") && words.ops.ends_with("f\n"));
        assert!(words.ops.matches(" c\n").count() > 10, "the glyphs' curves");
        // Below the words' top, which is 700 up the page; above their foot.
        let b = words.bounds(0.0);
        assert!(b.y1 <= 700.0 && b.y0 > 650.0 && b.x0 >= 50.0, "{b:?}");
    }

    #[test]
    fn a_dictionary_s_values_are_read_as_written() {
        let text = b"<</Size 25/Root 1 0 R /Info 2 0 R/ID[<3A78><045C>]/W[0 4 1]/Name(a (nested) \\) one)/Next<</X 1>>/Length 55>>stream";
        let read: Vec<(String, String)> = entries(text, 0).into_iter().map(|(k, r)| (String::from_utf8(k).unwrap(), String::from_utf8_lossy(&text[r]).into_owned())).collect();
        assert_eq!(
            read,
            [("Size", "25"), ("Root", "1 0 R"), ("Info", "2 0 R"), ("ID", "[<3A78><045C>]"), ("W", "[0 4 1]"), ("Name", "(a (nested) \\) one)"), ("Next", "<</X 1>>"), ("Length", "55")]
                .map(|(a, b)| (a.to_string(), b.to_string()))
        );
        let spans = dictionaries(b"<</A<</B(>>)>>/C<</D 1>>>>");
        assert_eq!(spans, [4..14, 16..24, 0..26]);
    }

    /// A file as PDFium writes one incrementally: the original, then the page again holding the
    /// annotations, the cross-reference -- a table, or a stream -- and the trailer.
    fn pdfium_like(stream: bool) -> (Vec<u8>, usize) {
        let original = b"%PDF-1.4\n1 0 obj\n<</Type/Catalog/Pages 2 0 R>>\nendobj\nxref\n0 2\ntrailer\n<</Size 5/Root 1 0 R>>\nstartxref\n50\n%%EOF\n".to_vec();
        let mut out = original.clone();
        let page_at = out.len();
        out.extend_from_slice(b"4 0 obj\r\n<</Annots[<</AP<</N 6 0 R >>/Contents(5 0 obj \\(sic\\))/NM(spatiloupe-1)/Subtype/FreeText>><</AP<</N 7 0 R >>/Border[ 0 0 3]/C[ 1 0 0]/InkList[[ 1 2 3 4][ 5 6 7 8]]/NM(spatiloupe-2)/Rect[ 0 0 10 10]/Subtype/Ink/Type/Annot>>]/Type/Page>>\r\nendobj\r\n");
        let xref = out.len();
        if stream {
            out.extend_from_slice(format!("8 0 obj <</Info 2 0 R /Root 1 0 R /Size 9/Prev 50/ID[<AA><BB>]/W[0 4 1]/Index[4 1 ]/Length 5>>stream\r\n\0\0\0\0\0\r\nendstream\r\nendobj\r\nstartxref\r\n{xref}\r\n%%EOF\r\n").as_bytes());
        } else {
            out.extend_from_slice(format!("xref\r\n4 1\r\n{page_at:010} 00000 n\r\ntrailer\r\n<</Root 1 0 R /Size 8/Prev 50/ID[<AA><BB>]>>\r\nstartxref\r\n{xref}\r\n%%EOF\r\n").as_bytes());
        }
        (out, original.len())
    }

    #[test]
    fn an_arrow_is_given_again_as_a_line_after_pdfium_s_update() {
        let arrow = ArrowLine { name: "spatiloupe-2".into(), line: [1.0, 2.0, 7.5, 8.0], rgb: [255, 0, 0], width: 3.0 };
        for stream in [false, true] {
            let (saved, original) = pdfium_like(stream);
            let lined = arrows_as_lines(&saved, original, std::slice::from_ref(&arrow)).unwrap();
            assert!(lined.starts_with(&saved), "PDFium's update kept as it was");
            let added = &lined[saved.len()..];
            let text = String::from_utf8_lossy(added);
            // The page again, under its own number; the words' dictionary as it was, the arrow's
            // a Line's, its ink list gone.
            assert!(text.starts_with("4 0 obj\r\n<</Annots[<</AP<</N 6 0 R >>/Contents(5 0 obj \\(sic\\))/NM(spatiloupe-1)/Subtype/FreeText>>"), "{text}");
            assert!(text.contains("<</AP<</N 7 0 R >>/Border[ 0 0 3]/C[ 1 0 0]/NM(spatiloupe-2)/Rect[ 0 0 10 10]/Subtype/Line/Type/Annot/L[1 2 7.5 8]/LE[/None/ClosedArrow]/IC[1 0 0]/BS<</W 3/S/S>>>>]/Type/Page>>\r\nendobj"), "{text}");
            assert!(!text.contains("InkList"));
            // Found where the new cross-reference says, which points back at PDFium's.
            let start = rfind(&lined, b"startxref", lined.len()).unwrap();
            let at: usize = String::from_utf8_lossy(&lined[start + 9..]).split_whitespace().next().unwrap().parse().unwrap();
            let page_at = saved.len();
            if stream {
                let own = &lined[at..];
                assert!(own.starts_with(b"9 0 obj\r\n<</Type/XRef/Size 10/Root 1 0 R/Info 2 0 R/ID [<AA><BB>]/Prev "), "{}", String::from_utf8_lossy(own));
                let rows_at = find(own, b"stream\r\n", 0).unwrap() + 8;
                let row = &own[rows_at..rows_at + 6];
                assert_eq!(row[0], 1);
                assert_eq!(u32::from_be_bytes(row[1..5].try_into().unwrap()) as usize, page_at);
                assert!(String::from_utf8_lossy(own).contains("/W[1 4 1]/Index[4 1 9 1]/Length 12>>"));
            } else {
                let own = String::from_utf8_lossy(&lined[at..]).into_owned();
                assert!(own.starts_with(&format!("xref\r\n4 1\r\n{page_at:010} 00000 n\r\ntrailer\r\n<</Size 8/Root 1 0 R/ID [<AA><BB>]/Prev ")), "{own}");
            }
        }
        // An arrow PDFium did not write is said, not guessed at.
        let (saved, original) = pdfium_like(false);
        let missing = ArrowLine { name: "spatiloupe-9".into(), ..arrow };
        assert!(arrows_as_lines(&saved, original, &[missing]).is_err());
    }
}

#[cfg(test)]
mod a_pdf_here {
    use super::*;
    use crate::markup::{Ink, Weight};

    /// A PDF on this computer marked up as SpatiLoupe marks one -- a box, an oval, an arrow and
    /// words on its first page, and that page cropped -- and written beside it as
    /// "<name> (edited).pdf", for other readers to open:
    /// `SPATILOUPE_PDF=/path/to/a.pdf cargo test -p spatiloupe a_pdf_here -- --ignored`.
    #[test]
    #[ignore]
    fn mark_a_pdf() {
        let path = std::path::PathBuf::from(std::env::var("SPATILOUPE_PDF").expect("SPATILOUPE_PDF"));
        let pdfium = crate::pdf::bind().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let (w, h) = {
            let document = pdfium.load_pdf_from_byte_slice(&bytes, None).unwrap();
            let page = document.pages().get(0).unwrap();
            (page.width().value, page.height().value)
        };
        let note = |shape, ink, size| Note { page: 0, shape, ink, weight: Weight::Medium, size };
        let words = "Markup: Grüße, Привет, 你好";
        let extent = ink::measure(&mut ink::fonts(), words, 18.0);
        let marks = Marks {
            notes: vec![
                note(Shape::Box(Area::new(w * 0.1, h * 0.1, w * 0.5, h * 0.25)), Ink::Red, 3.0),
                note(Shape::Oval(Area::new(w * 0.55, h * 0.1, w * 0.9, h * 0.25)), Ink::Green, 3.0),
                note(Shape::Arrow { from: (w * 0.2, h * 0.5), to: (w * 0.7, h * 0.35) }, Ink::Blue, 3.0),
                note(Shape::Text { at: (w * 0.1, h * 0.6), words: words.into(), extent }, Ink::Black, 18.0),
            ],
            crops: vec![(0, Area::new(w * 0.05, h * 0.05, w * 0.95, h * 0.8))],
            turns: 0,
        };
        let out = annotate(&pdfium, &bytes, &marks, None).unwrap();
        let to = path.with_file_name(format!("{} (edited).pdf", path.file_stem().unwrap().to_string_lossy()));
        std::fs::write(&to, out).unwrap();
        println!("{}", to.display());
    }
}
