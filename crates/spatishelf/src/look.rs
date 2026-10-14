//! How SpatiShelf draws a file: a picture of it if there is one, and if not a mark for what
//! kind of thing it is on a wash of that kind's colour -- the Spatiand mark's colours and their
//! neighbours, so that a folder of mixed things reads at a glance from across a room.

use spatiand_app::ui::{look, theme, Color, Painter, Rect, Texture};
use spatiand_app::Gl;

use crate::proto::Kind;

pub fn tint(kind: Kind) -> (Color, Color) {
    let (r, g, b) = match kind {
        Kind::Folder => (0.086, 0.902, 0.753),
        Kind::Picture => (0.216, 0.651, 0.969),
        Kind::Film => (0.561, 0.420, 0.949),
        Kind::Music => (0.95, 0.45, 0.72),
        Kind::Pdf => (1.0, 0.42, 0.48),
        Kind::Model => (0.65, 0.50, 1.0),
        Kind::Document => (0.55, 0.65, 0.85),
        Kind::Archive => (0.95, 0.75, 0.35),
        Kind::Code => (0.30, 0.88, 0.60),
        Kind::Other => (0.55, 0.60, 0.70),
    };
    ([r, g, b, 0.34], [r * 0.4, g * 0.4, b * 0.4, 0.20])
}

pub fn glyph(kind: Kind) -> &'static str {
    match kind {
        Kind::Folder => "\u{25a4}",
        Kind::Picture => "\u{25e9}",
        Kind::Film => "\u{25b6}",
        Kind::Music => "\u{266a}",
        Kind::Pdf => "PDF",
        Kind::Model => "\u{25c6}",
        Kind::Document => "\u{2261}",
        Kind::Archive => "\u{25a3}",
        Kind::Code => "</>",
        Kind::Other => "\u{2022}",
    }
}

/// A file's face, in `rect`: its picture, or its kind's mark.
pub fn face(p: &mut Painter, gl: &Gl, rect: Rect, kind: Kind, picture: Option<Texture>, radius: f32) {
    match picture {
        Some(texture) => {
            p.fill(gl, rect, radius, [0.0, 0.0, 0.0, 0.35]);
            // Whole, not cropped: a file's picture is the file, and its edges are part of it.
            let (tw, th) = (texture.width as f32, texture.height.max(1) as f32);
            let s = (rect.w / tw).min(rect.h / th);
            let (w, h) = (tw * s, th * s);
            let at = Rect::new(rect.x + (rect.w - w) * 0.5, rect.y + (rect.h - h) * 0.5, w, h);
            p.image(gl, texture, at, [0.0, 0.0, 1.0, 1.0], radius * 0.6, theme::INK, true);
        }
        None => {
            let (top, foot) = tint(kind);
            p.gradient(gl, rect, radius, top, foot);
            let em = (rect.h * 0.34).min(rect.w * 0.30) * if glyph(kind).len() > 2 { 0.7 } else { 1.0 };
            p.label(gl, glyph(kind), rect, em, [1.0, 1.0, 1.0, 0.85], 0.5);
        }
    }
}

/// A short form of a size.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    match unit {
        0 => format!("{bytes} bytes"),
        _ if value < 10.0 => format!("{value:.1} {}", UNITS[unit]),
        _ => format!("{value:.0} {}", UNITS[unit]),
    }
}

/// When, as a person says it: the time today, the day this year, the date otherwise.
pub fn when(seconds: Option<i64>, now: i64) -> String {
    let Some(at) = seconds else { return String::new() };
    let (days, secs) = (at.div_euclid(86400), at.rem_euclid(86400));
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let month = MONTHS[(m - 1).clamp(0, 11) as usize];
    if now.div_euclid(86400) == days {
        format!("Today {:02}:{:02}", secs / 3600, secs % 3600 / 60)
    } else if now - at < 300 * 86400 {
        format!("{d} {month}")
    } else {
        format!("{d} {month} {y}")
    }
}

/// A line at the foot of a pane: what keys do, or what went wrong.
pub fn status(p: &mut Painter, gl: &Gl, rect: Rect, text: &str, trouble: bool) {
    p.label(gl, text, rect, theme::FOOTER_EM, if trouble { look::TROUBLE } else { theme::INK_HINT }, 0.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_read_as_a_person_says_them() {
        let now = 1791989283; // 14 Oct 2026, 14:48
        assert_eq!(when(Some(now - 60), now), "Today 14:47");
        assert_eq!(when(Some(now - 10 * 86400), now), "4 Oct");
        assert_eq!(when(Some(0), now), "1 Jan 1970");
        assert_eq!(when(None, now), "");
    }
}
