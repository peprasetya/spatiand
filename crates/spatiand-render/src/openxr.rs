//! The eyes as OpenXR describes them: what an application drawing its own views is handed.
//!
//! Spatiand thinks in +X forward, +Y left, +Z up. OpenXR's frame is +X right, +Y up, −Z
//! forward, and its field of view is four signed angles (`XrFovf`). Everything a pose leaves
//! this process in -- the local pose channel on the Deck, a viewport sent to a host from the
//! Deck or the Beam Pro -- is in OpenXR's terms, and is converted here, once.

use glam::{DQuat, DVec3};

use crate::camera::{eye_for, EyeSide, StereoConfig};

/// One eye, in OpenXR's frame and field order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OpenXrEye {
    /// x, y, z, w.
    pub orientation: [f32; 4],
    /// Metres.
    pub position: [f32; 3],
    /// angleLeft, angleRight, angleUp, angleDown, radians, signed: `XrFovf` exactly.
    pub fov: [f32; 4],
}

/// One eye of a head, the way the renderer places it, in OpenXR's terms.
///
/// Built on [`eye_for`], the same function the eyes are drawn with, so an application drawing
/// from these and the session drawing the room cannot disagree about where the eyes are. Two
/// nearly identical camera models is how content ends up half a centimetre out with nobody
/// able to say why.
pub fn openxr_eye(
    side: EyeSide,
    orientation: DQuat,
    head_position: DVec3,
    stereo: &StereoConfig,
) -> OpenXrEye {
    let eye = eye_for(side, orientation, head_position, stereo);
    let (q, p) = to_openxr(eye.orientation, eye.position);
    let half_h = (stereo.h_fov_deg.to_radians() * 0.5) as f32;
    // Vertical from the horizontal and the eye's aspect, matching the projection matrix.
    let aspect = stereo.per_eye.1.max(1) as f32 / stereo.per_eye.0.max(1) as f32;
    let half_v = (half_h.tan() * aspect).atan();
    OpenXrEye {
        orientation: q,
        position: p,
        // Signed, and symmetric here because the projection is.
        fov: [-half_h, half_h, half_v, -half_v],
    }
}

/// Spatiand's frame to OpenXR's.
///
/// A vector `(x, y, z)` becomes `(−y, z, −x)`, which is a pure rotation -- the determinant is
/// +1 -- and a rotation may be applied to a quaternion's vector part alone, leaving `w`
/// untouched.
pub fn to_openxr(orientation: DQuat, position: DVec3) -> ([f32; 4], [f32; 3]) {
    let q = [
        -orientation.y as f32,
        orientation.z as f32,
        -orientation.x as f32,
        orientation.w as f32,
    ];
    let p = [-position.y as f32, position.z as f32, -position.x as f32];
    (q, p)
}

/// OpenXR's frame back to Spatiand's: the inverse of [`to_openxr`], for an orientation.
pub fn from_openxr(orientation: [f32; 4]) -> DQuat {
    let [x, y, z, w] = orientation.map(f64::from);
    DQuat::from_xyzw(-z, -x, y, w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turned_head_survives_the_round_trip() {
        let turned = DQuat::from_rotation_z(0.4) * DQuat::from_rotation_y(-0.2);
        let (q, _) = to_openxr(turned, DVec3::ZERO);
        let back = from_openxr(q);
        // Through f32 on the way, so equal to f32 precision.
        assert!((back - turned).length() < 1e-6, "{back:?} vs {turned:?}");
    }

    #[test]
    fn forward_becomes_negative_z() {
        let (_, p) = to_openxr(DQuat::IDENTITY, DVec3::X);
        assert_eq!(p, [0.0, 0.0, -1.0]);
    }
}
