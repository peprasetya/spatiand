//! The Beam Pro's part of the recorder (`record.rs`): the phone's hardware H.264 encoder
//! (MediaCodec), drawn into through its input surface, and where a finished recording goes.
//!
//! The Mac's counterpart is `crates/spatiand-mac/src/mac/record_codec.rs` (VideoToolbox); what
//! both are asked for is the same, and everything around it -- the sound, the file, the clock --
//! is `record.rs`'s, shared.

use std::ffi::{c_void, CString};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use ndk_sys as ndk;
use smithay::backend::egl::{EGLContext, EGLDisplay, EGLSurface};
use smithay::backend::renderer::gles::{ffi, GlesRenderer};
use smithay::backend::renderer::{Bind, Frame, Renderer};
use smithay::utils::Transform;

use super::egl::AndroidWindow;

/// What Android calls an encoder's input surface, as a colour format.
const COLOR_FORMAT_SURFACE: i32 = 0x7F00_0789;

// --- where recordings go ---

/// Recordings finished and waiting for the app to move them to `Movies/Spatiand`.
fn finished_list() -> &'static Mutex<Vec<String>> {
    static FINISHED: Mutex<Vec<String>> = Mutex::new(Vec::new());
    &FINISHED
}

/// The next finished recording or screenshot, for the app to publish.
pub fn take_finished() -> Option<String> {
    finished_list().lock().ok()?.pop()
}

/// Hand a finished file to the app to publish: a video to `Movies/Spatiand`, a picture to
/// `Pictures/Spatiand`.
pub fn publish(path: String) {
    if let Ok(mut done) = finished_list().lock() {
        done.push(path);
    }
}

/// A recording, finished.
pub fn finished(path: &Path) {
    publish(path.display().to_string());
}

/// Screenshots saved before they were published -- by a build that kept them to itself -- are
/// handed over now; and so is every recording still in the app's own storage, where no gallery
/// shows it: one cut short when the app was killed or the storage ran full (it plays as far as
/// it was written: its segment has no size until it is finished), and one the media store would
/// not take last time. At the session's start, before anything is recorded.
pub fn publish_leftovers() {
    let dir = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join("screenshots");
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "png") {
            publish(path.display().to_string());
        }
    }
    for entry in std::fs::read_dir(directory()).into_iter().flatten().flatten() {
        let path = entry.path();
        let ours = path.file_name().is_some_and(|n| n.to_string_lossy().starts_with("spatiand-"));
        if ours && path.extension().is_some_and(|e| e == "mkv") {
            unfinished(&path);
        }
    }
}

/// A recording that was not finished: handed to the app as it is if anything was written, and
/// removed if nothing was.
pub fn unfinished(path: &Path) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > 0) {
        log::info!("recording: {} is handed over as far as it was written", path.display());
        publish(path.display().to_string());
    } else {
        let _ = std::fs::remove_file(path);
    }
}

/// Where recordings are written before the app publishes them: `SPATIAND_RECORDINGS`, which the
/// app sets to its own `Movies` directory.
pub fn directory() -> PathBuf {
    super::record::directory_asked()
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join("Movies"))
}

// --- the encoder ---

struct Codec(*mut ndk::AMediaCodec);

unsafe impl Send for Codec {}

impl Drop for Codec {
    fn drop(&mut self) {
        unsafe {
            ndk::AMediaCodec_stop(self.0);
            ndk::AMediaCodec_delete(self.0);
        }
    }
}

struct Format(*mut ndk::AMediaFormat);

impl Format {
    fn new(mime: &str) -> Format {
        let format = Format(unsafe { ndk::AMediaFormat_new() });
        format.string("mime", mime);
        format
    }

    fn int(&self, key: &str, value: i32) {
        let key = CString::new(key).unwrap();
        unsafe { ndk::AMediaFormat_setInt32(self.0, key.as_ptr(), value) };
    }

    fn float(&self, key: &str, value: f32) {
        let key = CString::new(key).unwrap();
        unsafe { ndk::AMediaFormat_setFloat(self.0, key.as_ptr(), value) };
    }

    fn string(&self, key: &str, value: &str) {
        let key = CString::new(key).unwrap();
        let value = CString::new(value).unwrap();
        unsafe { ndk::AMediaFormat_setString(self.0, key.as_ptr(), value.as_ptr()) };
    }
}

