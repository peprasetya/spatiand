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

#[cfg(not(target_os = "linux"))]
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
    /// How fast the film is going: a second of sound heard is this many seconds of film.
    speed: AtomicU32,
    rate: u32,
    channels: u16,
    /// Where the sound was going has gone (the room's socket closed, `pw-cat` gone, the device
    /// unplugged): nothing is heard any more, so the film keeps time by the wall instead.
    dead: AtomicBool,
}

pub(crate) struct Sound {
    inner: std::sync::Arc<Inner>,
    /// The output it is mixed into, with whatever else shares it.
    bus: std::sync::Arc<Bus>,
}

/// Sounds that go out as one: a web page's films, its `<audio>` and its Web Audio are one
/// stream into the room -- one output, mixed here -- rather than one each. Every sound in it
/// keeps its own clock (what of *it* is being heard), so a film still keeps time.
///
/// A sound opened on its own ([`Stream::open`], a player's own) has a group of its own.
#[derive(Clone, Default)]
pub struct SoundGroup(std::sync::Arc<Bus>);

impl SoundGroup {
    pub fn new() -> SoundGroup {
        SoundGroup::default()
    }
}

#[derive(Default)]
struct Bus {
    voices: Mutex<Vec<std::sync::Arc<Inner>>>,
    /// The output, open while there is a voice to hear; its rate and channels.
    out: Mutex<Option<(Going, u32, u16)>>,
    dead: std::sync::Arc<AtomicBool>,
}

/// Each count of channels the room and the Deck's sink place, by name, in order.
fn channel_names(channels: u16) -> &'static [&'static str] {
    match channels {
        1 => &["MONO"],
        2 => &["FL", "FR"],
        6 => &["FL", "FR", "FC", "LFE", "RL", "RR"],
        8 => &["FL", "FR", "FC", "LFE", "RL", "RR", "SL", "SR"],
        10 => &["FL", "FR", "FC", "LFE", "RL", "RR", "TFL", "TFR", "TRL", "TRR"],
        _ => &["FL", "FR", "FC", "LFE", "RL", "RR", "SL", "SR", "TFL", "TFR", "TRL", "TRR"],
    }
}

/// The fewest channels that hold every one of these voices' own.
fn covering(counts: impl Iterator<Item = u16>) -> u16 {
    let counts: Vec<u16> = counts.collect();
    let holds = |layout: u16, voice: u16| {
        let names = channel_names(layout);
        channel_names(voice).iter().all(|n| names.contains(n) || (*n == "MONO" && names.contains(&"FL")))
    };
    crate::audio::SPATIAND_LAYOUTS.iter().map(|(c, _)| *c).find(|&layout| counts.iter().all(|&v| holds(layout, v))).unwrap_or(12)
}

impl Bus {
    /// The output's call: every voice's sound for the moment `at`, added together.
    fn mix(&self, out: &mut [f32], channels: u16, at: Instant, scratch: &mut Vec<f32>) {
        out.fill(0.0);
        let frames = out.len() / channels.max(1) as usize;
        let names = channel_names(channels);
        let voices = self.voices.lock().unwrap().clone();
        for voice in voices {
            let own = voice.channels as usize;
            scratch.resize(frames * own, 0.0);
            voice.fill(scratch, at);
            if own == channels as usize {
                for (to, from) in out.iter_mut().zip(scratch.iter()) {
                    *to += from;
                }
                continue;
            }
            // Each of the voice's channels to the output's of the same name; a mono voice to
            // the front two, as a voice from the middle.
            let to: Vec<Vec<(usize, f32)>> = channel_names(own as u16)
                .iter()
                .map(|n| match names.iter().position(|m| m == n) {
                    Some(i) => vec![(i, 1.0)],
                    None if *n == "MONO" && channels >= 2 => vec![(0, std::f32::consts::FRAC_1_SQRT_2), (1, std::f32::consts::FRAC_1_SQRT_2)],
                    None => Vec::new(),
                })
                .collect();
            for f in 0..frames {
                for (c, targets) in to.iter().enumerate() {
                    let v = scratch[f * own + c];
                    for &(i, gain) in targets {
                        out[f * channels as usize + i] += v * gain;
                    }
                }
            }
        }
    }

