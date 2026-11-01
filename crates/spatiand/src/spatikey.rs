//! The room's half of SpatiKey (docs/spatikey.md): typing an entry into a window, knowing which
//! window that is, and the card that says a Mac is being asked.
//!
//! SpatiKey's service runs on a thread of its own and asks the room through
//! `spatikey_core::hooks::Room`; what it asks is queued here and done each frame, on the
//! compositor's own thread ([`serve`]), as the assistant's requests are. On the Deck and the
//! Beam Pro the service itself is started here too: with no fingerprint reader, it forwards every
//! request to a paired Mac on the same network. On the Mac the service lives in the app, which
//! is up whether or not the room is.

use std::sync::{mpsc, Arc, Mutex, Once};
use std::time::{Duration, Instant};

use spatikey_core::hooks::{Room, Step, Target};

use crate::state::Spatiand;

/// SpatiKey's own windows, which are never the window to type into.
const APP_ID: &str = "id.prasetya.spatiand.spatikey";

/// A card says a Mac is being asked for this long at most, unless the service takes it away.
const CARD: Duration = Duration::from_secs(120);

struct TypeAsked {
    window: u64,
    steps: Vec<Step>,
    answer: mpsc::Sender<Result<(), String>>,
}

static ASKED: Mutex<Vec<TypeAsked>> = Mutex::new(Vec::new());
/// The window the wearer was in last that is not SpatiKey's own.
static TARGET: Mutex<Option<Target>> = Mutex::new(None);
static NOTICE: Mutex<Option<(String, Instant)>> = Mutex::new(None);

struct TheRoom;

impl Room for TheRoom {
    fn target(&self) -> Option<Target> {
        TARGET.lock().unwrap().clone()
    }

    fn type_into(&self, window: u64, steps: &[Step]) -> Result<(), String> {
        let (answer, answered) = mpsc::channel();
        ASKED.lock().unwrap().push(TypeAsked { window, steps: steps.to_vec(), answer });
        answered.recv_timeout(Duration::from_secs(10)).map_err(|_| "the room did not get to it".to_string())?
    }

    fn notice(&self, text: &str) {
        *NOTICE.lock().unwrap() = (!text.is_empty()).then(|| (text.to_string(), Instant::now()));
    }
}

/// The room set up for SpatiKey, and on a machine with no Touch ID of its own, the service that
/// forwards to a Mac. Once per process.
fn start() {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        spatikey_core::hooks::set_room(Some(Arc::new(TheRoom)));
        #[cfg(not(target_os = "macos"))]
        forward();
    });
}

