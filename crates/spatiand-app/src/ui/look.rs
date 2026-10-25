//! How Spatiand's own applications look: the room's own ground and type, with the three colours of the
//! Spatiand mark -- teal, blue, violet -- as light behind it rather than as paint on it.

use std::time::Instant;

use super::{theme, Color, Painter, Rect};
use crate::Gl;

pub const TEAL: Color = [0.086, 0.902, 0.753, 1.0];
pub const BLUE: Color = [0.216, 0.651, 0.969, 1.0];
pub const VIOLET: Color = [0.561, 0.420, 0.949, 1.0];
/// What went wrong is said in.
pub fn trouble() -> Color {
    if theme::dark() { [1.0, 0.62, 0.55, 1.0] } else { [0.78, 0.18, 0.12, 1.0] }
}

fn faint(color: Color, alpha: f32) -> Color {
    [color[0], color[1], color[2], alpha]
}

/// What is behind everything: the ground, deepening downwards, with a glow of teal high on
/// the left and of violet low on the right, as though lit from outside the window.
pub fn backdrop(p: &mut Painter, gl: &Gl) {
    let all = p.bounds();
    let dark = theme::dark();
    if dark {
        p.gradient(gl, all, 0.0, [0.070, 0.082, 0.130, 1.0], [0.024, 0.027, 0.043, 1.0]);
    } else {
        p.gradient(gl, all, 0.0, [0.975, 0.980, 0.990, 1.0], [0.905, 0.915, 0.940, 1.0]);
    }
    let reach = all.w * 0.42;
    let glow = if dark { 1.0 } else { 0.6 };
    p.glow(gl, Rect::new(all.x - reach * 0.55, all.y - reach * 0.75, reach, reach), reach * 0.5, faint(TEAL, 0.10 * glow), reach * 0.9);
    p.glow(gl, Rect::new(all.right() - reach * 0.5, all.bottom() - reach * 0.4, reach, reach), reach * 0.5, faint(VIOLET, 0.12 * glow), reach * 0.9);
}

/// A screen's head: its name, a quieter line beside it, and a hairline in the mark's
/// colours under both. Returns what is left of `all` below it.
pub fn header(p: &mut Painter, gl: &Gl, all: &mut Rect, title: &str, beside: &str) {
    let line = all.take_top(theme::TITLE_EM * 1.5);
    let taken = p.label(gl, title, line, theme::TITLE_EM, theme::ink(), 0.0);
    if !beside.is_empty() {
        let rest = Rect { x: line.x + taken + 22.0, w: (line.w - taken - 22.0).max(0.0), y: line.y + 6.0, ..line };
        p.label(gl, beside, rest, theme::DETAIL_EM, theme::ink_hint(), 0.0);
    }
    all.take_top(10.0);
    rule(p, gl, all.take_top(2.0));
    all.take_top(18.0);
}

/// A hairline that is teal, then blue, then violet, and fades out to the right.
pub fn rule(p: &mut Painter, gl: &Gl, line: Rect) {
    let third = line.w / 3.0;
    for (index, color) in [TEAL, BLUE, VIOLET].into_iter().enumerate() {
        let part = Rect { x: line.x + third * index as f32, w: third + 1.0, ..line };
        p.fill(gl, part, 0.0, faint(color, 0.75 - index as f32 * 0.22));
    }
}

/// The name of a shelf.
pub fn shelf(p: &mut Painter, gl: &Gl, all: &mut Rect, name: &str, lit: bool) {
    let line = all.take_top(theme::DETAIL_EM * 1.7);
    let mark = Rect::new(line.x + 14.0, line.y + line.h * 0.5 - 4.0, 8.0, 8.0);
    p.fill(gl, mark, 4.0, if lit { TEAL } else { theme::ink_hint() });
    let text = Rect { x: line.x + 34.0, w: line.w - 34.0, ..line };
    p.label(gl, &name.to_uppercase(), text, theme::FOOTER_EM * 0.92, if lit { theme::ink_detail() } else { theme::ink_hint() }, 0.0);
}

/// Something said in the middle of an empty place.
/// Wrapped to a column, so that all of it shows however long it is.
pub fn message(p: &mut Painter, gl: &Gl, rect: Rect, text: &str, color: Color) {
    let em = theme::ROW_EM * 0.9;
    let column = rect.w.min(1100.0);
    let (w, h) = p.measure(gl, text, em, column);
    p.text(gl, text, rect.x + (rect.w - w) * 0.5, rect.y + (rect.h - h).max(0.0) * 0.5, em, column, color);
}

