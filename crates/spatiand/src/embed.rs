//! Another application's window inside a window of the room's: `spatiand_xr_embed_v1`.
//!
//! The owner, 2026-11-27: "the build true webview, spatiloupe use webview for chromium best for.
//! this can be useful in future". A viewer shows a PDF with SpatiMesh's own viewer, inside its
//! own window; any application of the room's may keep such a place. Wayland has no way for one
//! client's window to be shown inside another's, so the room does it: the host asks its window
//! for a place (`get_embed`) and is given a token; the window that is to go there says the token
//! (`set_embedded`) before its first picture; from then it is drawn in the host's window at the
//! place's rectangle, pointed at and typed into there, and goes wherever the host goes.
//!
//! The window inside stays a window of the `Space` -- frame callbacks, its menus, the seat's
//! focus all need one -- but it has no place in the room of its own (`WindowLayout`), which is
//! what keeps it out of every list of windows, the switcher and the room's drawing of windows.
//! `scene::collect_windows` draws it as part of its host, in the host's pixels, the way a menu
//! is drawn; a press in it gives it the keyboard ([`pressed`]).

use std::time::{Duration, Instant};

use smithay::desktop::Window;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::seat::WaylandFocus;
use spatiand_proto::server::spatiand_xr_embed_v1::SpatiandXrEmbedV1;

use crate::state::Spatiand;

/// A place's rectangle, in the host surface's own coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn shows(&self) -> bool {
        self.w > 0 && self.h > 0
    }
}

/// One place in a host's window.
pub struct Slot {
    pub token: String,
    pub object: SpatiandXrEmbedV1,
    /// The host window's toplevel surface: the surface whose xr object made the place.
    pub host: WlSurface,
    /// The rectangle in effect: the one the host last committed.
    pub rect: Option<Rect>,
    /// The rectangle asked for, which takes effect at the host's next commit.
    pending_rect: Option<Rect>,
    /// The window inside, by its toplevel surface.
    pub guest: Option<WlSurface>,
}

/// Every place, and which window inside one has the keyboard.
#[derive(Default)]
pub struct Embeds {
    pub slots: Vec<Slot>,
    /// The window inside a place that the wearer last pressed in: the keyboard's while its host
    /// has the keyboard. Remembered when the wearer goes to another window, so that coming back
    /// to the host comes back to where they were typing, as in a browser's page.
    pub focus: Option<WlSurface>,
    /// Places taken away before their window came, by token and when: a window that still comes
    /// saying one is asked to close as it arrives, never shown -- it was meant for that place alone.
    ended: Vec<(String, Instant)>,
    /// Places SpatiMesh was asked into (`own::open_embedded`) and has not come into yet, by token
    /// and when: a SpatiMesh too old for places opens the page in a window of its own instead,
    /// and the place is told so as that window arrives ([`committed`]).
    awaited: Vec<(String, Instant)>,
}

/// How long a place taken away is remembered for a window still on its way.
const ENDED_KEPT: Duration = Duration::from_secs(600);
/// How long SpatiMesh is waited for, for a place it was asked into, before its next new window
/// is no longer taken for that page's.
const AWAITED_KEPT: Duration = Duration::from_secs(60);

/// What SpatiMesh too old for places opening the page in a window of its own is told to the
/// place as (`spatiand_xr_embed_v1.refused`).
const TOO_OLD: &str = "This SpatiMesh cannot show a page inside another window yet: it opened it in a window of its own.";

/// SpatiMesh was asked into the place with this token.
pub fn awaiting(state: &mut Spatiand, token: &str) {
    state.embeds.awaited.retain(|(t, since)| t != token && since.elapsed() < AWAITED_KEPT);
    state.embeds.awaited.push((token.to_string(), Instant::now()));
}

