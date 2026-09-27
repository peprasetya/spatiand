//! Recording a video of the session on the Beam Pro: what the glasses show, and what the
//! wearer hears.
//!
//! The Deck's recorder (`crates/spatiand/src/record.rs`) with Android's parts: the phone's
//! hardware H.264 encoder fed through its own input surface, and Android's MP4 muxer. So:
//!
//! * **the picture** -- the frame sent to the glasses, both eyes side by side, at [`FPS`] and
//!   40 Mbit/s, as on the Deck. The frame is copied once on the GPU while it is still in the
//!   glasses' window, and the copy drawn into the encoder's surface after the glasses have
//!   theirs; the encoder and the file are this module's thread's.
//! * **"What you heard"** -- stereo, after the HRTF: exactly what the session's one output was
//!   given (`spatiand_audio::server::record_heard`), as AAC.
//! * **"Microphone"** -- the phone's microphone, as AAC.
//!
//! **No surround track.** The Deck's third track is twelve channels before the HRTF; MP4 on
//! Android carries AAC, and Android's AAC encoder stops at eight.
//!
//! The file is written in the app's own storage and handed to the app when finished, which
//! puts it in the phone's `Movies/Spatiand`, where a gallery or a computer finds it: Android
//! lets an app write there only through its media store.

use std::ffi::{c_void, CString};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ndk_sys as ndk;
use smithay::backend::egl::{EGLContext, EGLDisplay, EGLSurface};
use smithay::backend::renderer::gles::{ffi, GlesRenderer};
use smithay::backend::renderer::{Bind, Frame, Renderer};
use smithay::utils::Transform;
use spatiand_audio::ring::Ring;

use super::egl::AndroidWindow;

/// Pictures a second: a video's rate, as on the Deck.
pub const FPS: u32 = 36;
const BITS_PER_SECOND: i32 = 40_000_000;
const RATE: u32 = 48_000;
/// Two seconds of sound waiting in each track, at most.
const RING_SECONDS: usize = 2;
/// How far a track may lag the clock before the gap is written as silence.
const LAG: Duration = Duration::from_millis(150);
/// What Android calls an encoder's input surface, as a colour format.
const COLOR_FORMAT_SURFACE: i32 = 0x7F00_0789;

/// Recordings finished and waiting for the app to move them to `Movies/Spatiand`.
fn finished() -> &'static Mutex<Vec<String>> {
    static FINISHED: Mutex<Vec<String>> = Mutex::new(Vec::new());
    &FINISHED
}

/// The next finished recording, for the app to publish.
pub fn take_finished() -> Option<String> {
    finished().lock().ok()?.pop()
}

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

/// One sound track: where it comes from, and its encoder.
struct Sound {
    title: &'static str,
    channels: usize,
    ring: Arc<Ring>,
    codec: Codec,
    /// Frames given to the encoder, which is also its clock.
    frames: u64,
    ended: bool,
}

type PresentationTime = unsafe extern "C" fn(*const c_void, *const c_void, i64) -> u32;

/// A recording in progress.
pub struct Recorder {
    /// The encoder's input surface, as something to draw into. `None` once stopped.
    surface: Option<EGLSurface>,
    size: (i32, i32),
    /// The frame, copied out of the glasses' window, and the framebuffer that holds it.
    copy: Option<(u32, u32)>,
    /// A copy was made this frame and is owed to the encoder.
    owed: bool,
    started: Instant,
    next_due: Instant,
    frames: u64,
    stop: Arc<AtomicBool>,
    present_time: Option<PresentationTime>,
    display: *const c_void,
    _microphone: Option<spatiand_audio::server::Capture>,
}

