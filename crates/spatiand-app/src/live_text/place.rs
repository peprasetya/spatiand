//! Where a picture's pixels are on the window: from the picture as it was read, through the way
//! it is stored (a photograph's EXIF orientation), its quarter turns, the part of it shown (a
//! crop), to where it is drawn, zoomed and moved.
//!
//! Every step is a turn, a mirror, a scale or a move, so the whole is one affine map, kept as
//! such: it is undone exactly, for a press on the window to be a place on the picture.

use super::text::Point;
use crate::ui::Rect;

/// `x' = m[0] x + m[1] y + m[2]`, `y' = m[3] x + m[4] y + m[5]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mapping {
    pub m: [f32; 6],
}

impl Default for Mapping {
    fn default() -> Self {
        Mapping::IDENTITY
    }
}

impl Mapping {
    pub const IDENTITY: Mapping = Mapping { m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0] };

    pub fn apply(&self, p: Point) -> Point {
        let m = &self.m;
        [m[0] * p[0] + m[1] * p[1] + m[2], m[3] * p[0] + m[4] * p[1] + m[5]]
    }

    /// This, and then `after`.
    pub fn then(&self, after: &Mapping) -> Mapping {
        let (a, b) = (&self.m, &after.m);
        Mapping {
            m: [
                b[0] * a[0] + b[1] * a[3],
                b[0] * a[1] + b[1] * a[4],
                b[0] * a[2] + b[1] * a[5] + b[2],
                b[3] * a[0] + b[4] * a[3],
                b[3] * a[1] + b[4] * a[4],
                b[3] * a[2] + b[4] * a[5] + b[5],
            ],
        }
    }

    /// The way back, if there is one (a picture drawn to nothing has none).
    pub fn inverse(&self) -> Option<Mapping> {
        let m = &self.m;
        let det = m[0] * m[4] - m[1] * m[3];
        if det.abs() < 1e-12 {
            return None;
        }
        let (a, b, d, e) = (m[4] / det, -m[1] / det, -m[3] / det, m[0] / det);
        Some(Mapping { m: [a, b, -(a * m[2] + b * m[5]), d, e, -(d * m[2] + e * m[5])] })
    }

    /// How many of the window's pixels one of the picture's is, across.
    pub fn scale(&self) -> f32 {
        let m = &self.m;
        (m[0] * m[4] - m[1] * m[3]).abs().sqrt()
    }

    /// A picture `size` across and down, turned `turns` quarter turns clockwise: into the turned
    /// picture's pixels. Its new size is the other way round for an odd number.
    pub fn turned(turns: u8, size: (f32, f32)) -> Mapping {
        let (w, h) = size;
        match turns % 4 {
            0 => Mapping::IDENTITY,
            // The top left goes to the top right.
            1 => Mapping { m: [0.0, -1.0, h, 1.0, 0.0, 0.0] },
            2 => Mapping { m: [-1.0, 0.0, w, 0.0, -1.0, h] },
            _ => Mapping { m: [0.0, 1.0, 0.0, -1.0, 0.0, w] },
        }
    }

    /// A stored picture `size` across and down, shown upright as its EXIF orientation (1 to 8)
    /// says: from its stored pixels -- what hemera reads, ignoring the orientation -- to the
    /// upright picture's. 5 to 8 lie on their side: the upright picture is the other way round.
    pub fn exif(orientation: u8, size: (f32, f32)) -> Mapping {
        let (w, h) = size;
        match orientation {
            2 => Mapping { m: [-1.0, 0.0, w, 0.0, 1.0, 0.0] },
            3 => Mapping::turned(2, size),
            4 => Mapping { m: [1.0, 0.0, 0.0, 0.0, -1.0, h] },
            5 => Mapping { m: [0.0, 1.0, 0.0, 1.0, 0.0, 0.0] },
            6 => Mapping::turned(1, size),
            7 => Mapping { m: [0.0, -1.0, h, -1.0, 0.0, w] },
            8 => Mapping::turned(3, size),
            _ => Mapping::IDENTITY,
        }
    }

    /// A picture `size` across and down stretched over `rect`.
    pub fn onto(size: (f32, f32), rect: Rect) -> Mapping {
        let (sx, sy) = (rect.w / size.0.max(1e-6), rect.h / size.1.max(1e-6));
        Mapping { m: [sx, 0.0, rect.x, 0.0, sy, rect.y] }
    }

    /// The part `cut` of a picture `size` across and down -- left, top, right, bottom, as
    /// fractions of it -- stretched over `rect`: a cropped picture drawn where its kept part is.
    pub fn cut_onto(size: (f32, f32), cut: [f32; 4], rect: Rect) -> Mapping {
        let kept = ((cut[2] - cut[0]).max(1e-6) * size.0, (cut[3] - cut[1]).max(1e-6) * size.1);
        let (sx, sy) = (rect.w / kept.0, rect.h / kept.1);
        Mapping { m: [sx, 0.0, rect.x - cut[0] * size.0 * sx, 0.0, sy, rect.y - cut[1] * size.1 * sy] }
    }

    /// A picture read at `read` (its pixels) and drawn from a copy `drawn` across and down -- one
    /// made smaller for the GPU, say: into the copy's pixels.
    pub fn resized(read: (f32, f32), drawn: (f32, f32)) -> Mapping {
        Mapping { m: [drawn.0 / read.0.max(1e-6), 0.0, 0.0, 0.0, drawn.1 / read.1.max(1e-6), 0.0] }
    }

    /// The four corners mapped, each a point on the window.
    pub fn corners(&self, corners: [Point; 4]) -> [(f32, f32); 4] {
        corners.map(|p| {
            let q = self.apply(p);
            (q[0], q[1])
        })
    }
}

