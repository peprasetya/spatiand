//! The launcher's icons that are names in a Linux icon theme rather than pictures: a computer, and the
//! groups of applications. The Deck finds them in the theme; a Mac has no such theme, so they are drawn
//! here, as simple outlined pictures in the glass's own pale blue.

type P = (f32, f32);

fn seg(p: P, a: P, b: P, r: f32) -> f32 {
    let (pa, ba) = ((p.0 - a.0, p.1 - a.1), (b.0 - a.0, b.1 - a.1));
    let t = ((pa.0 * ba.0 + pa.1 * ba.1) / (ba.0 * ba.0 + ba.1 * ba.1).max(1e-6)).clamp(0.0, 1.0);
    ((pa.0 - ba.0 * t).powi(2) + (pa.1 - ba.1 * t).powi(2)).sqrt() - r
}
fn rbox(p: P, c: P, h: P, r: f32) -> f32 {
    let (dx, dy) = ((p.0 - c.0).abs() - h.0 + r, (p.1 - c.1).abs() - h.1 + r);
    (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() + dx.max(dy).min(0.0) - r
}
fn ring(p: P, c: P, radius: f32, width: f32) -> f32 {
    (((p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).sqrt() - radius).abs() - width
}
fn disc(p: P, c: P, radius: f32) -> f32 {
    ((p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).sqrt() - radius
}
fn outline(d: f32, width: f32) -> f32 {
    d.abs() - width
}
/// A triangle's signed distance, by the three half planes (points counter-clockwise, y down).
fn tri(p: P, a: P, b: P, c: P) -> f32 {
    let edge = |u: P, v: P| {
        let (e, n) = ((v.0 - u.0, v.1 - u.1), (p.0 - u.0, p.1 - u.1));
        (e.0 * n.1 - e.1 * n.0) / (e.0 * e.0 + e.1 * e.1).sqrt()
    };
    -edge(a, b).min(edge(b, c)).min(edge(c, a))
}

/// A picture for an icon-theme name, 128 square, straight RGBA; `None` for a name that is not one of the
/// launcher's own.
pub fn themed(name: &str) -> Option<(u32, u32, Vec<u8>)> {
    // The union of what is drawn (negative inside) with what is cut out of it.
    let shape: Box<dyn Fn(P) -> f32> = match name {
        "network-server" => Box::new(|p| {
            let screen = outline(rbox(p, (0.0, -0.12), (0.78, 0.52), 0.10), 0.075);
            let neck = rbox(p, (0.0, 0.62), (0.07, 0.14), 0.0);
            let foot = rbox(p, (0.0, 0.78), (0.36, 0.06), 0.05);
            let light = disc(p, (0.52, 0.20), 0.045);
            screen.min(neck).min(foot).min(light)
        }),
        "applications-games" => Box::new(|p| {
            let body = rbox(p, (0.0, 0.05), (0.84, 0.46), 0.30);
            let pad = rbox(p, (-0.42, 0.05), (0.20, 0.06), 0.02).min(rbox(p, (-0.42, 0.05), (0.06, 0.20), 0.02));
            let buttons = disc(p, (0.34, 0.10), 0.075).min(disc(p, (0.55, -0.04), 0.075));
            body.max(-pad.min(buttons))
        }),
        "applications-internet" => Box::new(|p| {
            let globe = ring(p, (0.0, 0.0), 0.78, 0.07);
            let equator = seg(p, (-0.78, 0.0), (0.78, 0.0), 0.06);
            let meridian = ring((p.0 * 1.9, p.1), (0.0, 0.0), 0.78 * 1.9 * 0.5 + 0.0, 0.07 * 1.4).max(disc(p, (0.0, 0.0), 0.78));
            let band = seg(p, (-0.66, -0.38), (0.66, -0.38), 0.05).min(seg(p, (-0.66, 0.38), (0.66, 0.38), 0.05)).max(disc(p, (0.0, 0.0), 0.78));
            globe.min(equator).min(meridian).min(band)
        }),
        "applications-multimedia" => Box::new(|p| {
            let play = tri(p, (-0.22, -0.38), (-0.22, 0.38), (0.42, 0.0));
            disc(p, (0.0, 0.0), 0.80).max(-play)
        }),
        "applications-graphics" => Box::new(|p| {
            let frame = outline(rbox(p, (0.0, 0.0), (0.80, 0.62), 0.12), 0.07);
            let sun = disc(p, (-0.34, -0.22), 0.13);
            let hill = tri(p, (-0.62, 0.46), (-0.12, -0.02), (0.38, 0.46)).max(rbox(p, (0.0, 0.0), (0.74, 0.56), 0.1));
            let hill2 = tri(p, (0.0, 0.46), (0.36, 0.08), (0.70, 0.46)).max(rbox(p, (0.0, 0.0), (0.74, 0.56), 0.1));
            frame.min(sun).min(hill).min(hill2)
        }),
        "applications-office" => Box::new(|p| {
            let page = outline(rbox(p, (0.0, 0.0), (0.56, 0.80), 0.10), 0.07);
            let lines = [-0.36f32, -0.10, 0.16, 0.42].iter().map(|y| seg(p, (-0.30, *y), (0.30, *y), 0.045)).fold(f32::MAX, f32::min);
            page.min(lines)
        }),
        "applications-development" => Box::new(|p| {
            let left = seg(p, (-0.26, -0.40), (-0.66, 0.0), 0.075).min(seg(p, (-0.66, 0.0), (-0.26, 0.40), 0.075));
            let right = seg(p, (0.26, -0.40), (0.66, 0.0), 0.075).min(seg(p, (0.66, 0.0), (0.26, 0.40), 0.075));
            left.min(right).min(seg(p, (0.12, -0.52), (-0.12, 0.52), 0.065))
        }),
        "applications-system" => Box::new(|p| {
            let (angle, r) = (p.1.atan2(p.0), (p.0 * p.0 + p.1 * p.1).sqrt());
            let teeth = (angle * 4.0).cos().abs().powf(0.6);
            let body = r - (0.56 + 0.18 * (teeth - 0.5).max(0.0) * 2.0);
            body.max(-(r - 0.24))
        }),
        "preferences-system" => Box::new(|p| {
            let rows = [-0.45f32, 0.0, 0.45];
            let knobs = [0.3f32, -0.35, 0.15];
            let mut d = f32::MAX;
            for (y, k) in rows.iter().zip(knobs) {
                d = d.min(seg(p, (-0.72, *y), (0.72, *y), 0.045)).min(disc(p, (k, *y), 0.13));
            }
            d
        }),
        "applications-utilities" => Box::new(|p| {
            let handle = seg(p, (-0.50, 0.50), (0.22, -0.22), 0.095);
            let head = ring(p, (0.38, -0.38), 0.30, 0.08);
            handle.min(head)
        }),
        "applications-other" => Box::new(|p| {
            let mut d = f32::MAX;
            for (x, y) in [(-0.38f32, -0.38f32), (0.38, -0.38), (-0.38, 0.38), (0.38, 0.38)] {
                d = d.min(rbox(p, (x, y), (0.30, 0.30), 0.09));
            }
            d
        }),
        _ => return None,
    };
    let size = 128u32;
    let mut out = vec![0u8; (size * size * 4) as usize];
    let half = size as f32 * 0.5;
    for y in 0..size {
        for x in 0..size {
            let p = ((x as f32 + 0.5 - half) / half, (y as f32 + 0.5 - half) / half);
            let cover = (0.5 - shape(p) * half).clamp(0.0, 1.0);
            if cover > 0.0 {
                let i = ((y * size + x) * 4) as usize;
                out[i..i + 4].copy_from_slice(&[226, 238, 255, (cover * 255.0) as u8]);
            }
        }
    }
    Some((size, size, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_the_launcher_uses_has_a_picture_with_some_ink() {
        for name in [
            "network-server", "applications-games", "applications-internet", "applications-multimedia",
            "applications-graphics", "applications-office", "applications-development", "applications-system",
            "preferences-system", "applications-utilities", "applications-other",
        ] {
            let (w, h, px) = themed(name).unwrap_or_else(|| panic!("{name}"));
            let ink = px.chunks_exact(4).filter(|p| p[3] > 128).count();
            assert!(ink > (w * h) as usize / 30 && ink < (w * h) as usize * 3 / 4, "{name}: {ink}");
        }
        assert!(themed("firefox").is_none());
    }
}
