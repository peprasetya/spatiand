//! An application's own menus, as a list in the room -- for every window whose menus are not in
//! the window, on every platform.
//!
//! A Mac application's menus are on the Mac's menu bar, out of sight in the glasses. Its window
//! gets a menu button on its title bar, and what the button opens is those menus read out a
//! level at a time and shown as one of the room's own lists (`Mode::AppMenu`); what is chosen
//! there is pressed in the real menu. Two kinds of window have them:
//!
//! * **a Mac window streamed from a host** -- to a Deck, a Beam Pro, or another Mac: the host
//!   offers the window's menus (`MenuSaid::Offered`), and the button, the levels and the choice
//!   travel over the host's control stream (`ClientMessage::Menu`, `HostMessage::Menu`);
//! * **this Mac's own windows in the Mac's room**, whose menus the app reads itself
//!   (`crate::mac`).
//!
//! The room does the same with both: the backends ask [`window_has_menu`], call [`open`] when
//! the button is pressed, [`chosen`] and [`back`] from the list, and show what [`said`] hands
//! them. Which of the two answers is this module's business alone.

use std::collections::HashMap;
use std::sync::Mutex;

use smithay::reexports::wayland_server::Resource;
use smithay::wayland::seat::WaylandFocus;
use spatiand_stream::{MenuAsk, MenuSaid};

use super::Command;

/// What was last said of the menu that is open, for the room to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Said {
    /// A level of it: what it is called, and each row's words and what is said under them.
    Level { title: String, rows: Vec<(String, String)> },
    Done,
}

/// Whose menu is open, so a choice goes back to it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Owner {
    /// A host's, by the name it is known by.
    Host(String),
    /// One of this Mac's own applications in the Mac's room.
    #[allow(dead_code)]
    Local,
}

#[derive(Default)]
struct Menus {
    /// Windows whose host has offered their menus: by the window's application id in the room
    /// and its surface's number on its host's connection (the same number on both ends of it),
    /// the host and the window's number there.
    offered: HashMap<(String, u32), (String, u32)>,
    /// Each host's way to its thread.
    hosts: HashMap<String, tokio::sync::mpsc::UnboundedSender<Command>>,
    open_on: Option<Owner>,
    said: Option<Said>,
}

fn menus() -> std::sync::MutexGuard<'static, Menus> {
    static MENUS: std::sync::LazyLock<Mutex<Menus>> = std::sync::LazyLock::new(Default::default);
    // A panic elsewhere while it was held leaves the list as it was, which is still the list.
    MENUS.lock().unwrap_or_else(|e| e.into_inner())
}

// --- the hosts' side, from their threads ---

/// A host's thread is up: its menus can be asked for through `commands`.
pub(crate) fn host_up(host: &str, commands: tokio::sync::mpsc::UnboundedSender<Command>) {
    menus().hosts.insert(host.to_string(), commands);
}

/// A host's thread is going: nothing of its is offered any more, and a menu of its that is open
/// is closed. Only if `commands` is still the one known for it -- a host started again has
/// already said it is up.
pub(crate) fn host_down(host: &str, commands: &tokio::sync::mpsc::UnboundedSender<Command>) {
    let mut m = menus();
    if m.hosts.get(host).is_some_and(|known| known.same_channel(commands)) {
        m.hosts.remove(host);
    }
    drop(m);
    forget_host(host);
}

/// The window numbered `window` on `host`, whose surface is `surface` on its connection and whose
/// application id in the room is `app_id`, has menus.
pub(crate) fn offered(host: &str, app_id: &str, surface: u32, window: u32) {
    menus().offered.insert((app_id.to_string(), surface), (host.to_string(), window));
}

/// That window has closed.
pub(crate) fn window_gone(host: &str, window: u32) {
    menus().offered.retain(|_, (h, w)| !(h == host && *w == window));
}

/// Everything a host offered goes -- its link is down, or starting afresh -- and a menu of its
/// that was open is done with.
pub(crate) fn forget_host(host: &str) {
    let mut m = menus();
    m.offered.retain(|_, (h, _)| h != host);
    if m.open_on == Some(Owner::Host(host.to_string())) {
        m.said = Some(Said::Done);
    }
}

/// What a host said of the menu that is open.
pub(crate) fn host_said(host: &str, said: MenuSaid) {
    let said = match said {
        MenuSaid::Offered { .. } => return,
        MenuSaid::Level { title, rows } => {
            Said::Level { title, rows: rows.into_iter().map(|r| (r.label, r.detail)).collect() }
        }
        MenuSaid::Done => Said::Done,
    };
    let mut m = menus();
    // Only the host whose menu is open speaks for it: a late word from one closed since is not
    // put in front of the wearer.
    if m.open_on == Some(Owner::Host(host.to_string())) {
        m.said = Some(said);
    }
}

/// What this Mac's own app said of the menu that is open (`crate::mac`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn local_said(said: Said) {
    let mut m = menus();
    if m.open_on == Some(Owner::Local) {
        m.said = Some(said);
    }
}

// --- the room's side ---

/// Which host's window this is and its number there, if its host offered its menus.
fn hosted(app_id: &str, surface: u32) -> Option<(String, u32)> {
    menus().offered.get(&(app_id.to_string(), surface)).cloned()
}

