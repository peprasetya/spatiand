//! The Mac without glasses: Spatiand's own applications as windows on the Mac's desktop.
//!
//! In 2D mode the app shows a host's windows itself (`RemoteWindow.swift`), and Spatiand's own
//! applications -- SpatiMesh, SpatiReel and the rest -- still need what they are written
//! against, which is this compositor. So the session runs with nothing to draw into (the
//! glasses' slot empty), and each of its windows is handed to the app to be shown as an
//! ordinary Mac window: the picture the application drew, which is an `IOSurface` already
//! (`spatiand_app::gl::mac`), named by number, and the window's title. The app sends back what
//! the mouse and keyboard did to that window, its new size when it is resized, and its close
//! button; they are given to the window here as the room's own seat would give them.
//!
//! Plugged in, the glasses take over the same windows: [`sp_desk`] off, and the app hands the
//! session the glasses' screen, as it would have at the start. Nothing is restarted.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_char, CStr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use smithay::backend::input::{Axis, AxisSource, ButtonState, KeyState};
use smithay::backend::renderer::utils::{with_renderer_surface_state, CommitCounter};
use smithay::input::keyboard::{FilterResult, Keycode};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::utils::{Point, SERIAL_COUNTER};
use smithay::wayland::seat::WaylandFocus;

use super::ffi::{self, Asked};

static ON: AtomicBool = AtomicBool::new(false);

/// Whether the session's windows are the Mac's to show.
pub fn on() -> bool {
    ON.load(Ordering::SeqCst)
}

/// What the app says happened to one of the windows it shows. Positions and sizes are in the
/// window's own pixels, from its top left: the Mac window is as many points as the picture is
/// pixels.
enum Input {
    Motion(u32, f64, f64),
    Leave(u32),
    Button(u32, u32, bool),
    /// Across and down, in the pixels the page moves; `true` for a wheel's notches.
    Scroll(u32, f64, f64, bool),
    Key(u32, u32, bool),
    Focus(u32, bool),
    Pinch(u32, f32),
    Resize(u32, i32, i32),
    Close(u32),
    Launch(String),
}

static INPUT: Mutex<Vec<Input>> = Mutex::new(Vec::new());

fn queue(input: Input) {
    INPUT.lock().unwrap().push(input);
}

/// What the app has been told of a window, so that only what changed is told again.
struct Told {
    window: smithay::desktop::Window,
    title: String,
    size: (i32, i32),
    commit: Option<CommitCounter>,
    picture: Option<u32>,
}

#[derive(Default)]
struct Desk {
    told: HashMap<u32, Told>,
    /// Was on at the last tick: going off lets the app's windows go, coming on tells them all.
    was_on: bool,
    started: Option<Instant>,
    last_frames: Option<Instant>,
    /// Keys held down, to be let go when their window loses the keyboard: a Mac window that
    /// loses it never hears the key come up.
    held: Vec<u32>,
    /// The window the keyboard is on, as the app last said.
    keyboard: Option<u32>,
}

thread_local! {
    // The compositor's thread's alone, as the windows are.
    static DESK: RefCell<Desk> = RefCell::new(Desk::default());
}

/// Every window this session has that the Mac could show: Wayland's, which Spatiand's own
/// applications' are. By the layout's id, which is the window's for its whole life.
fn windows(state: &crate::Spatiand) -> Vec<(u32, smithay::desktop::Window)> {
    state
        .space
        .elements()
        .filter(|w| w.toplevel().is_some())
        .filter_map(|w| Some((state.layout.id_of(w)? as u32, w.clone())))
        .collect()
}

/// The picture a window shows: the `IOSurface` its placeholder names, and the commit it came
/// with. `None` for a picture in shared memory, which no application of Spatiand's sends.
fn picture_of(window: &smithay::desktop::Window) -> Option<(u32, CommitCounter, (i32, i32))> {
    let surface = window.wl_surface()?;
    let (surface_id, _) = super::remote_video::shared_surface_of(&surface)?;
    let (commit, size) = with_renderer_surface_state(&surface, |st| (st.current_commit(), st.buffer_size()))?;
    let size = size.map(|s| (s.w, s.h)).unwrap_or((0, 0));
    Some((surface_id, commit, size))
}

