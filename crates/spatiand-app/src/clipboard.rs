//! The clipboard: the room's, which is every machine's it can see.
//!
//! The compositor holds one clipboard for everything in the session -- its own windows, a
//! host's applications, the Mac's (see `crates/spatiand/src/clipboard.rs`) -- and an
//! application joins it as any Wayland client does. Text is all an application here copies or
//! pastes. What is on the clipboard is read as soon as it changes, on a thread of its own, so a
//! paste is never a wait: [`text`] answers at once with what was last read.
//!
//! [`copy`] may be called from anywhere -- a field in the middle of handling a key -- and takes
//! effect at the application's next [`crate::App::wait`].

use std::sync::Mutex;

struct Board {
    /// The clipboard's text as last read, or as last copied here.
    held: Option<String>,
    /// Bumped whenever the clipboard changes, so that a slow read of an older one cannot
    /// overwrite a newer.
    generation: u64,
    /// Copied here, not yet told the compositor.
    wanted: Option<String>,
}

static BOARD: Mutex<Board> = Mutex::new(Board { held: None, generation: 0, wanted: None });

/// What is on the clipboard, if it is text.
pub fn text() -> Option<String> {
    BOARD.lock().unwrap().held.clone()
}

/// Put `text` on the clipboard.
pub fn copy(text: impl Into<String>) {
    let text = text.into();
    let mut board = BOARD.lock().unwrap();
    board.generation += 1;
    board.held = Some(text.clone());
    board.wanted = Some(text);
}

pub(crate) fn take_wanted() -> Option<String> {
    BOARD.lock().unwrap().wanted.take()
}

/// The clipboard changed to something not yet read: its generation, for [`read`].
pub(crate) fn changed() -> u64 {
    let mut board = BOARD.lock().unwrap();
    board.generation += 1;
    board.held = None;
    board.generation
}

/// What a clipboard turned out to hold, if it is still the clipboard.
pub(crate) fn read(generation: u64, text: Option<String>) {
    let mut board = BOARD.lock().unwrap();
    if board.generation == generation {
        board.held = text;
    }
}
