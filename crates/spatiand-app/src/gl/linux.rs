//! The Deck's way of handing a picture over: a dmabuf.
//!
//! The GL context is on no window system at all -- Mesa gives one on a bare render node -- and
//! each picture is an ordinary texture, exported as a dmabuf (`EGL_MESA_image_dma_buf_export`)
//! and given to the compositor through `zwp_linux_dmabuf_v1`, which is how every other client's
//! pictures arrive there. Nothing is copied.

use std::ffi::c_void;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};

use khronos_egl as egl;

use super::{Egl, Gl};
use crate::wire::Wire;
use crate::WindowId;

const PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31DD;
const GL_TEXTURE_2D_KHR: egl::Enum = 0x30B1;

/// A display with no window system behind it, where Mesa has one; the default otherwise.
pub(super) fn display(egl: &Egl) -> Result<egl::Display, String> {
    if let Some(modern) = egl.upcast::<egl::EGL1_5>() {
        let surfaceless =
            unsafe { modern.get_platform_display(PLATFORM_SURFACELESS_MESA, egl::DEFAULT_DISPLAY, &[egl::ATTRIB_NONE]) };
        if let Ok(display) = surfaceless {
            return Ok(display);
        }
    }
    unsafe { egl.get_display(egl::DEFAULT_DISPLAY) }.ok_or_else(|| "EGL has no display".to_string())
}

type QueryExport = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut i32, *mut i32, *mut u64) -> u32;
type Export = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut i32, *mut i32, *mut i32) -> u32;

/// One picture: the texture's memory, as descriptors the compositor can open.
pub(super) struct Shared {
    image: egl::Image,
    fourcc: u32,
    modifier: u64,
    planes: Vec<(OwnedFd, u32, u32)>,
}

impl Shared {
    /// Export `texture`, which is bound and has its storage.
    pub(super) fn new(gl: &Gl, texture: glow::Texture, _size: (u32, u32)) -> Result<Shared, String> {
        let modern = gl.egl.upcast::<egl::EGL1_5>().ok_or("EGL is older than 1.5")?;
        let context = gl.egl.get_current_context().ok_or("no current context")?;
        let query = gl.egl.get_proc_address("eglExportDMABUFImageQueryMESA").ok_or("no dmabuf export")?;
        let export = gl.egl.get_proc_address("eglExportDMABUFImageMESA").ok_or("no dmabuf export")?;
        // SAFETY: the two functions of EGL_MESA_image_dma_buf_export, by their published shapes.
        let (query, export): (QueryExport, Export) = unsafe { (std::mem::transmute(query), std::mem::transmute(export)) };
        unsafe {
            let image = modern
                .create_image(
                    gl.display,
                    context,
                    GL_TEXTURE_2D_KHR,
                    egl::ClientBuffer::from_ptr(texture.0.get() as usize as *mut c_void),
                    &[egl::ATTRIB_NONE],
                )
                .map_err(|e| format!("a texture would not become an image: {e}"))?;
            let fail = |what: &str| {
                let _ = modern.destroy_image(gl.display, image);
                Err(what.to_string())
            };
            let (mut fourcc, mut count, mut modifier) = (0i32, 0i32, 0u64);
            if query(gl.display.as_ptr(), image.as_ptr(), &mut fourcc, &mut count, &mut modifier) == 0 || count < 1 || count > 4 {
                return fail("the texture's memory could not be described");
            }
            let (mut fds, mut strides, mut offsets) = ([-1i32; 4], [0i32; 4], [0i32; 4]);
            if export(gl.display.as_ptr(), image.as_ptr(), fds.as_mut_ptr(), strides.as_mut_ptr(), offsets.as_mut_ptr()) == 0 {
                return fail("the texture's memory could not be exported");
            }
            let planes = (0..count as usize)
                .map(|i| (OwnedFd::from_raw_fd(fds[i]), offsets[i] as u32, strides[i] as u32))
                .collect();
            Ok(Shared { image, fourcc: fourcc as u32, modifier, planes })
        }
    }

    pub(super) fn show(&self, wire: &mut Wire, window: WindowId, key: u32, size: (u32, u32)) -> Result<(), String> {
        if !wire.takes_dmabufs() {
            return Err("the compositor takes no dmabufs".into());
        }
        let planes: Vec<_> = self.planes.iter().map(|(fd, offset, stride)| (fd.as_fd(), *offset, *stride)).collect();
        wire.show_dmabuf(window, key, size, self.fourcc, self.modifier, &planes)
    }

    pub(super) fn destroy(self, gl: &Gl) {
        if let Some(modern) = gl.egl.upcast::<egl::EGL1_5>() {
            let _ = modern.destroy_image(gl.display, self.image);
        }
    }
}
