//! Recording a video of the session: what the glasses show, and what the wearer hears.
//!
//! The screenshot's moving sibling, and for the same reason: the world is only visible inside
//! the glasses, and a phone held up to them shows neither the stereo nor where the sound is.
//! One Matroska file in `~/Videos/Spatiand`, with:
//!
//! * **the picture** -- the frame sent to the glasses, both eyes side by side, exactly as
//!   drawn: remote applications' decoded pictures, the HUD, the pointer, all of it. Taken at
//!   [`FPS`], which is a video's rate rather than the glasses'.
//! * **"What you heard"** -- stereo, after the HRTF: every window's placed sound and anything
//!   that plays straight to the output (a VR application's own mix, system sounds), as the
//!   output device itself was given it. For YouTube, and first, so a player plays it.
//! * **"Surround"** -- 7.1.4, before the HRTF: every window's channels on the speakers they
//!   point at from the wearer's head (see `spatiand_audio::stage::bed_gains`), for a room with
//!   speakers in it. A VR application's own stereo goes to the front pair as it is.
//! * **"Microphone"** -- the default input, as captured, apart so it can be replaced.
//!
//! The picture is H.264 at 40 Mbit/s; see [`Recorder::start`] for why not HEVC.
//!
//! **Where the time goes.** The picture is copied into the encoder's own buffer on the GPU --
//! one blit -- and everything else happens on the recorder's thread: waiting for that copy,
//! the colour conversion and the hardware encode, and writing the file. The frame loop pays
//! for the blit and nothing more. A frame that finds every buffer still being encoded is not
//! recorded, and the log counts them.
//!
//! **Keeping the tracks together.** The picture is stamped with the frame loop's clock; each
//! sound is written as it arrives, and a track that falls behind that clock -- a capture that
//! started late, a sink with nothing playing into it -- is caught up with silence. Over a long
//! recording the sound card's clock and the frame clock drift apart by a few tens of
//! milliseconds an hour, which nobody editing a video will find.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use smithay::backend::allocator::dmabuf::{Dmabuf, DmabufFlags};
use smithay::backend::allocator::{Fourcc, Modifier};
use smithay::backend::renderer::gles::{ffi, GlesRenderer};
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::{Bind, Frame, Renderer};
use smithay::utils::{Physical, Size, Transform};
use spatiand_audio::ring::Ring;
use spatiand_video::record::{AudioTrack, VideoEncoder, VideoSettings, Writer};

/// Pictures a second. A video's rate, not the glasses': every other frame at 72 Hz, so the
/// motion is even, and half the work.
pub const FPS: u32 = 36;
/// Bit rate of the picture, kbit/s: clean for side-by-side 3840x1080 in H.264 at [`FPS`], and
/// about 300 MB a minute.
const KBIT: u32 = 40_000;
/// Buffers to draw into, so a frame can be copied while the last one is encoding.
const CANVASES: usize = 3;
/// Two seconds of sound waiting in each track, at most.
const RING_SECONDS: usize = 2;
/// How far a track may lag the clock before it is caught up with silence. Sound arrives in
/// bursts, and a burst that is late is not a gap.
const LAG: Duration = Duration::from_millis(150);

enum Message {
    Frame {
        index: usize,
        sync: SyncPoint,
        pts_ms: i64,
    },
    Stop,
}

/// A sound being captured into a ring, and how many channels it has.
struct Source {
    title: &'static str,
    layout: String,
    channels: usize,
    ring: Arc<Ring>,
}

/// A recording in progress.
pub struct Recorder {
    to_thread: mpsc::Sender<Message>,
    free: mpsc::Receiver<usize>,
    /// Buffers whose copy failed, so never went to the thread: free again at once.
    spare: Vec<usize>,
    canvases: Vec<Dmabuf>,
    size: (u32, u32),
    started: Instant,
    next_due: Instant,
    frames: u64,
    skipped: u64,
    captures: Vec<Child>,
}

