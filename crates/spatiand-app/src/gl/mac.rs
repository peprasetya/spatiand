//! A Mac's way of handing a picture over: an `IOSurface`, found by its number.
//!
//! The picture is drawn straight into an `IOSurface` -- ANGLE wraps one as a pbuffer, and a
//! pbuffer can be a texture's image, which a framebuffer can draw into. The compositor is
//! another process, so the surface is made *global*: one any process of this user's can open
//! by its number, which is what goes in the window's placeholder (see `Wire::show_named`, and
//! `crates/spatiand-mac/src/mac/remote_video.rs` for the other end). Nothing is copied.
//!
//! Which picture may be drawn into next is the surface's own *use count*, which the
//! compositor raises on what it is showing and which is seen across processes: the same
//! arrangement a screen capture's pool and a decoder's already have with it.

use std::ffi::c_void;

use khronos_egl as egl;

use super::Gl;

use spatiand_proto::handover::MAC_SURFACE_MAGIC;

const IOSURFACE_ANGLE: egl::Enum = 0x3454;
const IOSURFACE_PLANE_ANGLE: egl::Int = 0x345A;
const TEXTURE_TYPE_ANGLE: egl::Int = 0x345C;
const TEXTURE_INTERNAL_FORMAT_ANGLE: egl::Int = 0x345D;
const GL_BGRA_EXT: egl::Int = 0x80E1;

type CFTypeRef = *const c_void;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    fn CFDictionaryCreateMutable(allocator: CFTypeRef, capacity: isize, keys: *const c_void, values: *const c_void) -> CFTypeRef;
    fn CFDictionarySetValue(dictionary: CFTypeRef, key: CFTypeRef, value: CFTypeRef);
    fn CFNumberCreate(allocator: CFTypeRef, kind: isize, value: *const c_void) -> CFTypeRef;
    fn CFRelease(object: CFTypeRef);
}

#[link(name = "IOSurface", kind = "framework")]
extern "C" {
    static kIOSurfaceWidth: CFTypeRef;
    static kIOSurfaceHeight: CFTypeRef;
    static kIOSurfaceBytesPerElement: CFTypeRef;
    static kIOSurfacePixelFormat: CFTypeRef;
    // Deprecated since 10.11 and still what it says: the only way one process's surface is
    // opened by another without a Mach port passed between them. Checked on macOS 26.
    fn IOSurfaceCreate(properties: CFTypeRef) -> CFTypeRef;
    fn IOSurfaceGetID(surface: CFTypeRef) -> u32;
    fn IOSurfaceIsInUse(surface: CFTypeRef) -> u8;
}

/// One picture: the surface, and the pbuffer that makes it a texture's image.
pub(super) struct Shared {
    surface: CFTypeRef,
    pbuffer: egl::Surface,
}

impl Shared {
    /// Make a surface of this size and make it the image of the texture now bound.
    pub(super) fn new(gl: &Gl, size: (u32, u32)) -> Result<Shared, String> {
        unsafe {
            let properties = CFDictionaryCreateMutable(
                std::ptr::null(),
                0,
                &kCFTypeDictionaryKeyCallBacks as *const c_void,
                &kCFTypeDictionaryValueCallBacks as *const c_void,
            );
            let number = |value: i32| {
                const INT32: isize = 3;
                CFNumberCreate(std::ptr::null(), INT32, &value as *const i32 as *const c_void)
            };
            for (key, value) in [
                (kIOSurfaceWidth, size.0 as i32),
                (kIOSurfaceHeight, size.1 as i32),
                (kIOSurfaceBytesPerElement, 4),
                (kIOSurfacePixelFormat, i32::from_be_bytes(*b"BGRA")),
            ] {
                let value = number(value);
                CFDictionarySetValue(properties, key, value);
                CFRelease(value);
            }
            // Not global, which any process could open by its number: Spatiand is handed it in
            // private each time it is shown (spatiand-surfaces).
            let surface = IOSurfaceCreate(properties);
            CFRelease(properties);
            if surface.is_null() {
                return Err("macOS would not make a surface".into());
            }
            let attributes = [
                egl::WIDTH, size.0 as egl::Int,
                egl::HEIGHT, size.1 as egl::Int,
                IOSURFACE_PLANE_ANGLE, 0,
                egl::TEXTURE_TARGET, egl::TEXTURE_2D,
                TEXTURE_INTERNAL_FORMAT_ANGLE, GL_BGRA_EXT,
                egl::TEXTURE_FORMAT, egl::TEXTURE_RGBA,
                TEXTURE_TYPE_ANGLE, glow::UNSIGNED_BYTE as egl::Int,
                egl::NONE,
            ];
            let pbuffer = gl.egl.create_pbuffer_from_client_buffer(
                gl.display,
                IOSURFACE_ANGLE,
                egl::ClientBuffer::from_ptr(surface as *mut c_void),
                gl.config,
                &attributes,
            );
            let pbuffer = match pbuffer {
                Ok(pbuffer) => pbuffer,
                Err(e) => {
                    CFRelease(surface);
                    return Err(format!("ANGLE would not wrap a surface: {e}"));
                }
            };
            if let Err(e) = gl.egl.bind_tex_image(gl.display, pbuffer, egl::BACK_BUFFER) {
                let _ = gl.egl.destroy_surface(gl.display, pbuffer);
                CFRelease(surface);
                return Err(format!("a surface would not become a texture: {e}"));
            }
            Ok(Shared { surface, pbuffer })
        }
    }

