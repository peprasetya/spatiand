//! The room's side of its assistant (`spatiand_assist_v1`; `docs/ai-integrations.md`).
//!
//! The assistant is one of Spatiand's own programs, and asks here what is in the room and does
//! what it was asked to there: lists the windows and the applications, reads a window (its own
//! account of itself, or a picture of it), points, clicks, scrolls and types on a window as the
//! wearer would, and hands the shell what only the shell does -- opening an application,
//! focusing, hiding, pinning or closing a window, recentring.
//!
//! **Refused to all but the assistant.** Every client can bind the global, and every question is
//! answered with an error until the client has said the session's key, which the room gives to
//! the assistant's process alone when it starts it ([`key`]).
//!
//! Questions arrive while the protocol is being dispatched, where only the room's state is to
//! hand; they wait in a queue and the backend answers them each frame with [`serve`], where the
//! renderer, the windows as drawn and the shell are.

use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};
use spatiand_proto::server::spatiand_assist_v1::{self, SpatiandAssistV1};
use spatiand_shell::ShellEvent;

use crate::state::Spatiand;

/// The session's key: what the assistant says to be listened to. Made once, from the process's
/// own randomness, and never written anywhere but the assistant's environment.
pub fn key() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| {
        use std::hash::{BuildHasher, Hasher};
        let mut out = String::new();
        for i in 0..2u64 {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(i);
            h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
            h.write_u32(std::process::id());
            out.push_str(&format!("{:016x}", h.finish()));
        }
        out
    })
}

/// A question, waiting for the backend to answer it.
pub struct Asked {
    pub what: Value,
    answer: OwnedFd,
}

impl Asked {
    /// Answer, and close: written on a thread of its own, because a big answer (a picture) fills
    /// a pipe and waits for the assistant to read it.
    pub fn answer(self, value: Value) {
        let text = value.to_string();
        let fd = self.answer;
        std::thread::spawn(move || {
            let mut file = std::fs::File::from(fd);
            let _ = std::io::Write::write_all(&mut file, text.as_bytes());
        });
    }

    fn refuse(self, why: &str) {
        self.answer(json!({ "error": why }));
    }

    fn get(&self, key: &str) -> Option<&Value> {
        self.what.get(key)
    }

    fn str(&self, key: &str) -> &str {
        self.get(key).and_then(Value::as_str).unwrap_or_default()
    }

    fn number(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(Value::as_f64)
    }
}

static ASKED: Mutex<Vec<Asked>> = Mutex::new(Vec::new());

/// The assistant's resources, to be told what happens.
static LISTENERS: Mutex<Vec<SpatiandAssistV1>> = Mutex::new(Vec::new());

/// The assistant's own connection: its windows are its own, never what it acts on unasked.
static ASSISTANT: Mutex<Option<smithay::reexports::wayland_server::backend::ClientId>> = Mutex::new(None);

/// Whether a window is the assistant's own (its chat window).
fn is_own(window: &smithay::desktop::Window) -> bool {
    let Some(assistant) = ASSISTANT.lock().unwrap().clone() else { return false };
    window.toplevel().and_then(|t| t.wl_surface().client()).is_some_and(|c| c.id() == assistant)
}

/// Tell the assistant something (`happened`): the focus moved, it was asked for.
pub fn tell(what: Value) {
    let text = what.to_string();
    let mut listeners = LISTENERS.lock().unwrap();
    listeners.retain(|r| r.is_alive());
    for r in listeners.iter() {
        r.happened(text.clone());
    }
}

/// Whether the assistant is in the room (has said its key).
pub fn present() -> bool {
    LISTENERS.lock().unwrap().iter().any(|r| r.is_alive())
}

/// Each binding of the global: whether it has said the key.
pub struct Trust(AtomicBool);

impl GlobalDispatch<SpatiandAssistV1, ()> for Spatiand {
    fn bind(_: &mut Self, _: &DisplayHandle, _: &Client, resource: New<SpatiandAssistV1>, _: &(), init: &mut DataInit<'_, Self>) {
        init.init(resource, Trust(AtomicBool::new(false)));
    }
}

