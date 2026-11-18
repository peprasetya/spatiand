//! The room's half of SpatiKey (docs/spatikey.md): typing an entry into a window, knowing which
//! window that is, and the card that says a Mac is being asked.
//!
//! SpatiKey's service runs on a thread of its own and asks the room through
//! `spatikey_core::hooks::Room`; what it asks is queued here and done each frame, on the
//! compositor's own thread ([`serve`]), as the assistant's requests are. On the Deck and on
//! Android the service itself is started here too: with no fingerprint reader (the Deck, the Beam
//! Pro), it forwards every request to a paired holder on the same network; on an Android device
//! with a strong biometric it holds the vault itself once added (docs/spatikey.md). On the Mac the
//! service lives in the app, which is up whether or not the room is.

use std::sync::{mpsc, Arc, Mutex, Once};
use std::time::{Duration, Instant};

use spatikey_core::hooks::{Room, Step, Target};

use crate::state::Spatiand;

/// SpatiKey's own windows, which are never the window to type into.
const APP_ID: &str = "id.prasetya.spatiand.spatikey";

/// A card says a Mac is being asked for this long at most, unless the service takes it away.
const CARD: Duration = Duration::from_secs(120);
/// It shows once the asking has gone on this long: a touch still covered by the one before (the
/// setting's 60 or 300 seconds) is answered at once, and a card for one frame says nothing.
const SHOW_AFTER: Duration = Duration::from_millis(300);
/// The card's last word, when the finger did not come, stays this long.
const BRIEF: Duration = Duration::from_secs(4);

/// How long the service waits for the room to type what it asked.
const ROOM_WAIT: Duration = Duration::from_secs(10);
/// How long after the asking the room still starts typing: short of `ROOM_WAIT`, with time for
/// the typing itself, so that what the service gave up on -- and SpatiKey has said was not typed
/// -- is not typed after all when frames come again.
const TYPE_WITHIN: Duration = Duration::from_secs(8);

struct TypeAsked {
    window: u64,
    at: Instant,
    steps: Vec<Step>,
    answer: mpsc::Sender<Result<(), String>>,
}

static ASKED: Mutex<Vec<TypeAsked>> = Mutex::new(Vec::new());
/// The window the wearer was in last that is not SpatiKey's own.
static TARGET: Mutex<Option<Target>> = Mutex::new(None);
static NOTICE: Mutex<Option<Card>> = Mutex::new(None);

/// SpatiKey's card: a finger wanted on the Mac, or the last word when none came.
struct Card {
    text: String,
    since: Instant,
    brief: bool,
}

impl Card {
    /// What it says `shown` after it was put up: None while it waits to show, or once it is over.
    fn says(&self, shown: Duration) -> Option<String> {
        let (from, until) = if self.brief { (Duration::ZERO, BRIEF) } else { (SHOW_AFTER, CARD) };
        (shown >= from && shown < until).then(|| format!("SpatiKey\n{}", self.text))
    }

    fn over(&self, shown: Duration) -> bool {
        shown >= if self.brief { BRIEF } else { CARD }
    }
}

struct TheRoom;

impl Room for TheRoom {
    fn target(&self) -> Option<Target> {
        TARGET.lock().unwrap().clone()
    }

    fn type_into(&self, window: u64, steps: &[Step]) -> Result<(), String> {
        let (answer, answered) = mpsc::channel();
        ASKED.lock().unwrap().push(TypeAsked { window, at: Instant::now(), steps: steps.to_vec(), answer });
        answered.recv_timeout(ROOM_WAIT).map_err(|_| "the room did not get to it".to_string())?
    }

    fn notice(&self, text: &str) {
        let mut notice = NOTICE.lock().unwrap();
        // Taking a card away leaves the last word of another alone.
        if text.is_empty() && notice.as_ref().is_some_and(|card| card.brief) {
            return;
        }
        // A card still asking, given new words (another asking still waiting, once the newest is
        // over): asked since it was first put up, so it is not hidden again for the moment a
        // new card waits before it shows.
        let since = notice.as_ref().filter(|card| !card.brief).map_or_else(Instant::now, |card| card.since);
        finger_no_longer_wanted(&notice, text.is_empty());
        *notice = (!text.is_empty()).then(|| Card { text: text.to_string(), since, brief: false });
    }

