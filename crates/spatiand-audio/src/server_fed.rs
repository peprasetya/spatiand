//! The audio engine where sound is handed over rather than played into a sink: the Deck's
//! placing, on AAudio (Android) or Core Audio (the Mac) instead of PipeWire.
//!
//! On the Deck an application plays into a sink of its window's, and the engine renders what
//! arrives there binaurally -- a measured head through [`crate::hrtf`] when there is one,
//! [`crate::Panner`] when not -- and plays it out. On the Beam Pro every window is a remote one
//! whose sound arrives over the network, so there is no sink to play into: the session hands
//! each window's sound to [`feed`] instead, and from there it is the Deck's path exactly -- the
//! same [`Binaural`] renderer, aimed by the same calls from the compositor, into a per-window
//! queue -- with one AAudio stream mixing the queues out to whatever the phone is playing to.
//!
//! On the Mac it is the same: remote windows' sound arrives over the network, and this Mac's own
//! applications' sound is tapped by the app and handed to [`feed`] too.
//!
//! The interface is `server.rs`'s, so the compositor drives it unchanged. What differs between
//! the two platforms is only the device at the end, which is [`device`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(target_os = "android")]
#[path = "out_aaudio.rs"]
mod device;
#[cfg(target_os = "macos")]
#[path = "out_coreaudio.rs"]
mod device;

use crate::render::{Binaural, Directness};
use crate::ring::Ring;
use crate::stage::{Layout, Speaker};

pub const ROUTING_ENV: &str = "PIPEWIRE_PROPS";
/// See `server.rs`; nothing is recorded here yet.
pub const SURROUND_SINK: &str = "spatiand.recording.surround";
pub const BED_CHANNELS: usize = 12;
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
    /// The same sound as 7.1.4 speakers before the HRTF, frame for frame with `queue`, while a
    /// recording wants it; see [`record_surround`].
    bed: Ring,
    /// Whether `bed` has been lined up with `queue` since the recording began.
    bed_lined_up: AtomicBool,
    /// Whether the queue has its cushion and is being played; see [`read_cushioned`].
    primed: AtomicBool,
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

/// Everything the engine shares with the session's sound threads and the device's callback.
struct Shared {
    slots: Mutex<HashMap<Slot, Arc<SlotState>>>,
    /// Sound with no window to be placed at, or with placing off: played as it came.
    unplaced: Ring,
    unplaced_primed: AtomicBool,
    /// The session's own short sounds -- the keyboard's click -- written only by its render
    /// thread, since a ring has one writer.
    cues: Ring,
    /// Where what is played goes as well, while a video is being recorded: "what you heard".
    /// Written only by AAudio's callback.
    heard: Mutex<Option<Arc<Ring>>>,
    /// Where the 7.1.4 surround mix goes while a recording wants it. Written only by the callback.
    surround: Mutex<Option<Arc<Ring>>>,
    head: Mutex<Head>,
    directness: Mutex<Directness>,
    rate: AtomicU32,
    /// Where to play: the platform's audio device id, or 0 for wherever the system routes it.
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
        unplaced_primed: AtomicBool::new(false),
        cues: Ring::new(QUEUE_FRAMES * 2),
        heard: Mutex::new(None),
        surround: Mutex::new(None),
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
    // The recording's surround: the same aim, as speakers instead of ears. Lined up first with
    // what is already waiting in `queue`, so the two come out of the callback together.
    if SURROUND_ON.load(Ordering::Relaxed) {
        if !state.bed_lined_up.swap(true, Ordering::Relaxed) {
            let waiting = state.queue.available() / 2;
            state.bed.clear();
            state.bed.write(&vec![0.0; waiting * SURROUND_CHANNELS]);
        }
        let mut bed = vec![0.0f32; frames * SURROUND_CHANNELS];
        render.render_bed(&input[..frames * channels], &mut bed);
        state.bed.write(&bed);
    } else {
        state.bed_lined_up.store(false, Ordering::Relaxed);
    }
    state.queue.write(&output);
}

/// Copy what the output plays, stereo at the engine's rate, into `into` from now on, or stop.
pub fn record_heard(into: Option<Arc<Ring>>) {
    *shared().heard.lock().unwrap() = into;
}

