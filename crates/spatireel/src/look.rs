//! How SpatiReel looks: the room's own ground and type, with the three colours of the
//! Spatiand mark -- teal, blue, violet -- as light behind it rather than as paint on it.

use std::time::Instant;

use spatiand_app::ui::{theme, Color, Painter, Rect};
use spatiand_app::Gl;

pub const TEAL: Color = [0.086, 0.902, 0.753, 1.0];
pub const BLUE: Color = [0.216, 0.651, 0.969, 1.0];
pub const VIOLET: Color = [0.561, 0.420, 0.949, 1.0];
pub const TROUBLE: Color = [1.0, 0.62, 0.55, 1.0];

fn faint(color: Color, alpha: f32) -> Color {
    [color[0], color[1], color[2], alpha]
}

/// What is behind everything: the ground, deepening downwards, with a glow of teal high on
/// the left and of violet low on the right, as though lit from outside the window.
pub fn backdrop(p: &mut Painter, gl: &Gl) {
    let all = p.bounds();
    p.gradient(gl, all, 0.0, [0.070, 0.082, 0.130, 1.0], [0.024, 0.027, 0.043, 1.0]);
    let reach = all.w * 0.42;
    p.glow(gl, Rect::new(all.x - reach * 0.55, all.y - reach * 0.75, reach, reach), reach * 0.5, faint(TEAL, 0.10), reach * 0.9);
    p.glow(gl, Rect::new(all.right() - reach * 0.5, all.bottom() - reach * 0.4, reach, reach), reach * 0.5, faint(VIOLET, 0.12), reach * 0.9);
}

/// A screen's head: its name, a quieter line beside it, and a hairline in the mark's
/// colours under both. Returns what is left of `all` below it.
pub fn header(p: &mut Painter, gl: &Gl, all: &mut Rect, title: &str, beside: &str) {
    let line = all.take_top(theme::TITLE_EM * 1.5);
    let taken = p.label(gl, title, line, theme::TITLE_EM, theme::INK, 0.0);
    if !beside.is_empty() {
        let rest = Rect { x: line.x + taken + 22.0, w: (line.w - taken - 22.0).max(0.0), y: line.y + 6.0, ..line };
        p.label(gl, beside, rest, theme::DETAIL_EM, theme::INK_HINT, 0.0);
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
    p.fill(gl, mark, 4.0, if lit { TEAL } else { theme::INK_HINT });
    let text = Rect { x: line.x + 34.0, w: line.w - 34.0, ..line };
    p.label(gl, &name.to_uppercase(), text, theme::FOOTER_EM * 0.92, if lit { theme::INK_DETAIL } else { theme::INK_HINT }, 0.0);
}

/// Something said in the middle of an empty place.
pub fn message(p: &mut Painter, gl: &Gl, rect: Rect, text: &str, color: Color) {
    p.label(gl, text, rect, theme::ROW_EM * 0.9, color, 0.5);
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

/// A line at the foot of a screen: which keys do what, or what went wrong.
pub fn foot(p: &mut Painter, gl: &Gl, all: &mut Rect, hint: &str, trouble: Option<&str>) {
    let line = all.take_bottom(theme::FOOTER_EM * 1.7);
    match trouble {
        Some(trouble) => p.label(gl, trouble, line, theme::FOOTER_EM, TROUBLE, 0.0),
        None => p.label(gl, hint, line, theme::FOOTER_EM, theme::INK_HINT, 0.0),
    };
}