/// SpatiKey with no room: the desktop's own session, where programs still want their passwords and
/// a finger is still asked for on the Mac. Runs as long as that desktop does, so that the socket
/// is free for the room when the session changes to it.
#[cfg(not(target_os = "macos"))]
pub fn desktop_service() -> Result<(), String> {
    use std::io::Read;
    let socket = std::env::var_os("XDG_RUNTIME_DIR")
        .zip(std::env::var_os("WAYLAND_DISPLAY"))
        .map(|(dir, display)| std::path::Path::new(&dir).join(display))
        .ok_or("no Wayland desktop here")?;
    let mut desktop = std::os::unix::net::UnixStream::connect(&socket).map_err(|e| format!("{}: {e}", socket.display()))?;
    spatikey_core::hooks::set_room(Some(Arc::new(Desktop)));
    forward();
    // Nothing is said on it: the compositor closes it as it ends.
    let _ = desktop.read(&mut [0u8; 1]);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
struct Desktop;

#[cfg(not(target_os = "macos"))]
impl Room for Desktop {
    fn target(&self) -> Option<Target> {
        None
    }

    fn type_into(&self, _window: u64, _steps: &[Step]) -> Result<(), String> {
        Err("typing into a window needs the room".to_string())
    }

    /// As a notification of the desktop's, which stays its few seconds after the finger.
    fn notice(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        let text = text.to_string();
        std::thread::spawn(move || {
            let _ = std::process::Command::new("notify-send").args(["-a", "SpatiKey", "SpatiKey", &text]).status();
        });
    }
}

/// The Deck's and the Beam Pro's service: everything to a paired Mac on this network, with the
/// identity this machine pairs with hosts as.
#[cfg(not(target_os = "macos"))]
fn forward() {
    use spatikey_core::paths;
    use spatikey_core::service::{Me, Mode, Service};
    let identity = match spatiand_stream::Identity::load_or_create(&paths::identity_dir()) {
        Ok(identity) => identity,
        Err(e) => {
            log::warn!("SpatiKey: no identity to ask a Mac with: {e}");
            return;
        }
    };
    let name = crate::system::machine_name();
    let fp = identity.fingerprint().to_string();
    let asker = Arc::new(spatikey_core::remote::Asker::new(identity, paths::local(), name.clone()));
    asker.also_ask(Box::new(|| {
        crate::prefs::Prefs::load()
            .remotes
            .into_iter()
            .map(|r| spatikey_core::wire::Device { fp: r.fingerprint, name: r.host, added: 0 })
            .collect()
    }));
    let service = Service::new(Mode::Forwarder(asker), Me { fp, name }, paths::shared(), paths::local());
    match spatikey_core::socket::serve(&paths::socket(), service) {
        Ok(_) => log::info!("SpatiKey: asking a Mac on this network, at {}", paths::socket().display()),
        Err(e) => log::warn!("SpatiKey: could not listen at {}: {e}", paths::socket().display()),
    }
}

/// SpatiKey's card, while a Mac is being asked.
pub fn notice() -> String {
    let mut notice = NOTICE.lock().unwrap();
    match notice.as_ref() {
        Some((text, at)) if at.elapsed() < CARD => text.clone(),
        Some(_) => {
            *notice = None;
            String::new()
        }
        None => String::new(),
    }
}

/// Each frame: the window SpatiKey would type into kept up to date, and what it asked done.
pub fn serve(state: &mut Spatiand, time_ms: u32) {
    start();
    if let Some(id) = state.layout.focused_id() {
        let app = state.app_id_of_id(id).unwrap_or_default();
        if app != APP_ID {
            let window = state.space.elements().find(|w| state.layout.id_of(w) == Some(id)).cloned();
            let title = window.as_ref().and_then(|w| state.title_of(w)).unwrap_or_default();
            let mut target = TARGET.lock().unwrap();
            if target.as_ref().map_or(true, |t| t.window != id as u64 || t.title != title) {
                *target = Some(Target { window: id as u64, app, title });
            }
        }
    }
    let asked = std::mem::take(&mut *ASKED.lock().unwrap());
    for a in asked {
        #[cfg(target_os = "macos")]
        {
            // A Mac's own window: typed by macOS, as the characters themselves, any layout; the
            // answer comes from the app's side when it is done, without holding this frame.
            let app = state.app_id_of_id(a.window as usize).unwrap_or_default();
            if crate::mac::is_mac_window(&app) {
                crate::mac::type_steps(&app, &a.steps, a.answer);
                continue;
            }
        }
        let done = type_into(state, a.window, &a.steps, time_ms);
        let _ = a.answer.send(done);
    }
}

/// Type into one window, all of it or nothing: every character is looked up on the room's
/// keyboard before the first key goes, and nothing goes unless the keyboard's focus is on that
/// window. (Dictation, typing what someone said, leaves out what it cannot type; a password with a
/// letter left out is worse than none.)
fn type_into(state: &mut Spatiand, window: u64, steps: &[Step], time_ms: u32) -> Result<(), String> {
    let target = state.space.elements().find(|w| state.layout.id_of(w) == Some(window as usize)).cloned();
    let Some(target) = target else { return Err("that window has gone".into()) };
    let mut strokes = Vec::new();
    for step in steps {
        match step {
            Step::Text(text) => {
                for c in text.chars() {
                    match spatiand_shell::keyboard::stroke_for(c) {
                        Some(stroke) => strokes.push((stroke.code, stroke.shift)),
                        None => return Err(format!("the room's keyboard has no key for a character in it ({})", describe(c))),
                    }
                }
            }
            Step::Tab => strokes.push((spatiand_shell::keyboard::KEY_TAB, false)),
            Step::Enter => strokes.push((spatiand_shell::keyboard::KEY_ENTER, false)),
        }
    }
    state.focus_window(&target);
    if state.layout.focused_id() != Some(window as usize) {
        return Err("the keyboard could not be given to that window".into());
    }
    for (code, shift) in strokes {
        crate::assist::stroke(state, code, shift, time_ms);
    }
    Ok(())
}

/// A character named without showing it, for a message about a password.
fn describe(c: char) -> String {
    if c.is_ascii_graphic() {
        "a symbol".into()
    } else if c.is_alphabetic() {
        "a letter with an accent, or of another alphabet".into()
    } else {
        format!("U+{:04X}", c as u32)
    }
}
