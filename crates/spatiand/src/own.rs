//! Spatiand's own applications: SpatiShelf, SpatiReel, SpatiLoupe, SpatiQuill, SpatiMesh.
//!
//! They are programs Spatiand carries, each a process of its own that is a client of this
//! compositor like any other (`crates/spatiand-app`), and the same programs on the Deck, the
//! Mac and the Beam Pro. The launcher shows them behind one bubble, always first; this is
//! where that list comes from and how one is started. See `docs/spatiand-apps.md`.
//!
//! An application is listed only if its program is there to run, so a build that carries two
//! of the four shows two.

use std::path::PathBuf;

use spatiand_shell::AppEntry;

struct Own {
    name: &'static str,
    /// The program's file name. On Android it is carried as `lib<program>.so`, which is the
    /// only name under which an APK's native directory will hold something to run.
    program: &'static str,
    icon: &'static str,
    /// What it is filed under for sound: a film's width is not a terminal's.
    categories: &'static [&'static str],
}

/// In the order the launcher shows them, which is not by name.
const OWN: &[Own] = &[
    Own {
        name: "SpatiShelf",
        program: "spatishelf",
        icon: include_str!("../../../assets/apps/spatishelf.svg"),
        categories: &["Utility"],
    },
    Own {
        name: "SpatiReel",
        program: "spatireel",
        icon: include_str!("../../../assets/apps/spatireel.svg"),
        categories: &["AudioVideo", "Video"],
    },
    Own {
        name: "SpatiLoupe",
        program: "spatiloupe",
        icon: include_str!("../../../assets/apps/spatiloupe.svg"),
        categories: &["Graphics", "Viewer"],
    },
    Own {
        name: "SpatiQuill",
        program: "spatiquill",
        icon: include_str!("../../../assets/apps/spatiquill.svg"),
        categories: &["System", "TerminalEmulator"],
    },
    Own {
        name: "SpatiMesh",
        program: "spatimesh",
        icon: include_str!("../../../assets/apps/spatimesh.svg"),
        categories: &["Network", "WebBrowser"],
    },
];

const GROUP_ICON: &str = include_str!("../../../assets/icons/spatiand.svg");

/// Where the programs are: beside this one, unless `SPATIAND_APPS_DIR` says otherwise -- which
/// it does on Android, where "beside this one" is the APK's native library directory and this
/// process is not a program in it.
fn directories() -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Some(named) = std::env::var_os("SPATIAND_APPS_DIR") {
        found.push(PathBuf::from(named));
    }
    if let Some(native) = native_library_dir() {
        found.push(native);
    }
    if let Some(beside) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(PathBuf::from)) {
        found.push(beside);
    }
    found
}

/// On Android, the APK's native library folder: where `libspatiand.so` -- this code -- was
/// loaded from, read off the process's own map of what it has loaded.
fn native_library_dir() -> Option<PathBuf> {
    if !cfg!(target_os = "android") {
        return None;
    }
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    maps.lines()
        .filter_map(|line| line.split_whitespace().last())
        .find(|path| path.ends_with("/libspatiand.so"))
        .and_then(|path| std::path::Path::new(path).parent().map(PathBuf::from))
}

fn program(own: &Own) -> Option<PathBuf> {
    let name = if cfg!(target_os = "android") { format!("lib{}.so", own.program) } else { own.program.to_string() };
    directories().into_iter().map(|dir| dir.join(&name)).find(|path| path.is_file())
}

/// An icon as a file, which is how the launcher's icons are all loaded. Written where the
/// session keeps its sockets, once, and written again by the next session.
fn icon_file(name: &str, svg: &str) -> Option<String> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("spatiand-own-icons");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{name}.svg"));
    if std::fs::read_to_string(&path).ok().as_deref() != Some(svg) {
        std::fs::write(&path, svg).ok()?;
    }
    Some(path.to_string_lossy().into_owned())
}

