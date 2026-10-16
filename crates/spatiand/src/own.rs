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
/// Not on the Deck, where an application plays into its window's PipeWire sink.
#[cfg(any(target_os = "macos", target_os = "android"))]
pub mod sound {
    use std::collections::HashMap;
    use std::io::Read;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::{Mutex, OnceLock};

    use spatiand_audio::server::Slot;

    /// Each sounding application, by its id, and its windows' slot once it has one.
    fn senders() -> &'static Mutex<HashMap<String, Option<Slot>>> {
        static SENDERS: OnceLock<Mutex<HashMap<String, Option<Slot>>>> = OnceLock::new();
        SENDERS.get_or_init(Default::default)
    }

    pub fn socket_path() -> Option<std::path::PathBuf> {
        std::env::var_os("XDG_RUNTIME_DIR").map(|d| std::path::PathBuf::from(d).join("spatiand-sound"))
    }

    fn serve(mut stream: UnixStream) {
        let mut head = [0u8; 12];
        if let Err(e) = stream.read_exact(&mut head) {
            log::warn!("a sound connection said nothing: {e}");
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
        let app = String::from_utf8_lossy(&id).into_owned();
        log::info!("sound from {app}: {channels} channels");
        senders().lock().unwrap().entry(app.clone()).or_insert(None);
        // A hundredth of a second at a time, as it comes.
        let mut bytes = vec![0u8; 480 * channels * 2];
        let mut pcm = vec![0i16; 480 * channels];
        let ended = loop {
            if let Err(e) = stream.read_exact(&mut bytes) {
                break e;
            }
            for (sample, pair) in pcm.iter_mut().zip(bytes.chunks_exact(2)) {
                *sample = i16::from_le_bytes([pair[0], pair[1]]);
            }
            let slot = senders().lock().unwrap().get(&app).copied().flatten();
            spatiand_audio::server::feed_local(slot, &pcm, channels);
        };
        senders().lock().unwrap().remove(&app);
        log::info!("sound from {app} has ended ({ended})");
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
        senders().lock().unwrap().iter().filter(|(_, s)| s.is_none()).map(|(a, _)| a.clone()).collect()
    }

    /// Say which slot a sounding application's windows have.
    pub fn assign(app: &str, slot: Slot) {
        if let Some(entry) = senders().lock().unwrap().get_mut(app) {
            if entry.is_none() {
                log::info!("sound from {app} is placed through slot {slot}");
            }
            *entry = Some(slot);
        }
    }
}
