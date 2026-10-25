//! SpatiMesh's engine interface over a native engine's C half.
//!
//! Each engine program (`spatimesh-wpe`, `spatimesh-webkitgtk`) compiles a small native shim --
//! C against WPE's headers, or WebKitGTK's -- that provides the same few `spatimesh_*`
//! functions, and links it with this crate,
//! which is the [`Engine`] over them. The engine's own structures are only ever written in its
//! own language, against its own headers.

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::sync::Mutex;
use std::time::Duration;

use spatimesh::extensions::dnr;
use spatimesh::extensions::set::{self, ExtensionSet, Script};

use spatimesh::engine::{
    Button, Canvas, Choice, Cookie, Cursor, DialogKind, Engine, Format, Happened, KeyPress, Modifiers, ProfileSetup, Rect, TabId, View,
};

#[repr(C)]
struct Raw {
    _private: [u8; 0],
}

/// `MeshEvent` in the shim.
#[repr(C)]
struct Event {
    kind: c_int,
    tab: u64,
    number: f64,
    a: c_int,
    b: c_int,
    text: *const c_char,
    text2: *const c_char,
    x: c_int,
    y: c_int,
    w: c_int,
    h: c_int,
}

extern "C" {
    fn spatimesh_version() -> *const c_char;
    fn spatimesh_new(data: *const c_char, cache: *const c_char) -> *mut Raw;
    fn spatimesh_watch(e: *mut Raw, fd: c_int);
    fn spatimesh_open(e: *mut Raw, id: u64, uri: *const c_char, width: c_int, height: c_int, zoom: f64) -> c_int;
    fn spatimesh_close(e: *mut Raw, id: u64);
    fn spatimesh_load(e: *mut Raw, id: u64, uri: *const c_char);
    fn spatimesh_back(e: *mut Raw, id: u64);
    fn spatimesh_forward(e: *mut Raw, id: u64);
    fn spatimesh_reload(e: *mut Raw, id: u64);
    fn spatimesh_stop(e: *mut Raw, id: u64);
    fn spatimesh_resize(e: *mut Raw, id: u64, width: c_int, height: c_int, zoom: f64);
    fn spatimesh_mute(e: *mut Raw, id: u64, muted: c_int);
    fn spatimesh_set_window(e: *mut Raw, id: u64, window: u32);
    fn spatimesh_pointer(e: *mut Raw, id: u64, x: f64, y: f64, button: c_int, pressed: c_int, modifiers: c_int);
    fn spatimesh_leave(e: *mut Raw, id: u64);
    fn spatimesh_scroll(e: *mut Raw, id: u64, x: f64, y: f64, dx: f64, dy: f64, modifiers: c_int);
    fn spatimesh_key(e: *mut Raw, id: u64, key: c_int, ch: u32, code: c_int, pressed: c_int, modifiers: c_int, text: *const c_char);
    fn spatimesh_focus(e: *mut Raw, id: u64, focused: c_int);
    fn spatimesh_choice(e: *mut Raw, id: u64, index: c_int, flags: *mut c_int) -> *const c_char;
    fn spatimesh_choose(e: *mut Raw, id: u64, index: c_int);
    fn spatimesh_answer(e: *mut Raw, id: u64, accepted: c_int, text: *const c_char);
    fn spatimesh_pump(e: *mut Raw, timeout_ms: c_int);
    fn spatimesh_next_event(e: *mut Raw, out: *mut Event) -> c_int;
    fn spatimesh_damage(e: *mut Raw, id: u64, x: *mut c_int, y: *mut c_int, w: *mut c_int, h: *mut c_int) -> c_int;
    fn spatimesh_copy(e: *mut Raw, id: u64, pixels: *mut u8, width: c_int, height: c_int) -> c_int;
    // The newest picture already on the GPU, by its IOSurface's number, where the engine has one.
    fn spatimesh_surface(e: *mut Raw, id: u64, surface: *mut u32, width: *mut c_int, height: *mut c_int) -> c_int;
    fn spatimesh_release_surface(e: *mut Raw, id: u64, surface: u32);
    // Extensions (docs/spatimesh.md, "Extensions").
    fn spatimesh_ext_server(serve: ServeFn, free_bytes: ServeFreeFn);
    fn spatimesh_ext_bridge() -> *const c_char;
    fn spatimesh_ext_clear(e: *mut Raw);
    fn spatimesh_ext_add(e: *mut Raw, id: *const c_char, scheme: *const c_char, world: *const c_char, content_boot: *const c_char, page_boot: *const c_char);
    fn spatimesh_ext_entry(
        e: *mut Raw,
        id: *const c_char,
        run_at: c_int,
        all_frames: c_int,
        main_world: c_int,
        allow: *const c_char,
        block: *const c_char,
        guard: *const c_char,
        exact: c_int,
    );
    fn spatimesh_ext_js(e: *mut Raw, code: *const c_char);
    fn spatimesh_ext_css(e: *mut Raw, code: *const c_char);
    fn spatimesh_ext_done(e: *mut Raw);
    fn spatimesh_run_script(e: *mut Raw, id: u64, world: *const c_char, all_frames: c_int, code: *const c_char, request: u64, boot: c_int);
    fn spatimesh_set_page_script(e: *mut Raw, code: *const c_char);
    fn spatimesh_ext_decider(decide: DecideFn);
    fn spatimesh_ext_rules(e: *mut Raw, id: *const c_char, webkit: *const c_char);
    // Cookies and visited links (docs/spatimesh.md, "Logins and history on every machine"):
    // all the cookies said later as one event (kind 16) of `Cookie::line`s; cookies set and
    // removed as those lines; pages visited elsewhere, one address to a line.
    fn spatimesh_cookies_ask(e: *mut Raw);
    fn spatimesh_cookies_set(e: *mut Raw, lines: *const c_char);
    fn spatimesh_cookies_remove(e: *mut Raw, lines: *const c_char);
    fn spatimesh_visited(e: *mut Raw, urls: *const c_char);
    fn spatimesh_inspect(e: *mut Raw, id: u64, inspector: u64, width: c_int, height: c_int) -> c_int;
}