impl Dispatch<SpatiandAssistV1, Trust> for Spatiand {
    fn request(
        _: &mut Self,
        _: &Client,
        resource: &SpatiandAssistV1,
        request: spatiand_assist_v1::Request,
        trust: &Trust,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            spatiand_assist_v1::Request::Hello { key: said } => {
                let right = said == key();
                trust.0.store(right, Ordering::Relaxed);
                if right {
                    LISTENERS.lock().unwrap().push(resource.clone());
                    *ASSISTANT.lock().unwrap() = resource.client().map(|c| c.id());
                    log::info!("assist: the assistant is in the room");
                } else {
                    log::warn!("assist: a client said the wrong key, and will be refused");
                }
            }
            spatiand_assist_v1::Request::Ask { json, answer } => {
                let asked = Asked { what: serde_json::from_str(&json).unwrap_or(Value::Null), answer };
                if !trust.0.load(Ordering::Relaxed) {
                    asked.refuse("only the room's own assistant may ask");
                } else if !asked.what.is_object() {
                    asked.refuse("a question is a JSON object");
                } else {
                    ASKED.lock().unwrap().push(asked);
                }
            }
            spatiand_assist_v1::Request::Listen { fd } => {
                if trust.0.load(Ordering::Relaxed) {
                    *WANTED_EARS.lock().unwrap() = Some(fd);
                }
            }
            spatiand_assist_v1::Request::Destroy => {
                if trust.0.load(Ordering::Relaxed) {
                    STOP_EARS.store(true, Ordering::Relaxed);
                }
            }
            _ => {}
        }
    }

    fn destroyed(_: &mut Self, _: smithay::reexports::wayland_server::backend::ClientId, _: &SpatiandAssistV1, trust: &Trust) {
        if trust.0.load(Ordering::Relaxed) {
            STOP_EARS.store(true, Ordering::Relaxed);
        }
    }
}

// --- the microphone, for the assistant ---

/// A `listen` not yet acted on: the room opens the microphone for it from [`serve`].
static WANTED_EARS: Mutex<Option<OwnedFd>> = Mutex::new(None);
/// The assistant went: its microphone closes.
static STOP_EARS: AtomicBool = AtomicBool::new(false);

struct Ears {
    _microphone: crate::listen::Microphone,
    /// Set by the sink when the assistant stopped reading.
    gone: Arc<AtomicBool>,
}

thread_local! {
    static EARS: std::cell::RefCell<Option<Ears>> = const { std::cell::RefCell::new(None) };
}

fn tend_ears() {
    if STOP_EARS.swap(false, Ordering::Relaxed) {
        EARS.with(|e| *e.borrow_mut() = None);
    }
    if let Some(fd) = WANTED_EARS.lock().unwrap().take() {
        let gone = Arc::new(AtomicBool::new(false));
        let flag = gone.clone();
        let raw = std::os::fd::AsRawFd::as_raw_fd(&fd);
        unsafe { libc::fcntl(raw, libc::F_SETFL, libc::fcntl(raw, libc::F_GETFL) | libc::O_NONBLOCK) };
        let mut file = std::fs::File::from(fd);
        let sink: crate::listen::Sink = Box::new(move |samples: &[i16]| {
            if flag.load(Ordering::Relaxed) {
                return;
            }
            let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
            match std::io::Write::write(&mut file, &bytes) {
                Ok(_) => {}
                // Full: the assistant is behind; this much of the room is not heard.
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => flag.store(true, Ordering::Relaxed),
            }
        });
        match crate::listen::Microphone::open(spatiand_voice::RATE, sink) {
            Ok(microphone) => {
                log::info!("assist: the assistant is listening");
                EARS.with(|e| *e.borrow_mut() = Some(Ears { _microphone: microphone, gone }));
            }
            Err(why) => {
                log::warn!("assist: the microphone would not open for the assistant: {why}");
                tell(json!({ "trouble": why }));
            }
        }
    }
    EARS.with(|e| {
        let mut e = e.borrow_mut();
        if e.as_ref().is_some_and(|ears| ears.gone.load(Ordering::Relaxed)) {
            *e = None;
            log::info!("assist: the assistant stopped listening");
        }
    });
}