impl Recorder {
    /// Start recording the `size` frame and the sound.
    pub fn start(size: (u32, u32), audio: &crate::audio::Audio) -> Result<Recorder, String> {
        let node =
            std::env::var("SPATIAND_RENDER_NODE").unwrap_or_else(|_| "/dev/dri/renderD128".into());
        let settings = VideoSettings {
            width: size.0,
            height: size.1,
            fps: FPS,
            kbit: KBIT,
            // **H.264, not HEVC.** The Deck's HEVC encoder (Mesa 25.3, ffmpeg 7.1) writes slice
            // headers that ffmpeg's own decoder refuses -- `alignment_bit_equal_to_one=0` on
            // every frame and not one decoded, in software or on the GPU, measured 2026-09-27
            // -- while H.264 decodes whole. It is also what every editor and YouTube take best.
            hevc: false,
        };
        let encoder = VideoEncoder::new(&node, settings, CANVASES).map_err(|e| e.to_string())?;
        let canvases = encoder
            .canvases()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|c| {
                let fourcc = Fourcc::try_from(c.fourcc)
                    .map_err(|_| format!("unknown buffer format {:#x}", c.fourcc))?;
                let mut builder = Dmabuf::builder(
                    (c.width as i32, c.height as i32),
                    fourcc,
                    Modifier::from(c.modifier),
                    DmabufFlags::empty(),
                );
                for (i, (fd, offset, pitch)) in c.planes.into_iter().enumerate() {
                    builder.add_plane(fd, i as u32, offset, pitch);
                }
                builder.build().ok_or_else(|| "a recording buffer with no planes".to_string())
            })
            .collect::<Result<Vec<_>, String>>()?;

        let dir = directory();
        std::fs::create_dir_all(&dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
        let path = dir.join(format!("spatiand-{}.mkv", stamp()));

        let rate = crate::audio::RATE;
        let ring = |channels: usize| Arc::new(Ring::new(rate as usize * RING_SECONDS * channels));
        let mut captures = Vec::new();
        let mut sources = Vec::new();

        // What you heard: the output device's own input, which is every placed window and
        // everything that went to the output straight. Captured from the default sink's monitor.
        let heard = ring(2);
        match default_sink().and_then(|sink| capture(&sink, true, 2, Arc::clone(&heard))) {
            Ok(child) => captures.push(child),
            Err(e) => log::warn!("recording: the stereo track will be silent ({e})"),
        }
        sources.push(Source {
            title: "What you heard (binaural stereo, after the HRTF)",
            layout: "stereo".into(),
            channels: 2,
            ring: heard,
        });

        let surround = ring(spatiand_audio::server::BED_CHANNELS);
        if !audio.record(Some(Arc::clone(&surround))) {
            log::warn!("recording: spatial sound is off, so the surround track is silent");
        }
        sources.push(Source {
            title: "Surround 7.1.4, before the HRTF (FL FR FC LFE BL BR SL SR TFL TFR TBL TBR)",
            layout: "7.1.4".into(),
            channels: spatiand_audio::server::BED_CHANNELS,
            ring: surround,
        });
        crate::remote::sound::set_recording(true);

        let mic_channels = default_source_channels().unwrap_or(1).clamp(1, 2);
        let mic = ring(mic_channels);
        match default_source().and_then(|source| capture(&source, false, mic_channels, Arc::clone(&mic))) {
            Ok(child) => captures.push(child),
            Err(e) => log::warn!("recording: the microphone track will be silent ({e})"),
        }
        sources.push(Source {
            title: "Microphone",
            layout: if mic_channels == 2 { "stereo" } else { "mono" }.into(),
            channels: mic_channels,
            ring: mic,
        });

        let tracks: Vec<AudioTrack> = sources
            .iter()
            .map(|s| AudioTrack {
                title: s.title.into(),
                layout: s.layout.clone(),
                channels: s.channels,
                rate,
            })
            .collect();
        let writer = Writer::create(&path, "Spatiand", &encoder, &tracks).map_err(|e| e.to_string())?;

        let (to_thread, from_session) = mpsc::channel();
        let (give_back, free) = mpsc::channel();
        for i in 0..CANVASES {
            let _ = give_back.send(i);
        }
        let started = Instant::now();
        let shown = path.clone();
        std::thread::Builder::new()
            .name("recorder".into())
            .spawn(move || {
                match run(encoder, writer, sources, rate, started, from_session, give_back) {
                    Ok(seconds) => log::info!(
                        "recording: saved {:.0} s to {}",
                        seconds,
                        shown.display()
                    ),
                    Err(e) => log::warn!("recording: {} is incomplete: {e}", shown.display()),
                }
            })
            .map_err(|e| format!("could not start the recorder: {e}"))?;

        log::info!("recording to {}", path.display());
        Ok(Recorder {
            to_thread,
            free,
            spare: Vec::new(),
            canvases,
            size,
            started,
            next_due: started,
            frames: 0,
            skipped: 0,
            captures,
        })
    }

