//! Where the assistant's figure stands (`crate::avatar`), worked out apart from its drawing so
//! that it can be tested. The owner, 2026-11-19, of a figure drawn over the chat: "The pal become
//! too center in screen."
//!
//! - **Beside the assistant's own window**, while that window is on show in the view and the place
//!   beside it is in sight: in the room with the window, not with the head -- on the side of it
//!   away from the middle of the view and from the other windows, its feet at the window's bottom
//!   edge, as tall as a share of the window, never over it. It goes where the window goes, at the
//!   size the window is.
//! - **In a corner of the view** otherwise (the assistant used by voice, its window put away or
//!   out of view -- or so large that the place beside it is out of sight, as SpatiPal's own 1500 by
//!   1000 is in a 40-degree view): low, on the side away from the pinned windows -- and never
//!   further in than the corner. A figure too large for it is drawn smaller there, not moved
//!   towards the middle.
//! - **Where the wearer put it**: pressed and dragged ([`Grip`]), beside the window on any side
//!   and anywhere along it, or to any corner of the view; remembered for each pal ([`Put`], in
//!   `prefs.toml`). A double press on it puts it back where it would stand by itself.
//!
//! The room's coordinates: +X ahead, +Y left, +Z up, metres, the wearer at the origin. A window's
//! own frame is its quad's ([`Quad`]): +Y across it to the left, +Z up it, bent round the wearer.
//! The same code on the Deck, the Beam Pro and the Mac: the room is built from one source.

use std::time::{Duration, Instant};

use glam::{DQuat, DVec2, DVec3};
use serde::{Deserialize, Serialize};
use spatiand_render::pip::Corner;
use spatiand_render::{Quad, Ray};

/// How wide the box the figure stands in is, against its height, when nothing says otherwise:
/// the room's robot, a body with its arms a little out. A figure of the wearer's is as wide as it
/// is made (its arms as they rest), and never narrower than this. What the window is kept clear
/// of, and what a press finds it by.
pub const WIDE: f64 = 0.6;

/// Beside a window, its height against the window's content at size 1 (a pal's `size` times
/// this), and the most it may be: as tall as the content.
pub const BESIDE_TALL: f64 = 0.42;
pub const BESIDE_MOST: f64 = 1.0;

/// How far from the window's edge it stands by itself, and how near and how far it may be put:
/// in the window's heights (its whole quad's, bar and frame and all).
pub const GAP: f64 = 0.03;
pub const GAP_LEAST: f64 = 0.015;
pub const GAP_MOST: f64 = 1.0;

/// The corner of the view it keeps to: as much of the view's width and height as it may take,
/// in normalised device coordinates (the view is 2 across). A figure larger is drawn smaller.
pub const CORNER_WIDE: f64 = 0.45;
pub const CORNER_TALL: f64 = 0.85;

/// Where its outer edges stand in a corner, in normalised device coordinates: just inside the
/// view's edges -- and in a top corner, below the room's status line (the time and the battery,
/// at the top right, the notices under it: `Scene::draw_status`, 10.2 degrees up).
pub const EDGE_X: f64 = 0.97;
pub const EDGE_Y: f64 = 0.95;
pub const EDGE_Y_TOP: f64 = 0.75;

/// How much of the view (normalised device coordinates, the smaller of across and up) the window
/// must take to be "on show" and have the figure come beside it. Once beside it, it stays until
/// none of the window is in view.
pub const ON_SHOW: f64 = 0.1;

/// How much of the place beside the window must be in view for the figure to stand there, and
/// how little of it may be once it does: a window filling the view leaves no room beside it in
/// sight (SpatiPal's own, 1500 by 1000 at 2.2 m, nearly fills a 40-degree view), and then the
/// figure keeps to a corner of the view rather than stand out of sight (2026-11-20).
pub const SEEN: f64 = 0.6;
pub const SEEN_KEEP: f64 = 0.35;

/// A press that went no further than this is not a drag, radians: a little over the room's own
/// hold on a pressed pointer (`press_hold::HOLD_DEGREES`, 0.4 of a degree).
const DRAG_AFTER: f64 = 0.5 * std::f64::consts::PI / 180.0;

/// Two presses within this, about the same place, are a double press: the SDK's
/// (`ui::DOUBLE_CLICK`), and a couple of degrees for a head that is never quite still.
const DOUBLE: Duration = Duration::from_millis(450);
const DOUBLE_SLOP: f64 = 2.0 * std::f64::consts::PI / 180.0;

/// How long a moved window must be still before the side the figure took by itself is chosen
/// again: not while it is being dragged, which would have the figure jump from side to side.
pub const STILL: Duration = Duration::from_millis(500);

/// Which side of the window, as the wearer sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Side {
    Left,
    Right,
    Above,
    Below,
}

/// A place beside a window, in the window's own measure: so it goes where the window goes, and
/// keeps its place at whatever size the window is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Beside {
    pub side: Side,
    /// Where along that side. Left or right of it: how far up the window its feet are, in the
    /// window's heights (0 at its bottom edge, 1 at its top). Over or under it: where across the
    /// window its middle is, 0 at its left edge and 1 at its right.
    pub along: f64,
    /// How far from the window's edge it stands, in the window's heights.
    pub gap: f64,
}