/// Three dots taking turns, for something being waited on.
pub fn waiting(p: &mut Painter, gl: &Gl, rect: Rect, since: Instant) {
    let beat = since.elapsed().as_secs_f32() * 2.4;
    let (cx, cy) = (rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);
    for (index, color) in [TEAL, BLUE, VIOLET].into_iter().enumerate() {
        let lift = ((beat - index as f32 * 0.55).sin() * 0.5 + 0.5).powi(2);
        let size = 12.0 + lift * 8.0;
        let dot = Rect::new(cx + (index as f32 - 1.0) * 34.0 - size * 0.5, cy - size * 0.5, size, size);
        p.fill(gl, dot, size * 0.5, faint(color, 0.35 + lift * 0.65));
    }
}

/// A line at the foot of a screen: which keys do what, or what went wrong. A hint written
/// with keys in brackets is drawn as [`keys`] draws it.
pub fn foot(p: &mut Painter, gl: &Gl, all: &mut Rect, hint: &str, trouble: Option<&str>) {
    // What went wrong is wrapped, and takes as many lines as it needs.
    let wrapped = trouble.map_or(0.0, |trouble| p.measure(gl, trouble, theme::FOOTER_EM, all.w).1);
    let line = all.take_bottom((theme::FOOTER_EM * 1.9).max(wrapped + theme::FOOTER_EM * 0.5));
    match trouble {
        Some(trouble) => {
            p.text(gl, trouble, line.x, line.y + (line.h - wrapped) * 0.5, theme::FOOTER_EM, line.w, self::trouble());
        }
        // Keys only where the hint is written as keys: a path may have brackets in it.
        None if hint.starts_with('[') => {
            keys(p, gl, line, hint, 0.0, 1.0);
        }
        None => {
            p.label(gl, hint, line, theme::FOOTER_EM, theme::ink_hint(), 0.0);
        }
    }
}

/// One part of a hint: a key, or the words saying what the keys before them do.
enum Said<'a> {
    Key(&'a str),
    Words(&'a str),
}

/// A hint split into its groups -- two spaces or more apart -- and each group into its keys
/// (in brackets) and its words.
fn said(hint: &str) -> Vec<Vec<Said<'_>>> {
    let mut groups = Vec::new();
    for group in hint.split("  ").map(str::trim).filter(|g| !g.is_empty()) {
        let mut parts = Vec::new();
        let mut rest = group;
        while !rest.is_empty() {
            match (rest.find('['), rest.find(']')) {
                (Some(0), Some(end)) => {
                    parts.push(Said::Key(&rest[1..end]));
                    rest = rest[end + 1..].trim_start();
                }
                (Some(at), _) => {
                    parts.push(Said::Words(rest[..at].trim()));
                    rest = &rest[at..];
                }
                _ => {
                    parts.push(Said::Words(rest.trim()));
                    rest = "";
                }
            }
        }
        groups.push(parts);
    }
    groups
}