/// The size of a picture `size` after `turns` quarter turns.
pub fn turned_size(turns: u8, size: (f32, f32)) -> (f32, f32) {
    if turns % 2 == 1 { (size.1, size.0) } else { size }
}

/// The size a stored picture `size` is shown at upright, for its EXIF orientation.
pub fn upright_size(orientation: u8, size: (f32, f32)) -> (f32, f32) {
    if (5..=8).contains(&orientation) { (size.1, size.0) } else { size }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: Point, b: Point) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    #[test]
    fn a_quarter_turn_takes_the_top_left_to_the_top_right() {
        let size = (400.0, 300.0);
        let one = Mapping::turned(1, size);
        assert!(near(one.apply([0.0, 0.0]), [300.0, 0.0]));
        assert!(near(one.apply([400.0, 0.0]), [300.0, 400.0]));
        assert!(near(one.apply([0.0, 300.0]), [0.0, 0.0]));
        assert_eq!(turned_size(1, size), (300.0, 400.0));
        // Four of them are none; three are one the other way.
        let mut all = Mapping::IDENTITY;
        let mut at = size;
        for _ in 0..4 {
            all = all.then(&Mapping::turned(1, at));
            at = turned_size(1, at);
        }
        assert!(near(all.apply([123.0, 45.0]), [123.0, 45.0]));
        let three = Mapping::turned(3, size);
        assert!(near(three.apply([0.0, 0.0]), [0.0, 400.0]));
        assert!(near(Mapping::turned(2, size).apply([10.0, 20.0]), [390.0, 280.0]));
    }

    #[test]
    fn exif_orientations_stand_a_stored_picture_upright() {
        // A phone held upright stores its photo on its side (6): its stored top left is the upright
        // picture's top right.
        let stored = (4000.0, 3000.0);
        assert!(near(Mapping::exif(6, stored).apply([0.0, 0.0]), [3000.0, 0.0]));
        assert!(near(Mapping::exif(8, stored).apply([0.0, 0.0]), [0.0, 4000.0]));
        assert!(near(Mapping::exif(3, stored).apply([0.0, 0.0]), [4000.0, 3000.0]));
        assert!(near(Mapping::exif(2, stored).apply([0.0, 0.0]), [4000.0, 0.0]));
        assert!(near(Mapping::exif(4, stored).apply([0.0, 0.0]), [0.0, 3000.0]));
        assert!(near(Mapping::exif(5, stored).apply([10.0, 20.0]), [20.0, 10.0]));
        assert!(near(Mapping::exif(7, stored).apply([0.0, 0.0]), [3000.0, 4000.0]));
        assert_eq!(upright_size(6, stored), (3000.0, 4000.0));
        for o in 1..=8 {
            let up = Mapping::exif(o, stored);
            let (w, h) = upright_size(o, stored);
            // Every corner lands on a corner of the upright picture.
            for corner in [[0.0, 0.0], [4000.0, 0.0], [0.0, 3000.0], [4000.0, 3000.0]] {
                let p = up.apply(corner);
                assert!((near([p[0], 0.0], [0.0, 0.0]) || near([p[0], 0.0], [w, 0.0])) && (near([0.0, p[1]], [0.0, 0.0]) || near([0.0, p[1]], [0.0, h])), "{o}: {p:?}");
            }
        }
    }

    #[test]
    fn zoomed_and_moved_a_press_comes_back_to_the_same_pixel() {
        // A 1170 x 2532 screenshot, turned a quarter, drawn zoomed so that it is twice the window
        // and moved: the way there and back meet.
        let size = (1170.0, 2532.0);
        let shown = Mapping::turned(1, size).then(&Mapping::onto(turned_size(1, size), Rect::new(-300.0, -80.0, 2532.0 * 2.0, 1170.0 * 2.0)));
        let back = shown.inverse().unwrap();
        for p in [[0.0, 0.0], [585.0, 1266.0], [1170.0, 2532.0], [33.3, 2000.0]] {
            assert!(near(back.apply(shown.apply(p)), p), "{p:?}");
        }
        assert!((shown.scale() - 2.0).abs() < 1e-4);
        // A crop: the kept part's corner is the rectangle's.
        let cut = Mapping::cut_onto((1000.0, 500.0), [0.25, 0.5, 0.75, 1.0], Rect::new(10.0, 20.0, 1000.0, 500.0));
        assert!(near(cut.apply([250.0, 250.0]), [10.0, 20.0]));
        assert!(near(cut.apply([750.0, 500.0]), [1010.0, 520.0]));
        assert!(Mapping::onto((10.0, 10.0), Rect::new(0.0, 0.0, 0.0, 0.0)).inverse().is_none());
    }
}
