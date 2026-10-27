//! Installing an application by dropping its `.spatiapp` into the room.
//!
//! A file manager in the room starts a drag; Spatiand asks it for the file's address
//! (`text/uri-list`) at once, because a drop onto nothing -- the room, not a window -- cancels the
//! drag before the compositor hears of the drop, and a source that has been cancelled no longer
//! answers. If the drag ends on the room with a `.spatiapp` in it, the package is read (not yet
//! installed) and the notice card under the status bar asks: "Install SpatiQuake 1.2? Press here".
//! Pressing the card installs it (`spatiand_platform::installed`), and the launcher's Spatiand
//! bubble lists it from then on. An offer not taken goes away after half a minute.
//!
//! The reading and the unpacking happen on threads of their own: a package of a game's data is
//! hundreds of megabytes, and the room must not stop for it.

use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use spatiand_platform::installed;

/// How long an offer, or what became of one, stays on the card.
const OFFER_FOR: Duration = Duration::from_secs(30);
const RESULT_FOR: Duration = Duration::from_secs(9);

enum Card {
    /// Read and waiting for a press.
    Offer { path: PathBuf, name: String, version: Option<String> },
    /// Being read, or being installed: nothing to press.
    Busy(String),
    /// What happened.
    Said(String),
}

struct State {
    card: Option<(Card, Instant)>,
    /// What the drag in progress carries, as the source answers.
    dragged: Option<std::sync::Arc<Mutex<Option<String>>>>,
    /// The launcher's list is to be read again.
    changed: bool,
}

static STATE: Mutex<State> = Mutex::new(State { card: None, dragged: None, changed: false });

fn set(card: Card) {
    STATE.lock().unwrap().card = Some((card, Instant::now()));
}

/// A drag has started with a source that offers these types. If it offers file addresses, the
/// source is asked for them now: `ask` is given the write end of a pipe to hand the source.
pub fn drag_started(mime_types: &[String], ask: impl FnOnce(std::os::fd::OwnedFd)) {
    let mut state = STATE.lock().unwrap();
    state.dragged = None;
    if !mime_types.iter().any(|m| m == "text/uri-list") {
        return;
    }
    let mut fds = [0; 2];
    // SAFETY: a plain pipe(2) into a two-element array.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return;
    }
    // SAFETY: both were just made and are owned by nothing else.
    let (read, write) = unsafe {
        use std::os::fd::FromRawFd;
        (std::fs::File::from_raw_fd(fds[0]), std::os::fd::OwnedFd::from_raw_fd(fds[1]))
    };
    let answer = std::sync::Arc::new(Mutex::new(None));
    state.dragged = Some(answer.clone());
    drop(state);
    ask(write);
    std::thread::spawn(move || {
        let mut text = String::new();
        // A source writes and closes; one that never does leaves only this thread waiting.
        if read.take(1 << 20).read_to_string(&mut text).is_ok() {
            *answer.lock().unwrap() = Some(text);
        }
    });
}

/// The drag ended. On the room (no window under the pointer) a package in it is offered.
pub fn drag_ended(on_room: bool) {
    let dragged = STATE.lock().unwrap().dragged.take();
    if !on_room {
        return;
    }
    let Some(dragged) = dragged else { return };
    std::thread::spawn(move || {
        // The source may still be writing: give it a moment, then go with what came.
        let deadline = Instant::now() + Duration::from_secs(2);
        let text = loop {
            if let Some(text) = dragged.lock().unwrap().clone() {
                break text;
            }
            if Instant::now() > deadline {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        if let Some(path) = packages(&text).into_iter().next() {
            offer(path);
        }
    });
}

/// The `.spatiapp` files in a `text/uri-list`.
pub fn packages(uri_list: &str) -> Vec<PathBuf> {
    uri_list
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("file://"))
        // file://host/path: only this machine's files are files here.
        .map(|rest| if rest.starts_with('/') { rest } else { rest.find('/').map_or(rest, |i| &rest[i..]) })
        .map(|p| PathBuf::from(percent_decode(p)))
        .filter(|p| installed::is_package(p))
        .collect()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(high << 4 | low);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Read a package and, if it is one, offer to install it.
pub fn offer(path: PathBuf) {
    set(Card::Busy(format!("Reading {}…", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default())));
    std::thread::spawn(move || match installed::inspect(&path) {
        Ok(manifest) => {
            log::info!("offering to install {} from {}", manifest.name, path.display());
            set(Card::Offer { path, name: manifest.name, version: manifest.version });
        }
        Err(e) => {
            log::warn!("{} is not an application to install: {e}", path.display());
            set(Card::Said(format!("Not installed: {e}")));
        }
    });
}

/// Whether the card is asking, so a press on it means "yes".
pub fn asking() -> bool {
    matches!(STATE.lock().unwrap().card, Some((Card::Offer { .. }, at)) if at.elapsed() < OFFER_FOR)
}

/// The card was pressed: install what was offered.
pub fn accept() {
    let mut state = STATE.lock().unwrap();
    let Some((Card::Offer { path, name, .. }, at)) = &state.card else {
        return;
    };
    if at.elapsed() >= OFFER_FOR {
        return;
    }
    let path = path.clone();
    state.card = Some((Card::Busy(format!("Installing {name}…")), Instant::now()));
    drop(state);
    std::thread::spawn(move || {
        let said = match installed::install(&path) {
            Ok(app) => format!("{} is installed: open it from the Spatiand bubble", app.manifest.name),
            Err(e) => {
                log::warn!("installing {}: {e}", path.display());
                format!("Not installed: {e}")
            }
        };
        let mut state = STATE.lock().unwrap();
        state.card = Some((Card::Said(said), Instant::now()));
        state.changed = true;
    });
}

/// Whether installed applications changed since last asked: the launcher's list is read again.
pub fn take_changed() -> bool {
    std::mem::take(&mut STATE.lock().unwrap().changed)
}

/// What the notice card says about installing, or nothing.
pub fn notice() -> String {
    let mut state = STATE.lock().unwrap();
    let text = match &state.card {
        Some((Card::Offer { name, version, .. }, at)) if at.elapsed() < OFFER_FOR => {
            let version = version.as_ref().map(|v| format!(" {v}")).unwrap_or_default();
            format!("Install {name}{version}?\nPress here to install it.")
        }
        Some((Card::Busy(text), _)) => text.clone(),
        Some((Card::Said(text), at)) if at.elapsed() < RESULT_FOR => text.clone(),
        _ => String::new(),
    };
    if text.is_empty() {
        state.card = None;
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packages_are_found_in_a_uri_list() {
        let list = "# a comment\r\nfile:///home/deck/Downloads/SpatiQuake-1.spatiapp\r\nfile:///home/deck/a.txt\r\n\
                    file://steamdeck/home/deck/My%20Games/Other.SPATIAPP\r\nhttps://example.org/x.spatiapp\r\n";
        assert_eq!(
            packages(list),
            vec![
                PathBuf::from("/home/deck/Downloads/SpatiQuake-1.spatiapp"),
                PathBuf::from("/home/deck/My Games/Other.SPATIAPP")
            ]
        );
    }

    #[test]
    fn percent_decoding_leaves_what_is_not_an_escape() {
        assert_eq!(percent_decode("a%20b%zz%2"), "a b%zz%2");
        assert_eq!(percent_decode("caf%C3%A9 é%"), "café é%");
    }
}