impl Beside {
    /// Where it stands beside a window by itself: on `side`, its feet at the bottom edge.
    pub fn by_itself(side: Side) -> Beside {
        Beside { side, along: 0.0, gap: GAP }
    }
}

/// Where the wearer put a pal's figure, if they did: beside the assistant's window, and in the
/// view when that window is not on show. Each is its own: one left out is where it stands by
/// itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Put {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beside: Option<Beside>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corner: Option<Corner>,
}

/// The window the figure stands beside: its quad -- the bar and the frame with the content, all
/// of which it is kept clear of -- and its content's height, which its size follows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub quad: Quad,
    pub content_height: f64,
}

impl Window {
    /// Whether two looks at a window are the same window where it was: not moved nor resized.
    pub fn same_as(&self, other: &Window) -> bool {
        let (a, b) = (&self.quad, &other.quad);
        a.centre.distance(b.centre) < 0.01 && (a.width - b.width).abs() < 0.01 * b.width.max(1e-6) && (a.height - b.height).abs() < 0.01 * b.height.max(1e-6)
    }

    /// A point across the window (`y`, along its surface, positive to the left) and up it (`z`),
    /// from its quad's middle, in the room: on the cylinder it is bent round.
    fn at(&self, y: f64, z: f64) -> DVec3 {
        let local = match self.quad.bend {
            Some(bend) => crate::window::on_cylinder(bend.radius, y, z).0,
            None => DVec3::new(0.0, y, z),
        };
        self.quad.centre + self.quad.orientation * local
    }

    /// The other way: where across and up the window a point in the room is.
    fn across(&self, point: DVec3) -> (f64, f64) {
        let local = self.quad.orientation.inverse() * (point - self.quad.centre);
        let y = match self.quad.bend {
            Some(bend) => local.y.atan2(local.x + bend.radius) * bend.radius,
            None => local.y,
        };
        (y, local.z)
    }
}

/// The box the figure stands in, in the room: upright, facing the wearer, `wide` by `tall`
/// metres about its middle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stand {
    pub middle: DVec3,
    pub wide: f64,
    pub tall: f64,
}

impl Stand {
    /// Its four corners: bottom left, bottom right, top left, top right (as the wearer sees it).
    pub fn corners(&self) -> [DVec3; 4] {
        let facing = DVec3::new(self.middle.x, self.middle.y, 0.0).normalize_or(DVec3::X);
        let left = DVec3::Z.cross(facing) * (self.wide * 0.5);
        let up = DVec3::Z * (self.tall * 0.5);
        let m = self.middle;
        [m + left - up, m - left - up, m + left + up, m - left + up]
    }
}

/// A direction from its angles: round to the left (`yaw`) and up (`pitch`), radians.
pub fn direction(yaw: f64, pitch: f64) -> DVec3 {
    DVec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin())
}

/// A direction's angles, as [`direction`] takes them.
pub fn angles_of(v: DVec3) -> (f64, f64) {
    (v.y.atan2(v.x), v.z.atan2(v.x.hypot(v.y)))
}

/// How tall it is beside `window`, metres: a share of the window's content, times the pal's
/// `size`, as tall as the content at the most.
pub fn tall_beside(window: &Window, size: f64) -> f64 {
    (BESIDE_TALL * size).min(BESIDE_MOST) * window.content_height
}

/// Where it stands beside `window`, `tall` by `wide` metres: never over the window, whatever
/// `place` says.
pub fn beside(window: &Window, place: &Beside, tall: f64, wide: f64) -> Stand {
    let (w, h) = (window.quad.width, window.quad.height);
    let gap = place.gap.clamp(GAP_LEAST, GAP_MOST) * h;
    let feet = -h * 0.5 + place.along.clamp(-tall / h.max(1e-6), 1.0) * h;
    let over = (w * 0.5 - place.along.clamp(0.0, 1.0) * w).clamp(-w * 0.5, w * 0.5);
    let (y, z) = match place.side {
        Side::Left => (w * 0.5 + gap + wide * 0.5, feet + tall * 0.5),
        Side::Right => (-(w * 0.5 + gap + wide * 0.5), feet + tall * 0.5),
        Side::Above => (over, h * 0.5 + gap + tall * 0.5),
        Side::Below => (over, -h * 0.5 - gap - tall * 0.5),
    };
    Stand { middle: window.at(y, z), wide, tall }
}

/// Where beside `window` a figure let go at `stand` is put: on the side it is furthest out past
/// -- or, let go over the window, the side it is nearest to leaving by -- and as far along and
/// out as it was, within what [`beside`] keeps to.
pub fn beside_of(window: &Window, stand: &Stand) -> Beside {
    let (w, h) = (window.quad.width, window.quad.height);
    let (y, z) = window.across(stand.middle);
    let (half_wide, half_tall) = (stand.wide * 0.5, stand.tall * 0.5);
    // How far out past each edge it is: negative, over the window.
    let out = [
        (Side::Left, y - half_wide - w * 0.5),
        (Side::Right, -y - half_wide - w * 0.5),
        (Side::Above, z - half_tall - h * 0.5),
        (Side::Below, -z - half_tall - h * 0.5),
    ];
    let (side, by) = out.into_iter().fold((Side::Left, f64::MIN), |best, this| if this.1 > best.1 { this } else { best });
    let along = match side {
        Side::Left | Side::Right => (z - half_tall + h * 0.5) / h.max(1e-6),
        Side::Above | Side::Below => (w * 0.5 - y) / w.max(1e-6),
    };
    let along = match side {
        Side::Left | Side::Right => along.clamp(-stand.tall / h.max(1e-6), 1.0),
        Side::Above | Side::Below => along.clamp(0.0, 1.0),
    };
    Beside { side, along, gap: (by / h.max(1e-6)).clamp(GAP_LEAST, GAP_MOST) }
}

