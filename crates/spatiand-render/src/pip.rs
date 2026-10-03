//! Picture in picture: a window that stays on the glass.
//!
//! An ordinary window lives in the room and you look at it by turning your head. A pinned one
//! lives in the *view*: wherever the wearer looks it sits in the same corner, at the same size,
//! and the room slides past behind it.
//!
//! Nothing here draws or aims. It answers one question -- given where the head is pointing,
//! where in the room is a window that is fixed to the head? -- and answers it as a position and
//! an orientation in the same world frame everything else is in. That is the whole trick: the
//! drawing and the pointer ray already know how to deal with a window at a place in the room,
//! so a window that is merely *recomputed every frame from the head* needs no path of its own
//! through either, and cannot disagree between them.
//!
//! The head is read from the very pose the eyes are built from for that frame, so the window is
//! not a frame behind the way a world-fixed one chasing the head would be.

use glam::{DQuat, DVec3};
use serde::{Deserialize, Serialize};

/// Which corner of the view a pinned window sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    /// Where a television puts its picture-in-picture, and where it is least in the way of
    /// reading: the eye starts at the top left and the lower right is where nothing is.
    #[default]
    BottomRight,
}

impl Corner {
    pub const ALL: [Corner; 4] = [
        Corner::BottomRight,
        Corner::BottomLeft,
        Corner::TopLeft,
        Corner::TopRight,
    ];

    /// The next corner round, which is what a row in the settings does when chosen.
    pub fn next(self) -> Self {
        let at = Self::ALL.iter().position(|c| *c == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }

    pub fn label(self) -> &'static str {
        match self {
            Corner::TopLeft => "Top left",
            Corner::TopRight => "Top right",
            Corner::BottomLeft => "Bottom left",
            Corner::BottomRight => "Bottom right",
        }
    }

    /// `(left, up)`: which way from the middle of the view this corner lies, as +1 or -1.
    fn signs(self) -> (f64, f64) {
        match self {
            Corner::TopLeft => (1.0, 1.0),
            Corner::TopRight => (-1.0, 1.0),
            Corner::BottomLeft => (1.0, -1.0),
            Corner::BottomRight => (-1.0, -1.0),
        }
    }
}

/// How big a pinned window is. Two sizes, as a television has: a setting with a slider is a
/// setting that gets fiddled with, and neither of these is wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Size {
    #[default]
    Small,
    Large,
}

impl Size {
    pub fn toggle(self) -> Self {
        match self {
            Size::Small => Size::Large,
            Size::Large => Size::Small,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Size::Small => "Small",
            Size::Large => "Large",
        }
    }

    /// How wide the picture looks, degrees of the wearer's view.
    ///
    /// Small is about a quarter of the field and large a little over a third: a film still
    /// reads at the first and is comfortable to follow at the second, and both leave most of
    /// the view to what you are actually doing.
    pub fn width_deg(self) -> f64 {
        match self {
            Size::Small => 9.0,
            Size::Large => 14.0,
        }
    }
}

/// How the glasses present the world, as far as where a corner is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// The field of view of one eye, `(horizontal, vertical)` degrees.
    pub fov_deg: (f64, f64),
    /// Where the eyes are, in the head's own frame -- the neck lever the eyes ride on. The
    /// window is placed against *this*, not the neck: from the eye, a thing hung off the neck
    /// by its own angle sits a couple of degrees low.
    pub eye: DVec3,
}

/// How far in front of the eye a pinned window floats, metres.
///
/// Nearer than the room's windows are, so that it reads as belonging to the glass and not to
/// the room, and no nearer than the optics are comfortable with. It is small and bright and
/// not read for long, which is why it can be this close.
pub const DISTANCE_M: f64 = 1.5;

/// How far a pinned window keeps from the edge of the field, degrees.
///
/// The edge of what each eye sees is soft and the two eyes' edges do not coincide, so a window
/// that touched it would be cut off for one eye or the other.
const MARGIN_DEG: f64 = 2.0;

/// How far apart two pinned windows in one corner are, degrees.
const GAP_DEG: f64 = 1.0;

