//! The audio engine on Android: the Deck's placing, on AAudio instead of PipeWire.
//!
//! On the Deck an application plays into a sink of its window's, and the engine renders what
//! arrives there binaurally -- a measured head through [`crate::hrtf`] when there is one,
//! [`crate::Panner`] when not -- and plays it out. On the Beam Pro every window is a remote one
//! whose sound arrives over the network, so there is no sink to play into: the session hands
//! each window's sound to [`feed`] instead, and from there it is the Deck's path exactly -- the
//! same [`Binaural`] renderer, aimed by the same calls from the compositor, into a per-window
//! queue -- with one AAudio stream mixing the queues out to whatever the phone is playing to.
//!
//! The interface is `server.rs`'s, so the compositor drives it unchanged.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use ndk_sys as ndk;

use crate::render::{Binaural, Directness};
use crate::ring::Ring;
use crate::stage::{Layout, Speaker};

pub const ROUTING_ENV: &str = "PIPEWIRE_PROPS";
pub const PULSE_ROUTING_ENV: &str = "PULSE_SINK";

/// A quarter of a second of stereo, as the Deck's queue.
const QUEUE_FRAMES: usize = 12_000;

/// How wide a window's sink should be. See `server.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    Stereo,
    Full,
}

impl Width {
    pub fn layout(self) -> Layout {
        match self {
            Width::Stereo => Layout::Stereo,
            Width::Full => Layout::Surround714,
        }
    }
}

pub type Slot = u64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Status {
    pub layout: Option<Layout>,
    pub sounding: Option<Layout>,
    pub peak: f32,
    pub muted: bool,
}

pub const SINK_DESCRIPTION: &str = "Spatiand window";

pub fn sink_name(slot: Slot) -> String {
    format!("spatiand.window.{slot}")
}

/// The slot a sink name names, for the session, which knows windows' sinks by name.
pub fn slot_of_sink(name: &str) -> Option<Slot> {
    name.strip_prefix("spatiand.window.")?.parse().ok()
}

pub fn routing_env(slot: Slot) -> Vec<(String, String)> {
    let sink = sink_name(slot);
    vec![
        (ROUTING_ENV.to_string(), format!("{{ target.object = \"{sink}\" }}")),
        (PULSE_ROUTING_ENV.to_string(), sink),
    ]
}

#[derive(Debug, Clone, Copy)]
pub enum Head {
    Measured,
    Reasoned,
}

/// One window's sound: its renderer, and the stereo waiting to go out.
struct SlotState {
    queue: Ring,
    render: Mutex<Option<Binaural>>,
    width: Layout,
    aim: Mutex<Option<(Vec<Speaker>, f64)>>,
    muted: AtomicBool,
    peak: AtomicU32,
    sounding: AtomicU32,
}

fn layout_code(layout: Layout) -> u32 {
    match layout {
        Layout::Mono => 1,
        Layout::Stereo => 2,
        Layout::Surround51 => 3,
        Layout::Surround71 => 4,
        Layout::Surround714 => 5,
        Layout::Surround514 => 6,
    }
}

fn layout_from_code(code: u32) -> Option<Layout> {
    Some(match code {
        1 => Layout::Mono,
        2 => Layout::Stereo,
        3 => Layout::Surround51,
        4 => Layout::Surround71,
        5 => Layout::Surround714,
        6 => Layout::Surround514,
        _ => return None,
    })
}

/// Everything the engine shares with the session's sound threads and AAudio's callback.
struct Shared {
    slots: Mutex<HashMap<Slot, Arc<SlotState>>>,
    /// Sound with no window to be placed at, or with placing off: played as it came.
    unplaced: Ring,
    head: Mutex<Head>,
    directness: Mutex<Directness>,
    rate: AtomicU32,
    /// Where to play: an Android audio device id, or 0 for wherever Android routes it.
    device: AtomicI32,
    /// The stream went away -- the device was unplugged -- or a new device was chosen.
    reopen: AtomicBool,
    running: AtomicBool,
}

fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(|| Shared {
        slots: Mutex::new(HashMap::new()),
        unplaced: Ring::new(QUEUE_FRAMES * 2),
        head: Mutex::new(Head::Measured),
        directness: Mutex::new(Directness::default()),
        rate: AtomicU32::new(48_000),
        device: AtomicI32::new(0),
        reopen: AtomicBool::new(false),
        running: AtomicBool::new(false),
    })
}

/// The measured head if the machine has one, as `server.rs`'s.
fn make_head(head: Head, rate: u32) -> Box<dyn crate::Spatialise> {
    match head {
        Head::Measured => match crate::hrtf::Hrtf::system(rate) {
            Ok(h) => Box::new(h),
            Err(e) => {
                log::warn!("spatial audio: no measured head ({e}); sounds will be placed but not convincingly behind you");
                Box::new(crate::Panner::new(rate))
            }
        },
        Head::Reasoned => Box::new(crate::Panner::new(rate)),
    }
}

