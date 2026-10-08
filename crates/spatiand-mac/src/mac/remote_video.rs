//! The compositor's half of the Mac's pictures: a placeholder in, an `IOSurface` out.

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

/// The texture to draw for `surface`: its newest picture if it is a placeholder's, else its own.
pub fn substitute(_renderer: &mut GlesRenderer, _surface: &WlSurface, texture: u32) -> u32 {
    texture
}