/// The size a window is, as the pointer measures it: its picture's, or the shape it asked for
/// (a film's two eyes side by side are a picture twice the window's width).
fn size_of(window: &smithay::desktop::Window) -> (i32, i32) {
    let size = window.geometry().size;
    (size.w.max(1), size.h.max(1))
}

/// Once a pass of the session's loop while there are no glasses to draw into: what is new is
/// told to the app, and what the app said is done. Returns whether 2D mode is on, so the loop
/// goes round as often as a window's frames want.
pub fn tick(state: &mut crate::Spatiand) -> bool {
    let on = on();
    DESK.with(|desk| {
        let mut desk = desk.borrow_mut();
        if !on {
            if desk.was_on {
                // The glasses have the windows now: the Mac's go.
                for (id, _) in desk.told.drain() {
                    ffi::tell_window(Asked::DeskClose, id, 0.0, 0.0, 0.0);
                }
                desk.was_on = false;
            }
            INPUT.lock().unwrap().clear();
            return;
        }
        if !desk.was_on {
            desk.was_on = true;
            desk.started = Some(Instant::now());
            // One picture: the applications draw for a flat screen.
            state.set_display_stereo(false);
        }
        tell(state, &mut desk);
        apply(state, &mut desk);
        // A window's next picture, asked for at the display's pace.
        let now = Instant::now();
        if desk.last_frames.is_none_or(|t| now.duration_since(t) >= Duration::from_millis(15)) {
            desk.last_frames = Some(now);
            let screen = state.screen.clone();
            state.send_frames(&screen, now.duration_since(desk.started.unwrap_or(now)));
        }
    });
    on
}

fn tell(state: &mut crate::Spatiand, desk: &mut Desk) {
    let now = windows(state);
    let gone: Vec<u32> = desk.told.keys().filter(|id| !now.iter().any(|(n, _)| n == *id)).copied().collect();
    for id in gone {
        desk.told.remove(&id);
        if desk.keyboard == Some(id) {
            desk.keyboard = None;
            desk.held.clear();
        }
        ffi::tell_window(Asked::DeskClose, id, 0.0, 0.0, 0.0);
    }
    for (id, window) in now {
        let title = state.title_of(&window).unwrap_or_default();
        let size = size_of(&window);
        let picture = picture_of(&window);
        // Nothing until there is something to show: a window opened on nothing is a grey box.
        let Some((surface, commit, _)) = picture else { continue };
        match desk.told.get_mut(&id) {
            None => {
                let app = state.app_id_of(&window).unwrap_or_default();
                ffi::call_text(Asked::DeskOpen, id, size.0 as f64, size.1 as f64, 0.0, &format!("{title}\n{app}"));
                ffi::tell_window(Asked::DeskPicture, id, surface as f64, 0.0, 0.0);
                desk.told.insert(id, Told { window, title, size, commit: Some(commit), picture: Some(surface) });
            }
            Some(told) => {
                if told.title != title {
                    ffi::call_text(Asked::DeskTitle, id, 0.0, 0.0, 0.0, &title);
                    told.title = title;
                }
                if told.size != size {
                    ffi::tell_window(Asked::DeskSize, id, size.0 as f64, size.1 as f64, 0.0);
                    told.size = size;
                }
                if told.commit != Some(commit) || told.picture != Some(surface) {
                    ffi::tell_window(Asked::DeskPicture, id, surface as f64, 0.0, 0.0);
                    told.commit = Some(commit);
                    told.picture = Some(surface);
                }
            }
        }
    }
}