    /// Offer the frame just drawn into `fbo`. Taken when one is due, and copied on the GPU.
    pub fn frame(&mut self, renderer: &mut GlesRenderer, fbo: u32) {
        let now = Instant::now();
        if now < self.next_due {
            return;
        }
        let interval = Duration::from_secs_f64(1.0 / FPS as f64);
        self.next_due += interval;
        if self.next_due < now {
            // Fell behind -- a stall, a long frame. Start the cadence again from here rather
            // than recording a burst to catch up.
            self.next_due = now + interval;
        }
        let Some(index) = self.spare.pop().or_else(|| self.free.try_recv().ok()) else {
            self.skipped += 1;
            if self.skipped % (FPS as u64 * 10) == 1 {
                log::warn!(
                    "recording: the encoder is behind; {} frame(s) not recorded so far",
                    self.skipped
                );
            }
            return;
        };
        let pts_ms = (now - self.started).as_millis() as i64;
        match blit(renderer, fbo, &mut self.canvases[index], self.size) {
            Ok(sync) => {
                self.frames += 1;
                let _ = self.to_thread.send(Message::Frame { index, sync, pts_ms });
            }
            Err(e) => {
                if self.skipped == 0 {
                    log::warn!("recording: could not copy a frame: {e}");
                }
                self.skipped += 1;
                self.spare.push(index);
            }
        }
    }

    /// Stop, and let the recorder's thread finish the file.
    pub fn stop(self, audio: &crate::audio::Audio) {
        audio.record(None);
        log::info!(
            "recording stopped after {:.0} s: {} frame(s), {} not recorded",
            self.started.elapsed().as_secs_f64(),
            self.frames,
            self.skipped
        );
        // The rest is `Drop`'s, which is also what runs when the displays are rebuilt with a
        // recording going: the file is finished either way.
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        crate::remote::sound::set_recording(false);
        for child in &mut self.captures {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = self.to_thread.send(Message::Stop);
    }
}

/// Copy the frame in `fbo` into a recording buffer, and return when the GPU will be done.
fn blit(
    renderer: &mut GlesRenderer,
    fbo: u32,
    canvas: &mut Dmabuf,
    (w, h): (u32, u32),
) -> Result<SyncPoint, String> {
    let size: Size<i32, Physical> = (w as i32, h as i32).into();
    let mut target = renderer.bind(canvas).map_err(|e| e.to_string())?;
    let mut frame = renderer
        .render(&mut target, size, Transform::Normal)
        .map_err(|e| e.to_string())?;
    frame
        .with_context(|gl| unsafe {
            // The frame's own buffer is bound to draw into; read from the scene's. The scene
            // is already held top row first, which is how the encoder's buffer is laid out,
            // so a straight copy lands the right way up -- as the screenshot's readback does.
            let mut drawing = 0;
            gl.GetIntegerv(ffi::DRAW_FRAMEBUFFER_BINDING, &mut drawing);
            gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, fbo);
            gl.BlitFramebuffer(
                0,
                0,
                w as i32,
                h as i32,
                0,
                0,
                w as i32,
                h as i32,
                ffi::COLOR_BUFFER_BIT,
                ffi::NEAREST,
            );
            gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, drawing as u32);
        })
        .map_err(|e| e.to_string())?;
    let sync = frame.finish().map_err(|e| e.to_string())?;
    drop(target);
    Ok(sync)
}

