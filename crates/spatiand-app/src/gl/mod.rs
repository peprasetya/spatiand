//! GL for an application, and the handing over of what it drew.
//!
//! An application draws with OpenGL ES 3 into a picture this module owns, and the picture goes
//! to the compositor in whichever way the platform has for it without a copy: an `IOSurface`
//! found by its number on a Mac ([`mac`]), a dmabuf on the Deck ([`linux`]). Where there is no
//! such way yet, or it fails, the picture is read back and sent as ordinary memory -- slower,
//! and the same picture -- so an application never has to know which it got.
//!
//! **Row 0 of a target is the top of the picture.** GL calls that row the bottom; nothing here
//! flips anything, so whatever draws into a target uses a projection with y downwards (as
//! [`crate::ui::Painter`] does) and the rows are already in the order every handover wants.

use std::ffi::c_void;

use glow::HasContext;
use khronos_egl as egl;

use crate::wire::Wire;
use crate::WindowId;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
pub use mac::SurfaceTexture;
#[cfg(target_os = "linux")]
pub use linux::DmabufTexture;

pub(crate) type Egl = egl::DynamicInstance<egl::EGL1_4>;

/// One GL context for the whole application, current on the thread that made it.
pub struct Gl {
    pub(crate) egl: Egl,
    pub(crate) display: egl::Display,
    pub(crate) config: egl::Config,
    context: egl::Context,
    /// What the context is current on when no target needs it to be on anything else.
    idle: Option<egl::Surface>,
    glow: glow::Context,
}

/// Where the platform keeps its EGL.
fn library() -> Result<std::path::PathBuf, String> {
    if let Some(named) = std::env::var_os("SPATIAND_EGL") {
        return Ok(named.into());
    }
    #[cfg(target_os = "macos")]
    {
        // ANGLE, which the Spatiand app carries: beside the program, in its bundle, or -- a
        // development build -- where `mac/fetch-angle.sh` put it.
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = exe.parent().unwrap_or(std::path::Path::new("."));
        let candidates = [
            dir.join("libEGL.dylib"),
            dir.join("../Frameworks/libEGL.dylib"),
            dir.join("../../mac/vendor/angle/libEGL.dylib"),
            dir.join("../../../mac/vendor/angle/libEGL.dylib"),
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../mac/vendor/angle/libEGL.dylib")),
        ];
        candidates
            .into_iter()
            .find(|p| p.exists())
            .ok_or_else(|| "ANGLE was not found: run mac/fetch-angle.sh, or set SPATIAND_EGL".to_string())
    }
    #[cfg(target_os = "android")]
    {
        Ok("libEGL.so".into())
    }
    #[cfg(not(any(target_os = "macos", target_os = "android")))]
    {
        Ok("libEGL.so.1".into())
    }
}

impl Gl {
    /// A context of its own, with no window anywhere: what [`crate::App::gl`] makes, and what a
    /// tool that only wants to draw into memory can make without a room to connect to.
    pub fn new() -> Result<Gl, String> {
        let path = library()?;
        // SAFETY: the platform's EGL, or ANGLE's, loaded once and kept for the process's life.
        let egl: Egl = unsafe { Egl::load_required_from_filename(&path) }
            .map_err(|e| format!("could not load EGL from {}: {e}", path.display()))?;
        let display = Self::display(&egl)?;
        let (major, minor) = egl.initialize(display).map_err(|e| format!("EGL would not start: {e}"))?;
        egl.bind_api(egl::OPENGL_ES_API).map_err(|e| format!("no OpenGL ES: {e}"))?;

        const OPENGL_ES3_BIT: egl::Int = 0x40;
        let wanted = |surface: egl::Int| {
            [
                egl::RENDERABLE_TYPE, OPENGL_ES3_BIT,
                egl::SURFACE_TYPE, surface,
                egl::RED_SIZE, 8,
                egl::GREEN_SIZE, 8,
                egl::BLUE_SIZE, 8,
                egl::ALPHA_SIZE, 8,
                egl::NONE,
            ]
        };
        // A pbuffer where there are pbuffers; a display with no surfaces at all (Mesa's, on a
        // render node) has configs with none, and a context that needs none.
        let config = egl
            .choose_first_config(display, &wanted(egl::PBUFFER_BIT))
            .ok()
            .flatten()
            .or_else(|| egl.choose_first_config(display, &wanted(0)).ok().flatten())
            .ok_or("EGL has no configuration for OpenGL ES 3")?;
        let context = egl
            .create_context(display, config, None, &[egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE])
            .map_err(|e| format!("no OpenGL ES 3 context: {e}"))?;
        let idle = egl.create_pbuffer_surface(display, config, &[egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE]).ok();
        egl.make_current(display, idle, idle, Some(context))
            .map_err(|e| format!("the GL context would not be made current: {e}"))?;
        // SAFETY: the context is current, and stays this thread's.
        let glow = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name).map_or(std::ptr::null(), |f| f as *const c_void)
            })
        };
        log::info!("EGL {major}.{minor}, {}", unsafe { glow.get_parameter_string(glow::RENDERER) });
        Ok(Gl { egl, display, config, context, idle, glow })
    }

    #[cfg(target_os = "macos")]
    fn display(egl: &Egl) -> Result<egl::Display, String> {
        // ANGLE on Metal, asked for by name: its default on a Mac is still desktop OpenGL.
        const PLATFORM_ANGLE: egl::Enum = 0x3202;
        const PLATFORM_ANGLE_TYPE: egl::Attrib = 0x3203;
        const PLATFORM_ANGLE_TYPE_METAL: egl::Attrib = 0x3489;
        let modern = egl.upcast::<egl::EGL1_5>().ok_or("ANGLE is older than EGL 1.5")?;
        unsafe {
            modern.get_platform_display(
                PLATFORM_ANGLE,
                egl::DEFAULT_DISPLAY,
                &[PLATFORM_ANGLE_TYPE, PLATFORM_ANGLE_TYPE_METAL, egl::ATTRIB_NONE],
            )
        }
        .map_err(|e| format!("ANGLE gave no display: {e}"))
    }

    #[cfg(target_os = "linux")]
    fn display(egl: &Egl) -> Result<egl::Display, String> {
        linux::display(egl)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn display(egl: &Egl) -> Result<egl::Display, String> {
        unsafe { egl.get_display(egl::DEFAULT_DISPLAY) }.ok_or_else(|| "EGL has no display".to_string())
    }

    /// The GL calls themselves.
    pub fn api(&self) -> &glow::Context {
        &self.glow
    }
}