/// A window's sound, as it arrives: interleaved 16-bit, `channels` of it. `None` plays it
/// unplaced.
pub fn feed(slot: Option<Slot>, pcm: &[i16], channels: usize) {
    let shared = shared();
    if channels == 0 || pcm.is_empty() {
        return;
    }
    let input: Vec<f32> = pcm.iter().map(|s| *s as f32 / 32768.0).collect();
    let state = slot.and_then(|slot| shared.slots.lock().ok()?.get(&slot).cloned());
    let Some(state) = state else {
        // Unplaced: straight to stereo.
        let frames = input.len() / channels;
        let mut stereo = Vec::with_capacity(frames * 2);
        for frame in input.chunks(channels) {
            let left = frame[0];
            let right = if channels > 1 { frame[1] } else { frame[0] };
            stereo.push(left);
            stereo.push(right);
        }
        shared.unplaced.write(&stereo);
        return;
    };
    let Ok(mut render) = state.render.lock() else { return };
    let layout = Layout::from_count(channels).unwrap_or(Layout::Stereo);
    if render.as_ref().is_none_or(|r| r.layout() != layout) {
        let rate = shared.rate.load(Ordering::Relaxed);
        let head = *shared.head.lock().unwrap();
        let directness = *shared.directness.lock().unwrap();
        log::info!("spatial audio: a window is sending {layout:?}");
        *render = Some(Binaural::new(layout, make_head(head, rate), directness, rate));
    }
    let Some(render) = render.as_mut() else { return };
    if let Ok(mut aim) = state.aim.try_lock() {
        if let Some((speakers, off_axis)) = aim.take() {
            render.aim(&speakers, off_axis);
        }
    }
    render.set_muted(state.muted.load(Ordering::Relaxed));
    let frames = input.len() / channels;
    let mut output = vec![0.0f32; frames * 2];
    render.render(&input[..frames * channels], &mut output);
    state.peak.store(render.peak().to_bits(), Ordering::Relaxed);
    state
        .sounding
        .store(render.sounding_layout().map(layout_code).unwrap_or(0), Ordering::Relaxed);
    state.queue.write(&output);
}

/// Play to this Android audio device from now on; 0 for wherever Android routes it.
pub fn set_output_device(id: i32) {
    let shared = shared();
    if shared.device.swap(id, Ordering::SeqCst) != id {
        shared.reopen.store(true, Ordering::SeqCst);
    }
}

unsafe extern "C" fn play(
    _stream: *mut ndk::AAudioStream,
    _user: *mut c_void,
    data: *mut c_void,
    frames: i32,
) -> ndk::aaudio_data_callback_result_t {
    let out = std::slice::from_raw_parts_mut(data as *mut f32, frames.max(0) as usize * 2);
    let shared = shared();
    out.fill(0.0);
    let mut scratch = vec![0.0f32; out.len()];
    // Short reads come back as silence rather than a stall, as on the Deck.
    shared.unplaced.read(&mut scratch);
    for (o, s) in out.iter_mut().zip(&scratch) {
        *o += *s;
    }
    if let Ok(slots) = shared.slots.try_lock() {
        for state in slots.values() {
            scratch.fill(0.0);
            state.queue.read(&mut scratch);
            for (o, s) in out.iter_mut().zip(&scratch) {
                *o += *s;
            }
        }
    }
    for o in out.iter_mut() {
        *o = o.clamp(-1.0, 1.0);
    }
    ndk::AAUDIO_CALLBACK_RESULT_CONTINUE as ndk::aaudio_data_callback_result_t
}

unsafe extern "C" fn lost(_stream: *mut ndk::AAudioStream, _user: *mut c_void, error: ndk::aaudio_result_t) {
    log::info!("spatial audio: the output went away ({error}); opening it again");
    shared().reopen.store(true, Ordering::SeqCst);
}

/// One AAudio output stream, closed when dropped.
struct Output(*mut ndk::AAudioStream);

impl Drop for Output {
    fn drop(&mut self) {
        unsafe { ndk::AAudioStream_close(self.0) };
    }
}

fn open_output(rate: u32, device: i32) -> Option<Output> {
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
        if ndk::AAudioStream_requestStart(stream) != ndk::AAUDIO_OK as i32 {
            log::warn!("spatial audio: the output would not start");
            return None;
        }
        log::info!(
            "spatial audio: playing at {} Hz to {}",
            ndk::AAudioStream_getSampleRate(stream),
            if device == 0 { "the default output".to_string() } else { format!("device {device}") }
        );
        Some(output)
    }
}

/// The engine: the session's handle on the windows' sound.
pub struct Engine {
    rate: u32,
}

