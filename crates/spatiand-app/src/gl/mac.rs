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
    static kCFBooleanTrue: CFTypeRef;
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
    static kIOSurfaceIsGlobal: CFTypeRef;
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
            CFDictionarySetValue(properties, kIOSurfaceIsGlobal, kCFBooleanTrue);
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

    /// What the window's placeholder says to name this picture.
    pub(super) fn name(&self, stream: u64) -> [u8; 16] {
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