/// Where a window fixed to the head is in the room.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub yaw: f64,
    pub pitch: f64,
    /// Metres from the centre of the room -- the neck -- not from the eye.
    pub radius: f64,
    /// The way the window faces: the head's own orientation, exactly.
    ///
    /// Flat to the view rather than turned to face the viewer from where it sits. A window in a
    /// corner that faces the centre of the room is seen from the eye at a slant, and its edges
    /// come out a degree or two off square -- true to the geometry, and wrong for something
    /// that is meant to look like a picture on the glass. Flat to the view, it is a rectangle,
    /// and it tilts with the head because it *is* the head's frame.
    pub facing: DQuat,
    /// The window's width in the room, metres.
    pub width: f64,
}

impl Pose {
    /// Where the window's centre is, in the world.
    pub fn position(&self) -> DVec3 {
        let horizontal = self.radius * self.pitch.cos();
        DVec3::new(
            horizontal * self.yaw.cos(),
            horizontal * self.yaw.sin(),
            self.radius * self.pitch.sin(),
        )
    }

    /// The way it faces: the head's.
    pub fn orientation(&self) -> DQuat {
        self.facing
    }
}

/// Where the `slot`th pinned window goes, for a head pointing along `head`.
///
/// `aspect` is the picture's width over its height. `slot` is how many other pinned windows
/// came before: a second one is stacked towards the middle of the view from the first, never
/// on top of it.
pub fn pose(
    head: DQuat,
    view: &View,
    corner: Corner,
    size: Size,
    aspect: f64,
    slot: usize,
) -> Pose {
    let aspect = aspect.max(0.1);
    let width_deg = size.width_deg();
    let half_w = (width_deg.to_radians() * 0.5).tan();
    // The picture's height as the eye sees it at this width.
    let height_deg = 2.0 * (half_w / aspect).atan().to_degrees();

    let (left, up) = corner.signs();
    let inset_h = (view.fov_deg.0 * 0.5 - width_deg * 0.5 - MARGIN_DEG).max(0.0);
    let inset_v = (view.fov_deg.1 * 0.5 - height_deg * 0.5 - MARGIN_DEG).max(0.0);
    // Further ones go towards the middle, by a whole window and a gap each.
    let stacked = (inset_v - slot as f64 * (height_deg + GAP_DEG)).max(0.0);

    let yaw = left * inset_h.to_radians();
    let pitch = up * stacked.to_radians();
    let direction = DVec3::new(
        pitch.cos() * yaw.cos(),
        pitch.cos() * yaw.sin(),
        pitch.sin(),
    );

    // The point in the head's frame, from the eye, and from there out into the room.
    let world = head * (view.eye + direction * DISTANCE_M);
    let radius = world.length().max(1e-6);
    let yaw = world.y.atan2(world.x);
    let pitch = (world.z / radius).clamp(-1.0, 1.0).asin();

    Pose {
        yaw,
        pitch,
        radius,
        facing: head,
        width: 2.0 * DISTANCE_M * half_w,
    }
}

/// Whether a window's title says it is a browser's picture-in-picture window.
///
/// The one thing a browser says about it, since Wayland has no way to ask: the windows are
/// ordinary toplevels, and what separates them is what they are called. Matched **exactly**,
/// after folding case, hyphens and spacing -- never "contains", because a page about the
/// feature has its name in the title of the ordinary window it is open in.
///
/// Titles are translated by the browser, so there is one for each language it ships in that I
/// know. A language that is not here is not wrong, only manual: the pin button does the same
/// thing by hand.
pub fn title_is_pip(title: &str) -> bool {
    let folded = fold(title);
    !folded.is_empty() && TITLES.iter().any(|t| *t == folded)
}