    /// Where the output has gone: none of its voices is heard any more.
    fn died(&self) {
        self.dead.store(true, Ordering::Relaxed);
        for voice in self.voices.lock().unwrap().iter() {
            voice.dead.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(not(target_os = "linux"))]
enum Going {
    Device(#[allow(dead_code)] cpal::Stream),
    /// Into the room: the thread writing to the session's socket, and the word that stops it.
    Room(std::sync::Arc<AtomicBool>),
}

#[cfg(not(target_os = "linux"))]
impl Drop for Going {
    fn drop(&mut self) {
        if let Going::Room(stop) = self {
            stop.store(true, Ordering::Relaxed);
        }
    }
}
// SAFETY: the stream is made, played, and then only ever dropped; nothing calls into it from
// two threads.
#[cfg(not(target_os = "linux"))]
unsafe impl Send for Going {}

/// On the Deck: the `pw-cat` the samples are written to, and the word that stops the thread
/// writing them.
#[cfg(target_os = "linux")]
struct Going {
    child: std::process::Child,
    stop: std::sync::Arc<AtomicBool>,
}

#[cfg(target_os = "linux")]
impl Drop for Going {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Sound {
    fn inner(rate: u32, channels: u16) -> std::sync::Arc<Inner> {
        std::sync::Arc::new(Inner {
            waiting: Mutex::new(Waiting::default()),
            heard: Mutex::new(None),
            paused: AtomicBool::new(false),
            // `SPATIAND_MEDIA_MUTE=1` keeps the device and its clock and plays nothing: a film
            // looked at by a test should not be heard by whoever is at the machine.
            volume: AtomicU32::new(if std::env::var_os("SPATIAND_MEDIA_MUTE").is_some() { 0f32 } else { 1f32 }.to_bits()),
            speed: AtomicU32::new(1f32.to_bits()),
            rate,
            channels,
            dead: AtomicBool::new(false),
        })
    }

    pub(crate) fn set_speed(&self, speed: f32) {
        // From where the film is now, at the new pace.
        let now = self.time_now();
        if let (Some(heard), Some(now)) = (self.inner.heard.lock().unwrap().as_mut(), now) {
            heard.time = now;
            heard.at = Instant::now();
        }
        self.inner.speed.store(speed.to_bits(), Ordering::Relaxed);
    }

    /// A sound for a track of `wanted` channels, mixed into `group`'s output -- opened now if
    /// it is not, or opened again with more channels if this needs channels it has not.
    pub(crate) fn open_in(group: &SoundGroup, wanted: u16) -> Result<Sound, String> {
        let bus = group.0.clone();
        let own = crate::audio::nearest_placeable(wanted.max(1));
        let mut out = bus.out.lock().unwrap();
        let needed = covering(bus.voices.lock().unwrap().iter().map(|v| v.channels).chain([own]));
        let fits = matches!(&*out, Some((_, _, channels)) if *channels == needed) && !bus.dead.load(Ordering::Relaxed);
        if !fits {
            // Closed first: the old output's thread stops before the new one starts.
            *out = None;
            bus.dead.store(false, Ordering::Relaxed);
            let (going, rate, channels) = Sound::open_output(&bus, needed)?;
            for voice in bus.voices.lock().unwrap().iter() {
                voice.dead.store(false, Ordering::Relaxed);
            }
            *out = Some((going, rate, channels));
        }
        let rate = out.as_ref().map_or(48_000, |(_, rate, _)| *rate);
        let inner = Sound::inner(rate, own);
        bus.voices.lock().unwrap().push(inner.clone());
        drop(out);
        Ok(Sound { inner, bus })
    }

    /// The output itself, for `channels` channels: the room's, or the machine's if there is
    /// no room to hand it to.
    #[cfg(not(target_os = "linux"))]
    fn open_output(bus: &std::sync::Arc<Bus>, channels: u16) -> Result<(Going, u32, u16), String> {
        match Sound::open_room(bus, channels) {
            Some(out) => Ok(out),
            None => Sound::open_device(bus),
        }
    }

    /// **Into the room**, where there is a session to hand sound to (the Mac, the Beam Pro):
    /// every channel of the film, placed at its window through Spatiand's HRTF and under the
    /// window's own mute and volume -- not straight out of the machine's speakers past it. The
    /// session's side is `crates/spatiand/src/own.rs`'s `sound`.
    #[cfg(not(target_os = "linux"))]
    fn open_room(bus: &std::sync::Arc<Bus>, wanted: u16) -> Option<(Going, u32, u16)> {
        use std::io::Write;
        if std::env::var("SPATIAND_MEDIA_ROOM").as_deref() == Ok("off") {
            return None;
        }
        let path = std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("spatiand-sound");
        let mut socket = std::os::unix::net::UnixStream::connect(&path).ok()?;
        let channels = crate::audio::nearest_placeable(wanted.max(1));
        let rate = 48_000u32;
        let mut head = b"SPSN".to_vec();
        head.extend_from_slice(&channels.to_le_bytes());
        head.extend_from_slice(&rate.to_le_bytes());
        // Whose sound it is: the session gives that application's windows the slot.
        let app = std::env::var("SPATIAND_APP_ID").unwrap_or_else(|_| "id.prasetya.spatiand.spatireel".into());
        head.extend_from_slice(&(app.len() as u16).to_le_bytes());
        head.extend_from_slice(app.as_bytes());
        socket.write_all(&head).ok()?;
        let feed = std::sync::Arc::downgrade(bus);
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::Builder::new()
            .name("film-to-room".into())
            .spawn(move || {
                // As the Deck's writer does: a hundredth of a second at a time, on the clock,
                // heard about this much after it is written.
                const BEHIND: Duration = Duration::from_millis(60);
                let step = Duration::from_millis(10);
                let mut block = vec![0f32; (rate as usize / 100) * channels as usize];
                let mut bytes = vec![0u8; block.len() * 2];
                let mut scratch = Vec::new();
                let mut due = Instant::now();
                while !stopped.load(Ordering::Relaxed) {
                    let Some(feed) = feed.upgrade() else { return };
                    let now = Instant::now();
                    if now < due {
                        std::thread::sleep(due - now);
                    } else if now - due > Duration::from_millis(250) {
                        due = now;
                    }
                    due += step;
                    feed.mix(&mut block, channels, Instant::now() + BEHIND, &mut scratch);
                    for (sample, out) in block.iter().zip(bytes.chunks_exact_mut(2)) {
                        out.copy_from_slice(&((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
                    }
                    if let Err(e) = socket.write_all(&bytes) {
                        log::warn!("the room stopped taking sound: {e}");
                        feed.died();
                        return;
                    }
                }
            })
            .ok()?;
        log::info!("sound: {channels} channels at {rate} Hz into the room, placed at the window");
        Some((Going::Room(stop), rate, channels))
    }

    /// The machine's own sound device, for a film played with no room around it.
    #[cfg(not(target_os = "linux"))]
    fn open_device(bus: &std::sync::Arc<Bus>) -> Result<(Going, u32, u16), String> {
        let device = cpal::default_host().default_output_device().ok_or("this machine has nowhere for sound to go")?;
        let supported = device.default_output_config().map_err(|e| format!("the sound device would not say what it takes: {e}"))?;
        let config: cpal::StreamConfig = supported.config();
        let (rate, channels) = (config.sample_rate.0, config.channels);
        let feed = std::sync::Arc::downgrade(bus);
        let failed = feed.clone();
        let mut scratch = Vec::new();
        let stream = device
            .build_output_stream(
                &config,
                move |out: &mut [f32], info: &cpal::OutputCallbackInfo| {
                    let stamp = info.timestamp();
                    // How long from now until what is written here is heard.
                    let delay = stamp.playback.duration_since(&stamp.callback).unwrap_or(Duration::ZERO);
                    match feed.upgrade() {
                        Some(feed) => feed.mix(out, channels, Instant::now() + delay, &mut scratch),
                        None => out.fill(0.0),
                    }
                },
                move |e| {
                    log::warn!("the sound device: {e}");
                    // A device gone (unplugged): nothing will be heard from it again.
                    if matches!(e, cpal::StreamError::DeviceNotAvailable) {
                        if let Some(bus) = failed.upgrade() {
                            bus.died();
                        }
                    }
                },
                None,
            )
            .map_err(|e| format!("the sound device would not open: {e}"))?;
        stream.play().map_err(|e| format!("the sound device would not start: {e}"))?;
        log::info!("sound: {channels} channels at {rate} Hz");
        Ok((Going::Device(stream), rate, channels))
    }

    /// Open a `pw-cat` of our own for a track of `wanted` channels.
    ///
    /// A child of this process, so it has this process's environment -- and that is where
    /// Spatiand says which window's sink the sound is for (`PIPEWIRE_PROPS`, docs/apps.md).
    /// The sink places channels by what they are, so the track is given one of the counts it
    /// knows, each channel named: a 5.1 film's six stay six and are heard from six places.
    #[cfg(target_os = "linux")]
    fn open_output(bus: &std::sync::Arc<Bus>, wanted: u16) -> Result<(Going, u32, u16), String> {
        use std::io::Write;
        let channels = crate::audio::nearest_placeable(wanted.max(1));
        let map = channel_names(channels).join(",");
        let rate = 48_000u32;
        let mut child = std::process::Command::new("pw-cat")
            .args(["--playback", "--raw", "--format", "f32", "--rate", &rate.to_string(), "--channels", &channels.to_string()])
            .args(["--channel-map", map, "--latency", "40ms", "--media-role", "Movie", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("pw-cat would not start: {e}"))?;
        let mut pipe = child.stdin.take().ok_or("pw-cat has nowhere to be written to")?;
        let feed = std::sync::Arc::downgrade(bus);
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::Builder::new()
            .name("film-speaker".into())
            .spawn(move || {
                // A hundredth of a second at a time, on the clock: written as fast as the
                // pipe would take it, the pipe would hold a sixth of a second nobody could
                // account for. What is written now is heard about this much later.
                const BEHIND: Duration = Duration::from_millis(70);
                let step = Duration::from_millis(10);
                let mut block = vec![0f32; (rate as usize / 100) * channels as usize];
                let mut scratch = Vec::new();
                let mut due = Instant::now();
                while !stopped.load(Ordering::Relaxed) {
                    let Some(feed) = feed.upgrade() else { return };
                    let now = Instant::now();
                    if now < due {
                        std::thread::sleep(due - now);
                    } else if now - due > Duration::from_millis(250) {
                        // Held up -- the machine slept, or stalled: start again from now
                        // and not with a burst to catch up.
                        due = now;
                    }
                    due += step;
                    feed.mix(&mut block, channels, Instant::now() + BEHIND, &mut scratch);
                    // SAFETY: f32 has no bytes that are not sound; the machine's own order
                    // is the order `f32` on the command line means.
                    let bytes = unsafe { std::slice::from_raw_parts(block.as_ptr() as *const u8, block.len() * 4) };
                    if pipe.write_all(bytes).is_err() {
                        log::warn!("sound: pw-cat has gone; the film keeps time without it");
                        feed.died();
                        return;
                    }
                }
            })
            .map_err(|e| format!("no thread for sound: {e}"))?;
        log::info!("sound: {channels} channels at {rate} Hz through pw-cat ({map})");
        Ok((Going { child, stop }, rate, channels))
    }

    pub(crate) fn rate(&self) -> u32 {
        self.inner.rate
    }

    pub(crate) fn channels(&self) -> u16 {
        self.inner.channels
    }

    /// Whether the sound is still going anywhere.
    pub(crate) fn alive(&self) -> bool {
        !self.inner.dead.load(Ordering::Relaxed)
    }

    /// Take a block if there is room for it; `false` is "not yet". Sound with nowhere to go is
    /// taken and dropped, so that the decoders go on.
    pub(crate) fn offer(&self, block: &Block) -> bool {
        if !self.alive() {
            return true;
        }
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
        let speed = f32::from_bits(self.inner.speed.load(Ordering::Relaxed)) as f64;
        Some(if heard.running && now >= heard.at {
            heard.time + (now - heard.at).as_secs_f64() * speed
        } else if heard.running {
            heard.time - (heard.at - now).as_secs_f64() * speed
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

    /// Out of the mix; the output closes when nothing is left in it.
    pub(crate) fn stop(&self) {
        let mut out = self.bus.out.lock().unwrap();
        let mut voices = self.bus.voices.lock().unwrap();
        voices.retain(|v| !std::sync::Arc::ptr_eq(v, &self.inner));
        if voices.is_empty() {
            drop(voices);
            out.take();
        }
    }
}

impl Drop for Sound {
    fn drop(&mut self) {
        self.stop();
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
                    let speed = f32::from_bits(self.speed.load(Ordering::Relaxed)) as f64;
                    let into = (taken / self.channels as usize) as f64 / self.rate as f64 * speed;
                    let before = (written / self.channels as usize) as f64 / self.rate as f64 * speed;
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

/// Sound made by something other than a film -- a web page's own, its Web Audio -- played as a
/// film's is: into the room at this application's window, or the machine's speakers where there
/// is no room. Pushed as it is made; what would wait more than a third of a second is dropped,
/// so a source that runs ahead never builds up a delay.
pub struct Stream {
    sound: Sound,
    /// Resampling from the source's rate, carried from one push to the next.
    phase: f64,
    last: Vec<f32>,
}

impl Stream {
    /// Open for sound of `channels` channels.
    pub fn open(channels: u16) -> Result<Stream, String> {
        Stream::open_in(&SoundGroup::new(), channels)
    }

    /// Open for sound of `channels` channels, mixed into `group`'s output.
    pub fn open_in(group: &SoundGroup, channels: u16) -> Result<Stream, String> {
        let sound = Sound::open_in(group, channels.max(1))?;
        let last = vec![0.0; sound.channels() as usize];
        Ok(Stream { sound, phase: 0.0, last })
    }

    /// Interleaved samples of `channels` channels at `rate` a second.
    pub fn push(&mut self, samples: &[f32], channels: usize, rate: u32) {
        let channels = channels.max(1);
        let frames = samples.len() / channels;
        if frames == 0 {
            return;
        }
        let out_channels = self.sound.channels() as usize;
        let step = rate as f64 / self.sound.rate() as f64;
        let mut out = Vec::with_capacity(((frames as f64 / step) as usize + 2) * out_channels);
        let pick = |frame: &[f32], c: usize| -> f32 {
            // Missing channels from those there are: mono to both, stereo's two to the front.
            if c < frame.len() {
                frame[c]
            } else if frame.len() == 1 {
                frame[0]
            } else {
                0.0
            }
        };
        let mut position = self.phase;
        while position <= (frames - 1) as f64 {
            let a = position.floor() as isize;
            let t = (position - a as f64) as f32;
            let b = ((a + 1) as usize).min(frames - 1);
            let later = &samples[b * channels..b * channels + channels];
            for c in 0..out_channels {
                let before = if a < 0 { *self.last.get(c).unwrap_or(&0.0) } else { pick(&samples[a as usize * channels..a as usize * channels + channels], c) };
                out.push(before * (1.0 - t) + pick(later, c) * t);
            }
            position += step;
        }
        self.phase = position - frames as f64;
        let tail = &samples[(frames - 1) * channels..frames * channels];
        self.last = (0..out_channels).map(|c| pick(tail, c)).collect();
        let _ = self.sound.offer(&Block { samples: out, time: f64::NAN, serial: 0 });
    }

    pub fn set_volume(&self, volume: f32) {
        self.sound.set_volume(volume);
    }
}
