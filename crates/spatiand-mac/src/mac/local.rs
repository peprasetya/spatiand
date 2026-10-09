//! This Mac's own applications: listed in the launcher, and their windows in the room.
//!
//! A Mac application is not a Wayland client and never will be. Its windows are captured by the
//! app (`ScreenCaptureKit`) and arrive here as `IOSurface`s; what the wearer does to them goes
//! back to the app, which does it to the real window. In between, **each Mac window is an
//! ordinary window of the compositor** -- made through the same in-process Wayland client a
//! host's windows are (`crate::remote::client`), with the same placeholder for a picture. So a
//! Mac window is framed, moved, resized, pushed away, pinned, hidden and pointed at by the
//! Deck's code, like every other window, and nothing in the scene knows it is a Mac's.
//!
//! This is the remote session's job with the network taken out: the same calls on the client,
//! fed by the app instead of a host.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use smithay::reexports::wayland_server::DisplayHandle;
use spatiand_shell::AppEntry;
use spatiand_stream::Input;
use spatiand_video::mac::{Output, Surface};
use spatiand_video::Converted;

use super::ffi::{self, Asked};
use crate::remote::client::{self, Client, Told};
use crate::state::ClientState;

/// What this Mac is called among the hosts: its windows' application ids are
/// `remote.mac.<bundle id>`, so that everything keyed on an application id -- a controller
/// layout, a title bar's icon, where its sound is placed -- works for them as for a host's.
pub const HOST: &str = "mac";

pub fn app_id(bundle: &str) -> String {
    crate::remote::app_id(HOST, bundle)
}

// --- the launcher's list ---

fn listed() -> &'static Mutex<Vec<AppEntry>> {
    static APPS: OnceLock<Mutex<Vec<AppEntry>>> = OnceLock::new();
    APPS.get_or_init(Default::default)
}

/// The app's list of what is installed, replaced whole.
pub fn set_apps(apps: Vec<AppEntry>) {
    *listed().lock().unwrap() = apps;
}

pub fn apps() -> Vec<AppEntry> {
    listed().lock().unwrap().clone()
}

/// Open an application, and bring its windows into the room: the app's to do.
pub fn launch(app: &AppEntry) {
    log::info!("launching {}", app.name);
    ffi::tell(Asked::Launch, &app.exec);
}

// --- windows ---

enum Command {
    Open { id: u32, bundle: String, title: String, size: (u32, u32) },
    /// The window has a picture of this size: show it, or show it at its new size.
    Show { id: u32, size: (u32, u32) },
    Title { id: u32, title: String },
    Close { id: u32 },
}

/// One of this Mac's windows, as the compositor's side knows it.
struct Entry {
    output: Arc<Output>,
    /// The size of the last picture put in it, or the size it was announced at.
    size: (u32, u32),
    /// Its surface's number on the client's connection, which is the same number the compositor
    /// knows that surface by: how a window in the room is known to be this one.
    surface: Option<u32>,
    /// It is to be put away as soon as it arrives: listed, and not on show.
    start_hidden: bool,
    /// The app has asked for it to be on show.
    show: bool,
}

struct Windows {
    commands: Sender<Command>,
    entries: Mutex<HashMap<u32, Entry>>,
    /// Which were on show last frame, to say when that changes.
    visible: Mutex<std::collections::HashSet<u32>>,
}

static WINDOWS: OnceLock<Windows> = OnceLock::new();

/// Connect the client the Mac's windows are made through. Once, from the compositor's thread,
/// before its loop starts.
pub fn attach(display: &mut DisplayHandle) -> Result<(), String> {
    if WINDOWS.get().is_some() {
        return Ok(());
    }
    let (server, stream) =
        std::os::unix::net::UnixStream::pair().map_err(|e| format!("could not make a socket pair: {e}"))?;
    display
        .insert_client(server, Arc::new(ClientState::default()))
        .map_err(|e| format!("could not attach the Mac's windows: {e}"))?;
    let (commands, inbox) = channel();
    std::thread::Builder::new()
        .name("mac-windows".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(stream, inbox)));
            if result.is_err() {
                log::error!("the Mac's windows died; see the panic above");
            }
        })
        .map_err(|e| format!("could not start the Mac's windows: {e}"))?;
    let _ = WINDOWS.set(Windows { commands, entries: Mutex::default(), visible: Mutex::default() });
    Ok(())
}

fn send(command: Command) {
    if let Some(windows) = WINDOWS.get() {
        let _ = windows.commands.send(command);
    }
}

/// A window of this Mac's exists, for the room to list -- and to show at once, unless `hidden`,
/// in which case it waits in the list of windows until it is asked for. `size` is its picture's,
/// in pixels, so that it is a window of the right shape before it has a picture at all.
pub fn open(id: u32, bundle: &str, title: &str, size: (u32, u32), hidden: bool) {
    let Some(windows) = WINDOWS.get() else { return };
    windows.entries.lock().unwrap().insert(
        id,
        Entry { output: Output::new(), size, surface: None, start_hidden: hidden, show: false },
    );
    send(Command::Open { id, bundle: bundle.to_string(), title: title.to_string(), size });
}

/// Bring a window that is listed out onto show, in front of the wearer.
pub fn show(id: u32) {
    if let Some(entry) = WINDOWS.get().and_then(|w| w.entries.lock().ok()?.get_mut(&id).map(|e| e.show = true)) {
        let _ = entry;
    }
}

/// A window's newest picture.
///
/// # Safety
/// `surface` must be a live `IOSurfaceRef`.
pub unsafe fn picture(id: u32, surface: *const std::ffi::c_void) {
    let Some(windows) = WINDOWS.get() else { return };
    let mut entries = windows.entries.lock().unwrap();
    let Some(entry) = entries.get_mut(&id) else { return };
    let surface = Surface::hold(surface);
    let size = surface.size();
    entry.output.put(surface, [0.0, 0.0, 1.0, 1.0]);
    if size != entry.size {
        entry.size = size;
        send(Command::Show { id, size });
    }
}