type ServeFn = unsafe extern "C" fn(*const c_char, *const c_char, *mut *const c_char, *mut *mut u8, *mut usize) -> c_int;
type ServeFreeFn = unsafe extern "C" fn(*mut u8, usize);
type DecideFn = unsafe extern "C" fn(*const c_char, *const c_char, c_int, *const c_char, *mut c_char, usize) -> c_int;

/// The profile's extensions' declarativeNetRequest rules, read: what [`decide`] asks.
static RULES: Mutex<Vec<dnr::Rules>> = Mutex::new(Vec::new());

/// What the profile's extensions' declarativeNetRequest rules make of a request, for an engine
/// that asks about each (Steam's Chromium): 0 nothing, 1 blocked, 2 to the address written into
/// `redirect` (`cap` bytes, ended by a NUL) instead. `kind` is the request's type, numbered as
/// [`dnr::ResourceType::ALL`] has them; `initiator` the page's origin, "" or "null" for the
/// browser's own.
unsafe extern "C" fn decide(url: *const c_char, initiator: *const c_char, kind: c_int, method: *const c_char, redirect: *mut c_char, cap: usize) -> c_int {
    let Some(resource_type) = usize::try_from(kind).ok().and_then(|k| dnr::ResourceType::ALL.get(k)).copied() else { return 0 };
    let Ok(rules) = RULES.lock() else { return 0 };
    if rules.is_empty() {
        return 0;
    }
    let (url, initiator, method) = (text(url), text(initiator), text(method));
    let request = dnr::Request {
        url: &url,
        initiator: Some(initiator.as_str()).filter(|i| !i.is_empty() && *i != "null"),
        resource_type,
        method: if method.is_empty() { "get" } else { &method },
        tab: None,
    };
    match dnr::decide(&rules, &request) {
        None => 0,
        Some(dnr::Decision::Block) => 1,
        Some(dnr::Decision::Redirect(to)) if !redirect.is_null() && to.len() < cap && !to.contains('\0') => {
            std::ptr::copy_nonoverlapping(to.as_ptr(), redirect as *mut u8, to.len());
            *redirect.add(to.len()) = 0;
            2
        }
        Some(dnr::Decision::Redirect(_)) => 0,
    }
}