fn apply(state: &mut crate::Spatiand, desk: &mut Desk) {
    let inputs = std::mem::take(&mut *INPUT.lock().unwrap());
    let started = desk.started;
    let time = || desk_time(started);
    for input in inputs {
        match input {
            Input::Launch(name) => {
                match crate::own::list().into_iter().find(|a| a.name.eq_ignore_ascii_case(&name)) {
                    Some(app) => {
                        log::info!("2D: opening {}", app.name);
                        crate::own::launch(&app, &state.socket_name.clone(), &[]);
                    }
                    None => log::warn!("2D: no application of Spatiand's called {name}"),
                }
            }
            Input::Motion(id, x, y) => {
                let Some(surface) = surface(desk, id) else { continue };
                let Some(pointer) = state.seat.get_pointer() else { continue };
                pointer.motion(
                    state,
                    Some((surface, Point::from((0.0, 0.0)))),
                    &MotionEvent { location: Point::from((x, y)), serial: SERIAL_COUNTER.next_serial(), time: time() },
                );
                pointer.frame(state);
            }
            Input::Leave(id) => {
                if surface(desk, id).is_none() {
                    continue;
                }
                let Some(pointer) = state.seat.get_pointer() else { continue };
                pointer.motion(state, None, &MotionEvent { location: Point::from((0.0, 0.0)), serial: SERIAL_COUNTER.next_serial(), time: time() });
                pointer.frame(state);
            }
            Input::Button(id, button, pressed) => {
                let Some(window) = desk.told.get(&id).map(|t| t.window.clone()) else { continue };
                if pressed {
                    focus(state, desk, id, &window);
                }
                let Some(pointer) = state.seat.get_pointer() else { continue };
                let button_state = if pressed { ButtonState::Pressed } else { ButtonState::Released };
                pointer.button(state, &ButtonEvent { button, state: button_state, serial: SERIAL_COUNTER.next_serial(), time: time() });
                pointer.frame(state);
            }
            Input::Scroll(id, dx, dy, wheel) => {
                if surface(desk, id).is_none() {
                    continue;
                }
                let Some(pointer) = state.seat.get_pointer() else { continue };
                let mut frame = AxisFrame::new(time()).source(if wheel { AxisSource::Wheel } else { AxisSource::Finger });
                if dx != 0.0 {
                    frame = frame.value(Axis::Horizontal, dx);
                    if wheel {
                        frame = frame.v120(Axis::Horizontal, (dx / 15.0 * 120.0).round() as i32);
                    }
                }
                if dy != 0.0 {
                    frame = frame.value(Axis::Vertical, dy);
                    if wheel {
                        frame = frame.v120(Axis::Vertical, (dy / 15.0 * 120.0).round() as i32);
                    }
                }
                pointer.axis(state, frame);
                pointer.frame(state);
            }
            Input::Key(id, code, pressed) => {
                let Some(window) = desk.told.get(&id).map(|t| t.window.clone()) else { continue };
                if desk.keyboard != Some(id) {
                    focus(state, desk, id, &window);
                }
                if pressed {
                    if !desk.held.contains(&code) {
                        desk.held.push(code);
                    }
                } else {
                    desk.held.retain(|&c| c != code);
                }
                key(state, code, pressed, time());
            }
            Input::Focus(id, focused) => {
                let Some(window) = desk.told.get(&id).map(|t| t.window.clone()) else { continue };
                if focused {
                    focus(state, desk, id, &window);
                } else if desk.keyboard == Some(id) {
                    for code in std::mem::take(&mut desk.held) {
                        key(state, code, false, time());
                    }
                }
            }
            Input::Pinch(id, scale) => {
                if let Some(surface) = surface(desk, id) {
                    crate::xr::tell_pinch(&surface, scale);
                }
            }
            Input::Resize(id, w, h) => {
                if let Some(told) = desk.told.get(&id) {
                    let window = told.window.clone();
                    state.request_size(&window, (w.max(64), h.max(64)).into());
                }
            }
            Input::Close(id) => {
                if let Some(told) = desk.told.get(&id) {
                    let window = told.window.clone();
                    state.close_window(&window);
                }
            }
        }
    }
}

fn surface(desk: &Desk, id: u32) -> Option<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface> {
    desk.told.get(&id).and_then(|t| t.window.wl_surface().map(|s| s.into_owned()))
}