// --- answering ---

/// Answer what the assistant asked since the last frame. What needs the shell comes back as its
/// events, for the backend to handle as it handles the wearer's.
pub fn serve(
    state: &mut Spatiand,
    shell: Option<&spatiand_shell::Shell>,
    windows: &[crate::scene::WindowQuad],
    renderer: &mut smithay::backend::renderer::gles::GlesRenderer,
    time_ms: u32,
) -> Vec<ShellEvent> {
    tend_ears();
    let mut events = Vec::new();
    if let Some(shell) = shell {
        events.extend(keep_running(state, shell));
    }
    let asked: Vec<Asked> = std::mem::take(&mut *ASKED.lock().unwrap());
    for asked in asked {
        let what = asked.str("do").to_string();
        match what.as_str() {
            "windows" => {
                let list = window_list(state, windows);
                asked.answer(json!({ "windows": list }));
            }
            "apps" | "open_app" if shell.is_none() => asked.refuse("this room has no applications to open"),
            "apps" => {
                let shell = shell.unwrap();
                let own: Vec<&str> = shell.own_apps().iter().map(|a| a.name.as_str()).collect();
                let others: Vec<&str> = shell.launcher().apps().iter().map(|a| a.name.as_str()).collect();
                asked.answer(json!({ "spatiand": own, "installed": others }));
            }
            "open_app" => {
                let shell = shell.unwrap();
                let name = asked.str("name").to_lowercase();
                let own = shell.own_apps().iter().find(|a| a.name.to_lowercase() == name);
                let other = shell.launcher().apps().iter().find(|a| a.name.to_lowercase() == name);
                let near = || shell.own_apps().iter().chain(shell.launcher().apps()).find(|a| a.name.to_lowercase().contains(&name));
                match (own, other) {
                    (Some(app), _) => {
                        events.push(ShellEvent::LaunchOwn(app.clone()));
                        asked.answer(json!({ "opened": app.name }));
                    }
                    (None, Some(app)) => {
                        events.push(ShellEvent::Launch(app.clone()));
                        asked.answer(json!({ "opened": app.name }));
                    }
                    _ => match near().filter(|_| !name.is_empty()) {
                        Some(app) if shell.own_apps().iter().any(|o| o.name == app.name) => {
                            events.push(ShellEvent::LaunchOwn(app.clone()));
                            asked.answer(json!({ "opened": app.name }));
                        }
                        Some(app) => {
                            events.push(ShellEvent::Launch(app.clone()));
                            asked.answer(json!({ "opened": app.name }));
                        }
                        None => asked.refuse("no application by that name"),
                    },
                }
            }
            "focus" | "close" | "hide" | "show" | "pin" | "unpin" => {
                let Some(id) = window_of(state, &asked) else {
                    asked.refuse("no such window");
                    continue;
                };
                events.push(match what.as_str() {
                    "focus" => ShellEvent::FocusWindow(id),
                    "close" => ShellEvent::CloseWindow(id),
                    "hide" => ShellEvent::HideWindow { id, hidden: true },
                    "show" => ShellEvent::HideWindow { id, hidden: false },
                    "pin" => ShellEvent::PinWindow { id, pinned: true },
                    _ => ShellEvent::PinWindow { id, pinned: false },
                });
                asked.answer(json!({ "done": what, "window": id }));
            }
            "recentre" => {
                events.push(ShellEvent::Hud(spatiand_shell::HudAction::Recentre));
                asked.answer(json!({ "done": "recentre" }));
            }
            // The room's own controls, as the wearer's menu has them.
            "control" => {
                use spatiand_shell::HudAction as H;
                let action = match asked.str("action") {
                    "keyboard" => Some(H::ToggleKeyboard),
                    "pinned_corner" => Some(H::PipCorner),
                    "pinned_size" => Some(H::PipSize),
                    "look" => Some(H::Look),
                    "screenshot" => Some(H::Screenshot),
                    "record" => Some(H::Record),
                    "windows" => Some(H::OpenSwitcher),
                    "wifi" => Some(H::OpenSystemSettings("wifi")),
                    "bluetooth" => Some(H::OpenSystemSettings("bluetooth")),
                    _ => None,
                };
                match action {
                    Some(action) => {
                        events.push(ShellEvent::Hud(action));
                        let done = asked.str("action").to_string();
                        asked.answer(json!({ "done": done }));
                    }
                    None => asked.refuse("not a control the room has"),
                }
            }
            "describe" => {
                let Some(id) = window_of(state, &asked) else {
                    asked.refuse("no such window");
                    continue;
                };
                describe(state, id, asked);
            }
            "picture" => {
                let Some(id) = window_of(state, &asked) else {
                    asked.refuse("no such window");
                    continue;
                };
                let largest = asked.number("largest").unwrap_or(1280.0).clamp(256.0, 2048.0) as u32;
                match windows.iter().find(|q| state.layout.id_of(&q.window) == Some(id)) {
                    Some(quad) => match picture(renderer, quad, largest) {
                        Ok((png, w, h)) => asked.answer(json!({ "png": spatiand_voice::speech::encode_base64(&png), "width": w, "height": h })),
                        Err(why) => asked.refuse(&why),
                    },
                    None => asked.refuse("that window is not on show: show it first"),
                }
            }
            "click" | "move" | "scroll" | "drag" => {
                let Some(id) = window_of(state, &asked) else {
                    asked.refuse("no such window");
                    continue;
                };
                let Some(quad) = windows.iter().find(|q| state.layout.id_of(&q.window) == Some(id)) else {
                    asked.refuse("that window is not on show: show it first");
                    continue;
                };
                let at = (asked.number("x").unwrap_or(0.5), asked.number("y").unwrap_or(0.5));
                let (window, surface, size) = (quad.window.clone(), quad.surface.clone(), quad.pixels);
                state.focus_window(&window);
                match what.as_str() {
                    "move" => point(state, &surface, size, at, None, time_ms),
                    "click" => {
                        let button = match asked.str("button") {
                            "right" => crate::pointer::BTN_RIGHT,
                            "middle" => 0x112,
                            _ => crate::pointer::BTN_LEFT,
                        };
                        let times = asked.number("count").unwrap_or(1.0).clamp(1.0, 3.0) as u32;
                        point(state, &surface, size, at, None, time_ms);
                        for _ in 0..times {
                            point(state, &surface, size, at, Some((button, true)), time_ms);
                            point(state, &surface, size, at, Some((button, false)), time_ms);
                        }
                    }
                    "drag" => {
                        let to = (asked.number("to_x").unwrap_or(at.0), asked.number("to_y").unwrap_or(at.1));
                        point(state, &surface, size, at, Some((crate::pointer::BTN_LEFT, true)), time_ms);
                        for step in 1..=8 {
                            let f = step as f64 / 8.0;
                            point(state, &surface, size, (at.0 + (to.0 - at.0) * f, at.1 + (to.1 - at.1) * f), None, time_ms);
                        }
                        point(state, &surface, size, to, Some((crate::pointer::BTN_LEFT, false)), time_ms);
                    }
                    _ => {
                        point(state, &surface, size, at, None, time_ms);
                        scroll(state, asked.number("dx").unwrap_or(0.0), asked.number("dy").unwrap_or(0.0), time_ms);
                    }
                }
                asked.answer(json!({ "done": what, "window": id }));
            }
            "type" | "key" => {
                if let Some(id) = window_of(state, &asked) {
                    let window = state.space.elements().find(|w| state.layout.id_of(w) == Some(id)).cloned();
                    if let Some(window) = window {
                        state.focus_window(&window);
                    }
                }
                if what == "type" {
                    let text = crate::dictate::typeable(asked.str("text"));
                    for c in text.chars() {
                        if let Some(stroke) = spatiand_shell::keyboard::stroke_for(c) {
                            stroke_keys(state, stroke.code, stroke.shift, false, false, false, time_ms);
                        }
                    }
                    asked.answer(json!({ "done": "type" }));
                } else {
                    match keys_of(asked.str("keys")) {
                        Some((code, shift, ctrl, alt, logo)) => {
                            stroke_keys(state, code, shift, ctrl, alt, logo, time_ms);
                            asked.answer(json!({ "done": "key" }));
                        }
                        None => asked.refuse("not keys this keyboard has"),
                    }
                }
            }
            "notice" => {
                *NOTICE.lock().unwrap() = Some((asked.str("text").to_string(), std::time::Instant::now()));
                asked.answer(json!({ "done": "notice" }));
            }
            "focused" => {
                let id = state.layout.focused_id();
                asked.answer(json!({ "window": id }));
            }
            // The window an action with no window named would be done to.
            "target" => {
                let id = window_of(state, &asked);
                asked.answer(json!({ "window": id }));
            }
            // Its figure: which, what it is doing, whether it is shown. Drawn by the scene
            // (`crate::avatar`) from this.
            // What it says is laid over what it said: a mood leaves the figure chosen as it was,
            // and `wave` counts up, each one a wave.
            "avatar" => {
                let mut figure = FIGURE.lock().unwrap();
                let mut now = figure.take().filter(Value::is_object).unwrap_or_else(|| json!({}));
                if let Some(said) = asked.what.as_object() {
                    for (key, value) in said {
                        match key.as_str() {
                            "do" => {}
                            "wave" if value.as_bool() == Some(true) => {
                                now["waves"] = json!(now.get("waves").and_then(Value::as_u64).unwrap_or(0) + 1);
                            }
                            _ => now[key.as_str()] = value.clone(),
                        }
                    }
                }
                *figure = Some(now);
                asked.answer(json!({ "done": "avatar" }));
            }
            other => asked.refuse(&format!("the room does not know how to {other:?}")),
        }
    }
    events
}