/// The profile's extensions as the window last gave them: what their addresses serve (one
/// engine to a process).
static EXTENSIONS: Mutex<Option<ExtensionSet>> = Mutex::new(None);

/// A type for C, with the charset text is in: extensions' files are UTF-8.
fn c_mime(mime: &str) -> &'static CStr {
    match mime {
        "text/html" => c"text/html; charset=utf-8",
        "text/javascript" => c"text/javascript; charset=utf-8",
        "text/css" => c"text/css; charset=utf-8",
        "application/json" => c"application/json; charset=utf-8",
        "text/plain" => c"text/plain; charset=utf-8",
        "application/xml" => c"application/xml; charset=utf-8",
        "image/svg+xml" => c"image/svg+xml",
        "image/png" => c"image/png",
        "image/jpeg" => c"image/jpeg",
        "image/gif" => c"image/gif",
        "image/webp" => c"image/webp",
        "image/x-icon" => c"image/x-icon",
        "font/woff" => c"font/woff",
        "font/woff2" => c"font/woff2",
        "font/ttf" => c"font/ttf",
        "font/otf" => c"font/otf",
        "application/wasm" => c"application/wasm",
        "audio/mpeg" => c"audio/mpeg",
        "audio/ogg" => c"audio/ogg",
        "audio/wav" => c"audio/wav",
        "video/mp4" => c"video/mp4",
        "video/webm" => c"video/webm",
        _ => c"application/octet-stream",
    }
}

/// What an engine's handler for extensions' addresses (`chrome-extension://`, `moz-extension://`,
/// `spatimesh-extension://`) asks: a file of an
/// extension's, for the page `initiator` (`set::serve`). The bytes are the caller's to give back
/// to [`serve_free`].
unsafe extern "C" fn serve(url: *const c_char, initiator: *const c_char, mime: *mut *const c_char, bytes: *mut *mut u8, len: *mut usize) -> c_int {
    let (url, initiator) = (text(url), text(initiator));
    let result = match EXTENSIONS.lock() {
        Ok(guard) => match guard.as_ref() {
            Some(extensions) => set::serve(extensions, &url, Some(initiator.as_str()).filter(|i| !i.is_empty())),
            None => Err(404),
        },
        Err(_) => Err(500),
    };
    let (status, kind, body) = match result {
        Ok(resource) => (200, resource.mime, resource.bytes),
        Err(status) => (status as c_int, "text/plain", Vec::new()),
    };
    let body = body.into_boxed_slice();
    *mime = c_mime(kind).as_ptr();
    *len = body.len();
    *bytes = Box::into_raw(body) as *mut u8;
    status
}

unsafe extern "C" fn serve_free(bytes: *mut u8, len: usize) {
    if !bytes.is_null() {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(bytes, len)));
    }
}

/// One of Chrome's match patterns as WebKit's (`webkit_user_script_new`'s lists), and whether it
/// says exactly the same: WebKit matches no port, and a path without its query.
fn webkit_pattern(pattern: &str) -> Option<(Vec<String>, bool)> {
    if pattern == "<all_urls>" {
        return Some((["http://*/*", "https://*/*", "file:///*", "ftp://*/*"].map(String::from).to_vec(), true));
    }
    let (scheme, rest) = pattern.split_once("://")?;
    let slash = rest.find('/')?;
    let (authority, path) = (&rest[..slash], &rest[slash..]);
    let mut exact = true;
    let host = if authority.starts_with('[') {
        exact = false;
        "*"
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => {
                exact &= port == "*";
                host
            }
            None => authority,
        }
    };
    let path = match path.split_once('?') {
        Some((before, _)) => {
            exact = false;
            format!("{before}*")
        }
        None => {
            // Chrome's "/" is that page without a query; WebKit's takes any query too.
            exact &= path.ends_with('*');
            path.to_string()
        }
    };
    let schemes: Vec<&str> = if scheme == "*" { vec!["http", "https"] } else { vec![scheme] };
    Some((schemes.into_iter().map(|s| if s == "file" { format!("file://{path}") } else { format!("{s}://{host}{path}") }).collect(), exact))
}