/// The applications that are here to run, in the launcher's order.
pub fn list() -> Vec<AppEntry> {
    OWN.iter()
        .filter_map(|own| {
            let program = program(own)?;
            Some(AppEntry {
                name: own.name.to_string(),
                // Quoted: a Mac's home folder may have a space in its name, and so then does
                // the bundle the program is in.
                exec: format!("\"{}\"", program.display()),
                icon: icon_file(own.program, own.icon),
                categories: own.categories.iter().map(|c| c.to_string()).collect(),
            })
        })
        .collect()
}

/// The picture for the bubble they are all behind.
pub fn group_icon() -> Option<String> {
    icon_file("spatiand", GROUP_ICON)
}

/// Tell the shell what there is. Nothing, where nothing is installed: the launcher is then
/// exactly what it was before these existed.
pub fn adopt(shell: &mut spatiand_shell::Shell) {
    let own = list();
    if own.is_empty() {
        return;
    }
    log::info!("Spatiand's own applications: {}", own.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", "));
    shell.set_own_apps(own, group_icon());
}

/// Start one, as a client of this compositor.
pub fn launch(app: &AppEntry, wayland_display: &str, extra: &[(String, String)]) {
    let mut extra = extra.to_vec();
    // On Android the programs and the libraries they need (FFmpeg, PDFium) are all in the
    // APK's library folder, and nothing tells the dynamic linker so but this.
    if let Some(native) = native_library_dir() {
        let dir = native.to_string_lossy().into_owned();
        extra.push(("SPATIAND_APPS_DIR".into(), dir.clone()));
        extra.push(("LD_LIBRARY_PATH".into(), dir));
    }
    if let Err(e) = spatiand_platform::launch(&app.exec, wayland_display, &extra) {
        log::warn!("could not start {}: {e}", app.name);
    }
}

