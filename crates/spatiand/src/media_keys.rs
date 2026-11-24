//! A keyboard's media keys: play and pause, forward and back.
//!
//! Every system has them, under its own names -- a Mac's F7 to F9, Android's `KEYCODE_MEDIA_*`,
//! the kernel's `KEY_PLAYPAUSE` and its neighbours -- and each platform's own code turns its
//! keyboard's into the kernel's numbers, as it does every other key. From there this is the
//! same everywhere: the keys are not the focused window's. They are taken before any window
//! sees them and handed to whatever said it is playing (`spatiand_xr_surface_v1.set_media_keys`),
//! wherever the keyboard is typing.
//!
//! The owner, 2026-11-24: "if spatireel not open, press the play, call the spatireel... if open
//! and no media played, then do nothing". So with nothing playing, the play key starts
//! SpatiReel if it has no window, and nothing else does anything.

use std::time::{Duration, Instant};

use crate::state::Spatiand;

/// The kernel's numbers for the keys (`input-event-codes.h`), and the protocol's for each.
const KEYS: [(u32, u32); 9] = [
    (164, 1), // KEY_PLAYPAUSE
    (200, 2), // KEY_PLAYCD
    (207, 2), // KEY_PLAY
    (201, 3), // KEY_PAUSECD
    (166, 4), // KEY_STOPCD
    (163, 5), // KEY_NEXTSONG
    (165, 6), // KEY_PREVIOUSSONG
    (208, 7), // KEY_FASTFORWARD
    (168, 8), // KEY_REWIND
];

const PLAY_PAUSE: u32 = 1;
const PLAY: u32 = 2;

/// SpatiReel's application id, and its name among the room's own applications.
const SPATIREEL: &str = "id.prasetya.spatiand.spatireel";
const SPATIREEL_NAME: &str = "SpatiReel";

/// How long after SpatiReel was started by the play key another press does not start it again:
/// its window takes a moment to come, and a second press meanwhile is the same wish.
const STARTING: Duration = Duration::from_secs(10);

/// The protocol's number for a key, if it is a media key.
pub fn of(code: u32) -> Option<u32> {
    KEYS.iter().find(|(key, _)| *key == code).map(|(_, media)| *media)
}

/// A key's press or release: `true` when it was a media key, which no window is then to be
/// sent as a key.
pub fn key(state: &mut Spatiand, code: u32, pressed: bool) -> bool {
    let Some(media) = of(code) else { return false };
    // A key held is told once: Android says a held key down again and again, and what holding
    // one means is the player's to do.
    static HELD: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
    {
        let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
        let was = held.contains(&code);
        if pressed == was {
            return true;
        }
        if pressed { held.push(code) } else { held.retain(|c| *c != code) }
    }
    let focused = state.layout.focused_id();
    // Whatever says it is playing: the window typed into first, or else the newest.
    let playing = state
        .space
        .elements()
        .filter_map(|window| {
            let surface = window.toplevel()?.wl_surface().clone();
            let id = state.layout.id_of(window)?;
            crate::xr::state_of(&surface).media_keys.then_some((focused == Some(id), id, surface))
        })
        .max_by_key(|(focused, id, _)| (*focused, *id));
    if let Some((_, _, surface)) = playing {
        crate::xr::tell_media_key(&surface, media, pressed);
        return true;
    }
    if pressed && matches!(media, PLAY | PLAY_PAUSE) {
        start_spatireel(state);
    }
    true
}

/// Start SpatiReel, unless it has a window or was started a moment ago.
fn start_spatireel(state: &mut Spatiand) {
    static STARTED: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
    let open = state.space.elements().any(|window| state.app_id_of(window).as_deref() == Some(SPATIREEL));
    if open {
        return;
    }
    let mut started = STARTED.lock().unwrap_or_else(|e| e.into_inner());
    if started.is_some_and(|at| at.elapsed() < STARTING) {
        return;
    }
    let Some(app) = crate::own::list().into_iter().find(|app| app.name == SPATIREEL_NAME) else {
        log::info!("the play key: SpatiReel is not installed here");
        return;
    };
    *started = Some(Instant::now());
    log::info!("the play key, with nothing playing and SpatiReel not open: starting it");
    match &mut state.own_launches {
        Some(waiting) => waiting.push(app),
        None => crate::own::launch(&app, &state.socket_name.clone(), &[]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kernels_media_keys_are_the_protocols() {
        assert_eq!(of(164), Some(PLAY_PAUSE));
        assert_eq!(of(207), Some(PLAY));
        assert_eq!(of(208), Some(7));
        assert_eq!(of(168), Some(8));
        // The volume keys are the room's own, and a letter is a letter.
        assert_eq!(of(115), None);
        assert_eq!(of(30), None);
    }
}