/// Whether a window has menus for its title bar's menu button: a host offered them, or it is one
/// of this Mac's own.
pub fn window_has_menu(state: &crate::Spatiand, window: &smithay::desktop::Window) -> bool {
    #[cfg(target_os = "macos")]
    if crate::mac::has_app_menu(state, window) {
        return true;
    }
    let (Some(app_id), Some(surface)) = (state.app_id_of(window), window.wl_surface()) else { return false };
    hosted(&app_id, surface.id().protocol_id()).is_some()
}

/// The menu button on a window's bar was pressed: its application's menus are asked for, and
/// come back through [`said`]. Whether anyone was asked.
pub fn open(state: &crate::Spatiand, window: &smithay::desktop::Window) -> bool {
    #[cfg(target_os = "macos")]
    if crate::mac::has_app_menu(state, window) {
        let mut m = menus();
        m.open_on = Some(Owner::Local);
        m.said = None;
        drop(m);
        crate::mac::app_menu_wanted(window);
        return true;
    }
    let (Some(app_id), Some(surface)) = (state.app_id_of(window), window.wl_surface()) else { return false };
    let Some((host, number)) = hosted(&app_id, surface.id().protocol_id()) else { return false };
    let mut m = menus();
    let Some(commands) = m.hosts.get(&host).cloned() else { return false };
    m.open_on = Some(Owner::Host(host.clone()));
    m.said = None;
    drop(m);
    log::info!("menus: asking {host} for the menus of its window {number}");
    commands.send(Command::Menu(MenuAsk::Open { window: spatiand_stream::WindowId(number) })).is_ok()
}

/// Row `row` of the level on show was chosen.
pub fn chosen(row: usize) {
    tell(MenuAsk::Choose { row: row as u32 });
}

/// Up a level, or out of the menu from the top.
pub fn back() {
    tell(MenuAsk::Back);
}

fn tell(ask: MenuAsk) {
    let m = menus();
    match m.open_on.clone() {
        Some(Owner::Host(host)) => {
            let sent = m.hosts.get(&host).is_some_and(|c| c.send(Command::Menu(ask)).is_ok());
            drop(m);
            if !sent {
                // Its host has gone: the list is closed rather than left to answer nothing.
                local_done();
            }
        }
        #[cfg(target_os = "macos")]
        Some(Owner::Local) => {
            drop(m);
            match ask {
                MenuAsk::Choose { row } => crate::mac::app_menu_chosen(row as usize),
                MenuAsk::Back => crate::mac::app_menu_back(),
                MenuAsk::Open { .. } => {}
            }
        }
        _ => {
            drop(m);
            local_done();
        }
    }
}

fn local_done() {
    menus().said = Some(Said::Done);
}

/// What was said of the open menu since the last look: a level to show, or that it is done.
pub fn said() -> Option<Said> {
    let mut m = menus();
    let said = m.said.take();
    if said == Some(Said::Done) {
        m.open_on = None;
    }
    said
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test, for the state is the session's one list.
    #[test]
    fn a_hosts_menu_is_asked_for_shown_and_closed() {
        let (commands, mut inbox) = tokio::sync::mpsc::unbounded_channel();
        host_up("testhost", commands.clone());
        offered("testhost", "remote.testhost.com.apple.TextEdit", 17, 4);
        assert_eq!(hosted("remote.testhost.com.apple.TextEdit", 17), Some(("testhost".into(), 4)));
        assert_eq!(hosted("remote.testhost.com.apple.TextEdit", 18), None, "another surface has none");

        // As `open` does, once it has found the window.
        {
            let mut m = menus();
            m.open_on = Some(Owner::Host("testhost".into()));
        }
        chosen(2);
        assert!(matches!(inbox.try_recv(), Ok(Command::Menu(MenuAsk::Choose { row: 2 }))));
        back();
        assert!(matches!(inbox.try_recv(), Ok(Command::Menu(MenuAsk::Back))));

        // A level from another host is not shown; this one's is.
        host_said("otherhost", MenuSaid::Done);
        assert_eq!(said(), None);
        host_said(
            "testhost",
            MenuSaid::Level {
                title: "TextEdit".into(),
                rows: vec![spatiand_stream::MenuRow { label: "File".into(), detail: "Opens more".into() }],
            },
        );
        assert_eq!(said(), Some(Said::Level { title: "TextEdit".into(), rows: vec![("File".into(), "Opens more".into())] }));
        assert_eq!(said(), None, "said once");

        // The host's link goes: the open menu closes, and nothing of its is offered.
        host_down("testhost", &commands);
        assert_eq!(said(), Some(Said::Done));
        assert_eq!(hosted("remote.testhost.com.apple.TextEdit", 17), None);
        // A choice with nobody to take it closes the list rather than hanging.
        chosen(0);
        assert_eq!(said(), Some(Said::Done));

        // A host started again before the old one's thread went keeps its new way in.
        let (newer, _new_inbox) = tokio::sync::mpsc::unbounded_channel();
        host_up("testhost", newer.clone());
        host_down("testhost", &commands);
        assert!(menus().hosts.get("testhost").is_some_and(|c| c.same_channel(&newer)));
        offered("testhost", "remote.testhost.x", 3, 9);
        window_gone("testhost", 9);
        assert_eq!(hosted("remote.testhost.x", 3), None);
    }
}
