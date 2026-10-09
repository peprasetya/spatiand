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
    Open { id: u32, bundle: String, title: String },
    /// The window has a picture of this size: show it, or show it at its new size.
    Show { id: u32, size: (u32, u32) },
    Title { id: u32, title: String },
    Close { id: u32 },
}

struct Windows {
    commands: Sender<Command>,
    /// Each window's output and the size of the last picture put in it.
    outputs: Mutex<HashMap<u32, (Arc<Output>, (u32, u32))>>,
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
    let _ = WINDOWS.set(Windows { commands, outputs: Mutex::default() });
    Ok(())
}

fn send(command: Command) {
    if let Some(windows) = WINDOWS.get() {
        let _ = windows.commands.send(command);
    }
}

/// A window of this Mac's is to be in the room.
pub fn open(id: u32, bundle: &str, title: &str) {
    let Some(windows) = WINDOWS.get() else { return };
    windows.outputs.lock().unwrap().insert(id, (Output::new(), (0, 0)));
    send(Command::Open { id, bundle: bundle.to_string(), title: title.to_string() });
}

/// A window's newest picture.
///
/// # Safety
/// `surface` must be a live `IOSurfaceRef`.
pub unsafe fn picture(id: u32, surface: *const std::ffi::c_void) {
    let Some(windows) = WINDOWS.get() else { return };
    let mut outputs = windows.outputs.lock().unwrap();
    let Some((output, last)) = outputs.get_mut(&id) else { return };
    let surface = Surface::hold(surface);
    let size = surface.size();
    output.put(surface, [0.0, 0.0, 1.0, 1.0]);
    if size != *last {
        *last = size;
        send(Command::Show { id, size });
    }
}

pub fn retitle(id: u32, title: &str) {
    send(Command::Title { id, title: title.to_string() });
}

/// The window has gone from the Mac, or is to leave the room.
pub fn close(id: u32) {
    if let Some(windows) = WINDOWS.get() {
        windows.outputs.lock().unwrap().remove(&id);
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
                Command::Open { id, bundle, title } => client.open(id, &app_id(&bundle), &title),
                Command::Show { id, size } => {
                    let output = WINDOWS
                        .get()
                        .and_then(|w| w.outputs.lock().unwrap().get(&id).map(|(o, _)| o.clone()));
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
