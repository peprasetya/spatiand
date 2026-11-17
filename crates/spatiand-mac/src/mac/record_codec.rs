//! The Mac's part of the recorder (the Beam Pro's `record.rs`, shared): VideoToolbox's hardware
//! H.264 instead of MediaCodec, and `~/Movies/Spatiand` instead of the phone's gallery.
//!
//! The encoder gives out a few `IOSurface`s to draw in ([`spatiand_video::mac::Encoder`]).
//! ANGLE wraps each as a pbuffer whose image a texture -- and so a framebuffer -- can have, as
//! `remote_video` does to show a picture, so the glasses' frame is copied into one with a single
//! blit on the GPU while it is still in the glasses' window. A fence follows it, and the copy
//! goes to the recorder's thread, which hands the surface to the encoder, once the fence has
//! passed: the frame loop pays for the blit and a look at the fence, never a wait, as on the
//! Deck. (The look is the frame loop's because ANGLE answers a fence only on a thread with its
//! context current -- asked from the recorder's thread, every wait was `EGL_BAD_CONTEXT`.) A
//! frame that finds every surface still being encoded is not recorded, and the log counts them.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use smithay::backend::egl::fence::EGLFence;
use smithay::backend::egl::ffi::egl;
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::renderer::gles::{ffi, GlesRenderer};
use spatiand_video::mac::{Encoded, Encoder, Surface};

/// Surfaces to draw into, so a frame can be copied while the last ones are encoding.
const CANVASES: usize = 3;

const IOSURFACE_ANGLE: u32 = 0x3454;
const IOSURFACE_PLANE_ANGLE: i32 = 0x345A;
const TEXTURE_TYPE_ANGLE: i32 = 0x345C;
const TEXTURE_INTERNAL_FORMAT_ANGLE: i32 = 0x345D;
const GL_BGRA_EXT: i32 = 0x80E1;

// --- where recordings go ---

/// `SPATIAND_RECORDINGS`, or `~/Movies/Spatiand`: the Mac's folder for films, as `~/Videos` is
/// the Deck's and `Movies` the phone's.
pub fn directory() -> PathBuf {
    super::record::directory_asked()
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join("Movies/Spatiand"))
}

/// A screenshot or a finished video: the app is told, which says where it is.
pub fn publish(path: String) {
    super::ffi::tell(super::ffi::Asked::Saved, &path);
}

/// Nothing waits to be published on the Mac: a recording is written where it stays.
pub fn publish_leftovers() {}

pub fn finished(path: &Path) {
    publish(path.display().to_string());
}

/// A recording cut short: kept as far as it was written (it plays that far), removed if
/// nothing was.
pub fn unfinished(path: &Path) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > 0) {
        publish(path.display().to_string());
    } else {
        let _ = std::fs::remove_file(path);
    }
}

// --- the encoder ---

/// What the recorder's thread takes out of the encoder: as on the Beam Pro.
pub enum Coded {
    /// The stream's parameter sets, Annex B.
    Config(Vec<u8>),
    /// A picture, Annex B, at `ms` of the recording.
    Picture { ms: i64, key: bool, data: Vec<u8> },
}

/// A frame copied into surface `index`, at `ms` into the recording.
struct Copied {
    index: usize,
    ms: i64,
}

/// One of the encoder's surfaces, and what ANGLE made of it to draw into: a pbuffer, the
/// texture that has its image, and a framebuffer of that texture. Made at the first copy into
/// it, inside a frame, where the context is current.
struct Canvas {
    surface: Surface,
    gl: Option<(*const c_void, u32, u32)>,
}

/// The session's side: the surfaces, and which are free to draw into.
pub struct Input {
    display: EGLDisplay,
    canvases: Vec<Canvas>,
    free: Receiver<usize>,
    /// Surfaces taken and not used after all: free again at once.
    spare: Vec<usize>,
    to_thread: Sender<Copied>,
    /// A copy made this frame, owed to the encoder.
    pending: Option<(usize, Option<EGLFence>)>,
    /// Copies handed over, waiting for the GPU to have written them, oldest first.
    fencing: VecDeque<(Copied, Option<EGLFence>)>,
    skipped: u64,
    complained: bool,
}