    fn notice_briefly(&self, text: &str) {
        let mut notice = NOTICE.lock().unwrap();
        finger_no_longer_wanted(&notice, true);
        *notice = (!text.is_empty()).then(|| Card { text: text.to_string(), since: Instant::now(), brief: true });
    }
}

/// When a finger was last wanted, once it no longer is: see [`finger_wanted_recently`].
static FINGER_ENDED: Mutex<Option<Instant>> = Mutex::new(None);

/// The card in `notice` is about to go (`going`): if it was asking for a finger, it no longer is.
fn finger_no_longer_wanted(notice: &Option<Card>, going: bool) {
    if going && notice.as_ref().is_some_and(|card| !card.brief) {
        *FINGER_ENDED.lock().unwrap() = Some(Instant::now());
    }
}

/// Whether a finger is wanted on the Mac now -- SpatiKey's card up, from the moment it was asked
/// for, the moment before it shows included -- or was until [`crate::window::HELD_AFTER_FINGER`]
/// ago. New windows open meanwhile where the wearer looked when it was asked for
/// ([`crate::state::Spatiand::aim_new_windows`]).
pub fn finger_wanted_recently() -> bool {
    let card = {
        let mut notice = NOTICE.lock().unwrap();
        let shown = notice.as_ref().map(|card| card.since.elapsed());
        if notice.as_ref().zip(shown).is_some_and(|(card, shown)| !card.brief && card.over(shown)) {
            // Asked for so long it is over: no longer wanted, from now.
            finger_no_longer_wanted(&notice, true);
            *notice = None;
        }
        notice.as_ref().map(|card| card.brief)
    };
    let ended = FINGER_ENDED.lock().unwrap().map(|at| at.elapsed());
    finger_wanted(card, ended)
}

/// Whether a finger is being asked for now: SpatiKey's card is up asking for it (not its brief
/// last word). Each new asking takes the aim new windows open at afresh.
pub fn finger_asked() -> bool {
    NOTICE.lock().unwrap().as_ref().is_some_and(|card| !card.brief)
}

/// [`finger_wanted_recently`] of a card (whether it is the brief last word) and how long since a
/// finger was last no longer wanted.
fn finger_wanted(card_is_brief: Option<bool>, ended_ago: Option<Duration>) -> bool {
    card_is_brief == Some(false) || ended_ago.is_some_and(|ago| ago < crate::window::HELD_AFTER_FINGER)
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
    let desktop = desktop_connection()?;
    spatikey_core::hooks::set_room(Some(Arc::new(Desktop)));
    forward();
    until_it_ends(desktop);
    Ok(())
}

/// A connection to the desktop's compositor, for knowing when it has gone.
#[cfg(not(target_os = "macos"))]
pub fn desktop_connection() -> Result<std::os::unix::net::UnixStream, String> {
    let socket = std::env::var_os("XDG_RUNTIME_DIR")
        .zip(std::env::var_os("WAYLAND_DISPLAY"))
        .map(|(dir, display)| std::path::Path::new(&dir).join(display))
        .ok_or("no Wayland desktop here")?;
    std::os::unix::net::UnixStream::connect(&socket).map_err(|e| format!("{}: {e}", socket.display()))
}

/// Nothing is said on it: the compositor closes it as it ends.
#[cfg(not(target_os = "macos"))]
pub fn until_it_ends(mut desktop: std::os::unix::net::UnixStream) {
    use std::io::Read;
    let _ = desktop.read(&mut [0u8; 1]);
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

    fn notice_briefly(&self, text: &str) {
        self.notice(text);
    }
}

