//! The phone's touch area: the left pad, and nothing drawn on it but its ground.
//!
//! It showed the machine's monitors for a while, as the Deck's sidecar does. Taken out on
//! request: under a thumb that is scrolling, nothing needs reading.

use smithay::backend::renderer::gles::ffi;

const GROUND: [f32; 4] = [0.035, 0.04, 0.055, 1.0];

/// Fill the touch area, `size` pixels.
///
/// # Safety
/// The context must be current, with the target bound.
pub unsafe fn draw(gl: &ffi::Gles2, size: (u32, u32)) {
    gl.Viewport(0, 0, size.0 as i32, size.1 as i32);
    gl.ClearColor(GROUND[0], GROUND[1], GROUND[2], GROUND[3]);
    gl.Clear(ffi::COLOR_BUFFER_BIT);
}