/// A short sound of the session's own, mono 16-bit at the engine's rate, played at once and
/// unplaced. From one thread only: the render loop's.
///
/// On the Deck the click is its own `pw-cat`; here there is one output, and it is this.
pub fn cue(pcm: &[i16]) {
    let stereo: Vec<f32> = pcm
        .iter()
        .flat_map(|s| {
            let v = *s as f32 / 32768.0;
            [v, v]
        })
        .collect();
    shared().cues.write(&stereo);
}

/// Play to this audio device from now on -- the platform's own id for it -- or 0 for wherever
/// the system routes it.
pub fn set_output_device(id: i32) {
    let shared = shared();
    if shared.device.swap(id, Ordering::SeqCst) != id {
        shared.reopen.store(true, Ordering::SeqCst);
    }
}

/// Fill `out`, interleaved stereo, with everything that is to be heard now. The device's own
/// callback calls this, on its own thread.
fn mix(out: &mut [f32]) {
    let shared = shared();
    out.fill(0.0);
    let mut scratch = vec![0.0f32; out.len()];
    // Short reads come back as silence rather than a stall, as on the Deck.
    let rate = shared.rate.load(Ordering::Relaxed) as usize;
    let frames = out.len() / 2;
    let surround = shared.surround.try_lock().ok().and_then(|s| s.clone());
    let mut bed = vec![0.0f32; if surround.is_some() { frames * SURROUND_CHANNELS } else { 0 }];
    let mut bed_scratch = bed.clone();
    read_cushioned(&shared.unplaced, &shared.unplaced_primed, &mut scratch, rate, None);
    for (o, s) in out.iter_mut().zip(&scratch) {
        *o += *s;
    }
    // Unplaced sound -- a VR application's own mix -- is the front pair of the surround, as is.
    if surround.is_some() {
        for (f, pair) in scratch.chunks_exact(2).enumerate() {
            bed[f * SURROUND_CHANNELS] += pair[0];
            bed[f * SURROUND_CHANNELS + 1] += pair[1];
        }
    }
    // The session's own clicks are made here and never late: straight out.
    scratch.fill(0.0);
    shared.cues.read(&mut scratch);
    for (o, s) in out.iter_mut().zip(&scratch) {
        *o += *s;
    }
    if let Ok(slots) = shared.slots.try_lock() {
        for state in slots.values() {
            let companion = surround.is_some().then(|| Companion { ring: &state.bed, out: &mut bed_scratch });
            read_cushioned(&state.queue, &state.primed, &mut scratch, rate, companion);
            for (o, s) in out.iter_mut().zip(&scratch) {
                *o += *s;
            }
            for (b, s) in bed.iter_mut().zip(&bed_scratch) {
                *b += *s;
            }
        }
    }
    for o in out.iter_mut() {
        *o = o.clamp(-1.0, 1.0);
    }
    // Never waits: a callback that cannot look this once leaves the recording a gap, which
    // the recorder fills with silence, rather than the output a glitch.
    if let Ok(heard) = shared.heard.try_lock() {
        if let Some(ring) = heard.as_ref() {
            ring.write(out);
        }
    }
    if let Some(ring) = surround {
        ring.write(&bed);
    }
}

/// The device went away under the output: open it again.
fn lost() {
    shared().reopen.store(true, Ordering::SeqCst);
}

/// How much sound from the network waits before it is played: what a burst of WiFi can be
/// late by without being heard -- 80 ms was not enough on the Beam Pro's WiFi, which loses
/// bursts of packets. The Deck's is the pipe into `pw-cat`, 170 ms of it; AAudio
/// asks for a few milliseconds at a time and a queue read that thinly runs dry at every late
/// packet, which is heard as a jitter.
const CUSHION_MS: usize = 150;
/// More than this waiting and the oldest is dropped, so a stall does not leave the sound
/// behind the picture for ever after.
const MOST_MS: usize = 400;

/// Read stereo from a queue of network sound, playing only once it has its cushion, and
/// gathering the cushion again whenever it runs dry.
/// How often network sound ran dry while playing, and how often too much was waiting and was
/// dropped: said every ten seconds, to tell a late network from a late phone.
static RAN_DRY: AtomicU32 = AtomicU32::new(0);
static TRIMMED: AtomicU32 = AtomicU32::new(0);

/// A queue's companion, read frame for frame with it: its 7.1.4 surround while recording.
struct Companion<'a> {
    ring: &'a Ring,
    out: &'a mut [f32],
}

