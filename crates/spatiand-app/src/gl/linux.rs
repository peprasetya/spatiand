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
            // A GL with no GPU (Mesa's software one) says yes and hands over no descriptor:
            // its memory is not a dmabuf. The picture goes as shared memory instead.
            if fds[..count as usize].iter().any(|&fd| fd < 0) {
                for &fd in fds[..count as usize].iter().filter(|&&fd| fd >= 0) {
                    libc::close(fd);
                }
                return fail("the texture's memory has no descriptor to hand over");
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

/// Somebody else's dmabuf -- one plane of a decoder's picture -- as a texture to sample, with
/// no copy.
pub struct DmabufTexture {
    pub texture: glow::Texture,
    image: egl::Image,
}

const LINUX_DMA_BUF_EXT: egl::Enum = 0x3270;
const LINUX_DRM_FOURCC_EXT: egl::Attrib = 0x3271;
const DMA_BUF_PLANE0_FD_EXT: egl::Attrib = 0x3272;
const DMA_BUF_PLANE0_OFFSET_EXT: egl::Attrib = 0x3273;
const DMA_BUF_PLANE0_PITCH_EXT: egl::Attrib = 0x3274;
const DMA_BUF_PLANE0_MODIFIER_LO_EXT: egl::Attrib = 0x3443;
const DMA_BUF_PLANE0_MODIFIER_HI_EXT: egl::Attrib = 0x3444;
/// "No modifier was said", which is not the same as a modifier of nought.
pub const NO_MODIFIER: u64 = 0x00ff_ffff_ffff_ffff;

type ImageTarget = unsafe extern "C" fn(u32, *mut c_void);

impl Gl {
    /// One plane of a dmabuf as a texture: `fourcc` says what a pixel of it is (`R8  ` for a
    /// picture's brightness, `GR88` for its two colour differences side by side). The
    /// descriptor is only borrowed, and must outlive the texture.
    pub fn dmabuf_texture(&self, fd: std::os::fd::BorrowedFd<'_>, offset: u32, pitch: u32, modifier: u64, size: (u32, u32), fourcc: u32) -> Result<DmabufTexture, String> {
        use glow::HasContext;
        use std::os::fd::AsRawFd;
        let modern = self.egl.upcast::<egl::EGL1_5>().ok_or("EGL is older than 1.5")?;
        let target = self.egl.get_proc_address("glEGLImageTargetTexture2DOES").ok_or("this GL cannot take an image as a texture")?;
        // SAFETY: the one function of GL_OES_EGL_image, by its published shape.
        let target: ImageTarget = unsafe { std::mem::transmute(target) };
        let mut attributes: Vec<egl::Attrib> = vec![
            egl::WIDTH as egl::Attrib, size.0 as egl::Attrib,
            egl::HEIGHT as egl::Attrib, size.1 as egl::Attrib,
            LINUX_DRM_FOURCC_EXT, fourcc as egl::Attrib,
            DMA_BUF_PLANE0_FD_EXT, fd.as_raw_fd() as egl::Attrib,
            DMA_BUF_PLANE0_OFFSET_EXT, offset as egl::Attrib,
            DMA_BUF_PLANE0_PITCH_EXT, pitch as egl::Attrib,
        ];
        if modifier != NO_MODIFIER {
            attributes.extend([DMA_BUF_PLANE0_MODIFIER_LO_EXT, (modifier & 0xffff_ffff) as egl::Attrib, DMA_BUF_PLANE0_MODIFIER_HI_EXT, (modifier >> 32) as egl::Attrib]);
        }
        attributes.push(egl::ATTRIB_NONE);
        unsafe {
            let image = modern
                .create_image(self.display, egl::Context::from_ptr(egl::NO_CONTEXT), LINUX_DMA_BUF_EXT, egl::ClientBuffer::from_ptr(std::ptr::null_mut()), &attributes)
                .map_err(|e| format!("a decoder's picture would not be imported: {e}"))?;
            let g = self.api();
            let texture = match g.create_texture() {
                Ok(texture) => texture,
                Err(e) => {
                    let _ = modern.destroy_image(self.display, image);
                    return Err(e);
                }
            };
            g.bind_texture(glow::TEXTURE_2D, Some(texture));
            target(glow::TEXTURE_2D, image.as_ptr());
            for (name, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                g.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
            }
            if g.get_error() != glow::NO_ERROR {
                g.delete_texture(texture);
                let _ = modern.destroy_image(self.display, image);
                return Err("a decoder's picture would not become a texture".into());
            }
            Ok(DmabufTexture { texture, image })
        }
    }

    pub fn release_dmabuf_texture(&self, held: DmabufTexture) {
        use glow::HasContext;
        unsafe { self.api().delete_texture(held.texture) };
        if let Some(modern) = self.egl.upcast::<egl::EGL1_5>() {
            let _ = modern.destroy_image(self.display, held.image);
        }
    }
}