/// The side of `window` the figure takes by itself: away from the other windows (`others`, what
/// of them it would cover), then away from the middle of the view as `head` looks; `otherwise`
/// (away from the pinned windows) when neither says.
pub fn side_for(window: &Window, tall: f64, wide: f64, head: DQuat, others: &[Quad], otherwise: Side) -> Side {
    let ahead = head * DVec3::X;
    let judged = |side: Side| {
        let stand = beside(window, &Beside::by_itself(side), tall, wide);
        let covered = angular(head, &stand.corners()).map_or(0.0, |mine| {
            let theirs: f64 = others.iter().filter_map(|q| angular(head, &quad_corners(q))).map(|o| overlap(&mine, &o)).sum();
            theirs / area(&mine).max(1e-12)
        });
        (covered, ahead.angle_between(stand.middle).to_degrees())
    };
    let (left, right) = (judged(Side::Left), judged(Side::Right));
    if (left.0 - right.0).abs() > 0.05 {
        return if left.0 < right.0 { Side::Left } else { Side::Right };
    }
    if (left.1 - right.1).abs() > 2.0 {
        return if left.1 > right.1 { Side::Left } else { Side::Right };
    }
    otherwise
}

/// The corner it keeps to by itself: low, on the side away from the windows pinned to the view.
pub fn corner_away_from(pinned: Corner) -> Corner {
    if pinned.signs().0 > 0.0 { Corner::BottomRight } else { Corner::BottomLeft }
}

/// The side beside the window it keeps to by itself when nothing else says: away from the
/// pinned windows, as in the view.
pub fn side_away_from(pinned: Corner) -> Side {
    if pinned.signs().0 > 0.0 { Side::Right } else { Side::Left }
}

/// The corner of the view nearest to where it was let go, as `head` sees it: `towards` is the
/// way from the wearer to it.
pub fn corner_of(head: DQuat, towards: DVec3) -> Corner {
    let seen = head.inverse() * towards;
    match (seen.y > 0.0, seen.z > 0.0) {
        (true, true) => Corner::TopLeft,
        (false, true) => Corner::TopRight,
        (true, false) => Corner::BottomLeft,
        (false, false) => Corner::BottomRight,
    }
}

/// How much of the view a window takes, through an eye's `project`ion (a point in the room to
/// normalised device coordinates, `None` behind the eye): the smaller of how far across and how
/// far up the view it reaches, 0 when none of it is in view.
pub fn on_show(quad: &Quad, project: &dyn Fn(DVec3) -> Option<DVec2>) -> f64 {
    let mut points = quad_corners(quad).to_vec();
    let (across, up) = (quad.orientation * DVec3::Y * (quad.width * 0.5), quad.orientation * DVec3::Z * (quad.height * 0.5));
    points.extend([quad.centre, quad.centre + across, quad.centre - across, quad.centre + up, quad.centre - up]);
    let seen: Vec<DVec2> = points.into_iter().filter_map(project).collect();
    if seen.len() < 2 {
        return 0.0;
    }
    let (x0, x1) = seen.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.x), b.max(p.x)));
    let (y0, y1) = seen.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.y), b.max(p.y)));
    (x1.min(1.0) - x0.max(-1.0)).min(y1.min(1.0) - y0.max(-1.0)).max(0.0)
}