/// The recorder's thread: encode what arrives, write the sound as it comes, finish the file.
fn run(
    mut encoder: VideoEncoder,
    mut writer: Writer,
    sources: Vec<Source>,
    rate: u32,
    started: Instant,
    messages: mpsc::Receiver<Message>,
    give_back: mpsc::Sender<usize>,
) -> Result<f64, String> {
    let mut scratch: Vec<f32> = Vec::new();
    let mut pump = |writer: &mut Writer, until: Duration| -> Result<(), String> {
        for (track, source) in sources.iter().enumerate() {
            let waiting = source.ring.available() / source.channels;
            if waiting > 0 {
                scratch.resize(waiting * source.channels, 0.0);
                source.ring.read(&mut scratch);
                writer.audio(track, &scratch).map_err(|e| e.to_string())?;
            }
            // Behind the clock by more than a burst: the gap was silence.
            let due = (until.saturating_sub(LAG).as_secs_f64() * rate as f64) as i64;
            let behind = due - writer.audio_frames(track);
            if behind > 0 {
                scratch.clear();
                scratch.resize(behind as usize * source.channels, 0.0);
                writer.audio(track, &scratch).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    };

    loop {
        match messages.recv_timeout(Duration::from_millis(20)) {
            Ok(Message::Frame { index, sync, pts_ms }) => {
                // The copy has to have landed before the encoder reads the buffer.
                let _ = sync.wait();
                let coded = encoder.encode(index, pts_ms).map_err(|e| e.to_string());
                let _ = give_back.send(index);
                for packet in coded? {
                    writer.video(&packet).map_err(|e| e.to_string())?;
                }
            }
            Ok(Message::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        pump(&mut writer, started.elapsed())?;
    }
    let ended = started.elapsed();
    for packet in encoder.finish().map_err(|e| e.to_string())? {
        writer.video(&packet).map_err(|e| e.to_string())?;
    }
    // Whatever sound is still waiting, and then the last of each track up to the end.
    pump(&mut writer, ended + LAG)?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(ended.as_secs_f64())
}

/// Capture `target` into `ring` as float samples, with `pw-record`. `monitor` records what a
/// sink is playing rather than a source.
fn capture(target: &str, monitor: bool, channels: usize, ring: Arc<Ring>) -> Result<Child, String> {
    let mut command = Command::new("pw-record");
    command.args([
        "--raw",
        "--format",
        "f32",
        "--rate",
        &crate::audio::RATE.to_string(),
        "--channels",
        &channels.to_string(),
        "--latency",
        "20ms",
        "--target",
        target,
    ]);
    if monitor {
        command.args(["-P", "{ stream.capture.sink = true media.name = \"Spatiand recording\" }"]);
    } else {
        command.args(["-P", "{ media.name = \"Spatiand recording\" }"]);
    }
    let mut child = command
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start pw-record: {e}"))?;
    let mut out = child.stdout.take().ok_or("pw-record has no output")?;
    std::thread::Builder::new()
        .name("recording-sound".into())
        .spawn(move || {
            let mut bytes = vec![0u8; 4 * channels * 480];
            let mut samples = vec![0f32; channels * 480];
            while out.read_exact(&mut bytes).is_ok() {
                for (s, b) in samples.iter_mut().zip(bytes.chunks_exact(4)) {
                    *s = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                }
                ring.write(&samples);
            }
        })
        .map_err(|e| format!("could not read pw-record: {e}"))?;
    Ok(child)
}

fn pactl(args: &[&str]) -> Result<String, String> {
    let out = Command::new("pactl")
        .args(args)
        .output()
        .map_err(|e| format!("could not ask pactl: {e}"))?;
    if !out.status.success() {
        return Err(format!("pactl {} failed", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn default_sink() -> Result<String, String> {
    pactl(&["get-default-sink"])
}

fn default_source() -> Result<String, String> {
    pactl(&["get-default-source"])
}

/// How many channels the default input has, from `pactl list sources short`, whose lines end
/// `... float32le 2ch 48000Hz STATE`.
fn default_source_channels() -> Option<usize> {
    let name = default_source().ok()?;
    let list = pactl(&["list", "sources", "short"]).ok()?;
    channels_of(&list, &name)
}

fn channels_of(list: &str, name: &str) -> Option<usize> {
    list.lines()
        .find(|line| line.split_whitespace().nth(1) == Some(name))?
        .split_whitespace()
        .find_map(|word| word.strip_suffix("ch")?.parse().ok())
}

fn stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs.to_string()
}

/// `XDG_VIDEOS_DIR/Spatiand`, or `~/Videos/Spatiand`. `SPATIAND_RECORDINGS` overrides it.
fn directory() -> PathBuf {
    if let Ok(explicit) = std::env::var("SPATIAND_RECORDINGS") {
        return PathBuf::from(explicit);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let config = std::path::Path::new(&home).join(".config/user-dirs.dirs");
    if let Ok(text) = std::fs::read_to_string(&config) {
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("XDG_VIDEOS_DIR=") {
                let cleaned = value.trim().trim_matches('"').replace("$HOME", &home);
                return PathBuf::from(cleaned).join("Spatiand");
            }
        }
    }
    std::path::Path::new(&home).join("Videos/Spatiand")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_microphones_channels_are_read_from_its_line() {
        let list = "58\talsa_input.usb-mic\tPipeWire\ts16le 1ch 48000Hz\tSUSPENDED\n\
                    61\talsa_loopback_device.internal\tPipeWire\tfloat32le 2ch 48000Hz\tRUNNING";
        assert_eq!(channels_of(list, "alsa_loopback_device.internal"), Some(2));
        assert_eq!(channels_of(list, "alsa_input.usb-mic"), Some(1));
        assert_eq!(channels_of(list, "missing"), None);
    }
}