/// A content script's pages as WebKit's allow and block lists -- the allow list never takes
/// fewer pages than Chrome would, the block list never more -- and whether they are exact.
fn webkit_lists(script: &Script) -> (Vec<String>, Vec<String>, bool) {
    let mut exact = script.include_globs.is_empty() && script.exclude_globs.is_empty();
    let mut allow = Vec::new();
    for m in &script.matches {
        match webkit_pattern(m) {
            Some((patterns, same)) => {
                allow.extend(patterns);
                exact &= same;
            }
            None => {
                allow.extend(["http://*/*", "https://*/*"].map(String::from));
                exact = false;
            }
        }
    }
    let mut block = Vec::new();
    for x in &script.exclude_matches {
        match webkit_pattern(x) {
            Some((patterns, true)) => block.extend(patterns),
            _ => exact = false,
        }
    }
    (allow, block, exact)
}

/// JavaScript that is true in a frame a content script wants, from its regular expressions.
fn guard(script: &Script) -> String {
    let quoted = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    let (include, globs, exclude) = (quoted(&script.include), quoted(&script.globs), quoted(&script.exclude));
    format!("(function(u){{return new RegExp({include}).test(u)&&({globs}===''||new RegExp({globs}).test(u))&&!({exclude}!==''&&new RegExp({exclude}).test(u));}})(location.href)")
}

pub struct Shim {
    raw: *mut Raw,
    views: HashMap<TabId, View>,
}