/// Where it stands in `corner` of the view, `tall` by `wide` metres at `distance` from the wearer
/// as `head` looks, through the eye's `project`ion -- measured through the eye itself rather
/// than trusted to the field of view's numbers, which left it a third of the way out from the
/// middle on the Mac (2026-10-30). Its outer edges just inside the view's; and when it is too
/// large for the corner, drawn smaller rather than moved towards the middle: the share of its
/// size it is drawn at is the second answer (1 when it fits).
pub fn in_corner(corner: Corner, head: DQuat, tall: f64, wide: f64, distance: f64, fov_deg: (f64, f64), project: &dyn Fn(DVec3) -> Option<DVec2>) -> (Stand, f64) {
    let (left, up) = corner.signs();
    let place = |yaw: f64, pitch: f64| head * direction(yaw.to_radians(), pitch.to_radians()) * distance;
    let half = |metres: f64| (metres * 0.5 / distance).atan().to_degrees();
    // A first guess from the field of view's numbers, then measured.
    let mut yaw = left * (fov_deg.0 * 0.5 - 2.5 - half(wide)).max(3.0);
    let mut pitch = up * (fov_deg.1 * 0.5 - 1.5 - half(tall)).max(1.0);
    let mut share = 1.0;
    let stand = |yaw: f64, pitch: f64, share: f64| Stand { middle: place(yaw, pitch), wide: wide * share, tall: tall * share };
    for _ in 0..16 {
        let here = stand(yaw, pitch, share);
        let (Some([x0, x1, y0, y1]), Some(at), Some(turned), Some(raised)) =
            (rect(&here, project), project(here.middle), project(place(yaw + 0.5, pitch)), project(place(yaw, pitch + 0.5)))
        else {
            break;
        };
        // How the view moves with each degree, as this eye has it: the Deck draws its eyes flipped.
        let (per_yaw, per_pitch) = ((turned.x - at.x) / 0.5, (raised.y - at.y) / 0.5);
        if per_yaw.abs() < 1e-9 || per_pitch.abs() < 1e-9 {
            break;
        }
        // Too large for the corner: smaller there, never further in.
        let smaller = (CORNER_WIDE / (x1 - x0).max(1e-9)).min(CORNER_TALL / (y1 - y0).max(1e-9)).min(1.0);
        // Its outer edges at the view's: the corner is the way turning towards it goes.
        let want_x = (left * per_yaw).signum() * EDGE_X;
        let want_y = (up * per_pitch).signum() * if up > 0.0 { EDGE_Y_TOP } else { EDGE_Y };
        let over_x = if want_x < 0.0 { x0 } else { x1 } - want_x;
        let over_y = if want_y < 0.0 { y0 } else { y1 } - want_y;
        if smaller > 0.999 && over_x.abs() < 0.002 && over_y.abs() < 0.002 {
            break;
        }
        share *= smaller;
        yaw -= over_x / per_yaw;
        pitch -= over_y / per_pitch;
    }
    (stand(yaw, pitch, share), share)
}

/// How much of a stand is in view, through `project`: 0 to 1, the share of its box inside the
/// view; none when any of it is behind the eye.
pub fn seen(stand: &Stand, project: &dyn Fn(DVec3) -> Option<DVec2>) -> f64 {
    let Some([x0, x1, y0, y1]) = rect(stand, project) else { return 0.0 };
    let inside = (x1.min(1.0) - x0.max(-1.0)).max(0.0) * (y1.min(1.0) - y0.max(-1.0)).max(0.0);
    inside / ((x1 - x0) * (y1 - y0)).max(1e-12)
}

/// Where a stand is in the view, through `project`: `[x0, x1, y0, y1]` in normalised device
/// coordinates; `None` when any of it is behind the eye.
pub fn rect(stand: &Stand, project: &dyn Fn(DVec3) -> Option<DVec2>) -> Option<[f64; 4]> {
    let mut r = [f64::MAX, f64::MIN, f64::MAX, f64::MIN];
    for corner in stand.corners() {
        let p = project(corner)?;
        r = [r[0].min(p.x), r[1].max(p.x), r[2].min(p.y), r[3].max(p.y)];
    }
    Some(r)
}

/// Whether `ray` meets the stand: a press on the figure.
pub fn hit(stand: &Stand, ray: &Ray) -> bool {
    let to = stand.middle - ray.origin;
    let facing = DVec3::new(to.x, to.y, 0.0).normalize_or(DVec3::X);
    let towards = facing.dot(ray.direction);
    if towards <= 1e-6 {
        return false;
    }
    let along = facing.dot(to) / towards;
    if along <= 0.0 {
        return false;
    }
    let off = ray.origin + ray.direction * along - stand.middle;
    off.dot(DVec3::Z.cross(facing)).abs() <= stand.wide * 0.5 && off.z.abs() <= stand.tall * 0.5
}

/// A quad's four corners, flat (a bent window's edges are near enough where a flat one's are).
fn quad_corners(quad: &Quad) -> [DVec3; 4] {
    let across = quad.orientation * DVec3::Y * (quad.width * 0.5);
    let up = quad.orientation * DVec3::Z * (quad.height * 0.5);
    let c = quad.centre;
    [c + across - up, c - across - up, c + across + up, c - across + up]
}

/// Points as the head sees them, `[yaw0, yaw1, pitch0, pitch1]` in radians; `None` for anything
/// behind the head, which cannot be beside what is in view.
fn angular(head: DQuat, points: &[DVec3]) -> Option<[f64; 4]> {
    let mut r = [f64::MAX, f64::MIN, f64::MAX, f64::MIN];
    for p in points {
        let seen = head.inverse() * *p;
        if seen.x <= 0.0 {
            return None;
        }
        let (yaw, pitch) = angles_of(seen);
        r = [r[0].min(yaw), r[1].max(yaw), r[2].min(pitch), r[3].max(pitch)];
    }
    Some(r)
}

fn area(r: &[f64; 4]) -> f64 {
    (r[1] - r[0]).max(0.0) * (r[3] - r[2]).max(0.0)
}

fn overlap(a: &[f64; 4], b: &[f64; 4]) -> f64 {
    (a[1].min(b[1]) - a[0].max(b[0])).max(0.0) * (a[3].min(b[3]) - a[2].max(b[2])).max(0.0)
}

