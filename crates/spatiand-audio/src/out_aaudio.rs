//! Android's end of the fed engine: one AAudio stream out, and the microphone in.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use ndk_sys as ndk;

use super::Sink;

pub const NAME: &str = "AAudio";

unsafe extern "C" fn lost(_stream: *mut ndk::AAudioStream, _user: *mut c_void, error: ndk::aaudio_result_t) {
    log::info!("spatial audio: the output went away ({error}); opening it again");
    super::lost();
}

unsafe extern "C" fn play(
    _stream: *mut ndk::AAudioStream,
    _user: *mut c_void,
    data: *mut c_void,
    frames: i32,
) -> ndk::aaudio_data_callback_result_t {
    super::mix(std::slice::from_raw_parts_mut(data as *mut f32, frames.max(0) as usize * 2));
    ndk::AAUDIO_CALLBACK_RESULT_CONTINUE as ndk::aaudio_data_callback_result_t
}

/// One AAudio output stream, closed when dropped.
pub struct Output(*mut ndk::AAudioStream);

impl Output {
    /// How often the device ran out of sound to play.
    pub fn xruns(&self) -> i32 {
        unsafe { ndk::AAudioStream_getXRunCount(self.0) }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        unsafe { ndk::AAudioStream_close(self.0) };
    }
}

pub fn open_output(rate: u32, device: i32) -> Option<Output> {
    unsafe {
        let mut builder = std::ptr::null_mut();
        if ndk::AAudio_createStreamBuilder(&mut builder) != ndk::AAUDIO_OK as i32 {
            return None;
        }
        ndk::AAudioStreamBuilder_setFormat(builder, ndk::AAUDIO_FORMAT_PCM_FLOAT as i32);
        ndk::AAudioStreamBuilder_setChannelCount(builder, 2);
        ndk::AAudioStreamBuilder_setSampleRate(builder, rate as i32);
        ndk::AAudioStreamBuilder_setDeviceId(builder, device);
        ndk::AAudioStreamBuilder_setPerformanceMode(builder, ndk::AAUDIO_PERFORMANCE_MODE_LOW_LATENCY as i32);
        ndk::AAudioStreamBuilder_setDataCallback(builder, Some(play), std::ptr::null_mut());
        ndk::AAudioStreamBuilder_setErrorCallback(builder, Some(lost), std::ptr::null_mut());
        let mut stream = std::ptr::null_mut();
        let opened = ndk::AAudioStreamBuilder_openStream(builder, &mut stream);
        ndk::AAudioStreamBuilder_delete(builder);
        if opened != ndk::AAUDIO_OK as i32 || stream.is_null() {
            log::warn!("spatial audio: could not open an output ({opened})");
            return None;
        }
        let output = Output(stream);
        // Four bursts queued at the device rather than the fewest it will run on: a few more
        // milliseconds of delay, and a callback that is late once in a while is not heard.
        let burst = ndk::AAudioStream_getFramesPerBurst(stream);
        let buffer = ndk::AAudioStream_setBufferSizeInFrames(stream, burst * 4);
        if ndk::AAudioStream_requestStart(stream) != ndk::AAUDIO_OK as i32 {
            log::warn!("spatial audio: the output would not start");
            return None;
        }
        log::info!(
            "spatial audio: playing at {} Hz ({burst}-frame bursts, {buffer} buffered) to {}",
            ndk::AAudioStream_getSampleRate(stream),
            if device == 0 { "the default output".to_string() } else { format!("device {device}") }
        );
        Some(output)
    }
}

// --- the microphone ---

/// The microphone, being recorded: 16-bit samples handed to a sink as they arrive, on AAudio's
/// own thread. Stops when dropped. What `pw-record` is on the Deck.
pub struct Capture {
    stream: *mut ndk::AAudioStream,
    _sink: Box<Sink>,
}

unsafe impl Send for Capture {}

unsafe extern "C" fn captured(
    _stream: *mut ndk::AAudioStream,
    user: *mut c_void,
    data: *mut c_void,
    frames: i32,
) -> ndk::aaudio_data_callback_result_t {
    let sink = &mut *(user as *mut Sink);
    let channels = CAPTURE_CHANNELS.load(Ordering::Relaxed).max(1) as usize;
    let samples = std::slice::from_raw_parts(data as *const i16, frames.max(0) as usize * channels);
    sink(samples);
    ndk::AAUDIO_CALLBACK_RESULT_CONTINUE as ndk::aaudio_data_callback_result_t
}

static CAPTURE_CHANNELS: AtomicU32 = AtomicU32::new(1);

impl Capture {
    /// Record `channels` at `rate`, as a voice: Android's echo cancelling and noise
    /// suppression on, since it is a voice going to someone else.
    pub fn open(rate: u32, channels: u16, sink: Sink) -> Result<Capture, String> {
        let mut sink: Box<Sink> = Box::new(sink);
        CAPTURE_CHANNELS.store(channels as u32, Ordering::Relaxed);
        unsafe {
            let mut builder = std::ptr::null_mut();
            if ndk::AAudio_createStreamBuilder(&mut builder) != ndk::AAUDIO_OK as i32 {
                return Err("no AAudio".into());
            }
            ndk::AAudioStreamBuilder_setDirection(builder, ndk::AAUDIO_DIRECTION_INPUT as i32);
            ndk::AAudioStreamBuilder_setFormat(builder, ndk::AAUDIO_FORMAT_PCM_I16 as i32);
            ndk::AAudioStreamBuilder_setChannelCount(builder, channels as i32);
            ndk::AAudioStreamBuilder_setSampleRate(builder, rate as i32);
            ndk::AAudioStreamBuilder_setDeviceId(builder, super::INPUT_DEVICE.load(Ordering::SeqCst));
            ndk::AAudioStreamBuilder_setInputPreset(builder, ndk::AAUDIO_INPUT_PRESET_VOICE_COMMUNICATION as i32);
            ndk::AAudioStreamBuilder_setPerformanceMode(builder, ndk::AAUDIO_PERFORMANCE_MODE_LOW_LATENCY as i32);
            let user = &mut *sink as *mut Sink as *mut c_void;
            ndk::AAudioStreamBuilder_setDataCallback(builder, Some(captured), user);
            let mut stream = std::ptr::null_mut();
            let opened = ndk::AAudioStreamBuilder_openStream(builder, &mut stream);
            ndk::AAudioStreamBuilder_delete(builder);
            if opened != ndk::AAUDIO_OK as i32 || stream.is_null() {
                return Err(format!("the microphone would not open ({opened}); has the app been allowed to record?"));
            }
            if ndk::AAudioStream_requestStart(stream) != ndk::AAUDIO_OK as i32 {
                ndk::AAudioStream_close(stream);
                return Err("the microphone would not start".into());
            }
            Ok(Capture { stream, _sink: sink })
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        unsafe { ndk::AAudioStream_close(self.stream) };
    }
}
