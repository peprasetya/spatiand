//! The room's side of its assistant (`spatiand_assist_v1`; `docs/ai-integrations.md`).
//!
//! The assistant is one of Spatiand's own programs, and asks here what is in the room and does
//! what it was asked to there: lists the windows and the applications, sees what the wearer sees
//! (`seen`: the window looked at as it is shown, cropped to the part in view, and the view), reads
//! a window (its own account of itself, or a picture of it), points, clicks, scrolls and types on a window as the
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
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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

/// Whether a window is the assistant's own (its chat window): what its figure stands beside
/// (`crate::avatar`).
pub fn is_own(window: &smithay::desktop::Window) -> bool {
    let Some(assistant) = ASSISTANT.lock().unwrap().clone() else { return false };
    window.toplevel().and_then(|t| t.wl_surface().client()).is_some_and(|c| c.id() == assistant)
}

/// SpatiKey's application id: its windows show passwords, codes and the recovery code.
const SPATIKEY_APP_ID: &str = "id.prasetya.spatiand.spatikey";

/// **What the assistant never sees** -- no picture of it goes to the model server, nor its title,
/// nor its own account of itself, and a picture of the whole view is not taken while it is in
/// view: SpatiKey's windows (a password revealed, the recovery code), and SpatiMesh's window that
/// asks for a profile's PIN or password, or sets one (its titles, from Chromium patch 0012:
/// "<profile> is locked", "What opens <profile>"). By application id, here or from a host
/// (`remote.<host>.<id>`). (No client can mark a window of its own so yet: `spatiand_xr_v1` has
/// no such request; one would be the proper way for any other application with secrets.)
fn secret_by(app: &str, title: &str) -> bool {
    let is = |id: &str| app == id || app.ends_with(&format!(".{id}"));
    if is(SPATIKEY_APP_ID) {
        return true;
    }
    is(crate::audio::CHROMIUM_APP_ID) && (title.ends_with(" is locked") || title.starts_with("What opens "))
}

fn secret(state: &Spatiand, window: &smithay::desktop::Window) -> bool {
    secret_by(&state.app_id_of(window).unwrap_or_default(), &state.title_of(window).unwrap_or_default())
}

/// What the assistant is told instead of what it may not see.
const NOT_SEEN: &str = "that window is SpatiKey's or asks for a profile's PIN or password: it is never shown to the assistant";
const VIEW_NOT_SEEN: &str = "a password, a PIN or a code is on show in the view (SpatiKey, a locked profile, the Touch ID card or a pairing code): no picture of the view is taken while it is";

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
    SHOWN_FOR_TEST.load(Ordering::Relaxed) || LISTENERS.lock().unwrap().iter().any(|r| r.is_alive())
}

/// The figure shown as a test asked, with no assistant there (`SPATIAND_FIGURE` in the snapshot
/// room).
static SHOWN_FOR_TEST: AtomicBool = AtomicBool::new(false);

