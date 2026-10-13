//! Where the sound goes, and the clock it keeps.
//!
//! The device asks for samples when it wants them, on a thread of its own, and is given the
//! next of what the decoder has made. Each block of samples knows what moment of the film it
//! belongs to, so the moment being *heard* is known exactly -- and that is the film's clock:
//! pictures are shown against it, never the other way round, because an ear notices a sound
//! that stumbles long before an eye notices a picture a few milliseconds late.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// How much decoded sound waits to be heard, in seconds: enough that a busy moment is not a
/// gap, little enough that a seek does not keep the decoder busy filling it.
const AHEAD: f64 = 0.35;

/// Samples for every channel in turn, and when in the film the first of them is.
pub(crate) struct Block {
    pub samples: Vec<f32>,
    /// Not a number, for a block whose packet did not say.
    pub time: f64,
    pub serial: u64,
}

#[derive(Default)]
struct Waiting {
    blocks: VecDeque<(Vec<f32>, f64, u64)>,
    /// How far into the first block the device has got, in samples.
    taken: usize,
    /// Samples waiting, over all blocks.
    samples: usize,
}

/// The film's time as last heard: this moment of the film reaches the ear at `at`.
#[derive(Clone, Copy)]
struct Heard {
    time: f64,
    at: Instant,
    serial: u64,
    /// The device is being given sound, so time is passing.
    running: bool,
}

struct Inner {
    waiting: Mutex<Waiting>,
    heard: Mutex<Option<Heard>>,
    paused: AtomicBool,
    volume: AtomicU32,
    rate: u32,
    channels: u16,
}

pub(crate) struct Sound {
    inner: std::sync::Arc<Inner>,
    /// The device's stream, which plays for as long as this is kept. Not `Send` on every
    /// platform, and never touched again: it is here only to be dropped.
    stream: Mutex<Option<SendStream>>,
}