/// Which keys do what, as keycaps and the words for them: `"[O] Contents  [PgUp] [PgDn] Turn"`.
/// Groups are two spaces apart in the text and well apart on the screen, so each key is read
/// with its own words; a hint with no brackets is words only. Placed in `line` by `align`
/// (0 left, 0.5 centred, 1 right), at `alpha`; returns how wide it was.
pub fn keys(p: &mut Painter, gl: &Gl, line: Rect, hint: &str, align: f32, alpha: f32) -> f32 {
    let em = theme::FOOTER_EM * 0.92;
    let cap_em = em * 0.86;
    let cap_h = (em * 1.55).min(line.h);
    let (key_gap, word_gap, group_gap) = (5.0, 9.0, 30.0);
    let groups = said(hint);
    let max = 8192.0 / p.scale();
    let cap_w = |p: &mut Painter, gl: &Gl, key: &str| (p.measure(gl, key, cap_em, max).0 + 16.0).max(cap_h);
    // Measured first, to be placed.
    let mut total = 0.0;
    for (g, group) in groups.iter().enumerate() {
        if g > 0 {
            total += group_gap;
        }
        for (i, part) in group.iter().enumerate() {
            total += match part {
                Said::Key(key) => cap_w(p, gl, key) + if i > 0 { key_gap } else { 0.0 },
                Said::Words(words) => p.measure(gl, words, em, max).0 + if i > 0 { word_gap } else { 0.0 },
            };
        }
    }
    let faded = |c: Color| [c[0], c[1], c[2], c[3] * alpha];
    let mut x = line.x + (line.w - total).max(0.0) * align;
    let cap_y = line.y + (line.h - cap_h) * 0.5;
    for (g, group) in groups.iter().enumerate() {
        if g > 0 {
            x += group_gap;
        }
        for (i, part) in group.iter().enumerate() {
            match part {
                Said::Key(key) => {
                    if i > 0 {
                        x += key_gap;
                    }
                    let w = cap_w(p, gl, key);
                    let cap = Rect::new(x, cap_y, w, cap_h);
                    // A key: a lighter rim, a face, and a shade along its foot for depth.
                    p.fill(gl, cap, 7.0, faded([1.0, 1.0, 1.0, 0.16]));
                    p.fill(gl, cap.inset(1.0), 6.0, faded([0.10, 0.115, 0.16, 0.95]));
                    p.fill(gl, Rect::new(cap.x + 3.0, cap.bottom() - 3.0, cap.w - 6.0, 2.0), 1.0, faded([0.0, 0.0, 0.0, 0.35]));
                    p.label(gl, key, Rect { y: cap.y - 1.0, ..cap }, cap_em, faded(theme::ink_detail()), 0.5);
                    x += w;
                }
                Said::Words(words) => {
                    if i > 0 {
                        x += word_gap;
                    }
                    let w = p.label(gl, words, Rect::new(x, line.y, (line.right() - x).max(0.0), line.h), em, faded(theme::ink_hint()), 0.0);
                    x += w;
                }
            }
        }
    }
    total
}

/// A pane floating over what is behind it -- a picture, a page -- dark enough to read on
/// whatever that is, with the room's faint rim.
pub fn glass(p: &mut Painter, gl: &Gl, rect: Rect, radius: f32, strength: f32) {
    if theme::dark() {
        p.glow(gl, rect, radius, [0.0, 0.0, 0.0, 0.30 * strength], 26.0);
        p.gradient(gl, rect, radius, [0.075, 0.088, 0.135, 0.86 * strength], [0.040, 0.046, 0.070, 0.90 * strength]);
        p.fill(gl, Rect { h: 1.5, ..rect.inset(radius * 0.6) }, 0.75, [1.0, 1.0, 1.0, 0.10 * strength]);
    } else {
        p.glow(gl, rect, radius, [0.10, 0.14, 0.25, 0.12 * strength], 26.0);
        p.gradient(gl, rect, radius, [1.0, 1.0, 1.0, 0.88 * strength], [0.955, 0.962, 0.978, 0.92 * strength]);
        p.fill(gl, Rect { h: 1.5, ..rect.inset(radius * 0.6) }, 0.75, [1.0, 1.0, 1.0, 0.6 * strength]);
    }
}

/// A small rounded word on a wash of colour: what a thing is, at a glance ("3D", "PDF").
pub fn chip(p: &mut Painter, gl: &Gl, x: f32, y: f32, text: &str, color: Color) -> f32 {
    let em = theme::FOOTER_EM * 0.85;
    let (w, _) = p.measure(gl, text, em, 8192.0 / p.scale());
    let rect = Rect::new(x, y, w + 24.0, em * 1.9);
    p.fill(gl, rect, rect.h * 0.5, faint(color, 0.22));
    p.label(gl, text, rect, em, if theme::dark() { [color[0] * 0.5 + 0.5, color[1] * 0.5 + 0.5, color[2] * 0.5 + 0.5, 1.0] } else { [color[0] * 0.55, color[1] * 0.55, color[2] * 0.55, 1.0] }, 0.5);
    rect.w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(hint: &str) -> Vec<Vec<String>> {
        said(hint).into_iter().map(|g| g.into_iter().map(|s| match s { Said::Key(k) => format!("[{k}]"), Said::Words(w) => w.to_string() }).collect()).collect()
    }

    #[test]
    fn a_hint_is_keys_and_words_in_groups() {
        assert_eq!(shape("[O] Contents  [PgUp] [PgDn] Turn pages"), vec![vec!["[O]", "Contents"], vec!["[PgUp]", "[PgDn]", "Turn pages"]]);
        assert_eq!(shape("Drag to turn it"), vec![vec!["Drag to turn it"]]);
        assert_eq!(shape("Drag  Turn  [Space] Stop"), vec![vec!["Drag"], vec!["Turn"], vec!["[Space]", "Stop"]]);
    }
}