/// Show the figure as `figure` says, as if the assistant had asked for it; its `level` is how
/// loud the room is heard.
pub fn show_for_test(figure: Value) {
    if let Some(level) = figure.get("level").and_then(Value::as_f64) {
        HEARD_LEVEL.store((level as f32).to_bits(), Ordering::Relaxed);
    }
    *FIGURE.lock().unwrap() = Some(figure);
    SHOWN_FOR_TEST.store(true, Ordering::Relaxed);
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
/// How loud what the assistant hears is, 0 to 1 (an `f32`'s bits): its figure listens visibly.
static HEARD_LEVEL: AtomicU32 = AtomicU32::new(0);

/// How loud the microphone the assistant listens to is now: 0 for the room's quiet, 1 for a
/// voice raised close by. 0 while it is not listening.
pub fn heard_level() -> f32 {
    f32::from_bits(HEARD_LEVEL.load(Ordering::Relaxed))
}
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
        HEARD_LEVEL.store(0, Ordering::Relaxed);
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
            // A voice at arm's length is about -30 dBFS; a quiet room under -55.
            let energy = samples.iter().map(|&s| (s as f32 / 32768.0).powi(2)).sum::<f32>() / samples.len().max(1) as f32;
            let level = ((10.0 * (energy + 1e-10).log10() + 55.0) / 30.0).clamp(0.0, 1.0);
            HEARD_LEVEL.store(level.to_bits(), Ordering::Relaxed);
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
            HEARD_LEVEL.store(0, Ordering::Relaxed);
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
    sight: Option<&Sight>,
) -> Vec<ShellEvent> {
    tend_ears();
    tend_view_owed();
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
                if state.space.elements().any(|w| state.layout.id_of(w) == Some(id) && secret(state, w)) {
                    asked.refuse(NOT_SEEN);
                    continue;
                }
                describe(state, id, asked);
            }
            "seen" => {
                // The room's own cards that carry a secret: SpatiKey's (the Touch ID card), and
                // the hosts' page where a pairing code is typed.
                let card = !crate::spatikey::notice().is_empty() || shell.is_some_and(|s| s.mode() == spatiand_shell::Mode::Hosts);
                seen(state, windows, renderer, sight, card, asked)
            }
            "picture" => {
                let Some(id) = window_of(state, &asked) else {
                    asked.refuse("no such window");
                    continue;
                };
                if state.space.elements().any(|w| state.layout.id_of(w) == Some(id) && secret(state, w)) {
                    asked.refuse(NOT_SEEN);
                    continue;
                }
                let largest = asked.number("largest").unwrap_or(1280.0).clamp(256.0, 2048.0) as u32;
                match windows.iter().find(|q| state.layout.id_of(&q.window) == Some(id)) {
                    Some(quad) => match picture(renderer, quad, largest, [0.0, 1.0, 0.0, 1.0]) {
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
            // What the wearer is looking at, when the room knows where they look: what they mean
            // by "this" is in front of them, focused or not. Else the one they work in.
            "target" => {
                let looked = asked.number("window").is_none().then(|| sight).flatten().and_then(|sight| {
                    let visible = seeable(state, windows);
                    let quads: Vec<spatiand_render::Quad> = visible
                        .iter()
                        .map(|&k| crate::scene::window_quad(&windows[k].placement, windows[k].pixels.0 as f64 / windows[k].pixels.1.max(1) as f64))
                        .collect();
                    let mut shown = on_show(&quads, sight);
                    for s in shown.iter_mut() {
                        s.quad = visible[s.quad];
                    }
                    looked_at(state, windows, &shown).filter(|&k| !is_own(&windows[k].window)).and_then(|k| state.layout.id_of(&windows[k].window))
                });
                let id = looked.or_else(|| window_of(state, &asked));
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
            // The inside of another window's place is shown as part of that one.
            if crate::embed::is_guest_window(state, w) {
                return None;
            }
            let id = state.layout.id_of(w)?;
            let size = windows.iter().find(|q| &q.window == w).map(|q| q.pixels);
            let private = secret(state, w);
            Some(json!({
                "window": id,
                "app": state.app_id_of_id(id).unwrap_or_default(),
                "title": if private { String::new() } else { state.title_of(w).unwrap_or_default() },
                "private": private,
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
/// focused one, unless that is the assistant's own (they have just been typing to it) or hidden,
/// and then the one on top of the others.
fn window_of(state: &Spatiand, asked: &Asked) -> Option<usize> {
    if let Some(id) = asked.number("window").map(|n| n as usize) {
        return state.space.elements().any(|w| state.layout.id_of(w) == Some(id)).then_some(id);
    }
    let focused = state.space.elements().find(|w| state.layout.id_of(w) == state.layout.focused_id() && state.layout.focused_id().is_some());
    if let Some(w) = focused.filter(|w| !is_own(w) && !state.layout.is_hidden(w)) {
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

/// A window's picture as the wearer sees it, at most `largest` pixels across, as PNG: the part
/// `part` of it (`[u0, u1, v0, v1]`, fractions from its top left).
fn picture(
    renderer: &mut smithay::backend::renderer::gles::GlesRenderer,
    quad: &crate::scene::WindowQuad,
    largest: u32,
    part: [f32; 4],
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
    // Only the part the window shows (a viewport may crop its buffer) -- and of that, the part
    // asked for -- no wider than asked.
    let [c0, c1, d0, d1] = quad.crop;
    let [p0, p1, q0, q1] = part;
    let (u0, u1) = (c0 + (c1 - c0) * p0, c0 + (c1 - c0) * p1);
    let (v0, v1) = (d0 + (d1 - d0) * q0, d0 + (d1 - d0) * q1);
    let (x, y) = (((u0 * tw as f32) as u32).min(tw as u32 - 1), ((v0 * th as f32) as u32).min(th as u32 - 1));
    let (w, h) = ((((u1 - u0) * tw as f32) as u32).max(1), (((v1 - v0) * th as f32) as u32).max(1));
    let image = image::imageops::crop_imm(&image, x, y, w.min(tw as u32 - x), h.min(th as u32 - y)).to_image();
    encode(image, largest)
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

// --- what the wearer sees ---

/// Where the wearer is looking from: the head as the frame is drawn, and the eyes' field. The
/// backend hands it to [`serve`] so that "what do you see" is answered with what is in front of
/// them, not with everything a window holds.
#[derive(Clone, Copy, Debug)]
pub struct Sight {
    pub orientation: glam::DQuat,
    pub stereo: spatiand_render::StereoConfig,
}

/// One window as the wearer sees it now.
#[derive(Clone, Debug, PartialEq)]
pub struct OnShow {
    /// Its index among the windows drawn.
    pub quad: usize,
    /// The part of it inside the view, `[u0, u1, v0, v1]`: fractions of the window, from its top
    /// left. `[0, 1, 0, 1]` when all of it is in view.
    pub part: [f32; 4],
    /// How much of the view it fills, not hidden behind another window: 0 to 1.
    pub share: f32,
    /// It is what the middle of the view falls on.
    pub centre: bool,
}

/// The windows in view, from the left eye: the part of each inside its field (a grid of points
/// on the window, each projected), and how much of the view each fills in front of the others
/// (a grid of rays from the eye, each finding the nearest window). Hidden windows are not drawn,
/// so are not here.
pub fn on_show(quads: &[spatiand_render::Quad], sight: &Sight) -> Vec<OnShow> {
    use glam::{DVec3, Vec4};
    let eye = spatiand_render::eye_for(spatiand_render::EyeSide::Left, sight.orientation, DVec3::ZERO, &sight.stereo);
    let vp = eye.view_projection();
    let (tx, ty) = ((sight.stereo.h_fov_deg / 2.0).to_radians().tan(), (sight.stereo.v_fov_deg() / 2.0).to_radians().tan());
    // How much of the view each fills: rays across it.
    let (across, down) = (64usize, 36usize);
    let mut hits = vec![0usize; quads.len()];
    let mut centre = None;
    for j in 0..=down {
        for i in 0..=across {
            let (x, y) = (2.0 * i as f64 / across as f64 - 1.0, 1.0 - 2.0 * j as f64 / down as f64);
            let direction = (sight.orientation * DVec3::new(1.0, -x * tx, y * ty)).normalize();
            let ray = spatiand_render::Ray { origin: eye.position, direction };
            if let Some((k, _)) = spatiand_render::pick(&ray, quads) {
                hits[k] += 1;
                if i == across / 2 && j == down / 2 {
                    centre = Some(k);
                }
            }
        }
    }
    let total = ((across + 1) * (down + 1)) as f32;
    let mut out = Vec::new();
    for (k, quad) in quads.iter().enumerate() {
        // The part inside the field: points on the window that project inside it.
        let n = 48;
        let (mut u0, mut u1, mut v0, mut v1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for j in 0..=n {
            for i in 0..=n {
                let (u, v) = (i as f64 / n as f64, j as f64 / n as f64);
                let p = quad.centre + quad.orientation * DVec3::new(0.0, (0.5 - u) * quad.width, (0.5 - v) * quad.height);
                let clip = vp * Vec4::new(p.x as f32, p.y as f32, p.z as f32, 1.0);
                if clip.w > 0.0 && (clip.x / clip.w).abs() <= 1.0 && (clip.y / clip.w).abs() <= 1.0 {
                    u0 = u0.min(u as f32);
                    u1 = u1.max(u as f32);
                    v0 = v0.min(v as f32);
                    v1 = v1.max(v as f32);
                }
            }
        }
        if u0 > u1 || hits[k] == 0 && centre != Some(k) {
            continue;
        }
        // Out to the edge of the field, half a step past the last point inside it.
        let half = 0.5 / n as f32;
        let widen = |a: f32, inward: bool| if inward { (a - half).max(0.0) } else { (a + half).min(1.0) };
        let part = [if u0 > 0.0 { widen(u0, true) } else { 0.0 }, if u1 < 1.0 { widen(u1, false) } else { 1.0 }, if v0 > 0.0 { widen(v0, true) } else { 0.0 }, if v1 < 1.0 { widen(v1, false) } else { 1.0 }];
        out.push(OnShow { quad: k, part, share: hits[k] as f32 / total, centre: centre == Some(k) });
    }
    out
}

/// The windows that count for what the wearer sees: those on show, **without the assistant's own**
/// -- its chat window is looked through, not at: asked "what do I see?" while it is in front of a
/// page (it opens where the wearer looks), the page is what is meant. Its own window only when
/// there is no other. Never one it may not see ([`secret_by`]).
fn seeable(state: &Spatiand, windows: &[crate::scene::WindowQuad]) -> Vec<usize> {
    let shown: Vec<usize> =
        (0..windows.len()).filter(|&k| !state.layout.is_hidden(&windows[k].window) && !secret(state, &windows[k].window)).collect();
    let theirs: Vec<usize> = shown.iter().copied().filter(|&k| !is_own(&windows[k].window)).collect();
    if theirs.is_empty() {
        shown
    } else {
        theirs
    }
}

/// The window the wearer is looking at, among those on show: the one in the middle of the view;
/// else the focused one, if it is in view; else the one filling the most of it. Never the
/// assistant's own chat window unless it is the only one.
fn looked_at(state: &Spatiand, windows: &[crate::scene::WindowQuad], shown: &[OnShow]) -> Option<usize> {
    let theirs: Vec<&OnShow> = shown.iter().filter(|s| !is_own(&windows[s.quad].window)).collect();
    let pick = |list: &[&OnShow]| -> Option<usize> {
        if let Some(c) = list.iter().find(|s| s.centre) {
            return Some(c.quad);
        }
        let focused = state.layout.focused_id();
        if let Some(f) = list.iter().find(|s| focused.is_some() && state.layout.id_of(&windows[s.quad].window) == focused) {
            return Some(f.quad);
        }
        list.iter().max_by(|a, b| a.share.partial_cmp(&b.share).unwrap_or(std::cmp::Ordering::Equal)).map(|s| s.quad)
    };
    pick(&theirs).or_else(|| pick(&shown.iter().collect::<Vec<_>>()))
}

/// `{"do": "seen"}`: **what the wearer sees** -- the window they are looking at as it is shown
/// (its picture cropped to the part in view, at whatever zoom it is drawn at), which part that is,
/// how much of the view it fills, and the other windows in view. With `"window"`, that window's
/// part in view instead (all of it, said as not on show, if it is out of view). With
/// `"view": true`, also the view itself as the left eye was last shown it, every window in it
/// (`view_png`) -- for a question about more than one window. `"picture": false` for the account
/// without pictures. The same in every room, for every kind of window: the room's own picture,
/// not the application's account of itself (that is `describe`).
///
/// What it may not see ([`secret_by`]) is neither looked at nor among the others; named, it is
/// said to be private, with no picture and no title. While one is in view, or a card of the
/// room's that carries a secret is up (`secret_card`), the view is not given.
fn seen(
    state: &Spatiand,
    windows: &[crate::scene::WindowQuad],
    renderer: &mut smithay::backend::renderer::gles::GlesRenderer,
    sight: Option<&Sight>,
    secret_card: bool,
    asked: Asked,
) {
    let largest = asked.number("largest").unwrap_or(1280.0).clamp(256.0, 2048.0) as u32;
    let visible = seeable(state, windows);
    let quads: Vec<spatiand_render::Quad> = visible
        .iter()
        .map(|&k| crate::scene::window_quad(&windows[k].placement, windows[k].pixels.0 as f64 / windows[k].pixels.1.max(1) as f64))
        .collect();
    let mut shown = match sight {
        Some(sight) => on_show(&quads, sight),
        None => Vec::new(),
    };
    for s in shown.iter_mut() {
        s.quad = visible[s.quad];
    }
    // No head to look from (a flat room, a desktop), or nothing found in view: every window is
    // seen whole, and the one worked in is the one looked at.
    let flat = shown.is_empty();
    if flat {
        shown = visible.iter().map(|&k| OnShow { quad: k, part: [0.0, 1.0, 0.0, 1.0], share: 0.0, centre: false }).collect();
    }
    let named = asked.number("window").map(|n| n as usize);
    let quad_of = |id: usize| windows.iter().position(|q| state.layout.id_of(&q.window) == Some(id));
    let target = match named {
        Some(id) => quad_of(id),
        None if flat => window_of(state, &asked).and_then(quad_of),
        None => looked_at(state, windows, &shown),
    };
    let describe = |k: usize, s: Option<&OnShow>| {
        let w = &windows[k];
        let id = state.layout.id_of(&w.window);
        let private = secret(state, &w.window);
        json!({
            "window": id,
            "app": id.and_then(|id| state.app_id_of_id(id)).unwrap_or_default(),
            "title": if private { String::new() } else { state.title_of(&w.window).unwrap_or_default() },
            "private": private,
            "own": is_own(&w.window),
            "in_view": s.is_some(),
            "part": s.map(|s| s.part),
            "share": s.map(|s| (s.share * 100.0).round() / 100.0),
        })
    };
    let mut answer = json!({ "flat": flat });
    if let Some(k) = target {
        let s = shown.iter().find(|s| s.quad == k);
        let mut about = describe(k, s);
        let part = s.map_or([0.0, 1.0, 0.0, 1.0], |s| s.part);
        about["whole"] = json!(part == [0.0, 1.0, 0.0, 1.0]);
        if secret(state, &windows[k].window) {
            about["picture_error"] = json!(NOT_SEEN);
        } else if asked.get("picture").and_then(Value::as_bool) != Some(false) {
            match picture(renderer, &windows[k], largest, part) {
                Ok((png, w, h)) => {
                    about["png"] = json!(spatiand_voice::speech::encode_base64(&png));
                    about["width"] = json!(w);
                    about["height"] = json!(h);
                }
                Err(why) => about["picture_error"] = json!(why),
            }
        }
        answer["looking_at"] = about;
    }
    answer["others"] = json!(shown.iter().filter(|s| Some(s.quad) != target).map(|s| describe(s.quad, Some(s))).collect::<Vec<_>>());
    if asked.get("view").and_then(Value::as_bool) == Some(true) {
        // Any window it may not see that is in view, by the same rays as the rest.
        let hidden_in_view = sight.is_some_and(|sight| {
            let private: Vec<usize> =
                (0..windows.len()).filter(|&k| !state.layout.is_hidden(&windows[k].window) && secret(state, &windows[k].window)).collect();
            let quads: Vec<spatiand_render::Quad> = private
                .iter()
                .map(|&k| crate::scene::window_quad(&windows[k].placement, windows[k].pixels.0 as f64 / windows[k].pixels.1.max(1) as f64))
                .collect();
            !quads.is_empty() && !on_show(&quads, sight).is_empty()
        });
        if hidden_in_view || secret_card {
            answer["view_error"] = json!(VIEW_NOT_SEEN);
            asked.answer(answer);
            return;
        }
        if sight.is_some() {
            // Answered once the frame being drawn is: what the eye was shown.
            VIEW_OWED.lock().unwrap().push((asked, answer, largest, std::time::Instant::now()));
            return;
        }
        answer["view_error"] = json!("this room draws no view of its own");
    }
    asked.answer(answer);
}

/// `seen` questions that want the view, waiting for the frame being drawn.
static VIEW_OWED: Mutex<Vec<(Asked, Value, u32, std::time::Instant)>> = Mutex::new(Vec::new());

/// Whether the assistant is waiting for a picture of the view: the backend reads the frame back
/// for it ([`view_read`]) only then.
pub fn view_wanted() -> bool {
    !VIEW_OWED.lock().unwrap().is_empty()
}

/// The frame has been drawn into `fbo`, the left eye in `eye` (`x, y, width, height`): what
/// asked for the view is answered with it, read back as the screenshot is -- exactly what was on
/// the glass. Called by the backend after it draws; what waits more than half a second (no frame
/// drawn: the glasses off) is answered without it.
pub fn frame_drawn(renderer: &mut smithay::backend::renderer::gles::GlesRenderer, fbo: u32, eye: (i32, i32, i32, i32)) {
    if !view_wanted() {
        return;
    }
    use smithay::backend::renderer::gles::ffi;
    let (x, y, w, h) = eye;
    let raw = renderer
        .with_context(|gl| unsafe {
            let mut raw = vec![0u8; (w.max(0) * h.max(0) * 4) as usize];
            gl.BindFramebuffer(ffi::FRAMEBUFFER, fbo);
            gl.ReadPixels(x, y, w, h, ffi::RGBA, ffi::UNSIGNED_BYTE, raw.as_mut_ptr() as *mut _);
            gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
            raw
        })
        .unwrap_or_default();
    // The scene is drawn flipped into this framebuffer, so read top row first it is the right way up.
    view_read(raw, w as u32, h as u32, false);
}

/// The left eye's picture, read back by the backend (`bottom_up` when its rows came bottom
/// first, as from a window's own framebuffer): what asked for the view is answered with it.
pub fn view_read(raw: Vec<u8>, w: u32, h: u32, bottom_up: bool) {
    let owed: Vec<_> = std::mem::take(&mut *VIEW_OWED.lock().unwrap());
    if owed.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        let raw = if bottom_up && w > 0 { raw.chunks((w * 4) as usize).rev().flatten().copied().collect() } else { raw };
        let image = image::RgbaImage::from_raw(w, h, raw);
        for (asked, mut answer, largest, _) in owed {
            match image.as_ref().map(|i| encode(i.clone(), largest)) {
                Some(Ok((png, pw, ph))) => {
                    answer["view_png"] = json!(spatiand_voice::speech::encode_base64(&png));
                    answer["view_width"] = json!(pw);
                    answer["view_height"] = json!(ph);
                }
                Some(Err(why)) => answer["view_error"] = json!(why),
                None => answer["view_error"] = json!("the view could not be read back"),
            }
            asked.answer(answer);
        }
    });
}

/// What waited too long for a frame: answered without the view.
fn tend_view_owed() {
    let mut owed = VIEW_OWED.lock().unwrap();
    let (late, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut *owed).into_iter().partition(|(_, _, _, at)| at.elapsed() > std::time::Duration::from_millis(500));
    *owed = waiting;
    drop(owed);
    for (asked, mut answer, _, _) in late {
        answer["view_error"] = json!("no frame was drawn to see the view in");
        asked.answer(answer);
    }
}

/// A picture scaled to at most `largest` across, as PNG.
fn encode(image: image::RgbaImage, largest: u32) -> Result<(Vec<u8>, u32, u32), String> {
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

#[cfg(test)]
mod tests {
    use super::{keys_of, on_show, secret_by, Sight};

    #[test]
    fn spatikey_and_a_profiles_lock_are_never_seen() {
        assert!(secret_by("id.prasetya.spatiand.spatikey", "SpatiKey"));
        assert!(secret_by("id.prasetya.spatiand.spatikey", ""));
        assert!(secret_by("remote.littleotter.id.prasetya.spatiand.spatikey", "SpatiKey"), "from a host too");
        assert!(secret_by("spatimesh-chromium", "Work is locked"));
        assert!(secret_by("spatimesh-chromium", "What opens Work"));
        assert!(secret_by("remote.deck.spatimesh-chromium", "Work is locked"));
        assert!(!secret_by("spatimesh-chromium", "Example Domain"), "a page is seen");
        assert!(!secret_by("org.gnome.TextEditor", "Work is locked"), "only SpatiMesh's lock");
        assert!(!secret_by("id.prasetya.spatiand.spatikeyboard", "x"));
    }
    use glam::{DQuat, DVec3};

    fn window(yaw_deg: f64, pitch_deg: f64, radius: f64, width: f64, aspect: f64) -> spatiand_render::Quad {
        let placement = crate::window::Placement { yaw: yaw_deg.to_radians(), pitch: pitch_deg.to_radians(), radius, width, ..Default::default() };
        crate::scene::window_quad(&placement, aspect)
    }

    fn ahead() -> Sight {
        Sight { orientation: DQuat::IDENTITY, stereo: spatiand_render::StereoConfig::default() }
    }

    #[test]
    fn a_window_straight_ahead_and_in_view_is_seen_whole() {
        let shown = on_show(&[window(0.0, 0.0, 2.2, 1.1, 1.6)], &ahead());
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].part, [0.0, 1.0, 0.0, 1.0]);
        assert!(shown[0].centre && shown[0].share > 0.3 && shown[0].share < 0.9, "{shown:?}");
    }

    #[test]
    fn a_window_too_big_for_the_view_is_seen_in_part() {
        // Brought close, or made huge: wider and taller than the field; only its middle is seen.
        let shown = on_show(&[window(0.0, 0.0, 1.0, 3.0, 1.6)], &ahead());
        let [u0, u1, v0, v1] = shown[0].part;
        assert!(u0 > 0.2 && u1 < 0.8 && v0 > 0.1 && v1 < 0.9, "{:?}", shown[0].part);
        // Near the middle: the left eye sits a little left of the head and its neck above it.
        assert!((u0 + u1 - 1.0).abs() < 0.05 && (v0 + v1 - 1.0).abs() < 0.12, "centred: {:?}", shown[0].part);
        assert!(shown[0].share > 0.95);
    }

    #[test]
    fn turned_away_the_window_leaves_the_view_by_its_side() {
        // A window to the left (+yaw), the head turned so that only its right part is in view.
        let quads = [window(25.0, 0.0, 2.2, 1.1, 1.6), window(0.0, 0.0, 2.2, 1.1, 1.6)];
        let sight = Sight { orientation: DQuat::from_axis_angle(DVec3::Z, (-5f64).to_radians()), ..ahead() };
        let shown = on_show(&quads, &sight);
        let left = shown.iter().find(|s| s.quad == 0).expect("the window to the left, in part");
        assert!(left.part[0] > 0.3 && left.part[1] == 1.0, "{:?}", left.part);
        assert!(!left.centre);
        assert!(shown.iter().find(|s| s.quad == 1).is_some_and(|s| s.centre));
        // Turned right round, neither is seen.
        let behind = Sight { orientation: DQuat::from_axis_angle(DVec3::Z, std::f64::consts::PI), ..ahead() };
        assert!(on_show(&quads, &behind).is_empty());
    }

    #[test]
    fn a_window_in_front_hides_the_one_behind_it() {
        let quads = [window(0.0, 0.0, 3.0, 1.2, 1.6), window(0.0, 0.0, 2.0, 1.2, 1.6)];
        let shown = on_show(&quads, &ahead());
        let far = shown.iter().find(|s| s.quad == 0);
        let near = shown.iter().find(|s| s.quad == 1).unwrap();
        assert!(near.centre && far.is_none_or(|f| f.share < near.share / 3.0), "{shown:?}");
    }

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