/// **The assistant runs when it is switched on** (`assistant.json` in the settings every machine
/// shares, `"on": true`): started with the session, in the background -- its avatar, its ears
/// and its reminders need no window -- and started again if it goes. Looked at every few seconds.
///
/// Started as the wearer would start it, through the shell, so that each backend gives it what it
/// gives the assistant: its key, and on the Deck the sink its voice is placed by.
fn keep_running(_state: &Spatiand, shell: &spatiand_shell::Shell) -> Option<ShellEvent> {
    static LOOKED: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    static STARTED: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    {
        let mut looked = LOOKED.lock().unwrap();
        if looked.is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(5)) {
            return None;
        }
        *looked = Some(std::time::Instant::now());
    }
    if present() || !switched_on() {
        return None;
    }
    // Given time to start and say its key before it is started again.
    let mut started = STARTED.lock().unwrap();
    if started.is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(20)) {
        return None;
    }
    let app = shell.own_apps().iter().find(|a| crate::own::is_assistant(a))?;
    log::info!("assist: starting the assistant, which is switched on");
    *started = Some(std::time::Instant::now());
    let mut background = app.clone();
    background.exec = format!("{} --background", app.exec);
    Some(ShellEvent::LaunchOwn(background))
}

/// Whether the wearer has switched the assistant on.
pub fn switched_on() -> bool {
    let path = spatiand_sync::default_folders().0.join("assistant.json");
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("on").and_then(Value::as_bool))
        .unwrap_or(false)
}

