//! The head-locked panel a message is shown on: `backend_drm`'s two helpers, which is a file
//! that is DRM from end to end and cannot be built here.

use glam::{DQuat, Mat4, Vec3};


const PANEL_DISTANCE: f32 = 1.4;

/// See `backend_drm::fit_panel`.
pub fn fit_panel(aspect: f32, h_fov_deg: f64, v_fov_deg: f64, portrait: bool) -> (f32, f32) {
    let (fov_for_width, fov_for_height) = if portrait { (v_fov_deg, h_fov_deg) } else { (h_fov_deg, v_fov_deg) };
    let usable = 0.68;
    let extent = |fov: f64| 2.0 * PANEL_DISTANCE * ((fov * usable / 2.0).to_radians().tan() as f32);
    let max_w = extent(fov_for_width);
    let max_h = extent(fov_for_height);
    let aspect = aspect.max(0.01);
    let width = max_w.min(max_h * aspect);
    (width, width / aspect)
}

/// See `backend_drm::head_locked_panel_sized`.
pub fn head_locked_panel_sized(orientation: DQuat, width: f32, height: f32, portrait: bool) -> Mat4 {
    let cfg = spatiand_render::StereoConfig::default();
    let centre = Vec3::new((cfg.neck_forward_m as f32) + PANEL_DISTANCE, 0.0, cfg.neck_up_m as f32);
    let basis = Mat4::from_cols(
        (-Vec3::Y * width).extend(0.0),
        (Vec3::Z * height).extend(0.0),
        Vec3::X.extend(0.0),
        centre.extend(1.0),
    );
    let roll = if portrait { Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2) } else { Mat4::IDENTITY };
    Mat4::from_quat(orientation.as_quat()) * roll * basis
}

/// Android draws the phone's touch area here. The Mac has no second screen of its own to draw.
///
/// # Safety
/// Never called: the Mac's `Shared::phone` holds no window.
pub unsafe fn draw(_gl: &smithay::backend::renderer::gles::ffi::Gles2, _size: (u32, u32)) {}