impl Drop for Format {
    fn drop(&mut self) {
        unsafe { ndk::AMediaFormat_delete(self.0) };
    }
}

fn encoder(mime: &str, format: &Format) -> Result<Codec, String> {
    let name = CString::new(mime).unwrap();
    let codec = unsafe { ndk::AMediaCodec_createEncoderByType(name.as_ptr()) };
    if codec.is_null() {
        return Err(format!("this phone has no {mime} encoder"));
    }
    let codec = Codec(codec);
    let status = unsafe {
        ndk::AMediaCodec_configure(
            codec.0,
            format.0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            ndk::AMEDIACODEC_CONFIGURE_FLAG_ENCODE as u32,
        )
    };
    if status != ndk::media_status_t::AMEDIA_OK {
        return Err(format!("the {mime} encoder would not take its settings ({status:?})"));
    }
    Ok(codec)
}

type PresentationTime = unsafe extern "C" fn(*const c_void, *const c_void, i64) -> u32;

/// What the recorder's thread takes out of the encoder.
pub enum Coded {
    /// The stream's parameter sets, Annex B.
    Config(Vec<u8>),
    /// A picture, Annex B, at `ms` of the recording.
    Picture { ms: i64, key: bool, data: Vec<u8> },
}

/// The session's side: the encoder's input surface, and the frame copied out of the glasses'
/// window to be drawn into it.
pub struct Input {
    surface: EGLSurface,
    /// The frame, copied out of the glasses' window, and the framebuffer that holds it.
    copy: Option<(u32, u32)>,
    present_time: Option<PresentationTime>,
    display: *const c_void,
}

/// The recorder thread's side: the encoder itself, whose coded pictures it takes out.
pub struct Output {
    video: Codec,
}

/// An encoder for `size` frames at `fps` and `bits` a second.
pub fn open(size: (i32, i32), display: &EGLDisplay, context: &EGLContext, fps: u32, bits: u32) -> Result<(Input, Output), String> {
    let video_format = Format::new("video/avc");
    video_format.int("width", size.0);
    video_format.int("height", size.1);
    video_format.int("color-format", COLOR_FORMAT_SURFACE);
    video_format.int("bitrate", bits as i32);
    video_format.int("bitrate-mode", 1); // variable
    video_format.float("frame-rate", fps as f32);
    video_format.int("i-frame-interval", 1);
    let video = encoder("video/avc", &video_format)?;
    let mut window = std::ptr::null_mut();
    if unsafe { ndk::AMediaCodec_createInputSurface(video.0, &mut window) } != ndk::media_status_t::AMEDIA_OK
        || window.is_null()
    {
        return Err("the video encoder gave no surface to draw into".into());
    }
    let native = AndroidWindow::new(window.cast());
    // Ours is held by `native` now; the one the codec handed over is let go of.
    unsafe { ndk::ANativeWindow_release(window) };
    let pixel_format = context.pixel_format().ok_or("the context has no pixel format")?;
    let surface = unsafe { EGLSurface::new(display, pixel_format, context.config_id(), native) }
        .map_err(|e| format!("could not draw into the encoder: {e}"))?;
    if unsafe { ndk::AMediaCodec_start(video.0) } != ndk::media_status_t::AMEDIA_OK {
        return Err("the video encoder would not start".into());
    }
    let present_time = unsafe {
        let name = CString::new("eglPresentationTimeANDROID").unwrap();
        let f = smithay::backend::egl::ffi::egl::GetProcAddress(name.as_ptr()) as *const c_void;
        (!f.is_null()).then(|| std::mem::transmute::<*const c_void, PresentationTime>(f))
    };
    let input = Input {
        surface,
        copy: None,
        present_time,
        display: display.get_display_handle().handle as *const c_void,
    };
    Ok((input, Output { video }))
}