impl Recorder {
    /// Start recording a `size` frame, drawn with `context` on `display`.
    pub fn start(size: (i32, i32), display: &EGLDisplay, context: &EGLContext) -> Result<Recorder, String> {
        // --- the picture ---
        let video_format = Format::new("video/avc");
        video_format.int("width", size.0);
        video_format.int("height", size.1);
        video_format.int("color-format", COLOR_FORMAT_SURFACE);
        video_format.int("bitrate", BITS_PER_SECOND);
        video_format.int("bitrate-mode", 1); // variable
        video_format.float("frame-rate", FPS as f32);
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

        // --- the sound ---
        let ring = |channels: usize| Arc::new(Ring::new(RATE as usize * RING_SECONDS * channels));
        let aac = |channels: usize, kbit: i32| -> Result<Codec, String> {
            let format = Format::new("audio/mp4a-latm");
            format.int("sample-rate", RATE as i32);
            format.int("channel-count", channels as i32);
            format.int("bitrate", kbit * 1000);
            format.int("aac-profile", 2); // LC
            format.int("max-input-size", 16 * 1024);
            let codec = encoder("audio/mp4a-latm", &format)?;
            if unsafe { ndk::AMediaCodec_start(codec.0) } != ndk::media_status_t::AMEDIA_OK {
                return Err("the sound encoder would not start".into());
            }
            Ok(codec)
        };
        let mut sounds = Vec::new();
        let heard = ring(2);
        sounds.push(Sound {
            title: "What you heard (binaural stereo, after the HRTF)",
            channels: 2,
            ring: heard.clone(),
            codec: aac(2, 256)?,
            frames: 0,
            ended: false,
        });
        let mic = ring(1);
        let into = mic.clone();
        let microphone = match spatiand_audio::server::Capture::open(
            RATE,
            1,
            Box::new(move |samples: &[i16]| {
                let floats: Vec<f32> = samples.iter().map(|s| *s as f32 / 32768.0).collect();
                into.write(&floats);
            }),
        ) {
            Ok(capture) => Some(capture),
            Err(e) => {
                log::warn!("recording: the microphone track will be silent ({e})");
                None
            }
        };
        sounds.push(Sound {
            title: "Microphone",
            channels: 1,
            ring: mic,
            codec: aac(1, 128)?,
            frames: 0,
            ended: false,
        });

        // --- the file ---
        let dir = directory();
        std::fs::create_dir_all(&dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
        let path = dir.join(format!("spatiand-{}.mp4", stamp()));
        let file = std::fs::File::create(&path).map_err(|e| format!("could not write {}: {e}", path.display()))?;
        let muxer = unsafe {
            ndk::AMediaMuxer_new(
                std::os::fd::AsRawFd::as_raw_fd(&file),
                ndk::OutputFormat::AMEDIAMUXER_OUTPUT_FORMAT_MPEG_4,
            )
        };
        if muxer.is_null() {
            return Err("Android would not write an MP4".into());
        }
        spatiand_audio::server::record_heard(Some(heard));

        let stop = Arc::new(AtomicBool::new(false));
        let started = Instant::now();
        let stopping = stop.clone();
        let shown = path.clone();
        let muxer = Muxer(muxer);
        std::thread::Builder::new()
            .name("recorder".into())
            .spawn(move || {
                let result = run(video, sounds, muxer, started, &stopping);
                drop(file);
                match result {
                    Ok(seconds) => {
                        log::info!("recording: saved {seconds:.0} s to {}", shown.display());
                        if let Ok(mut done) = finished().lock() {
                            done.push(shown.display().to_string());
                        }
                    }
                    Err(e) => log::warn!("recording: {} is incomplete: {e}", shown.display()),
                }
            })
            .map_err(|e| format!("could not start the recorder: {e}"))?;

        let present_time = unsafe {
            let name = CString::new("eglPresentationTimeANDROID").unwrap();
            let f = smithay::backend::egl::ffi::egl::GetProcAddress(name.as_ptr()) as *const c_void;
            (!f.is_null()).then(|| std::mem::transmute::<*const c_void, PresentationTime>(f))
        };
        log::info!("recording to {}", path.display());
        Ok(Recorder {
            surface: Some(surface),
            size,
            copy: None,
            owed: false,
            started,
            next_due: started,
            frames: 0,
            stop,
            present_time,
            display: display.get_display_handle().handle as *const c_void,
            _microphone: microphone,
        })
    }

    /// Whether this frame is one to record. Asked once a frame, before it is drawn.
    pub fn due(&mut self) -> bool {
        let now = Instant::now();
        if now < self.next_due {
            return false;
        }
        let interval = Duration::from_secs_f64(1.0 / FPS as f64);
        self.next_due += interval;
        if self.next_due < now {
            // Fell behind: start the cadence again rather than record a burst.
            self.next_due = now + interval;
        }
        true
    }

    /// Copy the frame just drawn into the glasses' window, while that window is bound.
    ///
    /// # Safety
    /// The GL context must be current with the glasses' window bound as framebuffer 0.
    pub unsafe fn copy(&mut self, gl: &ffi::Gles2) {
        let (w, h) = self.size;
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
        self.owed = true;
    }

    /// Hand the copied frame to the encoder. After the glasses' own swap, so they never wait
    /// on the encoder.
    pub fn present(&mut self, renderer: &mut GlesRenderer) {
        if !std::mem::take(&mut self.owed) {
            return;
        }
        let (Some(surface), Some((_, fbo))) = (self.surface.as_mut(), self.copy) else { return };
        let (w, h) = self.size;
        let drawn = (|| -> Result<(), String> {
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
            Ok(())
        })();
        if let Err(e) = drawn {
            log::warn!("recording: could not hand a frame to the encoder: {e}");
            return;
        }
        // Stamped with the recording's own clock, which the sound is written against too.
        if let Some(present_time) = self.present_time {
            let ns = self.started.elapsed().as_nanos() as i64;
            unsafe { present_time(self.display, surface.get_surface_handle() as *const c_void, ns) };
        }
        if let Err(e) = surface.swap_buffers(None) {
            log::warn!("recording: the encoder would not take a frame ({e:?})");
            return;
        }
        self.frames += 1;
    }

    /// Stop, and let the recorder's thread finish the file. Gives back the copy's GL objects.
    pub fn stop(mut self, renderer: &mut GlesRenderer) {
        if let Some((texture, fbo)) = self.copy.take() {
            let _ = renderer.with_context(|gl| unsafe {
                gl.DeleteFramebuffers(1, &fbo);
                gl.DeleteTextures(1, &texture);
            });
        }
        log::info!(
            "recording stopped after {:.0} s: {} frame(s)",
            self.started.elapsed().as_secs_f64(),
            self.frames
        );
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        spatiand_audio::server::record_heard(None);
        // Nothing more is drawn into the encoder once its surface is gone; then it is told the
        // stream has ended, and the thread finishes the file.
        self.surface = None;
        self.stop.store(true, Ordering::SeqCst);
    }
}

struct Muxer(*mut ndk::AMediaMuxer);

unsafe impl Send for Muxer {}

impl Drop for Muxer {
    fn drop(&mut self) {
        unsafe { ndk::AMediaMuxer_delete(self.0) };
    }
}

/// A coded packet held until the file has all its tracks and can start.
struct Held {
    track: usize,
    bytes: Vec<u8>,
    pts_us: i64,
    flags: u32,
}

/// Where each encoder's packets go: its format once known, and its track in the file.
struct Output {
    format: Option<*mut ndk::AMediaFormat>,
    track: Option<usize>,
    ended: bool,
}

/// The recorder's thread: feed the sound in, take coded packets out, write the file.
fn run(video: Codec, mut sounds: Vec<Sound>, muxer: Muxer, started: Instant, stop: &AtomicBool) -> Result<f64, String> {
    let codecs: Vec<*mut ndk::AMediaCodec> =
        std::iter::once(video.0).chain(sounds.iter().map(|s| s.codec.0)).collect();
    let mut outputs: Vec<Output> = codecs.iter().map(|_| Output { format: None, track: None, ended: false }).collect();
    let mut held: Vec<Held> = Vec::new();
    let mut muxing = false;
    let mut stopping_since: Option<Instant> = None;
    let mut ended_at = Duration::ZERO;

    loop {
        let stopping = stop.load(Ordering::SeqCst);
        if stopping && stopping_since.is_none() {
            stopping_since = Some(Instant::now());
            ended_at = started.elapsed();
            unsafe { ndk::AMediaCodec_signalEndOfInputStream(video.0) };
        }

        // --- sound in ---
        let clock = if stopping { ended_at } else { started.elapsed() };
        for sound in sounds.iter_mut() {
            if sound.ended {
                continue;
            }
            feed(sound, clock, stopping);
        }

        // --- packets out ---
        for (i, codec) in codecs.iter().enumerate() {
            loop {
                let mut info = ndk::AMediaCodecBufferInfo { offset: 0, size: 0, presentationTimeUs: 0, flags: 0 };
                let index = unsafe { ndk::AMediaCodec_dequeueOutputBuffer(*codec, &mut info, 0) };
                if index == ndk::AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED as isize {
                    outputs[i].format = Some(unsafe { ndk::AMediaCodec_getOutputFormat(*codec) });
                    continue;
                }
                if index < 0 {
                    break;
                }
                let flags = info.flags;
                if flags & ndk::AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM as u32 != 0 {
                    outputs[i].ended = true;
                }
                let is_config = flags & ndk::AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG as u32 != 0;
                if info.size > 0 && !is_config {
                    let mut size = 0usize;
                    let buffer = unsafe { ndk::AMediaCodec_getOutputBuffer(*codec, index as usize, &mut size) };
                    if !buffer.is_null() {
                        let bytes = unsafe {
                            std::slice::from_raw_parts(buffer.add(info.offset as usize), info.size as usize)
                        };
                        match outputs[i].track {
                            Some(track) if muxing => write(&muxer, track, bytes, info.presentationTimeUs, flags),
                            _ => held.push(Held {
                                track: i,
                                bytes: bytes.to_vec(),
                                pts_us: info.presentationTimeUs,
                                flags,
                            }),
                        }
                    }
                }
                unsafe { ndk::AMediaCodec_releaseOutputBuffer(*codec, index as usize, false) };
            }
        }

        // --- the file starts once every track has said what it is ---
        if !muxing && outputs.iter().all(|o| o.format.is_some()) {
            for (i, output) in outputs.iter_mut().enumerate() {
                let track = unsafe { ndk::AMediaMuxer_addTrack(muxer.0, output.format.unwrap()) };
                if track < 0 {
                    // Some muxers take one sound track only: the rest are left out.
                    log::warn!(
                        "recording: the file will not hold {}",
                        if i == 0 { "the picture" } else { sounds[i - 1].title }
                    );
                    if i == 0 {
                        return Err("the file would not take the picture".into());
                    }
                } else {
                    output.track = Some(track as usize);
                }
            }
            if unsafe { ndk::AMediaMuxer_start(muxer.0) } != ndk::media_status_t::AMEDIA_OK {
                return Err("the file would not start".into());
            }
            muxing = true;
            for h in held.drain(..) {
                if let Some(track) = outputs[h.track].track {
                    write(&muxer, track, &h.bytes, h.pts_us, h.flags);
                }
            }
        }

        if stopping {
            let done = outputs.iter().all(|o| o.ended);
            let waited = stopping_since.map(|t| t.elapsed()).unwrap_or_default();
            if done || waited > Duration::from_secs(3) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    for output in &outputs {
        if let Some(format) = output.format {
            unsafe { ndk::AMediaFormat_delete(format) };
        }
    }
    if !muxing {
        return Err("nothing was recorded".into());
    }
    if unsafe { ndk::AMediaMuxer_stop(muxer.0) } != ndk::media_status_t::AMEDIA_OK {
        return Err("the file would not close".into());
    }
    drop(muxer);
    drop(sounds);
    drop(video);
    Ok(ended_at.as_secs_f64())
}

fn write(muxer: &Muxer, track: usize, bytes: &[u8], pts_us: i64, flags: u32) {
    let info = ndk::AMediaCodecBufferInfo {
        offset: 0,
        size: bytes.len() as i32,
        presentationTimeUs: pts_us,
        flags: flags & !(ndk::AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM as u32),
    };
    unsafe { ndk::AMediaMuxer_writeSampleData(muxer.0, track, bytes.as_ptr(), &info) };
}

/// Give one track's encoder what its ring holds, and silence for a gap longer than [`LAG`]; at
/// the end, the rest and the end of the stream.
fn feed(sound: &mut Sound, clock: Duration, ending: bool) {
    let due = (clock.saturating_sub(if ending { Duration::ZERO } else { LAG }).as_secs_f64() * RATE as f64) as u64;
    loop {
        let waiting = sound.ring.available() / sound.channels;
        let gap = due.saturating_sub(sound.frames + waiting as u64);
        if waiting == 0 && gap == 0 {
            break;
        }
        let index = unsafe { ndk::AMediaCodec_dequeueInputBuffer(sound.codec.0, 0) };
        if index < 0 {
            return;
        }
        let mut capacity = 0usize;
        let buffer = unsafe { ndk::AMediaCodec_getInputBuffer(sound.codec.0, index as usize, &mut capacity) };
        if buffer.is_null() {
            return;
        }
        let room = (capacity / (2 * sound.channels)).min(1024);
        let (frames, samples) = if waiting > 0 {
            let frames = waiting.min(room);
            let mut floats = vec![0f32; frames * sound.channels];
            sound.ring.read(&mut floats);
            (frames, floats)
        } else {
            let frames = (gap as usize).min(room);
            (frames, vec![0f32; frames * sound.channels])
        };
        let out = unsafe { std::slice::from_raw_parts_mut(buffer as *mut i16, frames * sound.channels) };
        for (o, s) in out.iter_mut().zip(&samples) {
            *o = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        }
        let pts_us = (sound.frames * 1_000_000 / RATE as u64) as u64;
        unsafe {
            ndk::AMediaCodec_queueInputBuffer(sound.codec.0, index as usize, 0, frames * 2 * sound.channels, pts_us, 0);
        }
        sound.frames += frames as u64;
    }
    if ending {
        let index = unsafe { ndk::AMediaCodec_dequeueInputBuffer(sound.codec.0, 0) };
        if index >= 0 {
            let pts_us = sound.frames * 1_000_000 / RATE as u64;
            unsafe {
                ndk::AMediaCodec_queueInputBuffer(
                    sound.codec.0,
                    index as usize,
                    0,
                    0,
                    pts_us,
                    ndk::AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM as u32,
                );
            }
            sound.ended = true;
        }
    }
}

fn stamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .to_string()
}

/// Where recordings are written before the app publishes them: `SPATIAND_RECORDINGS`, which the
/// app sets to its own `Movies` directory.
fn directory() -> PathBuf {
    std::env::var("SPATIAND_RECORDINGS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join("Movies"))
}