impl Engine {
    pub fn start(rate: u32, head: Head, directness: Directness) -> Engine {
        let shared = shared();
        *shared.head.lock().unwrap() = head;
        *shared.directness.lock().unwrap() = directness;
        shared.rate.store(rate, Ordering::Relaxed);
        if !shared.running.swap(true, Ordering::SeqCst) {
            // One thread keeps the output open: opening one is not something to do from a
            // callback, and a device going away is reported from one.
            let _ = std::thread::Builder::new().name("spatial-audio".into()).spawn(move || {
                let mut output = open_output(rate, 0);
                while shared.running.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    if shared.reopen.swap(false, Ordering::SeqCst) || output.is_none() {
                        // The old one closes before the new one opens: two on one device fight.
                        drop(output.take());
                        output = open_output(rate, shared.device.load(Ordering::SeqCst));
                    }
                }
                drop(output);
            });
        }
        // Said once, at the start, rather than when the first sound arrives: whether sound can
        // come from behind is worth knowing before anything plays.
        if let Head::Measured = head {
            match crate::hrtf::Hrtf::system(rate) {
                Ok(h) => log::info!("spatial audio: a measured head, {} taps", h.taps()),
                Err(e) => log::warn!("spatial audio: no measured head ({e}); placing by panning"),
            }
        }
        log::info!("spatial audio: running at {rate} Hz, on AAudio");
        Engine { rate }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn open(&self, slot: Slot, width: Width) {
        let Ok(mut slots) = shared().slots.lock() else { return };
        slots.entry(slot).or_insert_with(|| {
            log::info!("spatial audio: opened {}", sink_name(slot));
            Arc::new(SlotState {
                queue: Ring::new(QUEUE_FRAMES * 2),
                render: Mutex::new(None),
                width: width.layout(),
                aim: Mutex::new(None),
                muted: AtomicBool::new(false),
                peak: AtomicU32::new(0),
                sounding: AtomicU32::new(0),
            })
        });
    }

    pub fn close(&self, slot: Slot) {
        if let Ok(mut slots) = shared().slots.lock() {
            slots.remove(&slot);
        }
    }

    pub fn aim(&self, slot: Slot, speakers: Vec<Speaker>, off_axis: f64) {
        if let Some(state) = shared().slots.lock().ok().and_then(|s| s.get(&slot).cloned()) {
            if let Ok(mut aim) = state.aim.lock() {
                *aim = Some((speakers, off_axis));
            }
        }
    }

    pub fn set_muted(&self, slot: Slot, muted: bool) {
        if let Some(state) = shared().slots.lock().ok().and_then(|s| s.get(&slot).cloned()) {
            state.muted.store(muted, Ordering::Relaxed);
        }
    }

    pub fn set_directness(&self, directness: Directness) {
        *shared().directness.lock().unwrap() = directness;
        if let Ok(slots) = shared().slots.lock() {
            for state in slots.values() {
                if let Ok(mut render) = state.render.lock() {
                    if let Some(render) = render.as_mut() {
                        render.set_directness(directness);
                    }
                }
            }
        }
    }

    pub fn status(&self, slot: Slot) -> Option<Status> {
        let state = shared().slots.lock().ok()?.get(&slot).cloned()?;
        let render = state.render.lock().ok()?;
        Some(Status {
            layout: render.as_ref().map(|r| r.layout()).or(Some(state.width)).filter(|_| render.is_some()),
            sounding: layout_from_code(state.sounding.load(Ordering::Relaxed)),
            peak: f32::from_bits(state.peak.load(Ordering::Relaxed)),
            muted: state.muted.load(Ordering::Relaxed),
        })
    }

    pub fn sounding(&self, floor: f32) -> Vec<(Slot, Status)> {
        let slots: Vec<Slot> = shared().slots.lock().map(|s| s.keys().copied().collect()).unwrap_or_default();
        let mut out: Vec<(Slot, Status)> = slots
            .into_iter()
            .filter_map(|slot| Some((slot, self.status(slot)?)))
            .filter(|(_, s)| s.peak >= floor)
            .collect();
        out.sort_by(|a, b| b.1.peak.total_cmp(&a.1.peak));
        out
    }
}

// --- the microphone ---

/// Which Android audio device to record from: an id, or 0 for wherever Android chooses.
static INPUT_DEVICE: AtomicI32 = AtomicI32::new(0);

/// Record from this Android audio device from the next capture on; 0 for Android's choice.
pub fn set_input_device(id: i32) {
    INPUT_DEVICE.store(id, Ordering::SeqCst);
}

type Sink = Box<dyn FnMut(&[i16]) + Send>;

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
            ndk::AAudioStreamBuilder_setDeviceId(builder, INPUT_DEVICE.load(Ordering::SeqCst));
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