/// The recorder thread's side: the encoder, the copies coming to it, and what it makes.
pub struct Output {
    encoder: Encoder,
    copies: Receiver<Copied>,
    coded: Receiver<Encoded>,
    give_back: Sender<usize>,
    ending: bool,
    ended: bool,
}

/// An encoder for `size` frames at `fps` and `bits` a second.
pub fn open(size: (i32, i32), display: &EGLDisplay, _context: &EGLContext, fps: u32, bits: u32) -> Result<(Input, Output), String> {
    let (encoder, coded) = Encoder::new(size.0 as u32, size.1 as u32, fps, bits, CANVASES).map_err(|e| e.to_string())?;
    let canvases = (0..encoder.buffers())
        .map(|i| encoder.surface(i).map(|surface| Canvas { surface, gl: None }))
        .collect::<Option<Vec<_>>>()
        .ok_or("the encoder's buffers have no surfaces")?;
    let (give_back, free) = channel();
    for i in 0..canvases.len() {
        let _ = give_back.send(i);
    }
    let (to_thread, copies) = channel();
    let input = Input {
        display: display.clone(),
        canvases,
        free,
        spare: Vec::new(),
        to_thread,
        pending: None,
        fencing: VecDeque::new(),
        skipped: 0,
        complained: false,
    };
    Ok((input, Output { encoder, copies, coded, give_back, ending: false, ended: false }))
}

/// The config the current context was made with, which a pbuffer for it must share.
unsafe fn current_config(display: *const c_void) -> Option<*const c_void> {
    let context = egl::GetCurrentContext();
    let mut id = 0;
    if egl::QueryContext(display, context, egl::CONFIG_ID as i32, &mut id) == egl::FALSE {
        return None;
    }
    let attributes = [egl::CONFIG_ID as i32, id, egl::NONE as i32];
    let mut config = std::ptr::null();
    let mut found = 0;
    (egl::ChooseConfig(display, attributes.as_ptr(), &mut config, 1, &mut found) != egl::FALSE && found > 0)
        .then_some(config)
}

/// A framebuffer drawing into `surface`, BGRA as VideoToolbox asked for it.
unsafe fn framebuffer_of(gl: &ffi::Gles2, surface: &Surface) -> Result<(*const c_void, u32, u32), String> {
    let display = egl::GetCurrentDisplay();
    let config = current_config(display).ok_or("no config for the frame's context")?;
    let (width, height) = surface.size();
    let attributes = [
        egl::WIDTH as i32, width as i32,
        egl::HEIGHT as i32, height as i32,
        IOSURFACE_PLANE_ANGLE, 0,
        egl::TEXTURE_TARGET as i32, egl::TEXTURE_2D as i32,
        TEXTURE_INTERNAL_FORMAT_ANGLE, GL_BGRA_EXT,
        egl::TEXTURE_FORMAT as i32, egl::TEXTURE_RGBA as i32,
        TEXTURE_TYPE_ANGLE, ffi::UNSIGNED_BYTE as i32,
        egl::NONE as i32,
    ];
    let pbuffer = egl::CreatePbufferFromClientBuffer(display, IOSURFACE_ANGLE, surface.raw() as *mut c_void, config, attributes.as_ptr());
    if pbuffer == egl::NO_SURFACE {
        return Err(format!("ANGLE would not wrap the encoder's surface ({:#x})", egl::GetError()));
    }
    let mut texture = 0;
    gl.GenTextures(1, &mut texture);
    gl.BindTexture(ffi::TEXTURE_2D, texture);
    if egl::BindTexImage(display, pbuffer, egl::BACK_BUFFER as i32) == egl::FALSE {
        let error = egl::GetError();
        gl.BindTexture(ffi::TEXTURE_2D, 0);
        gl.DeleteTextures(1, &texture);
        egl::DestroySurface(display, pbuffer);
        return Err(format!("the encoder's surface would not become a texture ({error:#x})"));
    }
    gl.BindTexture(ffi::TEXTURE_2D, 0);
    let mut fbo = 0;
    let mut was = 0;
    gl.GetIntegerv(ffi::DRAW_FRAMEBUFFER_BINDING, &mut was);
    gl.GenFramebuffers(1, &mut fbo);
    gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, fbo);
    gl.FramebufferTexture2D(ffi::DRAW_FRAMEBUFFER, ffi::COLOR_ATTACHMENT0, ffi::TEXTURE_2D, texture, 0);
    let status = gl.CheckFramebufferStatus(ffi::DRAW_FRAMEBUFFER);
    gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, was as u32);
    if status != ffi::FRAMEBUFFER_COMPLETE {
        gl.DeleteFramebuffers(1, &fbo);
        gl.DeleteTextures(1, &texture);
        egl::ReleaseTexImage(display, pbuffer, egl::BACK_BUFFER as i32);
        egl::DestroySurface(display, pbuffer);
        return Err(format!("the encoder's surface cannot be drawn into ({status:#x})"));
    }
    Ok((pbuffer, texture, fbo))
}