/// The keyboard to a window, the keys held for the last one let go of first.
fn focus(state: &mut crate::Spatiand, desk: &mut Desk, id: u32, window: &smithay::desktop::Window) {
    if desk.keyboard == Some(id) {
        return;
    }
    let time = desk_time(desk.started);
    for code in std::mem::take(&mut desk.held) {
        key(state, code, false, time);
    }
    desk.keyboard = Some(id);
    state.focus_window(window);
}

fn key(state: &mut crate::Spatiand, code: u32, pressed: bool, time: u32) {
    let Some(keyboard) = state.seat.get_keyboard() else { return };
    let key_state = if pressed { KeyState::Pressed } else { KeyState::Released };
    keyboard.input::<(), _>(state, Keycode::new(code + 8), key_state, SERIAL_COUNTER.next_serial(), time, |_, _, _| FilterResult::Forward);
}

fn desk_time(started: Option<Instant>) -> u32 {
    started.map_or(0, |s| s.elapsed().as_millis() as u32)
}

// --- the app's calls ---

/// Show the session's windows on the Mac (2D mode), or stop and let the glasses have them.
#[no_mangle]
pub extern "C" fn sp_desk(on: bool) {
    ON.store(on, Ordering::SeqCst);
}

/// Open one of Spatiand's own applications by its name (`SpatiMesh`), in whatever mode the
/// session is in.
///
/// # Safety
/// `name` is a C string.
#[no_mangle]
pub unsafe extern "C" fn sp_desk_launch(name: *const c_char) {
    if name.is_null() {
        return;
    }
    queue(Input::Launch(CStr::from_ptr(name).to_string_lossy().into_owned()));
}

/// Spatiand's own applications that are here to run, by name, one a line, into `buffer`.
///
/// # Safety
/// `buffer` holds `capacity` bytes.
#[no_mangle]
pub unsafe extern "C" fn sp_desk_apps(buffer: *mut c_char, capacity: usize) {
    if buffer.is_null() || capacity == 0 {
        return;
    }
    let names = crate::own::list().into_iter().map(|a| a.name).collect::<Vec<_>>().join("\n");
    let bytes = names.as_bytes();
    let n = bytes.len().min(capacity - 1);
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer as *mut u8, n);
    *buffer.add(n) = 0;
}

#[no_mangle]
pub extern "C" fn sp_desk_motion(id: u32, x: f64, y: f64) {
    queue(Input::Motion(id, x, y));
}

#[no_mangle]
pub extern "C" fn sp_desk_leave(id: u32) {
    queue(Input::Leave(id));
}

/// `button`: evdev's, 0x110 left, 0x111 right, 0x112 middle.
#[no_mangle]
pub extern "C" fn sp_desk_button(id: u32, button: u32, pressed: bool) {
    queue(Input::Button(id, button, pressed));
}

/// Across and down by this many pixels; `wheel` for a mouse wheel's notches (15 a notch).
#[no_mangle]
pub extern "C" fn sp_desk_scroll(id: u32, dx: f64, dy: f64, wheel: bool) {
    queue(Input::Scroll(id, dx, dy, wheel));
}

/// A Mac key (virtual key code) down or up.
#[no_mangle]
pub extern "C" fn sp_desk_key(id: u32, mac_code: u16, pressed: bool) {
    if let Some(code) = super::keycodes::evdev_of(mac_code) {
        queue(Input::Key(id, code, pressed));
    }
}

#[no_mangle]
pub extern "C" fn sp_desk_focus(id: u32, focused: bool) {
    queue(Input::Focus(id, focused));
}

/// Two fingers spread by this factor since the last: for a window that zooms what it shows.
#[no_mangle]
pub extern "C" fn sp_desk_pinch(id: u32, scale: f32) {
    queue(Input::Pinch(id, scale));
}

#[no_mangle]
pub extern "C" fn sp_desk_resize(id: u32, width: i32, height: i32) {
    queue(Input::Resize(id, width, height));
}

#[no_mangle]
pub extern "C" fn sp_desk_close(id: u32) {
    queue(Input::Close(id));
}
