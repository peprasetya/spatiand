//! The microphone, for what the room itself listens with: dictation now, the assistant later.
//!
//! The same devices as a remote host's microphone (`remote/microphone.rs`): `pw-record` on the
//! default source on Linux, which is whatever the desktop's sound settings chose, and the
//! platform's own capture on the Mac and Android. 16 kHz mono, Whisper's own, handed to a sink
//! in ten-millisecond pieces on a thread of the capture's. Stopped when dropped.
//!
//! `SPATIAND_LISTEN_WAV=<file>` makes a WAV file the microphone, played at its own pace and
//! followed by a room's quiet: how dictation is tested with nobody there to speak.

/// What is recording.
pub struct Microphone {
    #[cfg(not(any(target_os = "android", target_os = "macos")))]
    child: Option<std::process::Child>,
    #[cfg(any(target_os = "android", target_os = "macos"))]
    _capture: Option<spatiand_audio::server::Capture>,
    /// A file playing as the microphone: told to stop when this is dropped.
    file: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

/// Where the samples go.
pub type Sink = Box<dyn FnMut(&[i16]) + Send>;

impl Microphone {
    /// Open the microphone at `rate`, mono.
    #[cfg(not(any(target_os = "android", target_os = "macos")))]
    pub fn open(rate: u32, mut sink: Sink) -> Result<Microphone, String> {
        use std::io::Read;
        if let Some(file) = std::env::var_os("SPATIAND_LISTEN_WAV") {
            return play_file(std::path::Path::new(&file), rate, sink);
        }
        let mut child = std::process::Command::new("pw-record")
            .args(["--raw", "--format", "s16", "--rate", &rate.to_string(), "--channels", "1", "--latency", "20ms"])
            .args(["-P", "{ media.name = \"Spatiand (dictation)\" }", "-"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start pw-record: {e}"))?;
        let mut out = child.stdout.take().ok_or("pw-record has no output")?;
        let piece = (rate as usize / 100) * 2;
        std::thread::Builder::new()
            .name("listen".into())
            .spawn(move || {
                let mut bytes = vec![0u8; piece];
                let mut samples = vec![0i16; piece / 2];
                while out.read_exact(&mut bytes).is_ok() {
                    for (s, pair) in samples.iter_mut().zip(bytes.chunks_exact(2)) {
                        *s = i16::from_le_bytes([pair[0], pair[1]]);
                    }
                    sink(&samples);
                }
            })
            .map_err(|e| format!("no thread for the microphone: {e}"))?;
        Ok(Microphone { child: Some(child), file: None })
    }

    /// Open the microphone at `rate`, mono.
    #[cfg(any(target_os = "android", target_os = "macos"))]
    pub fn open(rate: u32, sink: Sink) -> Result<Microphone, String> {
        if let Some(file) = std::env::var_os("SPATIAND_LISTEN_WAV") {
            return play_file(std::path::Path::new(&file), rate, sink);
        }
        // On the Mac, opening it before its owner has allowed it puts macOS's question on a
        // screen they cannot see or click, with the mouse the room's.
        #[cfg(target_os = "macos")]
        if !crate::mac::MICROPHONE_ALLOWED.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("Spatiand may not use the microphone yet: choose \u{201C}Allow the microphone\u{201D} in Spatiand's menu on the Mac".into());
        }
        Ok(Microphone { _capture: Some(spatiand_audio::server::Capture::open(rate, 1, sink)?), file: None })
    }
}

impl Drop for Microphone {
    fn drop(&mut self) {
        if let Some(stop) = &self.file {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        #[cfg(not(any(target_os = "android", target_os = "macos")))]
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// A WAV file as the microphone: at `rate`, in real time, then a quiet room until stopped.
fn play_file(path: &std::path::Path, rate: u32, mut sink: Sink) -> Result<Microphone, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (pcm, from) = spatiand_voice::hemera::read_wav(&bytes).ok_or_else(|| format!("{} is not a 16-bit WAV file", path.display()))?;
    let n = (pcm.len() as u64 * rate as u64 / from.max(1) as u64) as usize;
    let samples: Vec<i16> = (0..n)
        .map(|i| {
            let x = i as f64 * from as f64 / rate as f64;
            let (a, f) = (x.floor() as usize, x.fract());
            let s0 = pcm[a.min(pcm.len() - 1)] as f64;
            let s1 = pcm[(a + 1).min(pcm.len() - 1)] as f64;
            (s0 + (s1 - s0) * f) as i16
        })
        .collect();
    log::info!("listen: playing {} as the microphone ({:.1} s)", path.display(), n as f32 / rate as f32);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped = stop.clone();
    std::thread::Builder::new()
        .name("listen-file".into())
        .spawn(move || {
            let piece = rate as usize / 100;
            let mut seed = 1u32;
            let mut at = 0;
            // A moment of the room first, as when a person presses the key and then speaks: the
            // pause detector learns the room's noise in its first moments.
            let mut lead = 80;
            while !stopped.load(std::sync::atomic::Ordering::Relaxed) {
                let chunk: Vec<i16> = if lead > 0 {
                    lead -= 1;
                    (0..piece)
                        .map(|_| {
                            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                            ((seed >> 16) as i16) % 40
                        })
                        .collect()
                } else if at < samples.len() {
                    let end = (at + piece).min(samples.len());
                    let chunk = samples[at..end].to_vec();
                    at = end;
                    chunk
                } else {
                    // A room's quiet: faint noise, never digital silence.
                    (0..piece)
                        .map(|_| {
                            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                            ((seed >> 16) as i16) % 40
                        })
                        .collect()
                };
                sink(&chunk);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        })
        .map_err(|e| format!("no thread for the microphone: {e}"))?;
    Ok(Microphone {
        #[cfg(not(any(target_os = "android", target_os = "macos")))]
        child: None,
        #[cfg(any(target_os = "android", target_os = "macos"))]
        _capture: None,
        file: Some(stop),
    })
}