impl Input {
    /// Copy the frame just drawn into the glasses' window into a free surface, while that window
    /// is bound. Whether a copy was made, to be presented.
    ///
    /// # Safety
    /// The GL context must be current with the glasses' window bound as framebuffer 0.
    pub unsafe fn copy(&mut self, gl: &ffi::Gles2, (w, h): (i32, i32)) -> bool {
        // One owed and never presented (the frame failed after it): its surface is free again.
        if let Some((index, _)) = self.pending.take() {
            self.spare.push(index);
        }
        let Some(index) = self.spare.pop().or_else(|| self.free.try_recv().ok()) else {
            self.skipped += 1;
            if self.skipped % (super::record::FPS as u64 * 10) == 1 {
                log::warn!("recording: the encoder is behind; {} frame(s) not recorded so far", self.skipped);
            }
            return false;
        };
        let canvas = &mut self.canvases[index];
        if canvas.gl.is_none() {
            match framebuffer_of(gl, &canvas.surface) {
                Ok(made) => canvas.gl = Some(made),
                Err(e) => {
                    if !std::mem::replace(&mut self.complained, true) {
                        log::error!("recording: {e}; nothing is recorded");
                    }
                    self.spare.push(index);
                    return false;
                }
            }
        }
        let (_, _, fbo) = canvas.gl.unwrap();
        let (mut read, mut draw) = (0, 0);
        gl.GetIntegerv(ffi::READ_FRAMEBUFFER_BINDING, &mut read);
        gl.GetIntegerv(ffi::DRAW_FRAMEBUFFER_BINDING, &mut draw);
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, 0);
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, fbo);
        // The window's bottom row is GL's first; the surface's first row is the picture's top.
        gl.BlitFramebuffer(0, 0, w, h, 0, h, w, 0, ffi::COLOR_BUFFER_BIT, ffi::NEAREST);
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, read as u32);
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, draw as u32);
        // The encoder reads the surface once the GPU has written it: a fence for the recorder's
        // thread to wait on, or -- with none to be had -- the wait here.
        let fence = EGLFence::create(&self.display).ok();
        if fence.is_none() {
            gl.Finish();
        }
        self.pending = Some((index, fence));
        true
    }

    /// Hand the copy on, stamped `at` into the recording: to the encoder's thread as soon as the
    /// GPU has written it, which is looked at -- never waited for -- here, each frame.
    pub fn present(&mut self, renderer: &mut GlesRenderer, _size: (i32, i32), at: Duration) -> Result<(), String> {
        if let Some((index, fence)) = self.pending.take() {
            self.fencing.push_back((Copied { index, ms: at.as_millis() as i64 }, fence));
        }
        self.hand_over(renderer, Duration::ZERO)
    }

    /// Send on every copy whose fence has passed, waiting up to `wait` for each.
    fn hand_over(&mut self, renderer: &mut GlesRenderer, wait: Duration) -> Result<(), String> {
        let fencing = &mut self.fencing;
        let to_thread = &self.to_thread;
        renderer
            .with_context(|_| {
                while let Some((_, fence)) = fencing.front() {
                    let passed = fence.as_ref().is_none_or(|f| f.client_wait(Some(wait), false).unwrap_or(true));
                    if !passed {
                        break;
                    }
                    let (copied, _) = fencing.pop_front().unwrap();
                    to_thread.send(copied).map_err(|_| "the recorder's thread has gone".to_string())?;
                }
                Ok(())
            })
            .map_err(|e| e.to_string())?
    }

    /// Give back what ANGLE made of the surfaces, the last copies handed over first.
    pub fn release(mut self, renderer: &mut GlesRenderer) {
        if let Err(e) = self.hand_over(renderer, Duration::from_millis(100)) {
            log::warn!("recording: the last frames were not handed over: {e}");
        }
        let canvases = std::mem::take(&mut self.canvases);
        let _ = renderer.with_context(|gl| unsafe {
            let display = egl::GetCurrentDisplay();
            for canvas in canvases {
                if let Some((pbuffer, texture, fbo)) = canvas.gl {
                    gl.DeleteFramebuffers(1, &fbo);
                    gl.DeleteTextures(1, &texture);
                    egl::ReleaseTexImage(display, pbuffer, egl::BACK_BUFFER as i32);
                    egl::DestroySurface(display, pbuffer);
                }
            }
        });
        if self.skipped > 0 {
            log::info!("recording: {} frame(s) not recorded, the encoder being behind", self.skipped);
        }
    }
}