/// The wearer's hand on the figure: a press takes hold of it, a drag carries it, letting go puts
/// it where it was let go -- and a second press soon after one that went nowhere puts it back.
#[derive(Debug, Default)]
pub struct Grip {
    held: Option<Held>,
    /// The last press on it that went nowhere: when, and along which way, for a second to make a
    /// double press.
    tapped: Option<(Instant, DVec3)>,
}

#[derive(Debug, Clone, Copy)]
struct Held {
    /// The angles from the ray to its middle when it was taken, and how far away it was: it
    /// hangs from the spot it was taken by, at its distance.
    yaw_offset: f64,
    pitch_offset: f64,
    distance: f64,
    origin: DVec3,
    /// Where it is now, at the size it was taken at.
    stand: Stand,
    /// The way the ray went when pressed, and whether it has gone far enough from it to be a drag.
    from: DVec3,
    moved: bool,
    pressed: Instant,
    /// Taken from beside the window (else from the view): where it is let go is said so.
    beside: bool,
}

impl Grip {
    /// Where it is while it is held: drawn there, at once.
    pub fn holding(&self) -> Option<Stand> {
        self.held.map(|h| h.stand)
    }

    /// A press along `ray` on the figure, standing at `stand` -- beside the window (`beside`) or
    /// in the view -- where the wearer put it being `put`: taken hold of. Or, soon after a press
    /// that went nowhere, put back where it stands by itself: then `Some` of where it was put,
    /// the place it is in now forgotten.
    pub fn press(&mut self, ray: &Ray, stand: Stand, beside: bool, put: Put, now: Instant) -> Option<Put> {
        if let Some((at, way)) = self.tapped.take() {
            if now.duration_since(at) < DOUBLE && way.angle_between(ray.direction) < DOUBLE_SLOP {
                self.held = None;
                let mut put = put;
                if beside {
                    put.beside = None;
                } else {
                    put.corner = None;
                }
                return Some(put);
            }
        }
        let (ray_yaw, ray_pitch) = angles_of(ray.direction);
        let to = stand.middle - ray.origin;
        let (yaw, pitch) = angles_of(to);
        self.held = Some(Held {
            yaw_offset: yaw - ray_yaw,
            pitch_offset: pitch - ray_pitch,
            distance: to.length(),
            origin: ray.origin,
            stand,
            from: ray.direction,
            moved: false,
            pressed: now,
            beside,
        });
        None
    }

    /// The pointer moved along `ray` while it is held: it goes with it.
    pub fn drag(&mut self, ray: &Ray) {
        let Some(held) = self.held.as_mut() else { return };
        let (yaw, pitch) = angles_of(ray.direction);
        held.stand.middle = ray.origin + direction(yaw + held.yaw_offset, pitch + held.pitch_offset) * held.distance;
        held.moved |= held.from.angle_between(ray.direction) > DRAG_AFTER;
    }

    /// Let go of: where the wearer has put it now, if it was moved -- beside `window` when it was
    /// taken from beside it and the window is still there, else in the corner of the view nearest,
    /// as `head` sees it -- over `put`, where they had put it before.
    pub fn let_go(&mut self, window: Option<&Window>, head: DQuat, put: Put) -> Option<Put> {
        let held = self.held.take()?;
        if !held.moved {
            self.tapped = Some((held.pressed, held.from));
            return None;
        }
        let mut put = put;
        match window.filter(|_| held.beside) {
            Some(window) => put.beside = Some(beside_of(window, &held.stand)),
            None => put.corner = Some(corner_of(head, held.stand.middle - held.origin)),
        }
        Some(put)
    }