pub fn retitle(id: u32, title: &str) {
    send(Command::Title { id, title: title.to_string() });
}

/// The window has gone from the Mac, or is to leave the room.
pub fn close(id: u32) {
    if let Some(windows) = WINDOWS.get() {
        windows.entries.lock().unwrap().remove(&id);
    }
    send(Command::Close { id });
}

fn run(stream: std::os::unix::net::UnixStream, inbox: Receiver<Command>) {
    let (mut client, mut queue) = match Client::new(stream) {
        Ok(pair) => pair,
        Err(e) => return log::error!("the Mac's windows: {e}"),
    };
    loop {
        // A few milliseconds at most between looks at the compositor: this is how long a
        // click waits to be passed on.
        let first = match inbox.recv_timeout(Duration::from_millis(3)) {
            Ok(command) => Some(command),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        for command in first.into_iter().chain(inbox.try_iter()) {
            match command {
                Command::Open { id, bundle, title, size } => {
                    client.open(id, &app_id(&bundle), &title);
                    let surface = client.windows.get(&id).map(|w| wayland_client::Proxy::id(&w.surface).protocol_id());
                    let output = WINDOWS.get().and_then(|w| {
                        let mut entries = w.entries.lock().unwrap();
                        let entry = entries.get_mut(&id)?;
                        entry.surface = surface;
                        Some(entry.output.clone())
                    });
                    // Shown at the size it was announced with, so that it is a window -- in the
                    // list, with a place in the room -- before any picture of it exists.
                    if let (Some(output), true) = (output, size.0 > 0 && size.1 > 0) {
                        if let Err(e) = client.show(id, Converted::of(output, size)) {
                            log::warn!("Mac window {id}: {e}");
                        }
                    }
                }
                Command::Show { id, size } => {
                    let output = WINDOWS
                        .get()
                        .and_then(|w| w.entries.lock().unwrap().get(&id).map(|e| e.output.clone()));
                    if let Some(output) = output {
                        if let Err(e) = client.show(id, Converted::of(output, size)) {
                            log::warn!("Mac window {id}: {e}");
                        }
                    }
                }
                Command::Title { id, title } => client.retitle(id, &title),
                Command::Close { id } => client.close(id),
            }
        }
        if let Err(e) = client::pump(&mut client, &mut queue) {
            return log::error!("the Mac's windows: {e}");
        }
        for (id, width, height) in std::mem::take(&mut client.resized) {
            ffi::tell_window(Asked::WindowResize, id, width as f64, height as f64, 0.0);
        }
        for id in std::mem::take(&mut client.closing) {
            ffi::tell_window(Asked::WindowClose, id, 0.0, 0.0, 0.0);
        }
        for told in std::mem::take(&mut client.input) {
            match told {
                Told::Focus(window) => ffi::tell_window(Asked::WindowFocus, window.unwrap_or(0), 0.0, 0.0, 0.0),
                Told::Input { window, input, .. } => match input {
                    Input::Motion { x, y } => ffi::tell_window(Asked::WindowMotion, window, x, y, 0.0),
                    Input::Leave => ffi::tell_window(Asked::WindowLeave, window, 0.0, 0.0, 0.0),
                    Input::Button { button, pressed } => {
                        ffi::tell_window(Asked::WindowButton, window, button as f64, pressed as u8 as f64, 0.0)
                    }
                    Input::Scroll { horizontal, vertical } => {
                        ffi::tell_window(Asked::WindowScroll, window, horizontal, vertical, 0.0)
                    }
                    Input::Key { code, pressed } => {
                        // As the Mac's own key code, which is what the app posts.
                        let mac = super::keycodes::mac_of(code).map(|m| m as f64).unwrap_or(-1.0);
                        ffi::tell_window(Asked::WindowKey, window, mac, pressed as u8 as f64, code as f64)
                    }
                },
            }
        }
    }
}

// --- the compositor's side: which of its windows are these, and which are on show ---

/// What the frame loop is to do with one of this Mac's windows this frame.
#[derive(Default, Clone, Copy)]
pub struct Wanted {
    /// Put it away: it has just arrived, and only to be listed.
    pub hide: bool,
    /// Bring it out, in front of the wearer.
    pub show: bool,
}

/// Which of this Mac's windows a surface is, by its number on this client's connection, and
/// what is wanted of it. Each wish is handed over once.
pub fn wanted(surface: u32) -> Option<(u32, Wanted)> {
    let windows = WINDOWS.get()?;
    let mut entries = windows.entries.lock().ok()?;
    let (id, entry) = entries.iter_mut().find(|(_, e)| e.surface == Some(surface))?;
    let wanted = Wanted {
        hide: std::mem::take(&mut entry.start_hidden),
        show: std::mem::take(&mut entry.show),
    };
    Some((*id, wanted))
}

/// Which of this Mac's windows are on show this frame. The app is told of each that has come
/// out or been put away since the last, so that only what can be seen is captured.
pub fn on_show(now: std::collections::HashSet<u32>) {
    let Some(windows) = WINDOWS.get() else { return };
    let Ok(mut before) = windows.visible.lock() else { return };
    if *before == now {
        return;
    }
    for id in now.difference(&before) {
        ffi::tell_window(Asked::WindowShown, *id, 0.0, 0.0, 0.0);
    }
    for id in before.difference(&now) {
        ffi::tell_window(Asked::WindowHidden, *id, 0.0, 0.0, 0.0);
    }
    *before = now;
}
