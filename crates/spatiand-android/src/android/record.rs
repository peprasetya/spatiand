//! Recording a video of the session on the Beam Pro and the Mac: what the glasses show, and what
//! the wearer hears.
//!
//! The Deck's recorder (`crates/spatiand/src/record.rs`) with these platforms' parts, and the
//! same file: Matroska, with
//!
//! * **the picture** -- the frame sent to the glasses, both eyes side by side, H.264 at
//!   [`FPS`] and 40 Mbit/s from the platform's hardware encoder: MediaCodec on the Beam Pro,
//!   VideoToolbox on the Mac (`super::record_codec`, the one piece that differs). The frame is
//!   copied on the GPU while it is still in the glasses' window, and handed to the encoder after
//!   the glasses have theirs.
//! * **"What you heard"** -- stereo, after the HRTF: exactly what the session's one output was
//!   given (`spatiand_audio::server::record_heard`).
//! * **"Surround 7.1.4"** -- before the HRTF: every window's channels on the speakers they
//!   point at from the wearer's head, unplaced sound on the front pair as it is
//!   (`spatiand_audio::server::record_surround`).
//! * **"Microphone"** -- the microphone the room records from.
//!
//! The sound is 24-bit PCM, as on the Deck. Android's own muxer writes MP4, which takes sound
//! only as AAC, and this phone's AAC encoder stops at six channels; so the file is written
//! here (`super::mkv`), on the recorder's thread, on both platforms. Where it goes is the
//! platform's: on the Beam Pro the app's own storage, handed to the app when finished, which
//! puts it in the phone's `Movies/Spatiand` (Android lets an app write there only through its
//! media store); on the Mac `~/Movies/Spatiand`, the Mac's own place for films.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::renderer::gles::{ffi, GlesRenderer};
use spatiand_audio::ring::Ring;

use super::record_codec::{self as codec, Coded};

pub use codec::{publish, publish_leftovers};
#[cfg(target_os = "android")]
pub use codec::take_finished;

/// Pictures a second: a video's rate, as on the Deck.
pub const FPS: u32 = 36;
pub const BITS_PER_SECOND: u32 = 40_000_000;
const RATE: u32 = 48_000;
/// Two seconds of sound waiting in each track, at most.
const RING_SECONDS: usize = 2;
/// How far a track may lag the clock before the gap is written as silence.
const LAG: Duration = Duration::from_millis(150);

/// Start or stop a recording at the next frame, as the settings' Record row does: for a test,
/// and for a platform's own way of asking.
pub fn toggle_asked() -> &'static AtomicBool {
    static ASKED: AtomicBool = AtomicBool::new(false);
    &ASKED
}

/// One sound track: where it comes from, and how much of it has been written.
struct Sound {
    title: &'static str,
    channels: usize,
    ring: Arc<Ring>,
    /// Frames written, which is also the track's clock.
    frames: u64,
    /// What was last written was silence for a stretch with nothing in it.
    silent: bool,
}

/// A recording in progress.
pub struct Recorder {
    /// The encoder's way in, on the session's side. `None` once stopped.
    input: Option<codec::Input>,
    size: (i32, i32),
    /// A copy was made this frame and is owed to the encoder.
    owed: bool,
    started: Instant,
    next_due: Instant,
    frames: u64,
    stop: Arc<AtomicBool>,
    /// The recorder's thread has ended in an error: no more pictures are made for it.
    failed: Arc<AtomicBool>,
    /// Why, for the wearer.
    why: Arc<Mutex<String>>,
    _microphone: Option<spatiand_audio::server::Capture>,
}

