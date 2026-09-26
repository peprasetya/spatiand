//! The compositor's half of Android's remote pictures: a placeholder in, the decoder's picture
//! out.
//!
//! A remote window's buffer on Android is a placeholder the size of its picture, carrying a
//! token (see `spatiand_video::android` and the remote client's `Placeholder`). Here, each frame,
//! the token finds the window's decoder, its newest picture is imported as an `EGLImage`, and
//! that is drawn into an ordinary RGBA texture which [`substitute`] hands the scene in place of
//! the placeholder's. From there on a remote window is drawn exactly as on the Deck: the same
//! quad, frame, title bar and pointer.
//!
//! The copy is one full-screen draw on the GPU, from the decoder's YUV straight to RGB. It exists
//! because the scene's shaders sample `sampler2D` and a decoder's buffer is only reachable as an
//! external texture -- and the draw is also where the decoder's crop is taken off.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;

use smithay::backend::renderer::gles::{ffi, GlesRenderer};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use spatiand_video::android::{lookup, Frame, PLACEHOLDER_MAGIC};

const GL_TEXTURE_EXTERNAL_OES: u32 = 0x8D65;
const EGL_NATIVE_BUFFER_ANDROID: u32 = 0x3140;
const EGL_IMAGE_PRESERVED_KHR: i32 = 0x30D2;
const EGL_NONE: i32 = 0x3038;

/// How many frames a picture taken off screen is kept before it goes back to its decoder, so the
/// GPU has surely finished reading it.
const RETIRE_AFTER: u64 = 3;

type GetNativeClientBuffer = unsafe extern "C" fn(*const ndk_sys::AHardwareBuffer) -> *mut c_void;
type CreateImage = unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *mut c_void, *const i32) -> *mut c_void;
type DestroyImage = unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32;
type GetCurrentDisplay = unsafe extern "C" fn() -> *mut c_void;

/// The EGL calls for Android's buffers, from libEGL itself.
struct Egl {
    client_buffer: GetNativeClientBuffer,
    create_image: CreateImage,
    destroy_image: DestroyImage,
    current_display: GetCurrentDisplay,
}

impl Egl {
    fn load() -> Option<Egl> {
        unsafe {
            let lib = libc::dlopen(c"libEGL.so".as_ptr(), libc::RTLD_NOW | libc::RTLD_NOLOAD);
            let lib = if lib.is_null() { libc::dlopen(c"libEGL.so".as_ptr(), libc::RTLD_NOW) } else { lib };
            if lib.is_null() {
                return None;
            }
            let find = |name: &std::ffi::CStr| {
                let f = libc::dlsym(lib, name.as_ptr());
                (!f.is_null()).then_some(f)
            };
            Some(Egl {
                client_buffer: std::mem::transmute::<*mut c_void, GetNativeClientBuffer>(find(c"eglGetNativeClientBufferANDROID")?),
                create_image: std::mem::transmute::<*mut c_void, CreateImage>(find(c"eglCreateImageKHR")?),
                destroy_image: std::mem::transmute::<*mut c_void, DestroyImage>(find(c"eglDestroyImageKHR")?),
                current_display: std::mem::transmute::<*mut c_void, GetCurrentDisplay>(find(c"eglGetCurrentDisplay")?),
            })
        }
    }
}

/// A picture on screen, and the image made of it.
struct Showing {
    frame: Frame,
    image: *mut c_void,
}

/// What one remote window's picture is drawn into.
struct Slot {
    /// The RGBA texture the scene draws, and the framebuffer that fills it.
    texture: u32,
    framebuffer: u32,
    size: (u32, u32),
    /// The external texture the decoder's buffer is bound to.
    external: u32,
    showing: Option<Showing>,
    /// Frames this slot was asked for last, so one nobody asks for any more can go.
    last_used: u64,
}

struct Blit {
    program: u32,
    vao: u32,
    crop: i32,
    source: i32,
}

#[derive(Default)]
struct Pictures {
    egl: Option<Egl>,
    tried_egl: bool,
    blit: Option<Blit>,
    slots: HashMap<u32, Slot>,
    retired: VecDeque<(Showing, u64)>,
    frame: u64,
}

thread_local! {
    // Everything here is GL state, which belongs to the compositor's thread and nowhere else.
    static PICTURES: RefCell<Pictures> = RefCell::new(Pictures::default());
}

/// The token in a surface's buffer, if its buffer is a placeholder.
fn token_of(surface: &WlSurface) -> Option<u32> {
    use smithay::backend::renderer::utils::with_renderer_surface_state;
    let buffer = with_renderer_surface_state(surface, |st| st.buffer().cloned()).flatten()?;
    smithay::wayland::shm::with_buffer_contents(&buffer, |ptr, len, data| {
        let at = data.offset as usize;
        if len < at + 8 {
            return None;
        }
        let head = unsafe { std::slice::from_raw_parts(ptr.add(at), 8) };
        let magic = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
        let token = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
        (magic == PLACEHOLDER_MAGIC).then_some(token)
    })
    .ok()
    .flatten()
}