fn read_cushioned(ring: &Ring, primed: &AtomicBool, out: &mut [f32], rate: usize, mut companion: Option<Companion>) {
    let cushion = rate * 2 * CUSHION_MS / 1000;
    let most = rate * 2 * MOST_MS / 1000;
    let waiting = ring.available();
    if let Some(c) = companion.as_mut() {
        c.out.fill(0.0);
    }
    if !primed.load(Ordering::Relaxed) {
        if waiting < cushion.max(out.len()) {
            out.fill(0.0);
            return;
        }
        primed.store(true, Ordering::Relaxed);
    }
    if waiting > most + out.len() {
        TRIMMED.fetch_add(1, Ordering::Relaxed);
        let mut dropped = vec![0.0f32; (waiting - cushion) & !1];
        ring.read(&mut dropped);
        if let Some(c) = companion.as_mut() {
            let mut dropped = vec![0.0f32; dropped.len() / 2 * SURROUND_CHANNELS];
            c.ring.read(&mut dropped);
        }
    }
    if let Some(c) = companion.as_mut() {
        c.ring.read(c.out);
    }
    if ring.read(out) > 0 {
        RAN_DRY.fetch_add(1, Ordering::Relaxed);
        primed.store(false, Ordering::Relaxed);
    }
}

/// Channels in a recording's surround track: the Deck's 7.1.4 bed, in its order --
/// FL FR FC LFE BL BR SL SR TFL TFR TBL TBR.
pub const SURROUND_CHANNELS: usize = BED_CHANNELS;

static SURROUND_ON: AtomicBool = AtomicBool::new(false);

/// Mix the 7.1.4 surround -- every window's sound on the speakers it points at, before the HRTF,
/// and unplaced sound on the front pair as it is -- into `into` from now on, or stop.
pub fn record_surround(into: Option<Arc<Ring>>) {
    SURROUND_ON.store(into.is_some(), Ordering::Relaxed);
    *shared().surround.lock().unwrap() = into;
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
                let mut output = device::open_output(rate, 0);
                let mut said = std::time::Instant::now();
                let mut xruns_before = 0;
                while shared.running.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    if said.elapsed() >= std::time::Duration::from_secs(10) {
                        said = std::time::Instant::now();
                        let dry = RAN_DRY.swap(0, Ordering::Relaxed);
                        let trimmed = TRIMMED.swap(0, Ordering::Relaxed);
                        // Late network: the queues run dry. Late phone: the output itself
                        // underran (AAudio's xruns), with sound waiting to go.
                        let xruns = output.as_ref().map(|o| o.xruns()).unwrap_or(0);
                        let new_xruns = xruns - xruns_before;
                        xruns_before = xruns;
                        if dry + trimmed > 0 || new_xruns > 0 {
                            log::info!(
                                "spatial audio: in 10 s, network sound ran dry {dry} times and was trimmed {trimmed}; the output underran {new_xruns} times"
                            );
                        }
                    }
                    if shared.reopen.swap(false, Ordering::SeqCst) || output.is_none() {
                        // The old one closes before the new one opens: two on one device fight.
                        drop(output.take());
                        output = device::open_output(rate, shared.device.load(Ordering::SeqCst));
                        xruns_before = 0;
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
        log::info!("spatial audio: running at {rate} Hz, on {}", device::NAME);
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
                bed: Ring::new(QUEUE_FRAMES * SURROUND_CHANNELS),
                bed_lined_up: AtomicBool::new(false),
                primed: AtomicBool::new(false),
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

    /// Accepted and ignored, so the session drives both engines the same way: the Beam Pro's
    /// recorder asks for its tracks directly -- [`record_heard`] and [`record_surround`].
    pub fn record(&self, _into: Option<Arc<Ring>>) {}

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

/// Which audio device to record from: the platform's id, or 0 for the system's choice.
static INPUT_DEVICE: AtomicI32 = AtomicI32::new(0);

/// Record from this audio device from the next capture on; 0 for the system's choice.
pub fn set_input_device(id: i32) {
    INPUT_DEVICE.store(id, Ordering::SeqCst);
}

pub use device::Capture;

/// What a capture hands its samples to.
pub type Sink = Box<dyn FnMut(&[i16]) + Send>;
