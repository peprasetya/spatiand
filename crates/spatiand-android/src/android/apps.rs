//! Android's own apps in the room: listed in the launcher, each on a display of its own, and each
//! display's picture a window like any other.
//!
//! An Android app is not a Wayland client and never will be: Android draws it on a display. An
//! ordinary app may not put another app on a display it made, touch it there or hear it -- only
//! the shell user may. So Spatiand's shell helper (`android/src/.../ShellHelper.java`, run as that
//! user) makes the displays and starts the apps on them, and the app's `Shell.java` talks to it
//! for the room. Here, for each app:
//!
//! ```text
//!   AImageReader ──its window──▶ Shell.openApp ──Binder──▶ the helper: a display drawing into
//!        │                                                 it, and the app started there
//!        └── newest picture ──▶ the placeholder's token ──▶ the compositor (remote_video.rs)
//!   what the wearer does to the window ──▶ Shell.appMotion / appKey ──▶ injected on that display
//! ```
//!
//! **Each app's window is made through the same in-process Wayland client a host's windows are**
//! (`crate::remote::client`), as the Mac's own windows are (`spatiand-mac`'s `local.rs`): it is
//! framed, moved, resized, pushed away and pointed at by the Deck's code, and nothing in the
//! scene knows it is Android's. Its pictures reach the compositor as a host's decoded ones do on
//! Android: an `AImageReader`'s newest buffer, found by the placeholder's token.
//!
//! A display is as big as its window, in pixels, at `Shell.DPI`: resizing the window makes a new
//! reader of the new size and hands the display its surface, so the app lays itself out afresh.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use jni_sys::jvalue;
use smithay::reexports::wayland_server::DisplayHandle;
use spatiand_shell::AppEntry;
use spatiand_stream::Input;
use spatiand_video::android::Output;
use spatiand_video::Converted;

use super::jni;
use crate::remote::client::{self, Client, Told};
use crate::state::ClientState;

/// What the phone is called among the hosts: its apps' windows have the application ids
/// `remote.android.<package>`, so that everything keyed on one -- a controller layout, a title
/// bar's icon, where its sound is placed -- works for them as for a host's.
pub const HOST: &str = "android";

pub fn app_id(package: &str) -> String {
    crate::remote::app_id(HOST, package)
}

/// An app's window when it first opens, in pixels: a small tablet on its side.
const FIRST_SIZE: (u32, u32) = (1280, 800);
/// The displays' density, as `Shell.DPI` makes them.
const DPI: f64 = 213.0;
/// How far one unit of Android's scroll axis moves a list: 64 dp, its ViewConfiguration's.
const PIXELS_PER_SCROLL: f64 = 64.0 * DPI / 160.0;
/// One wheel notch, in the window's scroll events (a wl_pointer axis value).
const NOTCH: f64 = 15.0;
/// No display is made smaller than this on a side: the room asks for odd sizes mid-drag.
const SMALLEST: u32 = 240;

// MotionEvent's actions and buttons, and KeyEvent's meta state: Android's numbers.
const ACTION_DOWN: i32 = 0;
const ACTION_UP: i32 = 1;
const ACTION_MOVE: i32 = 2;
const ACTION_HOVER_MOVE: i32 = 7;
const ACTION_SCROLL: i32 = 8;
const ACTION_HOVER_ENTER: i32 = 9;
const ACTION_HOVER_EXIT: i32 = 10;
const ACTION_BUTTON_PRESS: i32 = 11;
const ACTION_BUTTON_RELEASE: i32 = 12;
const KEY_DOWN: i32 = 0;
const KEY_UP: i32 = 1;
const KEYCODE_BACK: i32 = 4;
const KEYCODE_FORWARD: i32 = 125;

// --- the launcher's list ---

fn listed() -> &'static Mutex<Vec<AppEntry>> {
    static APPS: OnceLock<Mutex<Vec<AppEntry>>> = OnceLock::new();
    APPS.get_or_init(Default::default)
}

/// Set when the list has changed and the launcher has not yet been given it.
static CHANGED: AtomicBool = AtomicBool::new(false);
/// Whether the shell helper is there.
static READY: AtomicBool = AtomicBool::new(false);