impl Drop for Gl {
    fn drop(&mut self) {
        let _ = self.egl.make_current(self.display, None, None, None);
        if let Some(idle) = self.idle.take() {
            let _ = self.egl.destroy_surface(self.display, idle);
        }
        let _ = self.egl.destroy_context(self.display, self.context);
    }
}

/// One picture of a target: a texture to draw into, and whatever it is to the platform.
struct Image {
    texture: glow::Texture,
    #[cfg(target_os = "macos")]
    shared: Option<mac::Shared>,
    #[cfg(target_os = "linux")]
    shared: Option<linux::Shared>,
    /// Handed over and not known to be finished with.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    busy: bool,
}

/// What a window is drawn into: a few pictures of its size, taken in turn.
pub(crate) struct Target {
    size: (u32, u32),
    framebuffer: glow::Framebuffer,
    images: Vec<Image>,
    /// The picture being drawn into, between `begin` and `present`.
    drawing: Option<usize>,
    /// The picture last handed over, which the compositor may be about to take up.
    last: Option<usize>,
    /// Pictures are handed over as they are; false once that has failed, after which they are
    /// read back and sent as memory.
    shared: bool,
    /// A name for this window's pictures that no other window in the session has.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    stream: u64,
    scratch: Vec<u8>,
}

impl Target {
    pub(crate) fn new(gl: &Gl, size: (u32, u32)) -> Result<Target, String> {
        let framebuffer = unsafe { gl.glow.create_framebuffer() }?;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let stream = ((std::process::id() as u64) << 32) | NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Target {
            size,
            framebuffer,
            images: Vec::new(),
            drawing: None,
            last: None,
            shared: std::env::var("SPATIAND_APP_COPY").is_err(),
            stream,
            scratch: Vec::new(),
        })
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        self.size
    }

    /// A picture nobody is looking at, made if there is none.
    fn free_image(&mut self, gl: &Gl) -> Result<usize, String> {
        let last = self.last;
        let shared = self.shared;
        // Not the one just handed over, which the compositor may not have taken up yet --
        // unless pictures are being copied, when what was handed over was the copy.
        let free = self.images.iter().enumerate().position(|(i, image)| {
            #[cfg(target_os = "macos")]
            let in_use = image.shared.as_ref().is_some_and(|s| s.in_use());
            #[cfg(not(target_os = "macos"))]
            let in_use = image.busy;
            !in_use && (Some(i) != last || !shared)
        });
        if let Some(index) = free {
            return Ok(index);
        }
        if self.images.len() >= 6 {
            // Something is not letting go. Better a picture drawn over than a window that
            // stops: take the oldest that is not the newest.
            log::debug!("every picture of a window is in use; drawing over one");
            return Ok((self.last.map_or(0, |l| l + 1)) % self.images.len());
        }
        let image = self.make_image(gl)?;
        self.images.push(image);
        Ok(self.images.len() - 1)
    }

    fn make_image(&mut self, gl: &Gl) -> Result<Image, String> {
        let g = &gl.glow;
        let (w, h) = self.size;
        unsafe {
            let texture = g.create_texture()?;
            g.bind_texture(glow::TEXTURE_2D, Some(texture));
            for (name, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                g.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
            }
            #[cfg(target_os = "macos")]
            let shared = if self.shared {
                match mac::Shared::new(gl, self.size) {
                    Ok(shared) => Some(shared),
                    Err(e) => {
                        log::warn!("pictures will be copied to the compositor: {e}");
                        self.shared = false;
                        None
                    }
                }
            } else {
                None
            };
            #[cfg(target_os = "macos")]
            let plain = shared.is_none();
            #[cfg(not(target_os = "macos"))]
            let plain = true;
            if plain {
                g.tex_image_2d(
                    glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w as i32, h as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
            }
            #[cfg(target_os = "linux")]
            let shared = if self.shared {
                match linux::Shared::new(gl, texture, self.size) {
                    Ok(shared) => Some(shared),
                    Err(e) => {
                        log::warn!("pictures will be copied to the compositor: {e}");
                        self.shared = false;
                        None
                    }
                }
            } else {
                None
            };
            g.bind_texture(glow::TEXTURE_2D, None);
            Ok(Image {
                texture,
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                shared,
                busy: false,
            })
        }
    }

    /// Make a picture ready to be drawn into, and the place GL draws.
    pub(crate) fn begin(&mut self, gl: &Gl, wire: &mut Wire, window: WindowId) -> Result<(), String> {
        self.reap(wire, window);
        let index = self.free_image(gl)?;
        let g = &gl.glow;
        unsafe {
            g.bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
            g.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(self.images[index].texture), 0);
            let status = g.check_framebuffer_status(glow::FRAMEBUFFER);
            if status != glow::FRAMEBUFFER_COMPLETE {
                return Err(format!("a window's picture cannot be drawn into ({status:#x})"));
            }
            g.viewport(0, 0, self.size.0 as i32, self.size.1 as i32);
        }
        self.drawing = Some(index);
        Ok(())
    }

    /// Take back the pictures the compositor has finished with.
    fn reap(&mut self, wire: &mut Wire, window: WindowId) {
        let mut kept = Vec::new();
        for (id, key) in std::mem::take(&mut wire.released) {
            if id == window {
                if let Some(image) = self.images.get_mut(key as usize) {
                    image.busy = false;
                }
            } else {
                kept.push((id, key));
            }
        }
        wire.released = kept;
    }

    /// Hand what was drawn to the compositor.
    pub(crate) fn present(&mut self, gl: &Gl, wire: &mut Wire, window: WindowId) -> Result<(), String> {
        let Some(index) = self.drawing.take() else { return Ok(()) };
        let g = &gl.glow;
        #[cfg(target_os = "macos")]
        if let Some(shared) = self.images[index].shared.as_ref() {
            // Finished, not merely sent: the compositor samples this memory from a context
            // of its own, and would otherwise sample it half drawn.
            unsafe { g.finish() };
            wire.show_named(window, self.size, &shared.name(self.stream))?;
            self.last = Some(index);
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        if let Some(shared) = self.images[index].shared.as_ref() {
            unsafe { g.finish() };
            match shared.show(wire, window, index as u32, self.size) {
                Ok(()) => {
                    self.images[index].busy = true;
                    self.last = Some(index);
                    return Ok(());
                }
                Err(e) => {
                    log::warn!("pictures will be copied to the compositor: {e}");
                    self.shared = false;
                }
            }
        }
        let (w, h) = self.size;
        let scratch = &mut self.scratch;
        scratch.resize((w * h * 4) as usize, 0);
        unsafe {
            g.read_pixels(0, 0, w as i32, h as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(scratch)));
        }
        wire.show_pixels(window, self.size, |memory| {
            // The wire's order is blue first; GL's is red first.
            for (to, from) in memory.chunks_exact_mut(4).zip(scratch.chunks_exact(4)) {
                to.copy_from_slice(&[from[2], from[1], from[0], from[3]]);
            }
        })?;
        self.last = Some(index);
        Ok(())
    }

    pub(crate) fn destroy(self, gl: &Gl, wire: &mut Wire, window: WindowId) {
        let _ = (&wire, window);
        let g = &gl.glow;
        unsafe {
            g.bind_framebuffer(glow::FRAMEBUFFER, None);
            g.delete_framebuffer(self.framebuffer);
            for (index, image) in self.images.into_iter().enumerate() {
                let _ = index;
                #[cfg(target_os = "linux")]
                wire.forget_dmabuf(window, index as u32);
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                if let Some(shared) = image.shared {
                    shared.destroy(gl);
                }
                g.delete_texture(image.texture);
            }
        }
    }
}