impl Output {
    /// Nothing more is coming: what was handed over is encoded, and the stream ends.
    pub fn end(&mut self) {
        self.ending = true;
    }

    /// What the encoder has coded since last asked, and whether its stream has ended.
    pub fn drain(&mut self) -> Result<(Vec<Coded>, bool), String> {
        for copied in self.copies.try_iter() {
            if let Err(e) = self.encoder.encode(copied.index, copied.ms) {
                log::warn!("recording: {e}");
                let _ = self.give_back.send(copied.index);
            }
        }
        if self.ending && !self.ended {
            // Every picture handed over comes out before this returns.
            self.encoder.finish();
            self.ended = true;
        }
        let mut out = Vec::new();
        for encoded in self.coded.try_iter() {
            match encoded {
                Encoded::Picture { buffer, ms, key, config, data } => {
                    let _ = self.give_back.send(buffer);
                    if let Some(config) = config {
                        out.push(Coded::Config(config));
                    }
                    out.push(Coded::Picture { ms, key, data });
                }
                Encoded::Lost { buffer, status } => {
                    let _ = self.give_back.send(buffer);
                    log::warn!("recording: the encoder lost a picture ({status})");
                }
            }
        }
        Ok((out, self.ended))
    }
}

#[cfg(test)]
mod tests {
    use smithay::backend::egl::EGLSurface;
    use smithay::backend::renderer::gles::ffi;
    use smithay::backend::renderer::{Bind, Frame, Renderer};
    use smithay::utils::Transform;