/// The service on a machine whose SpatiKey is not in an app of its own (the Mac's is in
/// Spatiand.app): on the Deck and the Beam Pro, everything to a paired holder on this network; on
/// an Android device with a strong biometric, the vault held here once it has been added, and
/// asked of a holder until then. Its identity is the one this machine pairs with hosts as.
#[cfg(not(target_os = "macos"))]
fn forward() {
    // Whether Android has a sensor here is known once the app has given its context, which may be
    // after the room's first frame: asked on a thread of its own, which then serves.
    #[cfg(target_os = "android")]
    let _ = std::thread::Builder::new().name("spatikey-start".into()).spawn(|| serve_here(crate::android::key::guard()));
    #[cfg(not(target_os = "android"))]
    serve_here(None);
}

#[cfg(not(target_os = "macos"))]
fn serve_here(guard: Option<Arc<dyn spatikey_core::service::Guard>>) {
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
    let asker = Arc::new(spatikey_core::remote::Asker::new(identity.clone(), paths::local(), name.clone()));
    asker.also_ask(Box::new(|| {
        crate::prefs::Prefs::load()
            .remotes
            .into_iter()
            .map(|r| spatikey_core::wire::Device { fp: r.fingerprint, name: r.host, added: 0 })
            .collect()
    }));
    let holds = guard.is_some();
    let mode = match guard {
        Some(guard) => Mode::Both(guard, asker),
        None => Mode::Forwarder(asker),
    };
    let service = Service::new(mode, Me { fp, name }, paths::shared(), paths::local());
    match spatikey_core::socket::serve(&paths::socket(), service.clone()) {
        Ok(_) if holds => log::info!("SpatiKey: this device's biometrics may hold the vault; at {}", paths::socket().display()),
        Ok(_) => log::info!("SpatiKey: asking a holder on this network, at {}", paths::socket().display()),
        Err(e) => log::warn!("SpatiKey: could not listen at {}: {e}", paths::socket().display()),
    }
    if holds {
        // Answering devices on this network, as a Mac does, once this one holds the vault.
        match spatikey_core::remote::serve(service, identity) {
            Ok(listening) => std::mem::forget(listening),
            Err(e) => log::warn!("SpatiKey: devices on this network cannot ask this one: {e}"),
        }
    }
}

/// SpatiKey's card, while a finger is wanted on the Mac, and for a few seconds after one did not
/// come; empty otherwise. **It goes over every other card** -- dictation's, the assistant's, a
/// notification -- since someone is to touch the Mac now: each frame asks this first and the
/// others only when it is empty, so the card it covered is back as it was once this one goes,
/// and a press on it is nobody's ([`owns_notice`]).
///
/// The same in every room, the Mac's included. Never on the Mac's desktop (2D), whose screen
/// shows macOS's own Touch ID panel: no card is drawn there.
pub fn notice() -> String {
    let mut notice = NOTICE.lock().unwrap();
    let Some(card) = notice.as_ref() else { return String::new() };
    let shown = card.since.elapsed();
    if card.over(shown) {
        finger_no_longer_wanted(&notice, true);
        *notice = None;
        return String::new();
    }
    card.says(shown).unwrap_or_default()
}

/// Whether the card in view is SpatiKey's. Pressed, it does nothing -- it asks for a finger on
/// the Mac, not here -- and above all does not reach the card it covers (dictation's text typed
/// into a terminal, a notification opened).
pub fn owns_notice() -> bool {
    !notice().is_empty()
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
        // Asked too long ago (no frames meanwhile: the display asleep, the session elsewhere):
        // the wearer has been told it failed, or is about to be, and is no longer looking at
        // that window to see a password arrive in it.
        if !in_time(a.at.elapsed()) {
            let _ = a.answer.send(Err("the room did not get to it".into()));
            continue;
        }
        #[cfg(target_os = "macos")]
        {
            // A Mac's own window: typed by macOS, as the characters themselves, any layout; the
            // answer comes from the app's side when it is done, without holding this frame.
            // Into that window and no other of its application's: the room's keyboard is given
            // to it first, as below, and the app's side is told which of the Mac's windows it
            // is, to bring it forward and see that the Mac's keys are its own before any go.
            let app = state.app_id_of_id(a.window as usize).unwrap_or_default();
            if crate::mac::is_mac_window(&app) {
                match give_keyboard(state, a.window) {
                    Ok(target) => crate::mac::type_steps(&target, &a.steps, a.answer),
                    Err(why) => {
                        let _ = a.answer.send(Err(why));
                    }
                }
                continue;
            }
        }
        let done = type_into(state, a.window, &a.steps, time_ms);
        let _ = a.answer.send(done);
    }
}

