//! Where a window sits in the viewer-centred frame, and how it is bent round the wearer.
//!
//! Windows are placed on a **cylinder** rather than a true sphere: at a fixed radius, with
//! yaw spread around the viewer and a small pitch, all facing inward. A cylinder keeps
//! vertical lines vertical, which matters a great deal for reading text.
//!
//! This is the Deck's own `window.rs`, moved here so the Mac's room and the Deck's are one: nothing in it
//! knows what draws it.

use glam::{DQuat, DVec3};

/// Where a window sits, in the viewer-centred frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Angle around the viewer, radians. 0 is straight ahead, positive is to the left
    /// (matching the tracker's +Y-is-left convention).
    pub yaw: f64,
    /// Angle above the horizon, radians.
    pub pitch: f64,
    /// Distance from the viewer, metres.
    pub radius: f64,
    /// Width of the window in the world, metres. Height follows from the surface's aspect.
    pub width: f64,
    /// The way the window faces, when it is not turned to face the viewer from where it sits:
    /// `None` for every window in the room, `Some` only for one pinned to the glass, which is flat
    /// to the view and so takes the head's own orientation. See [`spatiand_render::pip`].
    pub facing: Option<DQuat>,
    /// Set for a window pinned to the glass, which is drawn and aimed at without a title bar
    /// and is never where the layout last put it: it is worked out afresh from the head every
    /// frame. See [`crate::pip`].
    pub pip: bool,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: 0.0,
            // Far enough that the eyes converge comfortably. Closer than about a metre and
            // the vergence/accommodation conflict starts to be felt on a fixed-focus display
            // like this one; much further and the window subtends too little of a 40 degree
            // field to read.
            radius: 2.2,
            // 1.1 m at 2.2 m is about 28 degrees, against one eye's 40. The first attempt used
            // 1.6 m at 2 m -- 44 degrees -- on the theory that a focused window should fill the
            // view. It does: it fills it completely, edges past the field on both sides, and a
            // window whose extent you cannot see is one you cannot aim a pointer at or judge
            // the size of. It also hid the entire world behind it.
            width: 1.1,
            facing: None,
            pip: false,
        }
    }
}

impl Placement {
    /// Position of the window's centre in world space.
    pub fn position(&self) -> DVec3 {
        // +X forward, +Y left, +Z up.
        let horizontal = self.radius * self.pitch.cos();
        DVec3::new(
            horizontal * self.yaw.cos(),
            horizontal * self.yaw.sin(),
            self.radius * self.pitch.sin(),
        )
    }

    /// Orientation that makes the window face the viewer.
    ///
    /// Yaw **and** pitch: a window placed high or low tips to face you, so it is square-on
    /// wherever it sits. The first version turned only in yaw -- a cylinder rather than a
    /// sphere -- on the theory that keeping every window's vertical axis parallel to the
    /// world's keeps text upright. It does, and it also means a window above the horizon is
    /// viewed at an angle and reads as a trapezoid.
    ///
    /// A sphere is the right shape here because tracking is 3DoF: the viewer is always at the
    /// centre and never moves, so "facing the viewer" is unambiguous. That stops being true
    /// the moment a window can be pinned to something in the room, which is a different
    /// feature and a different placement rule.
    pub fn orientation(&self) -> DQuat {
        if let Some(facing) = self.facing {
            return facing;
        }
        // Yaw about up, then pitch about the rotated left axis, so the window's own up stays
        // as close to world-up as facing the viewer allows.
        DQuat::from_axis_angle(DVec3::Z, self.yaw) * DQuat::from_axis_angle(DVec3::Y, -self.pitch)
    }

    /// The radius of the cylinder the window is bent round, or `None` for a window that is flat.
    ///
    /// A window in the room is bent round the wearer, who is the cylinder's axis. One pinned to
    /// the glass is flat: it is small, and flat to the view, and the wearer is not on any axis
    /// that passes through it.
    pub fn bend_radius(&self) -> Option<f64> {
        (!self.pip).then_some(self.radius)
    }
}

/// How far round the wearer one strip of a bent window may turn, radians.
///
/// A window is bent by drawing it as flat strips, each tangent to the cylinder, and with no
/// lighting to show a crease the only thing the strip width has to answer to is the silhouette.
/// A degree and a half is far inside what the optics resolve -- the width of about seventy
/// pixels -- and keeps even a wide window to a few dozen strips.
const STRIP_STEP: f64 = 0.026;
const MAX_STRIPS: usize = 64;

/// How far each strip reaches over its neighbour, metres.
///
/// Strips that meet exactly may not *land* exactly: each has its own transform, so the shared
/// edge is computed twice from slightly different numbers. This is more than the difference and
/// a thirtieth of a pixel at two metres, so it cannot show as a doubled edge on glass. (The
/// larger fault -- both strips fading out over the same pixel -- is the shader's `u_seam`.)
pub const STRIP_OVERLAP_M: f64 = 0.00005;

/// A point across a window, put on the cylinder the window is bent round.
///
/// `y` is how far across, along the surface, metres and positive to the left like the world's
/// +Y; `z` is how far up, which a vertical cylinder does not change. The answer is relative to
/// the middle of the window in the window's own frame -- +X away from the wearer -- along with
/// the angle the surface has turned there, which is the rotation about the vertical that makes a
/// flat piece lie along it.
///
/// The wearer is the cylinder's axis, `radius` in front of the window's middle, so everything is
/// the same distance from the eye: `x` comes *towards* the wearer by the cylinder's sagitta.
pub fn on_cylinder(radius: f64, y: f64, z: f64) -> (DVec3, f64) {
    let angle = y / radius.max(1e-6);
    (
        DVec3::new(radius * (angle.cos() - 1.0), radius * angle.sin(), z),
        angle,
    )
}