/// The assistant's figure, as it last asked for it.
pub static FIGURE: Mutex<Option<Value>> = Mutex::new(None);

/// What the assistant asked the room to show on its notice card, and since when.
static NOTICE: Mutex<Option<(String, std::time::Instant)>> = Mutex::new(None);

/// The assistant's card, if it has one up: for a minute, or until pressed.
pub fn notice() -> String {
    let mut n = NOTICE.lock().unwrap();
    match n.as_ref() {
        Some((text, at)) if at.elapsed() < std::time::Duration::from_secs(60) => text.clone(),
        Some(_) => {
            *n = None;
            String::new()
        }
        None => String::new(),
    }
}

/// The card was pressed: put away, and the assistant told.
pub fn notice_pressed() -> bool {
    let had = NOTICE.lock().unwrap().take().is_some();
    if had {
        tell(json!({ "notice_pressed": true }));
    }
    had
}

/// The windows in the room, as the assistant is told them.
fn window_list(state: &Spatiand, windows: &[crate::scene::WindowQuad]) -> Vec<Value> {
    let focused = state.layout.focused_id();
    state
        .space
        .elements()
        .filter_map(|w| {
            let id = state.layout.id_of(w)?;
            let size = windows.iter().find(|q| &q.window == w).map(|q| q.pixels);
            Some(json!({
                "window": id,
                "app": state.app_id_of_id(id).unwrap_or_default(),
                "title": state.title_of(w).unwrap_or_default(),
                "focused": focused == Some(id),
                "hidden": state.layout.is_hidden(w),
                "pinned": state.layout.is_pinned(w),
                "own": is_own(w),
                "size": size.map(|(w, h)| json!([w, h])),
            }))
        })
        .collect()
}

