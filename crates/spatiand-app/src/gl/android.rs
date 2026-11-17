//! Android's pictures of somebody else's: an Android hardware buffer -- a video decoder's picture,
//! YUV in the GPU's own layout -- as a texture to sample, with no copy.
//!
//! The buffer becomes an EGLImage (`EGL_ANDROID_get_native_client_buffer`,
//! `EGL_ANDROID_image_native_buffer`) bound to an external texture (`GL_OES_EGL_image_external`):
//! a shader samples it with `samplerExternalOES`, and the GPU turns its YUV into colour as it
//! reads, by what the buffer says it is -- as Android's own players show a decoder's pictures.

use std::ffi::c_void;

use khronos_egl as egl;

use super::Gl;

/// A texture target that is an image of the GPU's own layout, sampled as colour.
pub const TEXTURE_EXTERNAL_OES: u32 = 0x8D65;

const NATIVE_BUFFER_ANDROID: egl::Enum = 0x3140;
const IMAGE_PRESERVED_KHR: egl::Int = 0x30D2;

type GetClientBuffer = unsafe extern "C" fn(*const c_void) -> *mut c_void;
type CreateImage = unsafe extern "C" fn(*mut c_void, *mut c_void, egl::Enum, *mut c_void, *const egl::Int) -> *mut c_void;
type DestroyImage = unsafe extern "C" fn(*mut c_void, *mut c_void) -> egl::Boolean;
type ImageTarget = unsafe extern "C" fn(u32, *mut c_void);

/// An Android hardware buffer as an external texture ([`TEXTURE_EXTERNAL_OES`]). The buffer is
/// only borrowed, and must outlive the texture.
pub struct HardwareBufferTexture {
    pub texture: glow::Texture,
    image: *mut c_void,
}

impl Gl {
    /// `buffer` (an `AHardwareBuffer*`) as an external texture.
    pub fn hardware_buffer_texture(&self, buffer: *mut c_void) -> Result<HardwareBufferTexture, String> {
        use glow::HasContext;
        let find = |name: &str| self.egl.get_proc_address(name).ok_or_else(|| format!("this EGL has no {name}"));
        // SAFETY: the extensions' functions, by their published shapes.
        let (client_buffer, create, destroy, target): (GetClientBuffer, CreateImage, DestroyImage, ImageTarget) = unsafe {
            (
                std::mem::transmute(find("eglGetNativeClientBufferANDROID")?),
                std::mem::transmute(find("eglCreateImageKHR")?),
                std::mem::transmute(find("eglDestroyImageKHR")?),
                std::mem::transmute(find("glEGLImageTargetTexture2DOES")?),
            )
        };
        unsafe {
            let client = client_buffer(buffer);
            if client.is_null() {
                return Err("a hardware buffer has no EGL client buffer".into());
            }
            let attributes = [IMAGE_PRESERVED_KHR, egl::TRUE as egl::Int, egl::NONE];
            let image = create(self.display.as_ptr(), egl::NO_CONTEXT, NATIVE_BUFFER_ANDROID, client, attributes.as_ptr());
            if image.is_null() {
                return Err(format!("a hardware buffer would not become an image ({:?})", self.egl.get_error()));
            }
            let g = self.api();
            let texture = match g.create_texture() {
                Ok(texture) => texture,
                Err(e) => {
                    destroy(self.display.as_ptr(), image);
                    return Err(e);
                }
            };
            g.bind_texture(TEXTURE_EXTERNAL_OES, Some(texture));
            target(TEXTURE_EXTERNAL_OES, image);
            for (name, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                g.tex_parameter_i32(TEXTURE_EXTERNAL_OES, name, value as i32);
            }
            g.bind_texture(TEXTURE_EXTERNAL_OES, None);
            if g.get_error() != glow::NO_ERROR {
                g.delete_texture(texture);
                destroy(self.display.as_ptr(), image);
                return Err("a hardware buffer would not become a texture".into());
            }
            Ok(HardwareBufferTexture { texture, image })
        }
    }

    pub fn release_hardware_buffer_texture(&self, held: HardwareBufferTexture) {
        use glow::HasContext;
        unsafe { self.api().delete_texture(held.texture) };
        if let Some(destroy) = self.egl.get_proc_address("eglDestroyImageKHR") {
            // SAFETY: as above.
            let destroy: DestroyImage = unsafe { std::mem::transmute(destroy) };
            unsafe { destroy(self.display.as_ptr(), held.image) };
        }
    }
}