/// One flat piece of a bent surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strip {
    /// Where its middle is across the surface, along it, in the same terms as
    /// [`on_cylinder`]'s `y`.
    pub y: f64,
    /// How wide the flat piece is. A little more than the arc it stands for, which is what
    /// makes the next strip start exactly where this one stops.
    pub chord: f64,
    /// The part of the surface's own width this covers, `0..1` running rightwards, which is
    /// what a texture is cut by.
    pub from: f64,
    pub to: f64,
}

/// A surface `width` metres across, centred `y` across the window, as the strips it is drawn in.
///
/// Rightwards first, so the texture runs the way it does on a flat quad. Each strip's chord is
/// the full tangent length -- `2 r tan(step / 2)` -- so neighbouring strips meet on the line
/// where their tangents cross rather than leaving a wedge between them or overlapping into one
/// another, which on translucent glass shows up as a bright seam.
pub fn strips(radius: f64, y: f64, width: f64) -> Vec<Strip> {
    strips_overlapping(radius, y, width, STRIP_OVERLAP_M)
}

/// [`strips`], with a chosen overlap between neighbours: none for glass drawn with blending, where the
/// overlap would be counted twice.
pub fn strips_overlapping(radius: f64, y: f64, width: f64, overlap: f64) -> Vec<Strip> {
    let radius = radius.max(1e-6);
    let span = width / radius;
    let count = ((span / STRIP_STEP).ceil() as usize).clamp(1, MAX_STRIPS);
    let step = span / count as f64;
    // Only between strips: a surface one strip wide has nothing to overlap.
    let overlap = if count > 1 { overlap } else { 0.0 };
    let share = overlap * 0.5 / width.max(1e-6);
    (0..count)
        .map(|i| {
            let from = i as f64 / count as f64;
            let to = (i + 1) as f64 / count as f64;
            Strip {
                y: y + width * (0.5 - (from + to) * 0.5),
                chord: 2.0 * radius * (step * 0.5).tan() + overlap,
                from: (from - share).max(0.0),
                to: (to + share).min(1.0),
            }
        })
        .collect()
}

/// How far above or below the horizon anything may be put, in radians: 89 degrees.
///
/// Pitch does not wrap the way yaw does -- a window taken past vertical ends up facing away
/// from a viewer who, being 3DoF, can only ever be at the centre of the sphere. So there has
/// to be a limit, and it has to be *short of* vertical rather than at it, where the frame a
/// window is built in has no heading left to turn by.
///
/// It was a little over sixty degrees, with a drag and the two-thumb gesture each clamping to
/// a different number of their own, and that was wrong for someone lying down. On your back
/// the ceiling is where you look, and a window asked for there -- brought to you, or dragged
/// up -- stopped a quarter turn short and hung off the bottom of the view. One number now,
/// used by everything that sets a pitch, so no two ways of placing a window can disagree about
/// where it is allowed to go.
pub const PITCH_LIMIT: f64 = 89.0 * std::f64::consts::PI / 180.0;

/// A pitch kept inside [`PITCH_LIMIT`].
pub fn clamp_pitch(pitch: f64) -> f64 {
    pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT)
}

/// The yaw, and the pitch above the horizon, of wherever `head` is looking. Radians.
///
/// Read from the head's forward and up directions together rather than from Euler angles,
/// because Euler angles fail in exactly the position this is for. Lying on your back and
/// looking straight up, forward is vertical, so the compass heading of forward is undefined:
/// Euler yaw there is whatever the head's roll happens to make it, and anything placed by it
/// would come up spun round in the plane of the ceiling.
///
/// The top of the head does not have that problem -- lying back, it points the way you were
/// facing, reversed. For a head with no roll, forward and up both lie in the one vertical plane
/// that contains the heading, and `up.z * forward.xy - forward.z * up.xy` is exactly the
/// heading's direction at every pitch: all forward when upright, all up when vertical, and the
/// right mixture in between. With some roll it is the nearest heading. Roll itself is left out
/// on purpose: something that tilted with a tilted head would stay crooked in the room once the
/// head straightened, and the wearer confirmed that would be disorienting.
///
/// Used for menus opening and for a window being brought to you, so the two always agree.
pub fn facing(head: DQuat) -> (f64, f64) {
    let forward = head * DVec3::X;
    let up = head * DVec3::Z;
    let heading = up.z * forward.truncate() - forward.z * up.truncate();
    let yaw = heading.y.atan2(heading.x);
    let pitch = clamp_pitch(forward.z.clamp(-1.0, 1.0).asin());
    (yaw, pitch)
}

/// `placement`, moved to the middle of where `head` is looking.
///
/// For bringing a window to the wearer. Only the direction changes -- size and distance are the
/// wearer's choices and stay as they were. Centred vertically as well as horizontally: the
/// horizon is in front of you sitting up and out of sight lying down, and a window asked for is
/// wanted where you are looking.
pub fn brought_here(placement: Placement, head: DQuat) -> Placement {
    let (yaw, pitch) = facing(head);
    Placement {
        yaw,
        pitch,
        ..placement
    }
}