/// The texture to draw for this surface: its decoder's newest picture if it is a remote
/// window's placeholder, and `texture` -- its own -- otherwise.
pub fn substitute(renderer: &mut GlesRenderer, surface: &WlSurface, texture: u32) -> u32 {
    let Some(token) = token_of(surface) else {
        return texture;
    };
    let Some(output) = lookup(token) else {
        return texture;
    };
    PICTURES.with(|pictures| {
        let mut pictures = pictures.borrow_mut();
        let pictures = &mut *pictures;
        if !pictures.tried_egl {
            pictures.tried_egl = true;
            pictures.egl = Egl::load();
            if pictures.egl.is_none() {
                log::error!("remote pictures: libEGL has no Android buffer import; windows stay blank");
            }
        }
        let Some(egl) = pictures.egl.as_ref() else {
            return texture;
        };
        let frame_no = pictures.frame;
        let fresh = output.take_latest();
        let drawn = renderer.with_context(|gl| unsafe {
            if pictures.blit.is_none() {
                pictures.blit = compile(gl);
            }
            let Some(blit) = pictures.blit.as_ref() else {
                return None;
            };
            let slot = pictures.slots.entry(token).or_insert_with(|| {
                let mut ids = [0u32; 2];
                gl.GenTextures(2, ids.as_mut_ptr());
                let mut framebuffer = 0;
                gl.GenFramebuffers(1, &mut framebuffer);
                Slot {
                    texture: ids[0],
                    framebuffer,
                    size: (0, 0),
                    external: ids[1],
                    showing: None,
                    last_used: frame_no,
                }
            });
            slot.last_used = frame_no;
            if let Some(frame) = fresh {
                let buffer = (egl.client_buffer)(frame.buffer);
                let display = (egl.current_display)();
                let attributes = [EGL_IMAGE_PRESERVED_KHR, 1, EGL_NONE];
                let image = if buffer.is_null() {
                    std::ptr::null_mut()
                } else {
                    (egl.create_image)(display, std::ptr::null_mut(), EGL_NATIVE_BUFFER_ANDROID, buffer, attributes.as_ptr())
                };
                if image.is_null() {
                    log::warn!("remote pictures: a picture could not be imported");
                } else {
                    if slot.size != frame.size {
                        slot.size = frame.size;
                        gl.BindTexture(ffi::TEXTURE_2D, slot.texture);
                        gl.TexImage2D(
                            ffi::TEXTURE_2D,
                            0,
                            ffi::RGBA8 as i32,
                            frame.size.0 as i32,
                            frame.size.1 as i32,
                            0,
                            ffi::RGBA,
                            ffi::UNSIGNED_BYTE,
                            std::ptr::null(),
                        );
                        for (name, value) in [
                            (ffi::TEXTURE_MIN_FILTER, ffi::LINEAR),
                            (ffi::TEXTURE_MAG_FILTER, ffi::LINEAR),
                            (ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE),
                            (ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE),
                        ] {
                            gl.TexParameteri(ffi::TEXTURE_2D, name, value as i32);
                        }
                        gl.BindFramebuffer(ffi::FRAMEBUFFER, slot.framebuffer);
                        gl.FramebufferTexture2D(ffi::FRAMEBUFFER, ffi::COLOR_ATTACHMENT0, ffi::TEXTURE_2D, slot.texture, 0);
                    }
                    gl.BindTexture(GL_TEXTURE_EXTERNAL_OES, slot.external);
                    gl.EGLImageTargetTexture2DOES(GL_TEXTURE_EXTERNAL_OES, image);
                    gl.TexParameteri(GL_TEXTURE_EXTERNAL_OES, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
                    gl.TexParameteri(GL_TEXTURE_EXTERNAL_OES, ffi::TEXTURE_MAG_FILTER, ffi::LINEAR as i32);

                    // Row 0 of the result is the picture's top row, as a client's buffer is.
                    gl.BindFramebuffer(ffi::FRAMEBUFFER, slot.framebuffer);
                    gl.Viewport(0, 0, slot.size.0 as i32, slot.size.1 as i32);
                    gl.Disable(ffi::BLEND);
                    gl.Disable(ffi::SCISSOR_TEST);
                    gl.UseProgram(blit.program);
                    gl.BindVertexArray(blit.vao);
                    gl.ActiveTexture(ffi::TEXTURE0);
                    gl.BindTexture(GL_TEXTURE_EXTERNAL_OES, slot.external);
                    gl.Uniform1i(blit.source, 0);
                    let c = frame.crop;
                    gl.Uniform4f(blit.crop, c[0], c[1], c[2], c[3]);
                    gl.DrawArrays(ffi::TRIANGLES, 0, 3);
                    gl.BindVertexArray(0);
                    gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
                    gl.BindTexture(GL_TEXTURE_EXTERNAL_OES, 0);

                    if let Some(old) = slot.showing.replace(Showing { frame, image }) {
                        pictures.retired.push_back((old, frame_no));
                    }
                }
            }
            (slot.size != (0, 0)).then_some(slot.texture)
        });
        drawn.ok().flatten().unwrap_or(texture)
    })
}

/// Once a frame, after it is drawn: pictures off screen long enough go back to their decoders,
/// and windows that are gone give their textures up.
pub fn end_frame(renderer: &mut GlesRenderer) {
    PICTURES.with(|pictures| {
        let mut pictures = pictures.borrow_mut();
        let pictures = &mut *pictures;
        pictures.frame += 1;
        let now = pictures.frame;
        let Some(egl) = pictures.egl.as_ref() else { return };
        let display = unsafe { (egl.current_display)() };
        while pictures.retired.front().is_some_and(|(_, at)| now >= at + RETIRE_AFTER) {
            let (showing, _) = pictures.retired.pop_front().unwrap();
            unsafe { (egl.destroy_image)(display, showing.image) };
            drop(showing.frame);
        }
        let stale: Vec<u32> = pictures
            .slots
            .iter()
            .filter(|(token, slot)| lookup(**token).is_none() && now > slot.last_used + RETIRE_AFTER)
            .map(|(token, _)| *token)
            .collect();
        for token in stale {
            if let Some(slot) = pictures.slots.remove(&token) {
                if let Some(showing) = slot.showing {
                    pictures.retired.push_back((showing, now));
                }
                let _ = renderer.with_context(|gl| unsafe {
                    gl.DeleteTextures(1, &slot.texture);
                    gl.DeleteTextures(1, &slot.external);
                    gl.DeleteFramebuffers(1, &slot.framebuffer);
                });
            }
        }
    });
}

const BLIT_VS: &str = "#version 300 es
out vec2 v_uv;
void main() {
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    v_uv = p;
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
";

const BLIT_FS: &str = "#version 300 es
#extension GL_OES_EGL_image_external_essl3 : require
precision mediump float;
in vec2 v_uv;
uniform samplerExternalOES u_source;
// (u0, v0, u1, v1): the part of the decoder's buffer that is picture.
uniform vec4 u_crop;
out vec4 f_color;
void main() {
    // v_uv.y is 0 on row 0 of the target, which is to hold the picture's top row.
    vec2 uv = vec2(mix(u_crop.x, u_crop.z, v_uv.x), mix(u_crop.y, u_crop.w, v_uv.y));
    f_color = vec4(texture(u_source, uv).rgb, 1.0);
}
";

unsafe fn compile(gl: &ffi::Gles2) -> Option<Blit> {
    let shader = |kind: u32, source: &str| -> Option<u32> {
        let id = gl.CreateShader(kind);
        let text = std::ffi::CString::new(source).ok()?;
        gl.ShaderSource(id, 1, &text.as_ptr(), std::ptr::null());
        gl.CompileShader(id);
        let mut ok = 0;
        gl.GetShaderiv(id, ffi::COMPILE_STATUS, &mut ok);
        if ok == 0 {
            let mut log = vec![0u8; 1024];
            let mut len = 0;
            gl.GetShaderInfoLog(id, 1024, &mut len, log.as_mut_ptr() as *mut _);
            log::error!("remote pictures: shader: {}", String::from_utf8_lossy(&log[..len.max(0) as usize]));
            return None;
        }
        Some(id)
    };
    let vs = shader(ffi::VERTEX_SHADER, BLIT_VS)?;
    let fs = shader(ffi::FRAGMENT_SHADER, BLIT_FS)?;
    let program = gl.CreateProgram();
    gl.AttachShader(program, vs);
    gl.AttachShader(program, fs);
    gl.LinkProgram(program);
    gl.DeleteShader(vs);
    gl.DeleteShader(fs);
    let mut ok = 0;
    gl.GetProgramiv(program, ffi::LINK_STATUS, &mut ok);
    if ok == 0 {
        log::error!("remote pictures: the copy would not link");
        return None;
    }
    let mut vao = 0;
    gl.GenVertexArrays(1, &mut vao);
    Some(Blit {
        program,
        vao,
        crop: gl.GetUniformLocation(program, c"u_crop".as_ptr()),
        source: gl.GetUniformLocation(program, c"u_source".as_ptr()),
    })
}