/// The window a question is about: its `window`, else the one the wearer is working in -- the
/// focused one, unless that is the assistant's own (they have just been typing to it), and then
/// the one on top of the others.
fn window_of(state: &Spatiand, asked: &Asked) -> Option<usize> {
    if let Some(id) = asked.number("window").map(|n| n as usize) {
        return state.space.elements().any(|w| state.layout.id_of(w) == Some(id)).then_some(id);
    }
    let focused = state.space.elements().find(|w| state.layout.id_of(w) == state.layout.focused_id() && state.layout.focused_id().is_some());
    if let Some(w) = focused.filter(|w| !is_own(w)) {
        return state.layout.id_of(w);
    }
    // The space lists windows bottom to top: the last that is not the assistant's.
    state.space.elements().filter(|w| !is_own(w) && !state.layout.is_hidden(w)).last().and_then(|w| state.layout.id_of(w))
}

/// A window's own account of itself, answered when it comes (or with what the room knows, if it
/// does not come in time).
fn describe(state: &Spatiand, id: usize, asked: Asked) {
    let window = state.space.elements().find(|w| state.layout.id_of(w) == Some(id)).cloned();
    let app = state.app_id_of_id(id).unwrap_or_default();
    let title = window.as_ref().and_then(|w| state.title_of(w)).unwrap_or_default();
    let own = window.as_ref().and_then(|w| w.toplevel()).and_then(|t| crate::xr::ask_description(t.wl_surface()));
    #[cfg(target_os = "macos")]
    let own = own.or_else(|| crate::mac::describe_window(&app));
    let basic = json!({ "app": app, "title": title });
    match own {
        Some(answer) => {
            std::thread::spawn(move || {
                let said = answer.recv_timeout(std::time::Duration::from_secs(2)).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok());
                let mut said = said.filter(Value::is_object).unwrap_or_else(|| json!({}));
                for key in ["app", "title"] {
                    if said.get(key).and_then(Value::as_str).unwrap_or_default().is_empty() {
                        said[key] = basic[key].clone();
                    }
                }
                asked.answer(said);
            });
        }
        None => asked.answer(basic),
    }
}