struct SendStream(#[allow(dead_code)] cpal::Stream);
// SAFETY: the stream is made, played, and then only ever dropped; nothing calls into it from
// two threads.
unsafe impl Send for SendStream {}

impl Sound {
    pub(crate) fn open() -> Result<Sound, String> {
        let device = cpal::default_host().default_output_device().ok_or("this machine has nowhere for sound to go")?;
        let supported = device.default_output_config().map_err(|e| format!("the sound device would not say what it takes: {e}"))?;
        let config: cpal::StreamConfig = supported.config();
        let (rate, channels) = (config.sample_rate.0, config.channels);
        let inner = std::sync::Arc::new(Inner {
            waiting: Mutex::new(Waiting::default()),
            heard: Mutex::new(None),
            paused: AtomicBool::new(false),
            // `SPATIAND_MEDIA_MUTE=1` keeps the device and its clock and plays nothing: a film
            // looked at by a test should not be heard by whoever is at the machine.
            volume: AtomicU32::new(if std::env::var_os("SPATIAND_MEDIA_MUTE").is_some() { 0f32 } else { 1f32 }.to_bits()),
            rate,
            channels,
        });
        let feed = inner.clone();
        let stream = device
            .build_output_stream(
                &config,
                move |out: &mut [f32], info: &cpal::OutputCallbackInfo| {
                    let stamp = info.timestamp();
                    // How long from now until what is written here is heard.
                    let delay = stamp.playback.duration_since(&stamp.callback).unwrap_or(Duration::ZERO);
                    feed.fill(out, Instant::now() + delay);
                },
                |e| log::warn!("the sound device: {e}"),
                None,
            )
            .map_err(|e| format!("the sound device would not open: {e}"))?;
        stream.play().map_err(|e| format!("the sound device would not start: {e}"))?;
        log::info!("sound: {channels} channels at {rate} Hz");
        Ok(Sound { inner, stream: Mutex::new(Some(SendStream(stream))) })
    }

    pub(crate) fn rate(&self) -> u32 {
        self.inner.rate
    }

    pub(crate) fn channels(&self) -> u16 {
        self.inner.channels
    }

    /// Take a block if there is room for it; `false` is "not yet".
    pub(crate) fn offer(&self, block: &Block) -> bool {
        let mut waiting = self.inner.waiting.lock().unwrap();
        let most = (AHEAD * self.inner.rate as f64) as usize * self.inner.channels as usize;
        if waiting.samples >= most {
            return false;
        }
        waiting.samples += block.samples.len();
        waiting.blocks.push_back((block.samples.clone(), block.time, block.serial));
        true
    }

    pub(crate) fn clear(&self) {
        let mut waiting = self.inner.waiting.lock().unwrap();
        *waiting = Waiting::default();
        if let Some(heard) = self.inner.heard.lock().unwrap().as_mut() {
            heard.running = false;
        }
    }

    pub(crate) fn empty(&self) -> bool {
        self.inner.waiting.lock().unwrap().samples == 0
    }

    pub(crate) fn set_paused(&self, paused: bool) {
        // Frozen where it is, and started again from there.
        let now = self.time_now();
        self.inner.paused.store(paused, Ordering::Relaxed);
        if let (Some(heard), Some(now)) = (self.inner.heard.lock().unwrap().as_mut(), now) {
            heard.time = now;
            heard.at = Instant::now();
            heard.running = false;
        }
    }

    pub(crate) fn set_volume(&self, volume: f32) {
        self.inner.volume.store(volume.clamp(0.0, 1.5).to_bits(), Ordering::Relaxed);
    }

    fn time_now(&self) -> Option<f64> {
        let heard = (*self.inner.heard.lock().unwrap())?;
        let now = Instant::now();
        Some(if heard.running && now >= heard.at {
            heard.time + (now - heard.at).as_secs_f64()
        } else if heard.running {
            heard.time - (heard.at - now).as_secs_f64()
        } else {
            heard.time
        })
    }

    /// The moment of the film being heard, once something since the seek numbered `serial` has
    /// been.
    pub(crate) fn time(&self, serial: u64) -> Option<f64> {
        let heard = (*self.inner.heard.lock().unwrap())?;
        if heard.serial != serial {
            return None;
        }
        self.time_now()
    }

    pub(crate) fn stop(&self) {
        self.stream.lock().unwrap().take();
    }
}

impl Inner {
    /// The device's call: fill `out`, all of which will be heard starting at `at`.
    fn fill(&self, out: &mut [f32], at: Instant) {
        let volume = f32::from_bits(self.volume.load(Ordering::Relaxed));
        let mut written = 0;
        let mut first: Option<(f64, u64)> = None;
        if !self.paused.load(Ordering::Relaxed) {
            let mut guard = self.waiting.lock().unwrap();
            let waiting = &mut *guard;
            while written < out.len() {
                let taken = waiting.taken;
                let Some((samples, time, serial)) = waiting.blocks.front() else { break };
                if first.is_none() && time.is_finite() {
                    // The moment of the film the first sample written here belongs to.
                    let into = (taken / self.channels as usize) as f64 / self.rate as f64;
                    let before = (written / self.channels as usize) as f64 / self.rate as f64;
                    first = Some((time + into - before, *serial));
                }
                let take = (samples.len() - taken).min(out.len() - written);
                for (to, from) in out[written..written + take].iter_mut().zip(&samples[taken..taken + take]) {
                    *to = from * volume;
                }
                written += take;
                waiting.taken += take;
                waiting.samples -= take;
                if waiting.taken >= samples.len() {
                    waiting.blocks.pop_front();
                    waiting.taken = 0;
                }
            }
        }
        out[written..].fill(0.0);
        let mut heard = self.heard.lock().unwrap();
        match first {
            Some((time, serial)) => *heard = Some(Heard { time, at, serial, running: true }),
            // Nothing to give. Time goes on from where it was -- at the end of a film whose
            // sound is shorter than its pictures, that is what keeps the pictures coming.
            None => {}
        }
    }
}