/// The app's list of the phone's apps (`Native.apps`), replaced whole. Each one's icon is kept
/// under its application id too, for its window's title bar.
pub fn set_apps(json: &str) {
    #[derive(serde::Deserialize)]
    struct Listed {
        name: String,
        exec: String,
        #[serde(default)]
        icon: Option<String>,
    }
    let all: Vec<Listed> = match serde_json::from_str(json) {
        Ok(all) => all,
        Err(e) => return log::warn!("the phone's apps could not be read: {e}"),
    };
    let mut apps: Vec<AppEntry> = all
        .into_iter()
        .map(|a| {
            let icon = a.icon.and_then(|path| {
                let png = std::fs::read(&path).ok()?;
                crate::remote::store_icon(&app_id(package_of(&a.exec)), &png).or(Some(path))
            });
            AppEntry { name: a.name, exec: a.exec, icon, categories: Vec::new() }
        })
        .collect();
    apps.sort_by_key(|a| a.name.to_lowercase());
    log::info!("{} of the phone's apps for the launcher", apps.len());
    *listed().lock().unwrap() = apps;
    CHANGED.store(true, Ordering::SeqCst);
}

pub fn apps() -> Vec<AppEntry> {
    listed().lock().unwrap().clone()
}

/// The list, if it has changed since the launcher was last given it.
pub fn changed() -> Option<Vec<AppEntry>> {
    CHANGED.swap(false, Ordering::SeqCst).then(apps)
}

fn package_of(component: &str) -> &str {
    component.split('/').next().unwrap_or(component)
}

/// The app asked for while the helper was not there, to open once it is.
static WAITING: Mutex<Option<AppEntry>> = Mutex::new(None);

/// The shell helper has come or gone. Gone, every display it made has gone with it; come, the
/// app asked for meanwhile is opened.
pub fn set_ready(ready: bool) {
    let was = READY.swap(ready, Ordering::SeqCst);
    if was != ready {
        log::info!("the shell helper is {}", if ready { "here" } else { "gone" });
        if !ready {
            send(Command::HelperGone);
        }
    }
    if ready {
        if let Some(app) = WAITING.lock().unwrap().take() {
            NOTICE.lock().unwrap().take();
            launch(&app);
        }
    }
}

/// Open an app from the launcher in a window of its own, or bring its window out if it has one.
/// With no helper yet, the app is asked to start it (through Wireless debugging), and this app
/// opens when it comes.
pub fn launch(app: &AppEntry) {
    if !READY.load(Ordering::SeqCst) {
        log::info!("{}: starting the shell helper first", app.name);
        *WAITING.lock().unwrap() = Some(app.clone());
        say(&format!("Android apps\nStarting Spatiand's helper to open {}…", app.name));
        java_start_helper();
        return;
    }
    log::info!("opening {}", app.name);
    send(Command::Open { component: app.exec.clone(), label: app.name.clone() });
}

/// Something the app has to say about the phone's apps: why a helper could not be started.
pub fn tell(text: &str) {
    say(text);
}

/// The helper says a display has nothing left on it: the app was closed from inside.
pub fn gone(display: i32) {
    send(Command::Gone { display });
}

/// A click at `x`,`y` (its own pixels) in the newest app's window, as the room's pointer makes
/// one: for trying the input path with no glasses (`MainActivity`'s tap extra).
pub fn tap(x: f64, y: f64) {
    send(Command::Tap { x, y });
}

/// The helper says an app has begun, or stopped, hiding its picture (a FLAG_SECURE window).
pub fn protected(display: i32, on: bool) {
    send(Command::Protected { display, on });
}

// --- what the wearer is told ---

static NOTICE: Mutex<Option<(String, Instant)>> = Mutex::new(None);

fn say(text: &str) {
    *NOTICE.lock().unwrap() = Some((text.to_string(), Instant::now()));
}

/// The card to show, for a while after something about the phone's apps went wrong.
pub fn notice() -> String {
    let mut n = NOTICE.lock().unwrap();
    match n.as_ref() {
        Some((text, at)) if at.elapsed() < Duration::from_secs(12) => text.clone(),
        Some(_) => {
            *n = None;
            String::new()
        }
        None => String::new(),
    }
}

pub fn notice_pressed() -> bool {
    NOTICE.lock().unwrap().take().is_some()
}

// --- windows ---

enum Command {
    Open { component: String, label: String },
    Gone { display: i32 },
    HelperGone,
    Tap { x: f64, y: f64 },
    Protected { display: i32, on: bool },
}