/// A window's picture as the wearer sees it, at most `largest` pixels across, as PNG.
fn picture(
    renderer: &mut smithay::backend::renderer::gles::GlesRenderer,
    quad: &crate::scene::WindowQuad,
    largest: u32,
) -> Result<(Vec<u8>, u32, u32), String> {
    use smithay::backend::renderer::gles::ffi;
    let (tw, th) = (quad.buffer.0 as i32, quad.buffer.1 as i32);
    let tex = quad.texture;
    let raw = renderer
        .with_context(|gl| unsafe {
            let mut fbo = 0;
            gl.GenFramebuffers(1, &mut fbo);
            gl.BindFramebuffer(ffi::FRAMEBUFFER, fbo);
            gl.FramebufferTexture2D(ffi::FRAMEBUFFER, ffi::COLOR_ATTACHMENT0, ffi::TEXTURE_2D, tex, 0);
            let mut raw = Vec::new();
            if gl.CheckFramebufferStatus(ffi::FRAMEBUFFER) == ffi::FRAMEBUFFER_COMPLETE && tw > 0 && th > 0 {
                raw = vec![0u8; (tw * th * 4) as usize];
                gl.ReadPixels(0, 0, tw, th, ffi::RGBA, ffi::UNSIGNED_BYTE, raw.as_mut_ptr() as *mut _);
            }
            gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
            gl.DeleteFramebuffers(1, &fbo);
            raw
        })
        .map_err(|e| format!("no GL context: {e}"))?;
    if raw.is_empty() {
        return Err("the window's picture could not be read".into());
    }
    let image = image::RgbaImage::from_raw(tw as u32, th as u32, raw).ok_or("the window's picture was the wrong size")?;
    // Only the part the window shows (a viewport may crop its buffer), and no wider than asked.
    let [u0, u1, v0, v1] = quad.crop;
    let (x, y) = ((u0 * tw as f32) as u32, (v0 * th as f32) as u32);
    let (w, h) = ((((u1 - u0) * tw as f32) as u32).max(1), (((v1 - v0) * th as f32) as u32).max(1));
    let image = image::imageops::crop_imm(&image, x, y, w.min(tw as u32 - x), h.min(th as u32 - y)).to_image();
    let image = if image.width() > largest {
        let h = (image.height() as u64 * largest as u64 / image.width() as u64).max(1) as u32;
        image::imageops::resize(&image, largest, h, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let (w, h) = image.dimensions();
    let rgb = image::DynamicImage::ImageRgba8(image).to_rgb8();
    let mut png = Vec::new();
    rgb.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok((png, w, h))
}

/// The pointer on a window at a fraction of it across and down, with a button going down or up.
fn point(
    state: &mut Spatiand,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    size: (u32, u32),
    at: (f64, f64),
    button: Option<(u32, bool)>,
    time_ms: u32,
) {
    use smithay::input::pointer::{ButtonEvent, MotionEvent};
    use smithay::utils::{Point, SERIAL_COUNTER};
    let Some(pointer) = state.seat.get_pointer() else { return };
    let location = Point::from((at.0.clamp(0.0, 1.0) * size.0 as f64, at.1.clamp(0.0, 1.0) * size.1 as f64));
    pointer.motion(state, Some((surface.clone(), Point::from((0.0, 0.0)))), &MotionEvent { location, serial: SERIAL_COUNTER.next_serial(), time: time_ms });
    pointer.frame(state);
    if let Some((button, pressed)) = button {
        let button_state = if pressed { smithay::backend::input::ButtonState::Pressed } else { smithay::backend::input::ButtonState::Released };
        pointer.button(state, &ButtonEvent { button, state: button_state, serial: SERIAL_COUNTER.next_serial(), time: time_ms });
        pointer.frame(state);
    }
}

/// Scroll where the pointer is, in pixels: down and right are positive.
fn scroll(state: &mut Spatiand, dx: f64, dy: f64, time_ms: u32) {
    use smithay::backend::input::{Axis, AxisSource};
    use smithay::input::pointer::AxisFrame;
    let Some(pointer) = state.seat.get_pointer() else { return };
    let mut frame = AxisFrame::new(time_ms).source(AxisSource::Finger);
    if dx != 0.0 {
        frame = frame.value(Axis::Horizontal, dx);
    }
    if dy != 0.0 {
        frame = frame.value(Axis::Vertical, dy);
    }
    pointer.axis(state, frame);
    pointer.frame(state);
}

/// A key, with modifiers held around it.
/// One key, with Shift held if `shift`: SpatiKey's typing (`crate::spatikey`).
pub fn stroke(state: &mut Spatiand, code: u32, shift: bool, time_ms: u32) {
    stroke_keys(state, code, shift, false, false, false, time_ms);
}

fn stroke_keys(state: &mut Spatiand, code: u32, shift: bool, ctrl: bool, alt: bool, logo: bool, time_ms: u32) {
    use spatiand_shell::keyboard as kb;
    let held: Vec<u32> = [(ctrl, kb::KEY_LEFTCTRL), (alt, kb::KEY_LEFTALT), (shift, kb::KEY_LEFTSHIFT), (logo, 125)]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, code)| *code)
        .collect();
    for c in &held {
        press(state, *c, true, time_ms);
    }
    press(state, code, true, time_ms);
    press(state, code, false, time_ms);
    for c in held.iter().rev() {
        press(state, *c, false, time_ms);
    }
}