    /// Not held any more, wherever it is: the room's drag went to something else (a menu opened).
    pub fn cancel(&mut self) {
        self.held = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spatiand_render::{EyeSide, StereoConfig};

    /// SpatiPal's window as the room opens it: 1500 by 1000 pixels, 1.1 m wide at 2.2 m.
    fn window_at(yaw_deg: f64, pitch_deg: f64) -> Window {
        let placement = crate::window::Placement { yaw: yaw_deg.to_radians(), pitch: pitch_deg.to_radians(), ..Default::default() };
        let pixels = (1500, 1000);
        Window { quad: crate::pointer::quad_of(pixels, &placement), content_height: placement.width / 1.5 }
    }

    fn eye(head: DQuat) -> impl Fn(DVec3) -> Option<DVec2> {
        let vp = spatiand_render::eye_for(EyeSide::Left, head, DVec3::ZERO, &StereoConfig::default()).view_projection();
        move |p: DVec3| {
            let c = vp * p.as_vec3().extend(1.0);
            (c.w > 0.01).then(|| DVec2::new((c.x / c.w) as f64, (c.y / c.w) as f64))
        }
    }

    fn fov() -> (f64, f64) {
        let s = StereoConfig::default();
        (s.h_fov_deg, s.v_fov_deg())
    }

    /// Whether a stand overlaps the window's quad, as seen from the wearer.
    fn over_the_window(stand: &Stand, window: &Window) -> bool {
        let (head, quad) = (DQuat::IDENTITY, &window.quad);
        match (angular(head, &stand.corners()), angular(head, &quad_corners(quad))) {
            (Some(a), Some(b)) => overlap(&a, &b) > 0.0,
            _ => false,
        }
    }

    #[test]
    fn beside_the_window_it_never_overlaps_it_on_any_side() {
        let w = window_at(0.0, 0.0);
        let tall = tall_beside(&w, 1.0);
        for side in [Side::Left, Side::Right, Side::Above, Side::Below] {
            for along in [-0.3, 0.0, 0.5, 1.0] {
                // A gap of nothing, or one made up beyond what may be: kept to what may.
                for gap in [-1.0, 0.0, GAP, 0.4, 5.0] {
                    let stand = beside(&w, &Beside { side, along, gap }, tall, tall * WIDE);
                    assert!(!over_the_window(&stand, &w), "{side:?} {along} {gap}");
                }
            }
        }
        // By itself: its feet at the window's bottom edge, its middle level with the window's
        // distance, a share of the window's height.
        let stand = beside(&w, &Beside::by_itself(Side::Left), tall, tall * WIDE);
        let (_, z) = w.across(stand.middle);
        assert!((z - stand.tall * 0.5 + w.quad.height * 0.5).abs() < 1e-9);
        assert!((stand.middle.length() - 2.2).abs() < 0.05, "{}", stand.middle.length());
        assert!((stand.tall - BESIDE_TALL * w.content_height).abs() < 1e-9);
        // A larger pal is larger beside it, up to the window's own height.
        assert!(tall_beside(&w, 1.6) > tall);
        assert!((tall_beside(&w, 3.0) - w.content_height).abs() < 1e-9);
    }

    #[test]
    fn by_itself_it_takes_the_side_away_from_the_middle_of_the_view_and_the_other_windows() {
        let tall = tall_beside(&window_at(0.0, 0.0), 1.0);
        let wide = tall * WIDE;
        let ahead = DQuat::IDENTITY;
        // The window to the left of the middle of the view: the figure on its left, further out.
        assert_eq!(side_for(&window_at(15.0, 0.0), tall, wide, ahead, &[], Side::Right), Side::Left);
        assert_eq!(side_for(&window_at(-15.0, 0.0), tall, wide, ahead, &[], Side::Left), Side::Right);
        // In the middle: away from the pinned windows, as said.
        assert_eq!(side_for(&window_at(0.0, 0.0), tall, wide, ahead, &[], Side::Right), Side::Right);
        assert_eq!(side_for(&window_at(0.0, 0.0), tall, wide, ahead, &[], Side::Left), Side::Left);
        // Another window just left of it: the figure goes to the right, though the middle of the
        // view would have it left.
        let other = window_at(48.0, 0.0).quad;
        assert_eq!(side_for(&window_at(15.0, 0.0), tall, wide, ahead, &[other], Side::Left), Side::Right);
        // As the head looks: turned well to the left, the same window is right of the middle.
        let turned = DQuat::from_axis_angle(DVec3::Z, 40f64.to_radians());
        assert_eq!(side_for(&window_at(15.0, 0.0), tall, wide, turned, &[], Side::Left), Side::Right);
    }

    #[test]
    fn it_follows_the_window_when_the_window_is_moved_or_resized() {
        let place = Beside { side: Side::Right, along: 0.2, gap: 0.05 };
        let here = window_at(0.0, 0.0);
        let there = window_at(-30.0, 10.0);
        let tall = tall_beside(&here, 1.0);
        let wide = tall * WIDE;
        let (a, b) = (beside(&here, &place, tall, wide), beside(&there, &place, tall, wide));
        // The same place beside it, in the window's own measure, wherever the window is.
        let back = beside_of(&there, &b);
        assert_eq!(back.side, Side::Right);
        assert!((back.along - 0.2).abs() < 1e-6 && (back.gap - 0.05).abs() < 1e-6, "{back:?}");
        // Turned with the window, as far round as the window went.
        let turned = angles_of(b.middle).0 - angles_of(a.middle).0;
        assert!((turned.to_degrees() + 30.0).abs() < 1.0, "{}", turned.to_degrees());
        // Wider, it stays clear of it, at the same gap; larger, as tall as its share.
        let mut wider = here;
        wider.quad.width *= 1.5;
        let stand = beside(&wider, &place, tall, wide);
        assert!(!over_the_window(&stand, &wider));
        // A figure with its arms out wide is kept as far clear of the window.
        let spread = beside(&here, &Beside::by_itself(Side::Left), tall, tall * 1.2);
        assert!(!over_the_window(&spread, &here));
        assert!(spread.middle.y > beside(&here, &Beside::by_itself(Side::Left), tall, wide).middle.y);
        assert!((beside_of(&wider, &stand).gap - 0.05).abs() < 1e-6);
        assert!(!here.same_as(&wider) && here.same_as(&window_at(0.0, 0.0)));
    }

    #[test]
    fn let_go_beside_the_window_it_is_put_on_the_side_it_was_let_go_by() {
        let w = window_at(0.0, 0.0);
        let tall = tall_beside(&w, 1.0);
        let (wide, h) = (tall * WIDE, w.quad.height);
        // Let go above the top edge, a third of the way across from the left.
        let up = Stand { middle: w.at(w.quad.width * (0.5 - 0.33), h * 0.5 + 0.1 + tall * 0.5), wide, tall };
        let put = beside_of(&w, &up);
        assert_eq!(put.side, Side::Above);
        assert!((put.along - 0.33).abs() < 1e-6 && (put.gap - 0.1 / h).abs() < 1e-6, "{put:?}");
        // Let go over the window, near its left edge: put out by that edge, never over it.
        let over = Stand { middle: w.at(w.quad.width * 0.4, 0.0), wide, tall };
        let put = beside_of(&w, &over);
        assert_eq!(put.side, Side::Left);
        assert_eq!(put.gap, GAP_LEAST);
        assert!(!over_the_window(&beside(&w, &put, tall, wide), &w));
    }

    #[test]
    fn in_a_corner_it_is_drawn_smaller_rather_than_moved_in() {
        let head = DQuat::IDENTITY;
        let project = eye(head);
        // The robot at its usual size fits the corner: its outer edges at the view's.
        let robot = 0.33 * 0.58;
        let (stand, share) = in_corner(Corner::BottomLeft, head, robot, robot * WIDE, 1.35, fov(), &project);
        assert_eq!(share, 1.0);
        let [x0, _, y0, _] = rect(&stand, &project).unwrap();
        assert!((x0 + EDGE_X).abs() < 0.01 && (y0 + EDGE_Y).abs() < 0.01, "{x0} {y0}");
        // A figure three times as large -- or as wide as it is tall -- no further in than the
        // corner, and drawn smaller.
        for (size, wide) in [(1.6, WIDE), (3.0, WIDE), (1.0, 2.5)] {
            let (large, share) = in_corner(Corner::BottomLeft, head, robot * size, robot * size * wide, 1.35, fov(), &project);
            assert!(share < 1.0, "{size}: {share}");
            let [x0, x1, y0, y1] = rect(&large, &project).unwrap();
            assert!((x0 + EDGE_X).abs() < 0.01 && (y0 + EDGE_Y).abs() < 0.01, "{size}: {x0} {y0}");
            assert!(x1 <= -EDGE_X + CORNER_WIDE + 0.01 && y1 <= -EDGE_Y + CORNER_TALL + 0.01, "{size}: {x1} {y1}");
        }
        // Each corner of the view, and a view drawn upside down (the Deck's eyes).
        for corner in Corner::ALL {
            let (stand, _) = in_corner(corner, head, robot, robot * WIDE, 1.35, fov(), &project);
            let [x0, x1, y0, y1] = rect(&stand, &project).unwrap();
            let (left, up) = corner.signs();
            assert!(((if left > 0.0 { x0 } else { x1 }) + left * EDGE_X).abs() < 0.01, "{corner:?}");
            // Low corners at the view's bottom; high ones under the room's status line.
            let edge = if up > 0.0 { EDGE_Y_TOP } else { EDGE_Y };
            assert!(((if up > 0.0 { y1 } else { y0 }) - up * edge).abs() < 0.01, "{corner:?}");
            assert_eq!(corner_of(head, stand.middle), corner);
        }
        let flipped = |p: DVec3| project(p).map(|n| DVec2::new(n.x, -n.y));
        let (stand, _) = in_corner(Corner::BottomRight, head, robot, robot * WIDE, 1.35, fov(), &flipped);
        assert!(stand.middle.z < 0.0 && stand.middle.y < 0.0, "{:?}", stand.middle);
        // With the head turned, the corner is the view's, not the room's.
        let turned = DQuat::from_axis_angle(DVec3::Z, 1.0);
        let (stand, _) = in_corner(Corner::BottomLeft, turned, robot, robot * WIDE, 1.35, fov(), &eye(turned));
        assert_eq!(corner_of(turned, stand.middle), Corner::BottomLeft);
        assert_eq!(corner_away_from(Corner::BottomRight), Corner::BottomLeft);
        assert_eq!(side_away_from(Corner::TopLeft), Side::Right);
    }

    #[test]
    fn beside_a_window_that_fills_the_view_there_is_no_room_in_sight() {
        // SpatiPal's windows as the room sizes them, by their points: its own 1500 by 1000, and
        // one made 1000 by 700, in the middle of the view at 2.2 m.
        let sized = |pixels: (u32, u32)| {
            let placement = crate::window::Placement { width: pixels.0 as f64 * crate::window::METRES_PER_POINT, ..Default::default() };
            Window { quad: crate::pointer::quad_of(pixels, &placement), content_height: placement.width * pixels.1 as f64 / pixels.0 as f64 }
        };
        let project = eye(DQuat::IDENTITY);
        let share = |w: &Window| {
            let tall = tall_beside(w, 1.0);
            seen(&beside(w, &Beside::by_itself(Side::Left), tall, tall * WIDE), &project)
        };
        // Its own size: the place beside it is out of view, and the figure keeps to a corner.
        let large = sized((1500, 1000));
        assert!(on_show(&large.quad, &project) > ON_SHOW);
        assert!(share(&large) < SEEN_KEEP, "{}", share(&large));
        // Smaller: beside it, in sight.
        assert!(share(&sized((1000, 700))) >= SEEN, "{}", share(&sized((1000, 700))));
        // Looked towards (left, and a little down to its feet), the place beside the large one
        // comes into view.
        let towards = DQuat::from_axis_angle(DVec3::Z, 14f64.to_radians()) * DQuat::from_axis_angle(DVec3::Y, 5f64.to_radians());
        let tall = tall_beside(&large, 1.0);
        assert!(seen(&beside(&large, &Beside::by_itself(Side::Left), tall, tall * WIDE), &eye(towards)) >= SEEN);
    }

    #[test]
    fn a_window_is_on_show_while_some_of_it_is_in_view() {
        let head = DQuat::IDENTITY;
        let project = eye(head);
        assert!(on_show(&window_at(0.0, 0.0).quad, &project) > 1.0);
        assert!(on_show(&window_at(30.0, 0.0).quad, &project) > ON_SHOW);
        assert_eq!(on_show(&window_at(60.0, 0.0).quad, &project), 0.0);
        assert_eq!(on_show(&window_at(180.0, 0.0).quad, &project), 0.0);
    }

    #[test]
    fn a_press_finds_the_figure_and_nothing_beside_it() {
        let stand = Stand { middle: DVec3::new(2.0, 0.5, -0.4), wide: 0.15, tall: 0.3 };
        let along = |to: DVec3| Ray { origin: DVec3::ZERO, direction: to.normalize() };
        assert!(hit(&stand, &along(stand.middle)));
        assert!(hit(&stand, &along(stand.middle + DVec3::new(0.0, 0.06, 0.13))));
        assert!(!hit(&stand, &along(stand.middle + DVec3::new(0.0, 0.1, 0.0))));
        assert!(!hit(&stand, &along(stand.middle + DVec3::new(0.0, 0.0, 0.2))));
        assert!(!hit(&stand, &along(-stand.middle)));
    }

    #[test]
    fn dragged_it_is_put_where_it_was_let_go_and_a_double_press_puts_it_back() {
        let w = window_at(0.0, 0.0);
        let tall = tall_beside(&w, 1.0);
        let stand = beside(&w, &Beside::by_itself(Side::Right), tall, tall * WIDE);
        let start = Instant::now();
        let ray_to = |p: DVec3| Ray { origin: DVec3::ZERO, direction: p.normalize() };
        let mut grip = Grip::default();
        // Taken by its middle, carried up and over to the window's left, and let go there.
        assert_eq!(grip.press(&ray_to(stand.middle), stand, true, Put::default(), start), None);
        let there = w.at(w.quad.width * 0.5 + 0.2, 0.1);
        for k in 1..=10 {
            grip.drag(&ray_to(stand.middle.lerp(there, k as f64 / 10.0)));
        }
        assert!(grip.holding().unwrap().middle.distance(there) < 0.02);
        let put = grip.let_go(Some(&w), DQuat::IDENTITY, Put::default()).unwrap();
        let beside_it = put.beside.unwrap();
        assert_eq!(beside_it.side, Side::Left);
        assert!(beside_it.along > 0.2, "{beside_it:?}");
        assert_eq!(put.corner, None);
        // Where it stands now is where it was let go.
        let now = beside(&w, &beside_it, tall, tall * WIDE);
        assert!(now.middle.distance(there) < 0.03, "{}", now.middle.distance(there));
        // Pressed and let go where it is: a tap, nothing changes...
        assert_eq!(grip.press(&ray_to(now.middle), now, true, put, start + Duration::from_secs(2)), None);
        assert_eq!(grip.let_go(Some(&w), DQuat::IDENTITY, put), None);
        // ...and pressed again at once, it is put back where it stands by itself, beside the
        // window; where it was put in the view is kept.
        let both = Put { corner: Some(Corner::TopRight), ..put };
        let back = grip.press(&ray_to(now.middle), now, true, both, start + Duration::from_millis(2300)).unwrap();
        assert_eq!(back, Put { beside: None, corner: Some(Corner::TopRight) });
        assert!(grip.holding().is_none());
        // Two presses far apart in time are not a double press.
        grip.press(&ray_to(now.middle), now, true, put, start + Duration::from_secs(5));
        grip.let_go(Some(&w), DQuat::IDENTITY, put);
        assert_eq!(grip.press(&ray_to(now.middle), now, true, put, start + Duration::from_secs(6)), None);
        grip.cancel();
        assert!(grip.holding().is_none());
        // In the view, with no window on show: let go up and to the right, it keeps to that corner.
        let corner = Stand { middle: DVec3::new(1.3, 0.3, -0.25), wide: 0.1, tall: 0.19 };
        assert_eq!(grip.press(&ray_to(corner.middle), corner, false, Put::default(), start + Duration::from_secs(9)), None);
        grip.drag(&ray_to(DVec3::new(1.3, -0.3, 0.25)));
        let put = grip.let_go(None, DQuat::IDENTITY, Put::default()).unwrap();
        assert_eq!(put, Put { beside: None, corner: Some(Corner::TopRight) });
    }

    #[test]
    fn where_it_was_put_is_kept_as_words() {
        let put = Put { beside: Some(Beside { side: Side::Above, along: 0.25, gap: 0.05 }), corner: Some(Corner::TopLeft) };
        let text = toml::to_string(&put).unwrap();
        assert!(text.contains("corner = \"top-left\"") && text.contains("side = \"above\""), "{text}");
        assert_eq!(toml::from_str::<Put>(&text).unwrap(), put);
        assert_eq!(toml::from_str::<Put>("").unwrap(), Put::default());
    }
}