impl Recorder {
    /// Start recording a `size` frame, drawn with `context` on `display`.
    pub fn start(size: (i32, i32), display: &EGLDisplay, context: &EGLContext) -> Result<Recorder, String> {
        // --- the picture ---
        let (input, output) = codec::open(size, display, context, FPS, BITS_PER_SECOND)?;

        // --- the sound ---
        let ring = |channels: usize| Arc::new(Ring::new(RATE as usize * RING_SECONDS * channels));
        let heard = ring(2);
        let surround = ring(spatiand_audio::server::SURROUND_CHANNELS);
        let mic = ring(1);
        let into = mic.clone();
        // **On the Mac, not before its owner has allowed it**, as dictation and a host's
        // microphone wait: opened earlier, macOS asks on a screen the wearer cannot see or click,
        // with the mouse the room's. Until then the track is silence.
        #[cfg(target_os = "macos")]
        let allowed = crate::mac::MICROPHONE_ALLOWED.load(Ordering::Relaxed);
        #[cfg(not(target_os = "macos"))]
        let allowed = true;
        let opened = if allowed {
            spatiand_audio::server::Capture::open(
                RATE,
                1,
                Box::new(move |samples: &[i16]| {
                    let floats: Vec<f32> = samples.iter().map(|s| *s as f32 / 32768.0).collect();
                    into.write(&floats);
                }),
            )
        } else {
            Err("Spatiand may not use the microphone yet: \u{201C}Allow the microphone\u{201D} in Spatiand's menu on the Mac".into())
        };
        let microphone = match opened {
            Ok(capture) => Some(capture),
            Err(e) => {
                log::warn!("recording: the microphone track will be silent ({e})");
                None
            }
        };
        let sounds = vec![
            Sound { title: "What you heard (binaural stereo, after the HRTF)", channels: 2, ring: heard.clone(), frames: 0, silent: false },
            Sound {
                title: "Surround 7.1.4, before the HRTF (FL FR FC LFE BL BR SL SR TFL TFR TBL TBR)",
                channels: spatiand_audio::server::SURROUND_CHANNELS,
                ring: surround.clone(),
                frames: 0,
                silent: false,
            },
            Sound { title: "Microphone", channels: 1, ring: mic, frames: 0, silent: false },
        ];

        // --- the file ---
        let dir = codec::directory();
        std::fs::create_dir_all(&dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
        let path = dir.join(format!("spatiand-{}.mkv", stamp()));
        let file = std::fs::File::create(&path).map_err(|e| format!("could not write {}: {e}", path.display()))?;
        spatiand_audio::server::record_heard(Some(heard));
        spatiand_audio::server::record_surround(Some(surround));

        let stop = Arc::new(AtomicBool::new(false));
        let started = Instant::now();
        let stopping = stop.clone();
        let failed = Arc::new(AtomicBool::new(false));
        let failing = failed.clone();
        let why = Arc::new(Mutex::new(String::new()));
        let saying = why.clone();
        let shown = path.clone();
        let size_now = (size.0 as u32, size.1 as u32);
        std::thread::Builder::new()
            .name("recorder".into())
            .spawn(move || {
                let result = run(output, sounds, file, size_now, started, &stopping);
                match result {
                    Ok(seconds) => {
                        log::info!("recording: saved {seconds:.0} s to {}", shown.display());
                        codec::finished(&shown);
                    }
                    // The storage full, most likely: what was written is still the wearer's,
                    // and must not stay where nothing shows it.
                    Err(e) => {
                        log::warn!("recording: {} is incomplete: {e}", shown.display());
                        *saying.lock().unwrap_or_else(|e| e.into_inner()) = e;
                        failing.store(true, Ordering::SeqCst);
                        codec::unfinished(&shown);
                    }
                }
            })
            .map_err(|e| format!("could not start the recorder: {e}"))?;

        log::info!("recording to {}", path.display());
        Ok(Recorder {
            input: Some(input),
            size,
            owed: false,
            started,
            next_due: started,
            frames: 0,
            stop,
            failed,
            why,
            _microphone: microphone,
        })
    }

    /// **The recording has stopped by itself** (the storage full, most likely), and why: the
    /// frame loop then stops it, so that the room no longer says it is recording, and says so.
    /// What was written up to then is finished and kept.
    pub fn failed(&self) -> Option<String> {
        self.failed.load(Ordering::SeqCst).then(|| self.why.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    /// Whether this frame is one to record. Asked once a frame, before it is drawn.
    pub fn due(&mut self) -> bool {
        // Its thread gone, the encoder takes nothing: no frame is copied for it.
        if self.failed.load(Ordering::SeqCst) {
            return false;
        }
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
        if let Some(input) = self.input.as_mut() {
            self.owed = input.copy(gl, self.size);
        }
    }

    /// Hand the copied frame to the encoder. After the glasses' own swap, so they never wait
    /// on the encoder.
    pub fn present(&mut self, renderer: &mut GlesRenderer) {
        if !std::mem::take(&mut self.owed) {
            return;
        }
        let Some(input) = self.input.as_mut() else { return };
        // Stamped with the recording's own clock, which the sound is written against too.
        match input.present(renderer, self.size, self.started.elapsed()) {
            Ok(()) => self.frames += 1,
            Err(e) => log::warn!("recording: could not hand a frame to the encoder: {e}"),
        }
    }

    /// Stop, and let the recorder's thread finish the file. Gives back the copy's GL objects.
    pub fn stop(mut self, renderer: &mut GlesRenderer) {
        if let Some(input) = self.input.take() {
            input.release(renderer);
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
        spatiand_audio::server::record_surround(None);
        // Nothing more is drawn for the encoder once its way in is gone; then it is told the
        // stream has ended, and the thread finishes the file.
        self.input = None;
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// The recorder's thread: take coded pictures out, write them and the sound as it comes,
/// finish the file -- **also when something failed on the way**, so that what was written up
/// to then is a file that says how long it is, with its last pictures in it.
fn run(
    video: codec::Output,
    sounds: Vec<Sound>,
    file: std::fs::File,
    size: (u32, u32),
    started: Instant,
    stop: &AtomicBool,
) -> Result<f64, String> {
    let mut writer: Option<super::mkv::Writer> = None;
    let result = record(video, sounds, file, size, started, stop, &mut writer);
    if result.is_err() {
        if let Some(w) = writer.take() {
            if let Err(e) = w.finish(started.elapsed().as_millis() as i64) {
                log::warn!("recording: could not finish the file either: {e}");
            }
        }
    }
    result
}

fn record(
    mut video: codec::Output,
    mut sounds: Vec<Sound>,
    file: std::fs::File,
    size: (u32, u32),
    started: Instant,
    stop: &AtomicBool,
    writer: &mut Option<super::mkv::Writer>,
) -> Result<f64, String> {
    let mut file = Some(file);
    // Pictures out before the stream's parameter sets, which the file's header needs.
    let mut held: Vec<(i64, bool, Vec<u8>)> = Vec::new();
    let mut stopping_since: Option<Instant> = None;
    let mut ended_at = Duration::ZERO;
    let mut video_ended = false;

    loop {
        let stopping = stop.load(Ordering::SeqCst);
        if stopping && stopping_since.is_none() {
            stopping_since = Some(Instant::now());
            ended_at = started.elapsed();
            video.end();
        }

        // --- pictures ---
        let (coded, ended) = video.drain()?;
        video_ended |= ended;
        for coded in coded {
            match coded {
                Coded::Config(bytes) => {
                    if writer.is_none() {
                        let private = super::mkv::avc_config(&bytes).ok_or("the encoder's stream has no SPS and PPS")?;
                        let mut tracks = vec![super::mkv::Track::Video { width: size.0, height: size.1, private }];
                        for (i, sound) in sounds.iter().enumerate() {
                            tracks.push(super::mkv::Track::Audio {
                                title: sound.title.into(),
                                channels: sound.channels,
                                rate: RATE,
                                default: i == 0,
                            });
                        }
                        let file = file.take().ok_or("the file is gone")?;
                        let mut w = super::mkv::Writer::create(file, "Spatiand", &tracks).map_err(|e| e.to_string())?;
                        for (ms, key, data) in held.drain(..) {
                            w.video(1, ms, key, &data).map_err(|e| e.to_string())?;
                        }
                        *writer = Some(w);
                    }
                }
                Coded::Picture { ms, key, data } => match writer.as_mut() {
                    Some(w) => w.video(1, ms, key, &data).map_err(|e| e.to_string())?,
                    None => held.push((ms, key, data)),
                },
            }
        }

        // --- sound ---
        if let Some(w) = writer.as_mut() {
            let now = if stopping { ended_at } else { started.elapsed() };
            let clock = if stopping { ended_at } else { now.saturating_sub(LAG) };
            for (i, sound) in sounds.iter_mut().enumerate() {
                pump(w, i + 2, sound, clock, now, stopping).map_err(|e| e.to_string())?;
            }
        }

        if stopping {
            let waited = stopping_since.map(|t| t.elapsed()).unwrap_or_default();
            if video_ended || waited > Duration::from_secs(3) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    let Some(mut w) = writer.take() else {
        return Err("nothing was recorded".into());
    };
    for (i, sound) in sounds.iter_mut().enumerate() {
        pump(&mut w, i + 2, sound, ended_at, ended_at, true).map_err(|e| e.to_string())?;
    }
    w.finish(ended_at.as_millis() as i64).map_err(|e| e.to_string())?;
    drop(video);
    Ok(ended_at.as_secs_f64())
}

/// Write what one track's ring holds, in blocks of at least 20 ms, and silence for any stretch
/// the clock has passed with nothing in it -- a microphone that started late, an output that
/// played nothing -- so every track stays with the picture.
///
/// **Sound that comes again after such a stretch is put where it began**, `now` less what is
/// waiting of it, and not where the silence so far ends (`clock`, [`LAG`] behind `now`), which
/// put it up to that much early.
fn pump(w: &mut super::mkv::Writer, track: usize, sound: &mut Sound, clock: Duration, now: Duration, all: bool) -> std::io::Result<()> {
    let block = RATE as usize / 50;
    let waiting = (sound.ring.available() / sound.channels) as u64;
    if sound.silent && waiting > 0 {
        let begun = ((now.as_secs_f64() * RATE as f64) as u64).saturating_sub(waiting);
        while sound.frames < begun {
            let frames = ((begun - sound.frames) as usize).min(RATE as usize / 5);
            let ms = (sound.frames * 1000 / RATE as u64) as i64;
            w.audio(track, ms, &vec![0f32; frames * sound.channels])?;
            sound.frames += frames as u64;
        }
        sound.silent = false;
    }
    loop {
        let waiting = sound.ring.available() / sound.channels;
        if waiting == 0 || (waiting < block && !all) {
            break;
        }
        let frames = waiting.min(RATE as usize / 5);
        let mut samples = vec![0f32; frames * sound.channels];
        sound.ring.read(&mut samples);
        let ms = (sound.frames * 1000 / RATE as u64) as i64;
        w.audio(track, ms, &samples)?;
        sound.frames += frames as u64;
        sound.silent = false;
    }
    let due = (clock.as_secs_f64() * RATE as f64) as u64;
    while sound.frames < due {
        let frames = ((due - sound.frames) as usize).min(RATE as usize / 5);
        let ms = (sound.frames * 1000 / RATE as u64) as i64;
        w.audio(track, ms, &vec![0f32; frames * sound.channels])?;
        sound.frames += frames as u64;
        sound.silent = true;
    }
    Ok(())
}

fn stamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .to_string()
}

/// `SPATIAND_RECORDINGS`, where a platform or a test says recordings go.
pub fn directory_asked() -> Option<PathBuf> {
    std::env::var("SPATIAND_RECORDINGS").ok().map(PathBuf::from)
}