fn press(state: &mut Spatiand, code: u32, pressed: bool, time_ms: u32) {
    use smithay::backend::input::KeyState;
    use smithay::input::keyboard::{FilterResult, Keycode};
    use smithay::utils::SERIAL_COUNTER;
    state.attention.stir();
    let Some(keyboard) = state.seat.get_keyboard() else { return };
    let key_state = if pressed { KeyState::Pressed } else { KeyState::Released };
    keyboard.input::<(), _>(state, Keycode::new(code + 8), key_state, SERIAL_COUNTER.next_serial(), time_ms, |_, _, _| FilterResult::Forward);
}

/// Keys as a person writes them -- `enter`, `ctrl+l`, `cmd+shift+t`, `pagedown`, `a` -- as an
/// evdev code and the modifiers held: (code, shift, ctrl, alt, logo).
fn keys_of(text: &str) -> Option<(u32, bool, bool, bool, bool)> {
    let (mut shift, mut ctrl, mut alt, mut logo) = (false, false, false, false);
    let mut code = None;
    for part in text.split('+').map(|p| p.trim().to_lowercase()) {
        match part.as_str() {
            "shift" => shift = true,
            "ctrl" | "control" => ctrl = true,
            "alt" | "option" | "opt" => alt = true,
            "cmd" | "command" | "super" | "logo" | "meta" | "win" => logo = true,
            name => {
                code = Some(match name {
                    "enter" | "return" => 28,
                    "escape" | "esc" => 1,
                    "tab" => 15,
                    "backspace" => 14,
                    "delete" | "del" => 111,
                    "space" => 57,
                    "up" => 103,
                    "down" => 108,
                    "left" => 105,
                    "right" => 106,
                    "home" => 102,
                    "end" => 107,
                    "pageup" => 104,
                    "pagedown" => 109,
                    f if f.starts_with('f') && f.len() <= 3 && f[1..].parse::<u32>().is_ok() => {
                        let n: u32 = f[1..].parse().ok()?;
                        match n {
                            1..=10 => 58 + n,
                            11 => 87,
                            12 => 88,
                            _ => return None,
                        }
                    }
                    one if one.chars().count() == 1 => {
                        let stroke = spatiand_shell::keyboard::stroke_for(one.chars().next()?)?;
                        shift |= stroke.shift;
                        stroke.code
                    }
                    _ => return None,
                })
            }
        }
    }
    Some((code?, shift, ctrl, alt, logo))
}

#[cfg(test)]
mod tests {
    use super::keys_of;

    #[test]
    fn keys_are_read_as_people_write_them() {
        assert_eq!(keys_of("enter"), Some((28, false, false, false, false)));
        assert_eq!(keys_of("ctrl+l"), Some((38, false, true, false, false)));
        assert_eq!(keys_of("cmd+shift+t"), Some((20, true, false, false, true)));
        assert_eq!(keys_of("f5"), Some((63, false, false, false, false)));
        assert_eq!(keys_of("?"), Some((53, true, false, false, false)));
        assert_eq!(keys_of("ctrl+nothing"), None);
    }
}