/// One app's window, as this side knows it.
struct Entry {
    component: String,
    label: String,
    /// The display it is on, the helper's number for it.
    display: i32,
    output: Arc<Output>,
    size: (u32, u32),
    /// Its surface's number on the client's connection: how a window in the room is known.
    surface: Option<u32>,
    /// It is to come out in front of the wearer: opened from the launcher again.
    show: bool,
    // What the pointer is doing to it, for the MotionEvents it makes.
    buttons: i32,
    down_time: i64,
    hovering: bool,
    pointer: (f32, f32),
    meta: i32,
    /// The app hides its picture (a window of it is FLAG_SECURE): the window shows the note in
    /// its place, and this is where the note's button is, in the window's pixels.
    button: Option<[f32; 4]>,
}

struct Windows {
    commands: Sender<Command>,
    /// Which surface is which window, and what is wanted of each, for the frame loop.
    shown: Mutex<HashMap<u32, (u32, bool)>>,
}

static WINDOWS: OnceLock<Windows> = OnceLock::new();

fn send(command: Command) {
    if let Some(windows) = WINDOWS.get() {
        let _ = windows.commands.send(command);
    }
}

/// Connect the client the phone's apps' windows are made through. Once, from the compositor's
/// thread, before its loop starts.
pub fn attach(display: &mut DisplayHandle) -> Result<(), String> {
    if WINDOWS.get().is_some() {
        return Ok(());
    }
    let (server, stream) =
        std::os::unix::net::UnixStream::pair().map_err(|e| format!("could not make a socket pair: {e}"))?;
    display
        .insert_client(server, Arc::new(ClientState::default()))
        .map_err(|e| format!("could not attach the phone's apps: {e}"))?;
    let (commands, inbox) = channel();
    std::thread::Builder::new()
        .name("android-apps".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(stream, inbox)));
            if result.is_err() {
                log::error!("the phone's apps' windows died; see the panic above");
            }
        })
        .map_err(|e| format!("could not start the phone's apps' windows: {e}"))?;
    let _ = WINDOWS.set(Windows { commands, shown: Mutex::default() });
    Ok(())
}