    /// The whole recorder on this Mac's GPU and encoder, as the frame loop drives it: frames
    /// drawn into a window, copied, handed over; a file that is the Deck's and the Beam Pro's.
    /// Each frame is four colours -- red top left, green top right, blue bottom left, white
    /// bottom right -- so the picture can be checked the right way up (`SPATIAND_TEST_KEEP=1`
    /// keeps the file and says where).
    #[test]
    fn the_room_is_recorded_as_on_the_deck() {
        let (w, h) = (1280, 360);
        let dir = std::env::temp_dir().join(format!("spatiand-record-test-{}", std::process::id()));
        std::env::set_var("SPATIAND_RECORDINGS", &dir);
        let display = super::super::egl::open_display().expect("ANGLE");
        let mut renderer = super::super::egl::renderer(&display).expect("a renderer");
        let window = super::super::NativeWindow::new(std::ptr::null_mut(), (w, h));
        let native = super::super::egl::surface_of(&window);
        let context = renderer.egl_context();
        let mut surface =
            unsafe { EGLSurface::new(&display, context.pixel_format().unwrap(), context.config_id(), native) }.expect("a surface");
        let mut recorder = super::super::record::Recorder::start((w, h), &display, &context).expect("a recorder");
        let started = std::time::Instant::now();
        let mut copies = 0;
        while started.elapsed() < std::time::Duration::from_secs(3) {
            let due = recorder.due();
            {
                let mut target = renderer.bind(&mut surface).unwrap();
                let mut frame = renderer.render(&mut target, (w, h).into(), Transform::Normal).unwrap();
                frame
                    .with_context(|gl| unsafe {
                        gl.Enable(ffi::SCISSOR_TEST);
                        // GL's first row is the bottom.
                        for (x, y, colour) in [
                            (0, h / 2, [1.0, 0.0, 0.0]),
                            (w / 2, h / 2, [0.0, 1.0, 0.0]),
                            (0, 0, [0.0, 0.0, 1.0]),
                            (w / 2, 0, [1.0, 1.0, 1.0]),
                        ] {
                            gl.Scissor(x, y, w / 2, h / 2);
                            gl.ClearColor(colour[0], colour[1], colour[2], 1.0);
                            gl.Clear(ffi::COLOR_BUFFER_BIT);
                        }
                        gl.Disable(ffi::SCISSOR_TEST);
                        if due {
                            recorder.copy(gl);
                            copies += 1;
                        }
                    })
                    .unwrap();
                let _ = frame.finish().unwrap();
            }
            surface.swap_buffers(None).unwrap();
            recorder.present(&mut renderer);
            std::thread::sleep(std::time::Duration::from_millis(14));
        }
        recorder.stop(&mut renderer);
        // The thread finishes the file: done when its segment has a size, the eight bytes after
        // the segment's ID no longer "unknown".
        let mut file = None;
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            file = std::fs::read_dir(&dir).ok().and_then(|mut d| d.next()).and_then(|e| e.ok()).map(|e| e.path());
            let finished = file.as_ref().and_then(|f| std::fs::read(f).ok()).is_some_and(|bytes| {
                bytes
                    .windows(4)
                    .position(|b| b == [0x18, 0x53, 0x80, 0x67])
                    .is_some_and(|at| bytes.len() > at + 12 && bytes[at + 5..at + 12].iter().any(|b| *b != 0xFF))
            });
            if finished {
                break;
            }
        }
        let file = file.expect("a recording");
        let bytes = std::fs::read(&file).unwrap();
        assert!(file.file_name().unwrap().to_string_lossy().starts_with("spatiand-"));
        assert_eq!(file.extension().unwrap(), "mkv");
        assert_eq!(&bytes[..4], &[0x1A, 0x45, 0xDF, 0xA3], "a Matroska file");
        for title in ["What you heard", "Surround 7.1.4", "Microphone"] {
            assert!(bytes.windows(title.len()).any(|b| b == title.as_bytes()), "a track called {title}");
        }
        assert!(copies >= 3 * super::super::record::FPS as usize - 10, "{copies} frames copied in 3 s");
        assert!(bytes.len() > 20_000, "pictures in it: {} bytes", bytes.len());
        if std::env::var("SPATIAND_TEST_KEEP").is_ok() {
            println!("kept {}", file.display());
        } else {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