/// Sound from Spatiand's own applications, placed where their windows are.
///
/// On the Mac and the Beam Pro there is no PipeWire to play into a window's sink, so an
/// application of Spatiand's own hands its sound to the session instead: it connects to
/// `$XDG_RUNTIME_DIR/spatiand-sound`, says `SPSN`, its channel count (u16), rate (u32, 48000)
/// and application id (u16 length, then the bytes), and then writes interleaved little-endian
/// 16-bit samples as they are to be heard. The session gives the windows of that application
/// id a slot (by id and not by process: a Mac's Wayland clients have no process number the
/// compositor can read) of the spatial audio engine -- the same as a Mac
/// application's, with the same mute and volume on its bar -- and every channel of a film,
/// 5.1 or 7.1, is placed in the room through the HRTF.
///
/// A feed may come under a name of its own -- a browser tab's, `<app id>/page<n>` -- which the
/// application then routes to one of its windows' names: `SPRT`, the feed's name (u16 length,
/// bytes), the window's (the same; empty to drop the route), on a connection of its own. A
/// feed follows its route as it changes, so a tab's sound moves with the tab.
///
/// Not on the Deck, where an application plays into its window's PipeWire sink.
#[cfg(any(target_os = "macos", target_os = "android"))]
pub mod sound {
    use std::collections::{HashMap, VecDeque};
    use std::io::Read;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};

    use spatiand_audio::server::Slot;

    /// Each name sound comes with (an application's instance, or a window of it: see
    /// `set_sound_key`), what is waiting from each connection under it, and the slot of its
    /// window once that is known.
    ///
    /// More than one connection may hand sound over under one name -- a browser's own player and
    /// its engine's -- and a window has one place in the room: what they send is added together
    /// here, a hundredth of a second at a time, before it is placed, so that the HRTF hears one
    /// sound of one layout and not two taking turns.
    #[derive(Default)]
    struct Named {
        feeds: HashMap<u64, Feed>,
        slot: Option<Slot>,
    }

    struct Feed {
        channels: usize,
        waiting: VecDeque<i16>,
        /// When it last sent: one that has not for a while is not waited for.
        heard: std::time::Instant,
    }

    fn senders() -> &'static Mutex<HashMap<String, Named>> {
        static SENDERS: OnceLock<Mutex<HashMap<String, Named>>> = OnceLock::new();
        SENDERS.get_or_init(Default::default)
    }

    /// Feeds' own names, to the window names they are heard under.
    fn routes() -> &'static Mutex<HashMap<String, String>> {
        static ROUTES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
        ROUTES.get_or_init(Default::default)
    }

    fn routed(name: &str) -> String {
        routes().lock().unwrap().get(name).cloned().unwrap_or_else(|| name.to_string())
    }

    /// How long a feed that has stopped sending is still waited for before the others go on.
    const STILL_SOUNDING: std::time::Duration = std::time::Duration::from_millis(60);

    /// Each channel count the room places, by its channels' names, in order.
    fn names(channels: usize) -> &'static [&'static str] {
        match channels {
            1 => &["MONO"],
            2 => &["FL", "FR"],
            6 => &["FL", "FR", "FC", "LFE", "RL", "RR"],
            8 => &["FL", "FR", "FC", "LFE", "RL", "RR", "SL", "SR"],
            10 => &["FL", "FR", "FC", "LFE", "RL", "RR", "TFL", "TFR", "TRL", "TRR"],
            _ => &["FL", "FR", "FC", "LFE", "RL", "RR", "SL", "SR", "TFL", "TFR", "TRL", "TRR"],
        }
    }

    /// The fewest channels that hold every feed's.
    fn covering(counts: impl Iterator<Item = usize>) -> usize {
        let counts: Vec<usize> = counts.collect();
        let holds = |layout: usize, feed: usize| {
            let have = names(layout);
            names(feed).iter().all(|n| have.contains(n) || (*n == "MONO" && have.contains(&"FL")))
        };
        [1, 2, 6, 8, 10, 12].into_iter().find(|&layout| counts.iter().all(|&c| holds(layout, c))).unwrap_or(12)
    }

    /// A hundredth of a second of every feed under a name, added together: its samples and
    /// channel count; `None` until there is that much to give. With more than one feed, it is
    /// given once every feed still sounding has its hundredth -- two players at once are each
    /// heard whole, not one in pieces -- while one that has stopped sending (an engine's tap
    /// with nothing playing, a paused page) holds nothing up; one that falls a tenth of a second
    /// behind the others is not waited for either.
    fn mixed(named: &mut Named, frames: usize) -> Option<(Vec<i16>, usize)> {
        if named.feeds.len() > 1 {
            let full = |f: &Feed| f.waiting.len() >= frames * f.channels;
            let anyone = named.feeds.values().any(full);
            let far_ahead = named.feeds.values().any(|f| f.waiting.len() >= 8 * frames * f.channels);
            let waited_for = named.feeds.values().all(|f| full(f) || f.heard.elapsed() > STILL_SOUNDING);
            if !anyone || !(waited_for || far_ahead) {
                return None;
            }
        }
        if named.feeds.len() == 1 {
            let feed = named.feeds.values_mut().next()?;
            let n = frames * feed.channels;
            if feed.waiting.len() < n {
                return None;
            }
            return Some((feed.waiting.drain(..n).collect(), feed.channels));
        }
        let channels = covering(named.feeds.values().map(|f| f.channels));
        let to_names = names(channels);
        let mut out = vec![0f32; frames * channels];
        for feed in named.feeds.values_mut() {
            let own = feed.channels;
            let n = (frames * own).min(feed.waiting.len() / own * own);
            let taken: Vec<i16> = feed.waiting.drain(..n).collect();
            let to: Vec<Vec<(usize, f32)>> = names(own)
                .iter()
                .map(|n| match to_names.iter().position(|m| m == n) {
                    Some(i) => vec![(i, 1.0)],
                    None if *n == "MONO" && channels >= 2 => vec![(0, std::f32::consts::FRAC_1_SQRT_2), (1, std::f32::consts::FRAC_1_SQRT_2)],
                    None => Vec::new(),
                })
                .collect();
            for (f, frame) in taken.chunks_exact(own).enumerate() {
                for (c, v) in frame.iter().enumerate() {
                    for &(i, gain) in &to[c] {
                        out[f * channels + i] += *v as f32 * gain;
                    }
                }
            }
        }
        Some((out.into_iter().map(|v| v.clamp(-32768.0, 32767.0) as i16).collect(), channels))
    }

    pub fn socket_path() -> Option<std::path::PathBuf> {
        std::env::var_os("XDG_RUNTIME_DIR").map(|d| std::path::PathBuf::from(d).join("spatiand-sound"))
    }

    fn serve(mut stream: UnixStream) {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let mut head = [0u8; 12];
        // A sink seeing whether the session is there says nothing and goes.
        if stream.read_exact(&mut head).is_err() {
            return;
        }
        if &head[..4] == b"SPRT" {
            // A route: the rest of the head is the start of the feed's name.
            let feed_len = u16::from_le_bytes([head[4], head[5]]) as usize;
            let mut rest = vec![0u8; feed_len.min(512)];
            rest[..6.min(feed_len)].copy_from_slice(&head[6..6 + 6.min(feed_len)]);
            if feed_len > 6 && stream.read_exact(&mut rest[6..]).is_err() {
                return;
            }
            let mut len = [0u8; 2];
            if stream.read_exact(&mut len).is_err() {
                return;
            }
            let mut to = vec![0u8; (u16::from_le_bytes(len) as usize).min(512)];
            if stream.read_exact(&mut to).is_err() {
                return;
            }
            let (feed, to) = (String::from_utf8_lossy(&rest).into_owned(), String::from_utf8_lossy(&to).into_owned());
            log::info!("sound from {feed} is heard at {}", if to.is_empty() { "its own name" } else { &to });
            let mut routes = routes().lock().unwrap();
            if to.is_empty() {
                routes.remove(&feed);
            } else {
                routes.insert(feed, to);
            }
            return;
        }
        if &head[..4] != b"SPSN" {
            return;
        }
        let channels = u16::from_le_bytes([head[4], head[5]]).max(1) as usize;
        let length = u16::from_le_bytes([head[10], head[11]]) as usize;
        let mut id = vec![0u8; length.min(256)];
        if stream.read_exact(&mut id).is_err() {
            return;
        }
        let own = String::from_utf8_lossy(&id).into_owned();
        let me = NEXT.fetch_add(1, Ordering::Relaxed);
        // Heard under its route, if it has one, and followed as that changes.
        let mut app = routed(&own);
        log::info!("sound from {own}: {channels} channels, heard as {app}");
        senders().lock().unwrap().entry(app.clone()).or_default().feeds.insert(me, Feed { channels, waiting: VecDeque::new(), heard: std::time::Instant::now() });
        // A hundredth of a second at a time, as it comes.
        const FRAMES: usize = 480;
        let mut bytes = vec![0u8; FRAMES * channels * 2];
        // SPATIAND_DEBUG_OWN_SOUND: how loud it is, once a second, for a test to read.
        let debug = std::env::var_os("SPATIAND_DEBUG_OWN_SOUND").is_some();
        let (mut peak, mut blocks) = (0i32, 0u32);
        let ended = loop {
            if let Err(e) = stream.read_exact(&mut bytes) {
                break e;
            }
            let pcm: Vec<i16> = bytes.chunks_exact(2).map(|pair| i16::from_le_bytes([pair[0], pair[1]])).collect();
            if debug {
                peak = peak.max(pcm.iter().map(|s| (*s as i32).abs()).max().unwrap_or(0));
                blocks += 1;
                if blocks == 100 {
                    log::info!("sound from {app}: peak {peak} in the last second");
                    (peak, blocks) = (0, 0);
                }
            }
            // Queued under the name, and as much handed over as there is to give.
            let out = {
                let now = routed(&own);
                let mut all = senders().lock().unwrap();
                if now != app {
                    // The tab moved to another window: so does its sound.
                    let moved = all.get_mut(&app).and_then(|named| named.feeds.remove(&me));
                    if all.get(&app).is_some_and(|named| named.feeds.is_empty()) {
                        all.remove(&app);
                    }
                    let feed = moved.unwrap_or(Feed { channels, waiting: VecDeque::new(), heard: std::time::Instant::now() });
                    all.entry(now.clone()).or_default().feeds.insert(me, feed);
                    log::info!("sound from {own} is now heard as {now}");
                    app = now;
                }
                let Some(named) = all.get_mut(&app) else { continue };
                if let Some(feed) = named.feeds.get_mut(&me) {
                    feed.heard = std::time::Instant::now();
                    feed.waiting.extend(pcm.iter().copied());
                    // Never more than a tenth of a second behind: a feed that runs ahead of
                    // the one setting the pace loses its oldest.
                    let most = FRAMES * 10 * feed.channels;
                    if feed.waiting.len() > most {
                        let extra = feed.waiting.len() - most;
                        feed.waiting.drain(..extra);
                    }
                }
                let mut out = Vec::new();
                while let Some((pcm, ch)) = mixed(named, FRAMES) {
                    out.push((pcm, ch, named.slot));
                }
                out
            };
            for (pcm, ch, slot) in out {
                spatiand_audio::server::feed_local(slot, &pcm, ch);
            }
        };
        {
            let mut all = senders().lock().unwrap();
            if let Some(named) = all.get_mut(&app) {
                named.feeds.remove(&me);
                if named.feeds.is_empty() {
                    all.remove(&app);
                }
            }
        }
        log::info!("sound from {own} has ended ({ended})");
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn feed(channels: usize, samples: usize, value: i16) -> Feed {
            Feed { channels, waiting: std::iter::repeat_n(value, samples * channels).collect(), heard: std::time::Instant::now() }
        }

        #[test]
        fn one_feed_that_sends_nothing_holds_nothing_up() {
            let mut named = Named::default();
            let mut quiet = feed(2, 0, 0);
            quiet.heard -= STILL_SOUNDING * 2;
            named.feeds.insert(1, quiet);
            named.feeds.insert(2, feed(2, 480, 1000));
            let (pcm, channels) = mixed(&mut named, 480).expect("the sounding one sets the pace");
            assert_eq!((pcm.len(), channels), (960, 2));
            assert!(pcm.iter().all(|s| *s == 1000));
        }

        #[test]
        fn two_sounding_feeds_are_each_heard_whole() {
            let mut named = Named::default();
            named.feeds.insert(1, feed(2, 480, 1000));
            named.feeds.insert(2, feed(2, 200, 100));
            assert!(mixed(&mut named, 480).is_none(), "the second, still sending, is waited for");
            named.feeds.get_mut(&2).unwrap().waiting.extend(std::iter::repeat_n(100, 280 * 2));
            let (pcm, _) = mixed(&mut named, 480).unwrap();
            assert!(pcm.iter().all(|s| *s == 1100), "both whole, added together");
        }

        #[test]
        fn mono_and_stereo_are_added_into_stereo() {
            let mut named = Named::default();
            named.feeds.insert(1, feed(1, 960, 1000));
            named.feeds.insert(2, feed(2, 960, 100));
            let (pcm, channels) = mixed(&mut named, 480).unwrap();
            assert_eq!(channels, 2);
            assert_eq!(pcm[0], 100 + (1000.0 * std::f32::consts::FRAC_1_SQRT_2) as i16);
        }
    }

    /// Start listening, once, where the session's own applications will look.
    pub fn listen() {
        static STARTED: OnceLock<()> = OnceLock::new();
        STARTED.get_or_init(|| {
            let Some(path) = socket_path() else { return };
            let _ = std::fs::remove_file(&path);
            match UnixListener::bind(&path) {
                Ok(listener) => {
                    let _ = std::thread::Builder::new().name("own-sound".into()).spawn(move || {
                        for stream in listener.incoming().flatten() {
                            let _ = std::thread::Builder::new().name("own-sound-in".into()).spawn(move || serve(stream));
                        }
                    });
                    log::info!("Spatiand's applications' sound is taken at {}", path.display());
                }
                Err(e) => log::warn!("no socket for Spatiand's applications' sound: {e}"),
            }
        });
    }

    /// The applications sounding that have no slot yet, by id.
    pub fn waiting() -> Vec<String> {
        senders().lock().unwrap().iter().filter(|(_, s)| s.slot.is_none()).map(|(a, _)| a.clone()).collect()
    }

    /// Say which slot a sounding application's windows have.
    pub fn assign(app: &str, slot: Slot) {
        if let Some(entry) = senders().lock().unwrap().get_mut(app) {
            if entry.slot.is_none() {
                log::info!("sound from {app} is placed through slot {slot}");
            }
            entry.slot = Some(slot);
        }
    }
}