/// A name nobody can guess, for a place.
pub fn new_token() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
    if read.is_err() {
        // Every system the room runs on has /dev/urandom; this is only so that a place is never
        // refused for want of one. Unique, if not secret.
        use std::hash::{BuildHasher, Hasher};
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        hasher.write_u32(std::process::id());
        let a = hasher.finish();
        hasher.write_u64(a);
        bytes[..8].copy_from_slice(&a.to_le_bytes());
        bytes[8..].copy_from_slice(&hasher.finish().to_le_bytes());
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn window_of(state: &Spatiand, surface: &WlSurface) -> Option<Window> {
    state.space.elements().find(|w| w.wl_surface().is_some_and(|s| *s == *surface)).cloned()
}

/// A host asked its window for a place: it is told the place's token at once.
pub fn made(state: &mut Spatiand, host: WlSurface, object: SpatiandXrEmbedV1) {
    let token = new_token();
    object.token(token.clone());
    log::info!("a window keeps a place for another's ({} now)", state.embeds.slots.len() + 1);
    state.embeds.slots.push(Slot { token, object, host, rect: None, pending_rect: None, guest: None });
}

/// The rectangle a host asked for, which takes effect at its next commit.
pub fn set_rect(state: &mut Spatiand, object: &SpatiandXrEmbedV1, rect: Rect) {
    if let Some(slot) = state.embeds.slots.iter_mut().find(|s| s.object == *object) {
        slot.pending_rect = Some(rect);
    }
}

/// A surface committed: a host's places take the rectangles it asked for, and a window that
/// said a place's token goes inside it (or comes out, given none).
pub fn committed(state: &mut Spatiand, surface: &WlSurface, embedded: Option<String>) {
    let mut resized = Vec::new();
    for slot in state.embeds.slots.iter_mut().filter(|s| s.host == *surface) {
        if let Some(rect) = slot.pending_rect.take() {
            if slot.rect != Some(rect) {
                slot.rect = Some(rect);
                if let Some(guest) = slot.guest.clone() {
                    resized.push((guest, rect));
                }
            }
        }
    }
    for (guest, rect) in resized {
        size_guest(state, &guest, rect);
    }
    match embedded.as_deref() {
        Some("") => come_out(state, surface),
        Some(token) => go_in(state, surface, token),
        None => came_without_place(state, surface),
    }
}

/// A window's first commit, naming no place. SpatiMesh bound to `spatiand_xr_v1` before 13,
/// asked into a place, opens the page in a window of its own -- this one -- and the place that has
/// waited longest for SpatiMesh is told so (`refused`) as it arrives. A SpatiMesh new enough for
/// places names one in the window it opens for a place, so a window of its naming none is one the
/// wearer opened, and nothing is told.
fn came_without_place(state: &mut Spatiand, surface: &WlSurface) {
    use smithay::reexports::wayland_server::Resource;
    state.embeds.awaited.retain(|(_, since)| since.elapsed() < AWAITED_KEPT);
    if state.embeds.awaited.is_empty() {
        return;
    }
    let Some(window) = window_of(state, surface) else { return };
    // Its first commit only: the initial configure is sent after this one (state.rs).
    if window.toplevel().is_none_or(|t| t.is_initial_configure_sent()) {
        return;
    }
    let spatimesh = state.app_id_of(&window).is_some_and(|a| a == crate::audio::CHROMIUM_APP_ID);
    // The version its window's xr object has, or else the one its client bound spatiand_xr_v1
    // at (a window says nothing through it until it has something to say); none at all, 0.
    let version = state
        .xr_surfaces
        .iter()
        .find(|(s, _)| s == surface)
        .map(|(_, o)| o.version())
        .or_else(|| {
            let client = surface.client()?;
            state.xr_globals.iter().filter(|g| g.client().is_some_and(|c| c.id() == client.id())).map(|g| g.version()).max()
        })
        .unwrap_or(0);
    if !spatimesh || version >= 13 {
        return;
    }
    let (token, _) = state.embeds.awaited.remove(0);
    if let Some(slot) = state.embeds.slots.iter().find(|s| s.token == token && s.guest.is_none()) {
        if slot.object.version() >= 13 {
            slot.object.refused(TOO_OLD.into());
        }
        log::warn!("SpatiMesh bound spatiand_xr_v1 {version}, too old for places: it opened the page in a window of its own, and the place is told so");
    }
}

/// The window inside is offered its place's size: whenever the place changes, and before its
/// first picture, when that is what is being committed (the initial configure follows this).
fn size_guest(state: &mut Spatiand, guest: &WlSurface, rect: Rect) {
    if !rect.shows() {
        return;
    }
    let Some(toplevel) = window_of(state, guest).and_then(|w| w.toplevel().cloned()) else {
        return;
    };
    toplevel.with_pending_state(|s| {
        s.size = Some((rect.w, rect.h).into());
    });
    if toplevel.is_initial_configure_sent() {
        toplevel.send_configure();
    }
}

fn go_in(state: &mut Spatiand, guest: &WlSurface, token: &str) {
    let Some(at) = state.embeds.slots.iter().position(|s| s.token == token) else {
        state.embeds.ended.retain(|(_, since)| since.elapsed() < ENDED_KEPT);
        if let Some(gone) = state.embeds.ended.iter().position(|(t, _)| t == token) {
            // Meant for a place taken away before it came: asked to close, and never shown.
            state.embeds.ended.remove(gone);
            if let Some(window) = window_of(state, guest) {
                state.layout.unplace(&window);
                state.close_window(&window);
            }
            log::info!("a window came for a place taken away before it did; it is asked to close");
            return;
        }
        log::warn!("a window named a place that is not there; it stays a window of its own");
        return;
    };
    state.embeds.awaited.retain(|(t, _)| t != token);
    if let Some(other) = state.embeds.slots[at].guest.as_ref() {
        if other == guest {
            return;
        }
        if other.is_alive() {
            log::warn!("a window named a place that has a window inside already; it stays a window of its own");
            return;
        }
    }
    // Out of any other place first: one window is inside one place.
    leave(state, guest);
    let slot = &mut state.embeds.slots[at];
    slot.guest = Some(guest.clone());
    slot.object.attached();
    let host = slot.host.clone();
    // The rectangle in effect, or else the one the host has asked for and not yet committed with
    // a picture: a host that draws slowly (its GPU's first picture) is still to have its window
    // inside offered the place's size first, not the room's.
    let rect = slot.rect.or(slot.pending_rect);
    // Not a window of the room: it was given a place and the keyboard as it arrived, as every
    // new window is, and both go -- the keyboard back to the host, which stays in front.
    if let Some(window) = window_of(state, guest) {
        state.layout.unplace(&window);
    }
    if let Some(host) = window_of(state, &host) {
        state.focus_window(&host);
    }
    if let Some(rect) = rect {
        size_guest(state, guest, rect);
    }
    log::info!("a window is inside another's place now");
}

/// A window said it is no place's inside any more: a window of the room's again.
fn come_out(state: &mut Spatiand, guest: &WlSurface) {
    if !leave(state, guest) {
        return;
    }
    if let Some(window) = window_of(state, guest) {
        state.layout.place(&window, state.spawn_yaw);
        state.layout.focus(&window);
    }
}

/// Take a window out of whatever place it is inside, telling the host; whether it was in one.
fn leave(state: &mut Spatiand, guest: &WlSurface) -> bool {
    let mut left = false;
    for slot in state.embeds.slots.iter_mut().filter(|s| s.guest.as_ref() == Some(guest)) {
        slot.guest = None;
        slot.object.detached();
        left = true;
    }
    if state.embeds.focus.as_ref() == Some(guest) {
        state.embeds.focus = None;
    }
    left
}

/// A surface has gone -- its toplevel destroyed, or its xr object with it. As the inside of a
/// place, the place is told and stays; as a host, the windows inside its places are asked to
/// close and the places are forgotten.
pub fn gone(state: &mut Spatiand, surface: &WlSurface) {
    leave(state, surface);
    let mut closing = Vec::new();
    let mut ended = Vec::new();
    state.embeds.slots.retain(|slot| {
        if slot.host == *surface {
            match &slot.guest {
                Some(guest) => closing.push(guest.clone()),
                None => ended.push(slot.token.clone()),
            }
            false
        } else {
            true
        }
    });
    remember_ended(state, ended);
    for guest in closing {
        close(state, &guest);
    }
}

/// The host took the place away (its object destroyed, however it went).
pub fn destroyed(state: &mut Spatiand, object: &SpatiandXrEmbedV1) {
    let mut closing = Vec::new();
    let mut ended = Vec::new();
    state.embeds.slots.retain(|slot| {
        if slot.object == *object {
            match &slot.guest {
                Some(guest) => closing.push(guest.clone()),
                None => ended.push(slot.token.clone()),
            }
            false
        } else {
            true
        }
    });
    remember_ended(state, ended);
    for guest in closing {
        if state.embeds.focus.as_ref() == Some(&guest) {
            state.embeds.focus = None;
        }
        close(state, &guest);
    }
}

/// Places taken away with no window inside: a window still on its way to one is closed as it
/// comes ([`go_in`]).
fn remember_ended(state: &mut Spatiand, tokens: Vec<String>) {
    let now = Instant::now();
    state.embeds.ended.retain(|(_, since)| since.elapsed() < ENDED_KEPT);
    state.embeds.awaited.retain(|(t, _)| !tokens.contains(t));
    state.embeds.ended.extend(tokens.into_iter().map(|t| (t, now)));
}

fn close(state: &mut Spatiand, guest: &WlSurface) {
    if let Some(window) = window_of(state, guest) {
        state.close_window(&window);
    }
}

/// What a host said to the window inside its place.
pub fn send(state: &Spatiand, object: &SpatiandXrEmbedV1, message: String) {
    let Some(guest) = state.embeds.slots.iter().find(|s| s.object == *object).and_then(|s| s.guest.clone()) else {
        return;
    };
    crate::xr::tell_host_message(&guest, message);
}

/// What a window inside a place said to its host.
pub fn post_to_host(state: &Spatiand, guest: &WlSurface, message: String) {
    if let Some(slot) = state.embeds.slots.iter().find(|s| s.guest.as_ref() == Some(guest)) {
        slot.object.message(message);
    }
}

/// The host gave the keyboard to the window inside, as a press in it would.
pub fn focus(state: &mut Spatiand, object: &SpatiandXrEmbedV1) {
    let Some((host, guest)) = state.embeds.slots.iter().find(|s| s.object == *object).and_then(|s| Some((s.host.clone(), s.guest.clone()?))) else {
        return;
    };
    state.embeds.focus = Some(guest);
    if let Some(host) = window_of(state, &host) {
        if state.layout.is_focused(&host) {
            state.focus_window(&host);
        }
    }
}

/// The windows inside a host's places that are shown, with their rectangles.
pub fn guests_of(state: &Spatiand, host: &WlSurface) -> Vec<(Rect, WlSurface)> {
    state
        .embeds
        .slots
        .iter()
        .filter(|s| s.host == *host)
        .filter_map(|s| Some((s.rect.filter(Rect::shows)?, s.guest.clone().filter(|g| g.is_alive())?)))
        .collect()
}

/// A window's own account of itself, as the assistant and dictation ask for it
/// ([`crate::xr::ask_description`]), with the accounts of the windows shown in its places: a PDF
/// SpatiLoupe shows through SpatiMesh is SpatiMesh's to describe, its page's words included, which
/// the host's own account cannot give. Without places, the window's own answer as it comes.
pub fn ask_description(state: &Spatiand, surface: &WlSurface) -> Option<std::sync::mpsc::Receiver<String>> {
    let own = crate::xr::ask_description(surface);
    let insides: Vec<_> = guests_of(state, surface).into_iter().filter_map(|(_, guest)| crate::xr::ask_description(&guest)).collect();
    if insides.is_empty() {
        return own;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("describe-insides".into())
        .spawn(move || {
            // Each is read by a thread of its own, with a second's patience: these only wait.
            let patience = std::time::Duration::from_millis(1200);
            let host = own.and_then(|r| r.recv_timeout(patience).ok());
            let said: Vec<String> = insides.into_iter().filter_map(|r| r.recv_timeout(patience).ok()).collect();
            let _ = tx.send(with_insides(host.as_deref(), &said));
        })
        .ok()?;
    Some(rx)
}

/// A host's account with those of the windows inside its places: their words after its own, an
/// empty line apart, and their vocabulary after its own; the rest is the host's.
pub fn with_insides(host: Option<&str>, insides: &[String]) -> String {
    use serde_json::{json, Value};
    let mut said = host.and_then(|h| serde_json::from_str::<Value>(h).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    for inside in insides {
        let Some(inner) = serde_json::from_str::<Value>(inside).ok().filter(Value::is_object) else { continue };
        if let Some(text) = inner.get("text").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
            let joined = match said.get("text").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
                Some(own) => format!("{own}\n\n{text}"),
                None => text.to_string(),
            };
            said["text"] = json!(joined);
        }
        if let Some(words) = inner.get("vocabulary").and_then(Value::as_array) {
            let mut all = said.get("vocabulary").and_then(Value::as_array).cloned().unwrap_or_default();
            all.extend(words.iter().filter(|w| !all.contains(w)).cloned().collect::<Vec<_>>());
            said["vocabulary"] = Value::Array(all);
        }
    }
    said.to_string()
}

/// The host of the window inside a place.
pub fn host_of(state: &Spatiand, guest: &WlSurface) -> Option<WlSurface> {
    state.embeds.slots.iter().find(|s| s.guest.as_ref() == Some(guest)).map(|s| s.host.clone())
}

/// Whether this toplevel surface is the inside of a place, and so no window of the room's.
pub fn is_guest(state: &Spatiand, surface: &WlSurface) -> bool {
    state.embeds.slots.iter().any(|s| s.guest.as_ref() == Some(surface))
}

/// The same, of a window.
pub fn is_guest_window(state: &Spatiand, window: &Window) -> bool {
    window.wl_surface().is_some_and(|s| is_guest(state, &s))
}

/// The window a window's sound is heard from: its host's, for the inside of a place.
pub fn heard_from(state: &Spatiand, window: &Window) -> Window {
    window
        .wl_surface()
        .and_then(|s| host_of(state, &s))
        .and_then(|host| window_of(state, &host))
        .unwrap_or_else(|| window.clone())
}

/// The toplevel surface a surface belongs to: a subsurface's root, a menu's window.
fn root_of(state: &Spatiand, surface: &WlSurface) -> WlSurface {
    let mut root = surface.clone();
    while let Some(parent) = smithay::wayland::compositor::get_parent(&root) {
        root = parent;
    }
    if let Some(popup) = state.popups.find_popup(&root) {
        if let Ok(window) = smithay::desktop::find_popup_root_surface(&popup) {
            return window;
        }
    }
    root
}

/// A button went down on `surface`: in a window inside a place, that window gets the keyboard;
/// elsewhere in a host's window, the host gets it back. Run by every backend, from the pointer's
/// own `button`, after whatever focusing the backend did for the window pressed.
pub fn pressed(state: &mut Spatiand, surface: Option<&WlSurface>) {
    let Some(surface) = surface else {
        return;
    };
    if state.embeds.slots.is_empty() {
        return;
    }
    let root = root_of(state, surface);
    if let Some(host) = host_of(state, &root) {
        if state.embeds.focus.as_ref() != Some(&root) {
            state.embeds.focus = Some(root);
        }
        if let Some(host) = window_of(state, &host) {
            state.focus_window(&host);
        }
    } else if let Some(inside) = state.embeds.focus.clone() {
        if host_of(state, &inside).as_ref() == Some(&root) {
            state.embeds.focus = None;
            if let Some(host) = window_of(state, &root) {
                state.focus_window(&host);
            }
        }
    }
}

/// Where the keyboard goes for a host's window: inside its place, when the wearer last pressed
/// there.
pub fn keyboard_inside(state: &Spatiand, host: &WlSurface) -> Option<WlSurface> {
    state
        .embeds
        .focus
        .as_ref()
        .filter(|g| g.is_alive() && host_of(state, g).as_ref() == Some(host))
        .cloned()
}

/// Places whose host has gone without a word -- a crashed client's surfaces can go before its
/// objects -- are forgotten, and their windows asked to close. Run once a frame.
pub fn tidy(state: &mut Spatiand) {
    if state.embeds.slots.iter().any(|s| !s.host.is_alive()) {
        let dead: Vec<WlSurface> = state.embeds.slots.iter().filter(|s| !s.host.is_alive()).map(|s| s.host.clone()).collect();
        for host in dead {
            gone(state, &host);
        }
    }
    for slot in state.embeds.slots.iter_mut() {
        if slot.guest.as_ref().is_some_and(|g| !g.is_alive()) {
            slot.guest = None;
            slot.object.detached();
        }
    }
    if state.embeds.focus.as_ref().is_some_and(|g| !g.is_alive()) {
        state.embeds.focus = None;
    }
}

#[cfg(test)]
mod tests {
    use super::with_insides;
    use serde_json::{json, Value};

    #[test]
    fn a_hosts_account_has_its_insides_words_after_its_own() {
        let host = json!({
            "kind": "viewer", "app": "SpatiLoupe", "title": "report.pdf",
            "text": "The PDF report.pdf, shown by SpatiMesh's PDF viewer inside SpatiLoupe's window.",
            "vocabulary": ["SpatiLoupe"],
        })
        .to_string();
        let inside = json!({
            "kind": "web", "app": "SpatiMesh", "title": "report.pdf",
            "text": "Page 1\nScanned invoice 4471", "vocabulary": ["SpatiLoupe", "Spatiand"],
        })
        .to_string();
        let said: Value = serde_json::from_str(&with_insides(Some(&host), &[inside])).unwrap();
        assert_eq!(said["app"], "SpatiLoupe");
        assert_eq!(said["kind"], "viewer");
        assert_eq!(
            said["text"],
            "The PDF report.pdf, shown by SpatiMesh's PDF viewer inside SpatiLoupe's window.\n\nPage 1\nScanned invoice 4471"
        );
        assert_eq!(said["vocabulary"], json!(["SpatiLoupe", "Spatiand"]), "each word once");
    }

    #[test]
    fn a_host_that_said_nothing_is_told_by_its_insides() {
        let inside = json!({"text": "Page 2\nTerima kasih"}).to_string();
        let said: Value = serde_json::from_str(&with_insides(None, &[inside, "not json".into()])).unwrap();
        assert_eq!(said["text"], "Page 2\nTerima kasih");
        assert!(said.get("vocabulary").is_none());
        // Nothing inside said anything: the host's account as it was.
        let host = json!({"app": "SpatiLoupe", "text": "a picture"}).to_string();
        let said: Value = serde_json::from_str(&with_insides(Some(&host), &[json!({"text": "  "}).to_string()])).unwrap();
        assert_eq!(said["text"], "a picture");
    }
}