fn run(stream: std::os::unix::net::UnixStream, inbox: Receiver<Command>) {
    let (mut client, mut queue) = match Client::new(stream) {
        Ok(pair) => pair,
        Err(e) => return log::error!("the phone's apps: {e}"),
    };
    let mut entries: HashMap<u32, Entry> = HashMap::new();
    let mut next_id = 1u32;
    loop {
        // A few milliseconds at most between looks at the compositor: how long a click waits.
        let first = match inbox.recv_timeout(Duration::from_millis(3)) {
            Ok(command) => Some(command),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        for command in first.into_iter().chain(inbox.try_iter()) {
            match command {
                Command::Open { component, label } => {
                    if let Some(entry) = entries.values_mut().find(|e| e.component == component) {
                        entry.show = true;
                        continue;
                    }
                    let id = next_id;
                    next_id += 1;
                    match open(&mut client, id, &component, &label) {
                        Ok(entry) => {
                            entries.insert(id, entry);
                        }
                        Err(e) => {
                            log::warn!("{label}: {e}");
                            say(&format!("Android apps\n{label} could not open here: {e}"));
                        }
                    }
                }
                Command::Gone { display } => {
                    let gone: Vec<u32> = entries.iter().filter(|(_, e)| e.display == display).map(|(id, _)| *id).collect();
                    for id in gone {
                        close(&mut client, &mut entries, id);
                    }
                }
                Command::Tap { x, y } => {
                    if let Some(entry) = entries.iter_mut().max_by_key(|(id, _)| **id).map(|(_, e)| e) {
                        pass_on(entry, Input::Motion { x, y });
                        pass_on(entry, Input::Button { button: 0x110, pressed: true });
                        pass_on(entry, Input::Button { button: 0x110, pressed: false });
                    }
                }
                Command::Protected { display, on } => {
                    if let Some((id, entry)) = entries.iter_mut().find(|(_, e)| e.display == display) {
                        protect(&mut client, *id, entry, on);
                    }
                }
                Command::HelperGone => {
                    let all: Vec<u32> = entries.keys().copied().collect();
                    for id in all {
                        client.close(id);
                        entries.remove(&id);
                    }
                }
            }
        }
        if let Err(e) = client::pump(&mut client, &mut queue) {
            return log::error!("the phone's apps: {e}");
        }
        for (id, width, height) in std::mem::take(&mut client.resized) {
            if let Some(entry) = entries.get_mut(&id) {
                resize(&mut client, id, entry, width, height);
            }
        }
        for id in std::mem::take(&mut client.closing) {
            close(&mut client, &mut entries, id);
        }
        for told in std::mem::take(&mut client.input) {
            if let Told::Input { window, input, .. } = told {
                if let Some(entry) = entries.get_mut(&window) {
                    pass_on(entry, input);
                }
            }
        }
        // What the frame loop reads: which surface is which window, and which is to come out.
        if let Some(windows) = WINDOWS.get() {
            let mut shown = windows.shown.lock().unwrap();
            // A wish the frame loop has not taken yet is kept: it looks once a frame, and this
            // list is made again every few milliseconds.
            let waiting: Vec<u32> = shown.values().filter(|(_, show)| *show).map(|(id, _)| *id).collect();
            shown.clear();
            for (id, entry) in entries.iter_mut() {
                if let Some(surface) = entry.surface {
                    let show = std::mem::take(&mut entry.show) || waiting.contains(id);
                    shown.insert(surface, (*id, show));
                }
            }
        }
    }
}

/// A new reader the window's size, a display drawing into it, and the app started there.
fn open(client: &mut Client, id: u32, component: &str, label: &str) -> Result<Entry, String> {
    let size = FIRST_SIZE;
    let (output, window) = Output::for_window(size).map_err(|e| e.to_string())?;
    let display = java_open(component, window, size).ok_or("the helper could not put it on a display")?;
    client.open(id, &app_id(package_of(component)), label);
    let surface = client.windows.get(&id).map(|w| wayland_client::Proxy::id(&w.surface).protocol_id());
    client.show(id, Converted::of(output.clone(), size))?;
    log::info!("{label} is on display {display}, window {id}");
    hear(display, package_of(component));
    Ok(Entry {
        component: component.to_string(),
        label: label.to_string(),
        display,
        output,
        size,
        surface,
        show: false,
        buttons: 0,
        down_time: 0,
        hovering: false,
        pointer: (0.0, 0.0),
        meta: 0,
        button: None,
    })
}

/// The room has asked the window to be this size: the display follows, with a new reader to
/// draw into at that size.
fn resize(client: &mut Client, id: u32, entry: &mut Entry, width: i32, height: i32) {
    if width <= 0 || height <= 0 {
        return;
    }
    let size = ((width as u32).max(SMALLEST), (height as u32).max(SMALLEST));
    if size == entry.size {
        return;
    }
    let (output, window) = match Output::for_window(size) {
        Ok(made) => made,
        Err(e) => return log::warn!("window {id}: no reader for {}x{}: {e}", size.0, size.1),
    };
    java_resize(entry.display, window, size);
    // The old reader lives on in any picture of it the compositor still draws.
    entry.output = output.clone();
    entry.size = size;
    if entry.button.is_some() {
        return protect(client, id, entry, true);
    }
    if let Err(e) = client.show(id, Converted::of(output, size)) {
        log::warn!("window {id}: {e}");
    }
}

/// Show the note in place of an app's picture while it hides it, or its picture again.
fn protect(client: &mut Client, id: u32, entry: &mut Entry, on: bool) {
    let shown = if on {
        log::info!("{} hides its picture: the note in its place", entry.label);
        let (pixels, button) = note(&entry.label, entry.size);
        entry.button = Some(button);
        client.show_pixels(id, entry.size, &pixels)
    } else {
        log::info!("{} shows its picture again", entry.label);
        entry.button = None;
        client.show(id, Converted::of(entry.output.clone(), entry.size))
    };
    if let Err(e) = shown {
        log::warn!("window {id}: {e}");
    }
}

/// The note an app that hides its picture gets in its place -- Android shows such an app only
/// on the phone's own screen and on the glasses', never in a picture of a display that is not
/// secure, which only a system app may make -- with a button that lends it the glasses: flat,
/// as on a screen. Xrgb8888 at `size`, and the button's place in it.
fn note(label: &str, size: (u32, u32)) -> (Vec<u8>, [f32; 4]) {
    use spatiand_render::text::{TextImage, TextRenderer};
    thread_local! {
        static TEXT: std::cell::RefCell<Option<TextRenderer>> = const { std::cell::RefCell::new(None) };
    }
    const PAPER: [u8; 3] = [24, 27, 36];
    const BUTTON: [u8; 3] = [58, 108, 228];
    let (w, h) = (size.0 as usize, size.1 as usize);
    let mut pixels = Vec::with_capacity(w * h * 4);
    for _ in 0..w * h {
        pixels.extend_from_slice(&[PAPER[2], PAPER[1], PAPER[0], 255]);
    }
    // A straight-alpha RGBA image laid on the BGRX paper, its top-left at (x, y).
    let lay = |pixels: &mut Vec<u8>, image: &TextImage, x: usize, y: usize| {
        for row in 0..image.height as usize {
            for col in 0..image.width as usize {
                let (px, py) = (x + col, y + row);
                if px >= w || py >= h {
                    continue;
                }
                let s = &image.rgba[(row * image.width as usize + col) * 4..][..4];
                let a = s[3] as u32;
                let d = &mut pixels[(py * w + px) * 4..][..4];
                for (channel, source) in [(0usize, 2usize), (1, 1), (2, 0)] {
                    d[channel] = ((s[source] as u32 * a + d[channel] as u32 * (255 - a)) / 255) as u8;
                }
            }
        }
    };
    let scale = (h as f32 / 800.0).clamp(0.35, 3.0);
    TEXT.with(|text| {
        let mut text = text.borrow_mut();
        let text = text.get_or_insert_with(TextRenderer::new);
        let title = text.render(&format!("{label} hides its picture"), 46.0 * scale, (w as f32 * 0.86) as u32, [240, 244, 255, 255]);
        let body = text.render(
            "Android shows it only on the phone's own screen and on the glasses', so here it would be black.",
            27.0 * scale,
            (w as f32 * 0.72) as u32,
            [176, 186, 206, 255],
        );
        let words = text.render("Show it on the glasses, flat", 31.0 * scale, (w as f32 * 0.7) as u32, [255, 255, 255, 255]);
        let top = (h as f32 * 0.24) as usize;
        lay(&mut pixels, &title, w.saturating_sub(title.width as usize) / 2, top);
        let below = top + title.height as usize + (22.0 * scale) as usize;
        lay(&mut pixels, &body, w.saturating_sub(body.width as usize) / 2, below);
        let (bw, bh) = (words.width as f32 + 88.0 * scale, words.height as f32 + 44.0 * scale);
        let (bx, by) = ((w as f32 - bw) / 2.0, (h as f32 * 0.68 - bh / 2.0).max((below + body.height as usize) as f32 + 30.0 * scale));
        let radius = bh / 2.0;
        for py in by as usize..((by + bh) as usize).min(h) {
            for px in bx as usize..((bx + bw) as usize).min(w) {
                // Round ends: a pill.
                let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                let cx = fx.clamp(bx + radius, bx + bw - radius);
                let cy = by + radius;
                if (fx - cx).powi(2) + (fy - cy).powi(2) > radius * radius {
                    continue;
                }
                pixels[(py * w + px) * 4..][..3].copy_from_slice(&[BUTTON[2], BUTTON[1], BUTTON[0]]);
            }
        }
        lay(&mut pixels, &words, (bx + (bw - words.width as f32) / 2.0) as usize, (by + (bh - words.height as f32) / 2.0) as usize);
        (pixels, [bx, by, bx + bw, by + bh])
    })
}

fn close(client: &mut Client, entries: &mut HashMap<u32, Entry>, id: u32) {
    if let Some(entry) = entries.remove(&id) {
        java_close(entry.display);
        log::info!("{} closed, display {}", entry.component, entry.display);
    }
    client.close(id);
}

/// The app's sound, which the helper takes off the speakers, placed at its window as the room
/// places a host's: 16-bit stereo at 48 kHz from a pipe, fed to its window's slot until the
/// helper closes it (the window closed, or the helper went).
fn hear(display: i32, package: &str) {
    let Some(fd) = java_sound(display, package) else {
        return log::info!("{package}: its sound stays where Android plays it");
    };
    let app_id = app_id(package);
    let started = std::thread::Builder::new().name(format!("sound {display}")).spawn(move || {
        use std::io::Read;
        use std::os::fd::FromRawFd;
        let mut pipe = unsafe { std::fs::File::from_raw_fd(fd) };
        let mut bytes = vec![0u8; 3840];
        let mut held: Vec<u8> = Vec::new();
        let mut samples: Vec<i16> = Vec::new();
        let mut heard = false;
        loop {
            let n = match pipe.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            held.extend_from_slice(&bytes[..n]);
            // Whole frames only: two channels of two bytes.
            let whole = held.len() / 4 * 4;
            samples.clear();
            samples.extend(held[..whole].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
            held.drain(..whole);
            if !heard && samples.iter().any(|s| s.unsigned_abs() > 64) {
                heard = true;
                let slot = crate::remote::sound::slot_for(&app_id);
                log::info!("{app_id}: its sound has reached the room (slot {slot:?})");
            }
            spatiand_audio::server::feed_local(crate::remote::sound::slot_for(&app_id), &samples, 2);
        }
        log::info!("{app_id}: its sound has ended");
    });
    if let Err(e) = started {
        log::warn!("{package}: no thread for its sound: {e}");
        unsafe { libc::close(fd) };
    }
}

// --- what the wearer does, as Android's events ---

fn pass_on(entry: &mut Entry, input: Input) {
    let display = entry.display;
    let now = uptime_ms();
    // The note on an app that hides its picture: its button is the one thing to press, and
    // nothing pointed at the black behind it reaches the app. Keys still do.
    if let Some([x0, y0, x1, y1]) = entry.button {
        match input {
            Input::Motion { x, y } => entry.pointer = (x as f32, y as f32),
            Input::Button { button: 0x110, pressed: false } => {
                let (x, y) = entry.pointer;
                if (x0..=x1).contains(&x) && (y0..=y1).contains(&y) {
                    log::info!("{}: shown flat on the glasses", entry.label);
                    java_show_flat(display, &entry.label);
                }
            }
            Input::Key { .. } => {}
            _ => {}
        }
        if !matches!(input, Input::Key { .. }) {
            return;
        }
    }
    match input {
        Input::Motion { x, y } => {
            entry.pointer = (x as f32, y as f32);
            if entry.buttons != 0 {
                java_motion(display, ACTION_MOVE, entry.buttons, 0, entry.pointer, (0.0, 0.0), entry.down_time);
            } else {
                if !entry.hovering {
                    entry.hovering = true;
                    java_motion(display, ACTION_HOVER_ENTER, 0, 0, entry.pointer, (0.0, 0.0), 0);
                }
                java_motion(display, ACTION_HOVER_MOVE, 0, 0, entry.pointer, (0.0, 0.0), 0);
            }
        }
        Input::Leave => {
            if entry.hovering && entry.buttons == 0 {
                entry.hovering = false;
                java_motion(display, ACTION_HOVER_EXIT, 0, 0, entry.pointer, (0.0, 0.0), 0);
            }
        }
        Input::Button { button, pressed } => {
            let bit = match button {
                0x110 => 1, // BTN_LEFT: BUTTON_PRIMARY
                0x111 => 2, // BTN_RIGHT: BUTTON_SECONDARY
                0x112 => 4, // BTN_MIDDLE: BUTTON_TERTIARY
                // The side buttons are back and forward, as Android's own mice have them.
                0x113 => return java_key(display, if pressed { KEY_DOWN } else { KEY_UP }, KEYCODE_BACK, 0, now),
                0x114 => return java_key(display, if pressed { KEY_DOWN } else { KEY_UP }, KEYCODE_FORWARD, 0, now),
                _ => return,
            };
            if pressed {
                if entry.buttons & bit != 0 {
                    return;
                }
                if entry.buttons == 0 {
                    if entry.hovering {
                        entry.hovering = false;
                        java_motion(display, ACTION_HOVER_EXIT, 0, 0, entry.pointer, (0.0, 0.0), 0);
                    }
                    entry.down_time = now;
                    entry.buttons = bit;
                    java_motion(display, ACTION_DOWN, entry.buttons, 0, entry.pointer, (0.0, 0.0), entry.down_time);
                } else {
                    entry.buttons |= bit;
                }
                java_motion(display, ACTION_BUTTON_PRESS, entry.buttons, bit, entry.pointer, (0.0, 0.0), entry.down_time);
            } else {
                if entry.buttons & bit == 0 {
                    return;
                }
                entry.buttons &= !bit;
                java_motion(display, ACTION_BUTTON_RELEASE, entry.buttons, bit, entry.pointer, (0.0, 0.0), entry.down_time);
                if entry.buttons == 0 {
                    java_motion(display, ACTION_UP, 0, 0, entry.pointer, (0.0, 0.0), entry.down_time);
                    entry.hovering = true;
                    java_motion(display, ACTION_HOVER_ENTER, 0, 0, entry.pointer, (0.0, 0.0), 0);
                }
            }
        }
        // Android's scroll axis is up for positive; the room's, down.
        Input::Scroll { horizontal, vertical } => java_motion(
            display,
            ACTION_SCROLL,
            entry.buttons,
            0,
            entry.pointer,
            ((-vertical / NOTCH) as f32, (horizontal / NOTCH) as f32),
            0,
        ),
        Input::FingerScroll { horizontal, vertical } => java_motion(
            display,
            ACTION_SCROLL,
            entry.buttons,
            0,
            entry.pointer,
            ((-vertical / PIXELS_PER_SCROLL) as f32, (horizontal / PIXELS_PER_SCROLL) as f32),
            0,
        ),
        Input::ScrollStop => {}
        Input::Key { code, pressed } => {
            let Some(key) = super::keys::to_android(code) else { return };
            entry.meta = meta_after(entry.meta, key, pressed);
            java_key(display, if pressed { KEY_DOWN } else { KEY_UP }, key, entry.meta, now);
        }
    }
}

/// KeyEvent's meta state after a key: the modifiers held, each side's own bit and the bit for
/// either; caps lock turned over on each press.
fn meta_after(meta: i32, key: i32, pressed: bool) -> i32 {
    const SHIFT: i32 = 0x1;
    const ALT: i32 = 0x2;
    const CTRL: i32 = 0x1000;
    const META: i32 = 0x10000;
    const CAPS: i32 = 0x100000;
    let side = match key {
        59 => 0x40,     // SHIFT_LEFT
        60 => 0x80,     // SHIFT_RIGHT
        57 => 0x10,     // ALT_LEFT
        58 => 0x20,     // ALT_RIGHT
        113 => 0x2000,  // CTRL_LEFT
        114 => 0x4000,  // CTRL_RIGHT
        117 => 0x20000, // META_LEFT
        118 => 0x40000, // META_RIGHT
        115 => return if pressed { meta ^ CAPS } else { meta },
        _ => return meta,
    };
    let mut meta = if pressed { meta | side } else { meta & !side };
    for (either, sides) in [(SHIFT, 0xC0), (ALT, 0x30), (CTRL, 0x6000), (META, 0x60000)] {
        meta = if meta & sides != 0 { meta | either } else { meta & !either };
    }
    meta
}

fn uptime_ms() -> i64 {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    now.tv_sec * 1000 + now.tv_nsec / 1_000_000
}

// --- the frame loop's side: which of its windows are these ---

/// Bring out a window opened again from the launcher, as the Mac's own are (`size_new_windows`).
pub fn size_new_windows(state: &mut crate::Spatiand) {
    use smithay::reexports::wayland_server::Resource;
    use smithay::wayland::seat::WaylandFocus;
    let Some(windows) = WINDOWS.get() else { return };
    let wanted: Vec<u32> = {
        let mut shown = windows.shown.lock().unwrap();
        shown.iter_mut().filter_map(|(surface, (_, show))| std::mem::take(show).then_some(*surface)).collect()
    };
    if wanted.is_empty() {
        return;
    }
    let all: Vec<smithay::desktop::Window> = state.space.elements().cloned().collect();
    for window in all {
        let Some(surface) = window.wl_surface() else { continue };
        if !wanted.contains(&surface.id().protocol_id()) {
            continue;
        }
        if let Some(mut placement) = state.layout.get(&window).filter(|_| !state.layout.is_pinned(&window)) {
            placement.yaw = state.spawn_yaw;
            placement.pitch = crate::window::clamp_pitch(state.layout.view_pitch);
            state.layout.set(&window, placement);
        }
        state.show_window(&window);
        state.focus_window(&window);
    }
}

// --- the calls on Shell.java ---

fn java_sound(display: i32, package: &str) -> Option<i32> {
    let fd = jni::with_shell(|env, class| unsafe {
        let method = jni::shell_method(env, class, c"appSound", c"(ILjava/lang/String;)I")?;
        let args = [jvalue { i: display }, jvalue { l: jni::new_string(env, package) }];
        Some(((**env).v1_1.CallStaticIntMethodA)(env, class, method, args.as_ptr()))
    })??;
    (fd >= 0).then_some(fd)
}

fn java_show_flat(display: i32, label: &str) {
    jni::with_shell(|env, class| unsafe {
        let Some(method) = jni::shell_method(env, class, c"showFlat", c"(ILjava/lang/String;)V") else { return };
        let args = [jvalue { i: display }, jvalue { l: jni::new_string(env, label) }];
        ((**env).v1_1.CallStaticVoidMethodA)(env, class, method, args.as_ptr());
    });
}

fn java_start_helper() {
    jni::with_shell(|env, class| unsafe {
        let Some(method) = jni::shell_method(env, class, c"startHelper", c"()V") else { return };
        ((**env).v1_1.CallStaticVoidMethodA)(env, class, method, std::ptr::null());
    });
}

fn java_open(component: &str, window: *mut ndk_sys::ANativeWindow, size: (u32, u32)) -> Option<i32> {
    let display = jni::with_shell(|env, class| unsafe {
        let method = jni::shell_method(env, class, c"openApp", c"(Ljava/lang/String;Landroid/view/Surface;II)I")?;
        let surface = ndk_sys::ANativeWindow_toSurface(env.cast(), window);
        if surface.is_null() {
            return None;
        }
        let args = [
            jvalue { l: jni::new_string(env, component) },
            jvalue { l: surface.cast() },
            jvalue { i: size.0 as i32 },
            jvalue { i: size.1 as i32 },
        ];
        Some(((**env).v1_1.CallStaticIntMethodA)(env, class, method, args.as_ptr()))
    })??;
    (display >= 0).then_some(display)
}

fn java_resize(display: i32, window: *mut ndk_sys::ANativeWindow, size: (u32, u32)) {
    jni::with_shell(|env, class| unsafe {
        let Some(method) = jni::shell_method(env, class, c"resizeApp", c"(ILandroid/view/Surface;II)V") else { return };
        let surface = ndk_sys::ANativeWindow_toSurface(env.cast(), window);
        if surface.is_null() {
            return;
        }
        let args = [jvalue { i: display }, jvalue { l: surface.cast() }, jvalue { i: size.0 as i32 }, jvalue { i: size.1 as i32 }];
        ((**env).v1_1.CallStaticVoidMethodA)(env, class, method, args.as_ptr());
    });
}

fn java_close(display: i32) {
    jni::with_shell(|env, class| unsafe {
        let Some(method) = jni::shell_method(env, class, c"closeApp", c"(I)V") else { return };
        let args = [jvalue { i: display }];
        ((**env).v1_1.CallStaticVoidMethodA)(env, class, method, args.as_ptr());
    });
}

fn java_motion(display: i32, action: i32, buttons: i32, action_button: i32, at: (f32, f32), scroll: (f32, f32), down_time: i64) {
    jni::with_shell(|env, class| unsafe {
        let Some(method) = jni::shell_method(env, class, c"appMotion", c"(IIIIFFFFJ)V") else { return };
        let args = [
            jvalue { i: display },
            jvalue { i: action },
            jvalue { i: buttons },
            jvalue { i: action_button },
            jvalue { f: at.0 },
            jvalue { f: at.1 },
            jvalue { f: scroll.0 },
            jvalue { f: scroll.1 },
            jvalue { j: down_time },
        ];
        ((**env).v1_1.CallStaticVoidMethodA)(env, class, method, args.as_ptr());
    });
}

fn java_key(display: i32, action: i32, key: i32, meta: i32, down_time: i64) {
    jni::with_shell(|env, class| unsafe {
        let Some(method) = jni::shell_method(env, class, c"appKey", c"(IIIIJ)V") else { return };
        let args = [jvalue { i: display }, jvalue { i: action }, jvalue { i: key }, jvalue { i: meta }, jvalue { j: down_time }];
        ((**env).v1_1.CallStaticVoidMethodA)(env, class, method, args.as_ptr());
    });
}