fn text(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

/// A string for C: what cannot cross (an inner NUL) is cut there.
fn c(s: &str) -> CString {
    CString::new(s.split('\0').next().unwrap_or("")).unwrap_or_default()
}

fn modifiers(m: Modifiers) -> c_int {
    m.shift as c_int | (m.control as c_int) << 1 | (m.alt as c_int) << 2 | (m.logo as c_int) << 3
}

impl Shim {
    pub fn new(setup: &ProfileSetup) -> Result<Shim, String> {
        let folders = if setup.private {
            None
        } else {
            for dir in [&setup.data, &setup.cache] {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            Some((c(&setup.data.to_string_lossy()), c(&setup.cache.to_string_lossy())))
        };
        let raw = unsafe {
            match &folders {
                Some((data, cache)) => spatimesh_new(data.as_ptr(), cache.as_ptr()),
                None => spatimesh_new(std::ptr::null(), std::ptr::null()),
            }
        };
        if raw.is_null() {
            return Err("the engine could not start (for WebKitGTK: no display to connect to)".into());
        }
        unsafe {
            spatimesh_ext_server(serve, serve_free);
            spatimesh_ext_decider(decide);
        }
        Ok(Shim { raw, views: HashMap::new() })
    }

    fn event(&self, e: &Event) -> Option<Happened> {
        let tab = e.tab;
        Some(match e.kind {
            1 => Happened::Title { tab, title: text(e.text) },
            2 => Happened::Url { tab, url: text(e.text) },
            3 => Happened::Progress { tab, progress: e.number as f32, loading: e.a != 0 },
            4 => Happened::History { tab, back: e.a != 0, forward: e.b != 0 },
            5 => Happened::Cursor {
                tab,
                cursor: match e.a {
                    1 => Cursor::Hand,
                    2 => Cursor::Text,
                    3 => Cursor::Wait,
                    4 => Cursor::Crosshair,
                    5 => Cursor::Move,
                    6 => Cursor::ResizeHorizontal,
                    7 => Cursor::ResizeVertical,
                    8 => Cursor::NotAllowed,
                    9 => Cursor::Grab,
                    10 => Cursor::Grabbing,
                    11 => Cursor::Hidden,
                    _ => Cursor::Arrow,
                },
            },
            6 => Happened::Sound { tab, playing: e.a != 0 },
            7 => Happened::OpenTab { opener: tab, url: text(e.text), background: e.a != 0 },
            8 => Happened::CloseTab { tab },
            9 => Happened::Crashed { tab, why: text(e.text) },
            10 => Happened::Failed { tab, url: text(e.text), why: text(e.text2) },
            11 => {
                let mut choices = Vec::new();
                for index in 0..e.a.max(0) {
                    let mut flags = 0;
                    let label = unsafe { spatimesh_choice(self.raw, tab, index, &mut flags) };
                    if label.is_null() {
                        break;
                    }
                    choices.push(Choice { label: text(label), enabled: flags & 1 != 0, selected: flags & 2 != 0, heading: flags & 4 != 0 });
                }
                let at = Rect { x: e.x.max(0) as u32, y: e.y.max(0) as u32, width: e.w.max(0) as u32, height: e.h.max(0) as u32 };
                Happened::Choose { tab, choices, at }
            }
            12 => Happened::Dialog {
                tab,
                kind: match e.a {
                    1 => DialogKind::Confirm,
                    2 => DialogKind::Prompt,
                    3 => DialogKind::Leave,
                    _ => DialogKind::Alert,
                },
                message: text(e.text),
                text: text(e.text2),
            },
            13 => Happened::Extension { tab, json: text(e.text) },
            15 => Happened::Media { tab, json: text(e.text) },
            16 => Happened::Cookies { cookies: Cookie::from_lines(&text(e.text)) },
            17 => Happened::SoundName { tab, name: text(e.text) },
            14 => Happened::ScriptResult { tab, request: e.number as u64, result: if e.a != 0 { Ok(text(e.text)) } else { Err(text(e.text)) } },
            _ => return None,
        })
    }
}

impl Engine for Shim {
    fn name(&self) -> String {
        text(unsafe { spatimesh_version() })
    }

    fn open(&mut self, tab: TabId, url: &str, view: View) -> Result<(), String> {
        let url = c(url);
        if unsafe { spatimesh_open(self.raw, tab, url.as_ptr(), view.width as c_int, view.height as c_int, view.zoom as f64) } == 0 {
            return Err(format!("tab {tab} is already open"));
        }
        self.views.insert(tab, view);
        Ok(())
    }

    fn close(&mut self, tab: TabId) {
        self.views.remove(&tab);
        unsafe { spatimesh_close(self.raw, tab) };
    }

    fn load(&mut self, tab: TabId, url: &str) {
        let url = c(url);
        unsafe { spatimesh_load(self.raw, tab, url.as_ptr()) };
    }

    fn back(&mut self, tab: TabId) {
        unsafe { spatimesh_back(self.raw, tab) };
    }

    fn forward(&mut self, tab: TabId) {
        unsafe { spatimesh_forward(self.raw, tab) };
    }

    fn reload(&mut self, tab: TabId) {
        unsafe { spatimesh_reload(self.raw, tab) };
    }

    fn stop(&mut self, tab: TabId) {
        unsafe { spatimesh_stop(self.raw, tab) };
    }

    fn resize(&mut self, tab: TabId, view: View) {
        if let Some(v) = self.views.get_mut(&tab) {
            *v = view;
            unsafe { spatimesh_resize(self.raw, tab, view.width as c_int, view.height as c_int, view.zoom as f64) };
        }
    }

    fn pointer(&mut self, tab: TabId, x: f32, y: f32, button: Option<(Button, bool)>, m: Modifiers) {
        let (number, pressed) = match button {
            None => (0, false),
            Some((b, pressed)) => (
                match b {
                    Button::Left => 1,
                    Button::Middle => 2,
                    Button::Right => 3,
                    Button::Back => 4,
                    Button::Forward => 5,
                },
                pressed,
            ),
        };
        unsafe { spatimesh_pointer(self.raw, tab, x as f64, y as f64, number, pressed as c_int, modifiers(m)) };
    }

    fn pointer_left(&mut self, tab: TabId) {
        unsafe { spatimesh_leave(self.raw, tab) };
    }

    fn scroll(&mut self, tab: TabId, x: f32, y: f32, dx: f32, dy: f32, m: Modifiers) {
        unsafe { spatimesh_scroll(self.raw, tab, x as f64, y as f64, dx as f64, dy as f64, modifiers(m)) };
    }

    fn key(&mut self, tab: TabId, key: &KeyPress) {
        let (number, ch) = spatimesh::wire::key_number(key.key);
        // With Control or the logo key held a key types nothing, whatever the room said.
        let typed = if key.modifiers.control || key.modifiers.logo { "" } else { key.text.as_deref().unwrap_or("") };
        let typed = c(typed);
        unsafe { spatimesh_key(self.raw, tab, number as c_int, ch, key.code as c_int, key.pressed as c_int, modifiers(key.modifiers), typed.as_ptr()) };
    }

    fn focus(&mut self, tab: TabId, focused: bool) {
        unsafe { spatimesh_focus(self.raw, tab, focused as c_int) };
    }

    fn set_window(&mut self, tab: TabId, window: u32) {
        unsafe { spatimesh_set_window(self.raw, tab, window) };
    }

    fn set_muted(&mut self, tab: TabId, muted: bool) {
        unsafe { spatimesh_mute(self.raw, tab, muted as c_int) };
    }

    fn choose(&mut self, tab: TabId, index: Option<usize>) {
        unsafe { spatimesh_choose(self.raw, tab, index.map_or(-1, |i| i as c_int)) };
    }

    fn answer(&mut self, tab: TabId, accepted: bool, text: Option<String>) {
        let text = text.map(|t| c(&t));
        unsafe { spatimesh_answer(self.raw, tab, accepted as c_int, text.as_ref().map_or(std::ptr::null(), |t| t.as_ptr())) };
    }

    fn wake_on(&mut self, fd: std::os::fd::RawFd) {
        unsafe { spatimesh_watch(self.raw, fd) };
    }

    fn set_extensions(&mut self, extensions: &ExtensionSet) {
        if let Ok(mut kept) = EXTENSIONS.lock() {
            *kept = Some(extensions.clone());
        }
        let bridge = text(unsafe { spatimesh_ext_bridge() });
        unsafe { spatimesh_ext_clear(self.raw) };
        for spec in &extensions.extensions {
            let (id, scheme, world) = (c(&spec.id), c(spec.scheme()), c(&spec.world()));
            // An engine with worlds of the extensions' own gives a bridge to put the API on
            // their globals with; one without gives none, and is handed the API as functions.
            let (content, page) = if bridge.is_empty() {
                (set::boot_function(&spec.content_config, true), set::boot_function(&spec.page_config, false))
            } else {
                (set::boot(&bridge, &spec.content_config), set::boot(&bridge, &spec.page_config))
            };
            let (content, page) = (c(&content), c(&page));
            unsafe { spatimesh_ext_add(self.raw, id.as_ptr(), scheme.as_ptr(), world.as_ptr(), content.as_ptr(), page.as_ptr()) };
            for script in &spec.scripts {
                let (allow, block, exact) = webkit_lists(script);
                let (allow, block, check) = (c(&allow.join("\n")), c(&block.join("\n")), c(&guard(script)));
                let run_at = match script.run_at.as_str() {
                    "document_start" => 0,
                    "document_end" => 1,
                    _ => 2,
                };
                unsafe {
                    spatimesh_ext_entry(
                        self.raw,
                        id.as_ptr(),
                        run_at,
                        script.all_frames as c_int,
                        script.main_world as c_int,
                        allow.as_ptr(),
                        block.as_ptr(),
                        check.as_ptr(),
                        exact as c_int,
                    )
                };
                for (name, code) in &script.js {
                    let code = c(&format!("{code}\n//# sourceURL={}/{}", spec.origin(), name.trim_start_matches('/')));
                    unsafe { spatimesh_ext_js(self.raw, code.as_ptr()) };
                }
                for (_, css) in &script.css {
                    // Written with __MSG_name__, as in Chrome.
                    let css = spatimesh::extensions::locale::substitute(css, &|name| spec.messages.get(&name.to_lowercase()).cloned());
                    let css = c(&css);
                    unsafe { spatimesh_ext_css(self.raw, css.as_ptr()) };
                }
            }
        }
        // declarativeNetRequest: for an engine that asks about each request, and as WebKit's
        // content blockers for one that applies them itself (another engine ignores those).
        let rules: Vec<dnr::Rules> = extensions.extensions.iter().filter(|s| !s.rules.is_empty()).map(|s| dnr::Rules::new(&s.id, &s.origin, &s.rules)).collect();
        for each in rules.iter().filter(|r| !r.is_empty()) {
            let (id, webkit) = (c(each.id()), c(&dnr::webkit_rules(each)));
            unsafe { spatimesh_ext_rules(self.raw, id.as_ptr(), webkit.as_ptr()) };
        }
        if let Ok(mut kept) = RULES.lock() {
            *kept = rules;
        }
        unsafe { spatimesh_ext_done(self.raw) };
    }

    fn set_page_script(&mut self, code: &str) {
        let code = c(code);
        unsafe { spatimesh_set_page_script(self.raw, code.as_ptr()) };
    }

    fn run_script(&mut self, tab: TabId, world: &str, all_frames: bool, code: &str, request: u64, boot: bool) {
        let (world, code) = (c(world), c(code));
        unsafe { spatimesh_run_script(self.raw, tab, world.as_ptr(), all_frames as c_int, code.as_ptr(), request, boot as c_int) };
    }

    fn release_surface(&mut self, tab: TabId, id: u32) {
        unsafe { spatimesh_release_surface(self.raw, tab, id) };
    }

    fn ask_cookies(&mut self, _addresses: &[String]) {
        unsafe { spatimesh_cookies_ask(self.raw) };
    }

    fn set_cookies(&mut self, set: &[Cookie], remove: &[Cookie]) {
        let lines = |all: &[Cookie]| c(&all.iter().map(Cookie::line).collect::<Vec<_>>().join("\n"));
        if !remove.is_empty() {
            unsafe { spatimesh_cookies_remove(self.raw, lines(remove).as_ptr()) };
        }
        if !set.is_empty() {
            unsafe { spatimesh_cookies_set(self.raw, lines(set).as_ptr()) };
        }
    }

    fn add_visited(&mut self, urls: &[String]) {
        let urls = c(&urls.iter().filter(|u| !u.contains('\n')).cloned().collect::<Vec<_>>().join("\n"));
        unsafe { spatimesh_visited(self.raw, urls.as_ptr()) };
    }

    fn inspect(&mut self, tab: TabId, inspector: TabId, view: View) -> bool {
        if unsafe { spatimesh_inspect(self.raw, tab, inspector, view.width as c_int, view.height as c_int) } == 0 {
            return false;
        }
        self.views.insert(inspector, view);
        true
    }

    fn pump(&mut self, wait: Duration, canvas: &mut dyn Canvas) -> Vec<Happened> {
        unsafe { spatimesh_pump(self.raw, wait.as_millis().min(1000) as c_int) };
        let mut out = Vec::new();
        let mut e = Event { kind: 0, tab: 0, number: 0.0, a: 0, b: 0, text: std::ptr::null(), text2: std::ptr::null(), x: 0, y: 0, w: 0, h: 0 };
        while unsafe { spatimesh_next_event(self.raw, &mut e) } != 0 {
            if let Some(happened) = self.event(&e) {
                out.push(happened);
            }
        }
        for (&tab, view) in &self.views {
            // On the GPU already: handed over by its number, nothing copied.
            let (mut id, mut sw, mut sh) = (0u32, 0, 0);
            if unsafe { spatimesh_surface(self.raw, tab, &mut id, &mut sw, &mut sh) } != 0 {
                canvas.surface(tab, id, sw as u32, sh as u32);
                out.push(Happened::Picture { tab });
                continue;
            }
            let (mut x, mut y, mut w, mut h) = (0, 0, 0, 0);
            if unsafe { spatimesh_damage(self.raw, tab, &mut x, &mut y, &mut w, &mut h) } == 0 {
                continue;
            }
            // Both buffers still with the window: the change waits, and is read next time.
            let Some(pixels) = canvas.begin(tab, view.width, view.height, Format::Bgra) else { continue };
            if unsafe { spatimesh_copy(self.raw, tab, pixels.as_mut_ptr(), view.width as c_int, view.height as c_int) } != 0 {
                canvas.finish(tab, &[Rect { x: x as u32, y: y as u32, width: w as u32, height: h as u32 }]);
                out.push(Happened::Picture { tab });
            }
        }
        out
    }
}