    /// Whether anybody -- the compositor -- is showing this picture.
    pub(super) fn in_use(&self) -> bool {
        unsafe { IOSurfaceIsInUse(self.surface) != 0 }
    }

    /// What the window's placeholder says to name this picture, the picture itself handed to
    /// Spatiand by that number just before.
    pub(super) fn name(&self, stream: u64) -> [u8; 16] {
        // SAFETY: the surface is this picture's, alive while it is.
        unsafe { spatiand_surfaces::post(self.surface, IOSurfaceGetID(self.surface)) };
        let mut name = [0u8; 16];
        name[..4].copy_from_slice(&MAC_SURFACE_MAGIC.to_le_bytes());
        name[4..8].copy_from_slice(&unsafe { IOSurfaceGetID(self.surface) }.to_le_bytes());
        name[8..].copy_from_slice(&stream.to_le_bytes());
        name
    }

    pub(super) fn destroy(self, gl: &Gl) {
        unsafe {
            let _ = gl.egl.release_tex_image(gl.display, self.pbuffer, egl::BACK_BUFFER);
            let _ = gl.egl.destroy_surface(gl.display, self.pbuffer);
            CFRelease(self.surface);
        }
    }
}

/// Somebody else's `IOSurface` -- a decoder's picture -- as a texture to sample, with no copy.
pub struct SurfaceTexture {
    pub texture: glow::Texture,
    pbuffer: egl::Surface,
}

impl Gl {
    /// One plane of an `IOSurface` as a texture: `channels` is 1 for a plane of single bytes
    /// (a picture's brightness), 2 for pairs (its two colour differences, interleaved), 4 for
    /// a whole BGRA picture. The surface must outlive the texture.
    ///
    /// # Safety
    /// `surface` must be a live `IOSurfaceRef` with such a plane of that size.
    pub unsafe fn surface_texture(&self, surface: *const c_void, plane: u32, size: (u32, u32), channels: u8) -> Result<SurfaceTexture, String> {
        use glow::HasContext;
        let (format, texture_format) = match channels {
            1 => (glow::RED as egl::Int, egl::TEXTURE_RGBA),
            2 => (glow::RG as egl::Int, egl::TEXTURE_RGBA),
            _ => (GL_BGRA_EXT, egl::TEXTURE_RGBA),
        };
        let attributes = [
            egl::WIDTH, size.0 as egl::Int,
            egl::HEIGHT, size.1 as egl::Int,
            IOSURFACE_PLANE_ANGLE, plane as egl::Int,
            egl::TEXTURE_TARGET, egl::TEXTURE_2D,
            TEXTURE_INTERNAL_FORMAT_ANGLE, format,
            egl::TEXTURE_FORMAT, texture_format,
            TEXTURE_TYPE_ANGLE, glow::UNSIGNED_BYTE as egl::Int,
            egl::NONE,
        ];
        let pbuffer = self
            .egl
            .create_pbuffer_from_client_buffer(self.display, IOSURFACE_ANGLE, egl::ClientBuffer::from_ptr(surface as *mut c_void), self.config, &attributes)
            .map_err(|e| format!("ANGLE would not wrap a decoder's surface: {e}"))?;
        let g = self.api();
        let texture = g.create_texture()?;
        g.bind_texture(glow::TEXTURE_2D, Some(texture));
        if let Err(e) = self.egl.bind_tex_image(self.display, pbuffer, egl::BACK_BUFFER) {
            g.delete_texture(texture);
            let _ = self.egl.destroy_surface(self.display, pbuffer);
            return Err(format!("a decoder's surface would not become a texture: {e}"));
        }
        for (name, value) in [
            (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
            (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        ] {
            g.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
        }
        Ok(SurfaceTexture { texture, pbuffer })
    }

    pub fn release_surface_texture(&self, held: SurfaceTexture) {
        use glow::HasContext;
        unsafe {
            let _ = self.egl.release_tex_image(self.display, held.pbuffer, egl::BACK_BUFFER);
            let _ = self.egl.destroy_surface(self.display, held.pbuffer);
            self.api().delete_texture(held.texture);
        }
    }
}