/// Is something asked `waited` ago still to be typed?
fn in_time(waited: Duration) -> bool {
    waited < TYPE_WITHIN
}

/// Type into one window, all of it or nothing: every character is looked up on the room's
/// keyboard before the first key goes, and nothing goes unless the keyboard's focus is on that
/// window. (Dictation, typing what someone said, leaves out what it cannot type; a password with a
/// letter left out is worse than none.)
fn type_into(state: &mut Spatiand, window: u64, steps: &[Step], time_ms: u32) -> Result<(), String> {
    if !state.space.elements().any(|w| state.layout.id_of(w) == Some(window as usize)) {
        return Err("that window has gone".into());
    }
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
    // The strokes go through the seat's own keyboard, where a Caps Lock left on (a real keyboard
    // on the Deck) turns every letter to its other case: a password that arrives wrong without
    // anyone seeing it. Refused, and said, rather than typed so.
    if state.seat.get_keyboard().is_some_and(|keyboard| keyboard.modifier_state().caps_lock) {
        return Err("Caps Lock is on: turn it off and try again.".into());
    }
    give_keyboard(state, window)?;
    for (code, shift) in strokes {
        crate::assist::stroke(state, code, shift, time_ms);
    }
    Ok(())
}

/// The room's keyboard given to one window, which is handed back; or why nothing may be typed.
fn give_keyboard(state: &mut Spatiand, window: u64) -> Result<smithay::desktop::Window, String> {
    let target = state.space.elements().find(|w| state.layout.id_of(w) == Some(window as usize)).cloned();
    let Some(target) = target else { return Err("that window has gone".into()) };
    state.focus_window(&target);
    if state.layout.focused_id() != Some(window as usize) {
        return Err("the keyboard could not be given to that window".into());
    }
    Ok(target)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn card(text: &str, brief: bool) -> Card {
        Card { text: text.into(), since: Instant::now(), brief }
    }

    /// A finger wanted: shown only once the asking has gone on a moment, then until the service
    /// takes it away, or two minutes at most.
    #[test]
    fn the_card_while_a_finger_is_wanted() {
        let wanted = card("Touch ID on your Mac to save a password for github.com", false);
        assert_eq!(wanted.says(Duration::from_millis(100)), None);
        assert!(!wanted.over(Duration::from_millis(100)));
        let says = "SpatiKey\nTouch ID on your Mac to save a password for github.com";
        assert_eq!(wanted.says(SHOW_AFTER).as_deref(), Some(says));
        assert_eq!(wanted.says(Duration::from_secs(119)).as_deref(), Some(says));
        assert_eq!(wanted.says(CARD), None);
        assert!(wanted.over(CARD));
    }

    /// The last word: at once, for a few seconds.
    #[test]
    fn the_last_word_when_no_finger_came() {
        let missed = card("Did not save a password for github.com: Touch ID was cancelled", true);
        assert!(missed.says(Duration::ZERO).is_some());
        assert!(missed.says(BRIEF - Duration::from_millis(1)).is_some());
        assert_eq!(missed.says(BRIEF), None);
        assert!(missed.over(BRIEF));
    }

    /// Put up, taken away, the last word kept when the card goes after it; and over the card it
    /// covers only while it is there. One test: the card is the process's.
    #[test]
    fn the_card_goes_over_the_others_and_gives_them_back() {
        let room = TheRoom;
        let under = || "Dictation \u{2014} type into the terminal?\nls -la".to_string();
        // As every frame loop does: SpatiKey's first, the others only when it is empty.
        let in_view = || {
            let ours = notice();
            if ours.is_empty() {
                under()
            } else {
                ours
            }
        };
        let back = |by: Duration| NOTICE.lock().unwrap().as_mut().unwrap().since -= by;
        assert_eq!(in_view(), under());
        assert!(!owns_notice());
        room.notice("Touch ID on your Mac to open SpatiKey");
        // Not yet: answered within the moment, it would never have shown.
        assert_eq!(in_view(), under());
        back(SHOW_AFTER);
        assert_eq!(in_view(), "SpatiKey\nTouch ID on your Mac to open SpatiKey");
        assert!(owns_notice());
        room.notice("");
        assert_eq!(in_view(), under());
        assert!(!owns_notice());
        // Refused: the card goes and the last word comes, which another card's going leaves alone.
        room.notice("Touch ID on your Mac to open SpatiKey");
        room.notice("");
        room.notice_briefly("Did not open SpatiKey: Touch ID was cancelled");
        room.notice("");
        assert_eq!(in_view(), "SpatiKey\nDid not open SpatiKey: Touch ID was cancelled");
        assert!(owns_notice());
        back(BRIEF);
        assert_eq!(in_view(), under());
        assert!(NOTICE.lock().unwrap().is_none());
        // A new asking takes the last word's place at once; and a card nobody took away goes by
        // itself after two minutes.
        room.notice_briefly("Did not open SpatiKey: Touch ID was cancelled");
        room.notice("Touch ID on your Mac to get a code");
        assert!(!NOTICE.lock().unwrap().as_ref().unwrap().brief);
        back(CARD);
        assert_eq!(in_view(), under());
        assert!(NOTICE.lock().unwrap().is_none());
        // Two askings: the newest over, the card goes back to the other at once, not after
        // the moment a new card waits.
        room.notice("Touch ID on your Mac to open SpatiKey");
        back(SHOW_AFTER);
        room.notice("Touch ID on your Mac to get a code");
        assert_eq!(in_view(), "SpatiKey\nTouch ID on your Mac to get a code");
        room.notice("Touch ID on your Mac to open SpatiKey");
        assert_eq!(in_view(), "SpatiKey\nTouch ID on your Mac to open SpatiKey");
        // A finger wanted from the moment it is asked for, the moment before the card shows
        // included, and for a while after.
        room.notice("");
        *FINGER_ENDED.lock().unwrap() = None;
        assert!(!finger_wanted_recently());
        room.notice("Touch ID on your Mac to open a profile");
        assert!(finger_wanted_recently());
        room.notice("");
        assert!(finger_wanted_recently());
        *FINGER_ENDED.lock().unwrap() = Some(Instant::now() - crate::window::HELD_AFTER_FINGER);
        assert!(!finger_wanted_recently());
        // Refused with a last word: no longer wanted from then.
        room.notice("Touch ID on your Mac to open a profile");
        room.notice_briefly("Did not open a profile: Touch ID was cancelled");
        assert!(FINGER_ENDED.lock().unwrap().unwrap().elapsed() < Duration::from_secs(1));
        back(BRIEF);
        notice();
    }

    #[test]
    fn new_windows_are_held_while_a_finger_is_wanted_and_a_while_after() {
        assert!(!finger_wanted(None, None));
        assert!(finger_wanted(Some(false), None), "asked, shown yet or not");
        assert!(!finger_wanted(Some(true), None), "the last word is not an asking");
        assert!(finger_wanted(None, Some(Duration::from_secs(3))));
        assert!(finger_wanted(Some(true), Some(Duration::from_secs(3))));
        assert!(!finger_wanted(None, Some(crate::window::HELD_AFTER_FINGER)));
    }

    /// What the room starts typing, it starts before the service stops waiting for it.
    #[test]
    fn what_was_given_up_on_is_not_typed() {
        assert!(in_time(Duration::ZERO));
        assert!(in_time(Duration::from_secs(7)));
        assert!(!in_time(TYPE_WITHIN));
        assert!(!in_time(ROOM_WAIT));
        assert!(TYPE_WITHIN < ROOM_WAIT);
    }
}