/// Lower case, with every hyphen, dash, underscore and apostrophe turned into a space and runs
/// of spaces made one, so `Picture-in-Picture` and `Picture in picture` are the same title.
fn fold(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut space = true;
    for c in title.chars() {
        let c = match c {
            '-' | '\u{2010}'..='\u{2015}' | '_' | '\'' | '\u{2019}' | '\u{00b4}' => ' ',
            c => c,
        };
        if c.is_whitespace() {
            if !space {
                out.push(' ');
                space = true;
            }
        } else {
            out.extend(c.to_lowercase());
            space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// What the browsers call the window, folded as [`fold`] folds. Chrome, Chromium, Edge, Brave
/// and Firefox between them; the languages are the ones the feature's own strings were found in.
const TITLES: &[&str] = &[
    "picture in picture",
    "imagen en imagen",
    "image dans l image",
    "bild in bild",
    "imagem em imagem",
    "beeld in beeld",
    "gambar dalam gambar",
    "картинка в картинке",
    "картинка в картинці",
    "ピクチャー イン ピクチャー",
    "ピクチャ イン ピクチャ",
    "ピクチャインピクチャ",
    "画中画",
    "畫中畫",
    "화면 속 화면",
    "pencere içinde pencere",
    "obraz w obrazie",
    "bild i bild",
    "billede i billede",
    "bilde i bilde",
    "kuva kuvassa",
    "obraz v obraze",
    "hình trong hình",
    "ภาพซ้อนภาพ",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View {
            fov_deg: (40.0, 22.3),
            eye: DVec3::new(0.10, 0.0, 0.075),
        }
    }

    fn heads() -> Vec<DQuat> {
        let turn = |yaw: f64, pitch: f64, roll: f64| {
            DQuat::from_axis_angle(DVec3::Z, yaw)
                * DQuat::from_axis_angle(DVec3::Y, -pitch)
                * DQuat::from_axis_angle(DVec3::X, roll)
        };
        vec![
            DQuat::IDENTITY,
            turn(1.0, 0.0, 0.0),
            turn(-2.4, 0.5, 0.0),
            turn(0.3, -0.6, 0.35),
            turn(3.0, 0.9, -0.5),
        ]
    }

    fn near(a: DVec3, b: DVec3, tol: f64) -> bool {
        (a - b).length() < tol
    }

    #[test]
    fn it_stays_where_it_is_in_the_view_however_the_head_turns() {
        // The whole point, stated as the thing that cannot change: seen from the head, the
        // window is at the same place whichever way the head is pointing.
        for corner in Corner::ALL {
            for size in [Size::Small, Size::Large] {
                let reference = {
                    let p = pose(DQuat::IDENTITY, &view(), corner, size, 16.0 / 9.0, 0);
                    p.position()
                };
                for head in heads() {
                    let p = pose(head, &view(), corner, size, 16.0 / 9.0, 0);
                    let in_head = head.inverse() * p.position();
                    assert!(
                        near(in_head, reference, 1e-9),
                        "{corner:?} {size:?} moved in the view: {in_head:?} vs {reference:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn it_is_flat_to_the_view_and_tilts_with_the_head() {
        // Tilt the head and the window tilts with it: stuck to the glass, not to the room. And
        // flat to the view, not turned to face the viewer from its corner, which would slant its
        // edges.
        for head in heads() {
            for corner in Corner::ALL {
                let p = pose(head, &view(), corner, Size::Small, 16.0 / 9.0, 0);
                assert!(near(p.orientation() * DVec3::Z, head * DVec3::Z, 1e-12));
                assert!(near(p.orientation() * DVec3::X, head * DVec3::X, 1e-12));
                assert!(near(p.orientation() * DVec3::Y, head * DVec3::Y, 1e-12));
            }
        }
    }

    #[test]
    fn every_corner_and_size_is_inside_the_field() {
        // Edges included, for a wide picture and a tall one: a window the eye cannot see all
        // of is a window that is cropped by the glasses.
        for aspect in [16.0 / 9.0, 4.0 / 3.0, 9.0 / 16.0] {
            for corner in Corner::ALL {
                for size in [Size::Small, Size::Large] {
                    let p = pose(DQuat::IDENTITY, &view(), corner, size, aspect, 0);
                    // As the eye sees it: from the eye, which is `view().eye` in the head frame.
                    let from_eye = p.position() - view().eye;
                    let yaw = from_eye.y.atan2(from_eye.x).to_degrees().abs();
                    let pitch = (from_eye.z / from_eye.length()).asin().to_degrees().abs();
                    let w = size.width_deg();
                    let h = 2.0
                        * ((w.to_radians() * 0.5).tan() / aspect).atan().to_degrees();
                    assert!(
                        yaw + w * 0.5 <= view().fov_deg.0 * 0.5 + 0.2,
                        "{corner:?} {size:?} {aspect}: runs off the side ({yaw} + {})",
                        w * 0.5
                    );
                    assert!(
                        pitch + h * 0.5 <= view().fov_deg.1 * 0.5 + 0.2 || h > view().fov_deg.1,
                        "{corner:?} {size:?} {aspect}: runs off the top ({pitch} + {})",
                        h * 0.5
                    );
                }
            }
        }
    }

    #[test]
    fn the_corners_are_where_they_are_called() {
        let at = |corner| {
            let p = pose(DQuat::IDENTITY, &view(), corner, Size::Small, 16.0 / 9.0, 0).position();
            (p.y > 0.0, p.z > view().eye.z)
        };
        assert_eq!(at(Corner::TopLeft), (true, true));
        assert_eq!(at(Corner::TopRight), (false, true));
        assert_eq!(at(Corner::BottomLeft), (true, false));
        assert_eq!(at(Corner::BottomRight), (false, false));
    }

    #[test]
    fn large_is_wider_than_small() {
        let small = pose(DQuat::IDENTITY, &view(), Corner::BottomRight, Size::Small, 1.78, 0);
        let large = pose(DQuat::IDENTITY, &view(), Corner::BottomRight, Size::Large, 1.78, 0);
        assert!(large.width > small.width * 1.4);
    }

    #[test]
    fn a_second_window_is_stacked_not_on_top() {
        let first = pose(DQuat::IDENTITY, &view(), Corner::BottomRight, Size::Small, 1.78, 0);
        let second = pose(DQuat::IDENTITY, &view(), Corner::BottomRight, Size::Small, 1.78, 1);
        // Higher up, for a bottom corner; and by at least a picture's height.
        assert!(second.pitch > first.pitch);
        let height = first.width / 1.78;
        let apart = (second.position() - first.position()).length();
        assert!(apart >= height * 0.95, "overlaps: {apart} against a height of {height}");
        let top = pose(DQuat::IDENTITY, &view(), Corner::TopRight, Size::Small, 1.78, 1);
        let top_first = pose(DQuat::IDENTITY, &view(), Corner::TopRight, Size::Small, 1.78, 0);
        assert!(top.pitch < top_first.pitch, "a top corner stacks downwards");
    }

    #[test]
    fn corners_cycle_through_all_four_and_come_back() {
        let mut c = Corner::default();
        let mut seen = vec![c];
        for _ in 0..3 {
            c = c.next();
            assert!(!seen.contains(&c));
            seen.push(c);
        }
        assert_eq!(c.next(), Corner::default());
        assert_eq!(Size::Small.toggle().toggle(), Size::Small);
    }

    #[test]
    fn the_browsers_titles_are_recognised() {
        for title in [
            "Picture in picture",
            "Picture-in-Picture",
            "  picture-in-picture ",
            "PICTURE IN PICTURE",
            "Imagen en imagen",
            "Image dans l\u{2019}image",
            "Bild-in-Bild",
            "Gambar dalam gambar",
            "画中画",
        ] {
            assert!(title_is_pip(title), "{title:?} should be recognised");
        }
    }

    #[test]
    fn ordinary_windows_are_not_taken_for_it() {
        // A page about the feature, a document that mentions it, nothing at all.
        for title in [
            "",
            "   ",
            "Picture in picture - Wikipedia",
            "How to use Picture-in-Picture in Chrome - Google Chrome",
            "Pictures",
            "Firefox",
        ] {
            assert!(!title_is_pip(title), "{title:?} should not be recognised");
        }
    }
}