impl Input {
    /// Copy the frame just drawn into the glasses' window, while that window is bound. Whether a
    /// copy was made, to be presented.
    ///
    /// # Safety
    /// The GL context must be current with the glasses' window bound as framebuffer 0.
    pub unsafe fn copy(&mut self, gl: &ffi::Gles2, (w, h): (i32, i32)) -> bool {
        let (_, fbo) = *self.copy.get_or_insert_with(|| {
            let mut texture = 0;
            gl.GenTextures(1, &mut texture);
            gl.BindTexture(ffi::TEXTURE_2D, texture);
            gl.TexStorage2D(ffi::TEXTURE_2D, 1, ffi::RGBA8, w, h);
            let mut fbo = 0;
            gl.GenFramebuffers(1, &mut fbo);
            gl.BindFramebuffer(ffi::FRAMEBUFFER, fbo);
            gl.FramebufferTexture2D(ffi::FRAMEBUFFER, ffi::COLOR_ATTACHMENT0, ffi::TEXTURE_2D, texture, 0);
            gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
            (texture, fbo)
        });
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, 0);
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, fbo);
        gl.BlitFramebuffer(0, 0, w, h, 0, 0, w, h, ffi::COLOR_BUFFER_BIT, ffi::NEAREST);
        gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
        true
    }

    /// Draw the copied frame into the encoder's surface, stamped `at` into the recording.
    pub fn present(&mut self, renderer: &mut GlesRenderer, (w, h): (i32, i32), at: Duration) -> Result<(), String> {
        let Some((_, fbo)) = self.copy else { return Ok(()) };
        let surface = &mut self.surface;
        {
            let mut target = renderer.bind(surface).map_err(|e| e.to_string())?;
            let mut frame = renderer
                .render(&mut target, (w, h).into(), Transform::Normal)
                .map_err(|e| e.to_string())?;
            frame
                .with_context(|gl| unsafe {
                    gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, fbo);
                    gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, 0);
                    gl.BlitFramebuffer(0, 0, w, h, 0, 0, w, h, ffi::COLOR_BUFFER_BIT, ffi::NEAREST);
                    gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
                })
                .map_err(|e| e.to_string())?;
            let _ = frame.finish().map_err(|e| e.to_string())?;
        }
        if let Some(present_time) = self.present_time {
            unsafe { present_time(self.display, self.surface.get_surface_handle() as *const c_void, at.as_nanos() as i64) };
        }
        self.surface
            .swap_buffers(None)
            .map_err(|e| format!("the encoder would not take a frame ({e:?})"))
    }

    /// Give back the copy's GL objects.
    pub fn release(mut self, renderer: &mut GlesRenderer) {
        if let Some((texture, fbo)) = self.copy.take() {
            let _ = renderer.with_context(|gl| unsafe {
                gl.DeleteFramebuffers(1, &fbo);
                gl.DeleteTextures(1, &texture);
            });
        }
    }
}

impl Output {
    /// Nothing more is coming: the encoder is told the stream has ended.
    pub fn end(&mut self) {
        unsafe { ndk::AMediaCodec_signalEndOfInputStream(self.video.0) };
    }

    /// What the encoder has coded since last asked, and whether its stream has ended.
    pub fn drain(&mut self) -> Result<(Vec<Coded>, bool), String> {
        let mut out = Vec::new();
        let mut ended = false;
        loop {
            let mut info = ndk::AMediaCodecBufferInfo { offset: 0, size: 0, presentationTimeUs: 0, flags: 0 };
            let index = unsafe { ndk::AMediaCodec_dequeueOutputBuffer(self.video.0, &mut info, 0) };
            if index == ndk::AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED as isize {
                continue;
            }
            // -1 is nothing yet and -3 buffers changed (-2 above); below that is the codec's
            // error, which was taken for "nothing yet" and left the recording going for ever.
            if index < ndk::AMEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED as isize {
                return Err(format!("the video encoder failed ({index})"));
            }
            if index < 0 {
                break;
            }
            let flags = info.flags;
            if flags & ndk::AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM as u32 != 0 {
                ended = true;
            }
            let mut capacity = 0usize;
            let buffer = unsafe { ndk::AMediaCodec_getOutputBuffer(self.video.0, index as usize, &mut capacity) };
            if info.size > 0 && !buffer.is_null() {
                let bytes = unsafe { std::slice::from_raw_parts(buffer.add(info.offset as usize), info.size as usize) }.to_vec();
                if flags & ndk::AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG as u32 != 0 {
                    out.push(Coded::Config(bytes));
                } else {
                    let key = flags & 1 != 0; // BUFFER_FLAG_KEY_FRAME
                    out.push(Coded::Picture { ms: info.presentationTimeUs / 1000, key, data: bytes });
                }
            }
            unsafe { ndk::AMediaCodec_releaseOutputBuffer(self.video.0, index as usize, false) };
        }
        Ok((out, ended))
    }
}
