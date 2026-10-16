//! SpatiMesh's engine interface on WebKitGTK, through the shim in `native/webkitgtk.c`.

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::time::Duration;

use spatimesh::engine::{
    Button, Canvas, Choice, Cursor, DialogKind, Engine, Format, Happened, KeyPress, Modifiers, ProfileSetup, Rect, TabId, View,
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
    fn mesh_wk_version() -> *const c_char;
    fn mesh_wk_new(data: *const c_char, cache: *const c_char) -> *mut Raw;
    fn mesh_wk_watch(e: *mut Raw, fd: c_int);
    fn mesh_wk_open(e: *mut Raw, id: u64, uri: *const c_char, width: c_int, height: c_int, zoom: f64) -> c_int;
    fn mesh_wk_close(e: *mut Raw, id: u64);
    fn mesh_wk_load(e: *mut Raw, id: u64, uri: *const c_char);
    fn mesh_wk_back(e: *mut Raw, id: u64);
    fn mesh_wk_forward(e: *mut Raw, id: u64);
    fn mesh_wk_reload(e: *mut Raw, id: u64);
    fn mesh_wk_stop(e: *mut Raw, id: u64);
    fn mesh_wk_resize(e: *mut Raw, id: u64, width: c_int, height: c_int, zoom: f64);
    fn mesh_wk_mute(e: *mut Raw, id: u64, muted: c_int);
    fn mesh_wk_pointer(e: *mut Raw, id: u64, x: f64, y: f64, button: c_int, pressed: c_int, modifiers: c_int);
    fn mesh_wk_leave(e: *mut Raw, id: u64);
    fn mesh_wk_scroll(e: *mut Raw, id: u64, x: f64, y: f64, dx: f64, dy: f64, modifiers: c_int);
    fn mesh_wk_key(e: *mut Raw, id: u64, key: c_int, ch: u32, code: c_int, pressed: c_int, modifiers: c_int, text: *const c_char);
    fn mesh_wk_focus(e: *mut Raw, id: u64, focused: c_int);
    fn mesh_wk_choice(e: *mut Raw, id: u64, index: c_int, flags: *mut c_int) -> *const c_char;
    fn mesh_wk_choose(e: *mut Raw, id: u64, index: c_int);
    fn mesh_wk_answer(e: *mut Raw, id: u64, accepted: c_int, text: *const c_char);
    fn mesh_wk_pump(e: *mut Raw, timeout_ms: c_int);
    fn mesh_wk_next_event(e: *mut Raw, out: *mut Event) -> c_int;
    fn mesh_wk_damage(e: *mut Raw, id: u64, x: *mut c_int, y: *mut c_int, w: *mut c_int, h: *mut c_int) -> c_int;
    fn mesh_wk_copy(e: *mut Raw, id: u64, pixels: *mut u8, width: c_int, height: c_int) -> c_int;
}

pub struct WebKitGtk {
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

impl WebKitGtk {
    pub fn new(setup: &ProfileSetup) -> Result<WebKitGtk, String> {
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
                Some((data, cache)) => mesh_wk_new(data.as_ptr(), cache.as_ptr()),
                None => mesh_wk_new(std::ptr::null(), std::ptr::null()),
            }
        };
        if raw.is_null() {
            return Err("GTK could not start: there is no display to connect to".into());
        }
        Ok(WebKitGtk { raw, views: HashMap::new() })
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
                    let label = unsafe { mesh_wk_choice(self.raw, tab, index, &mut flags) };
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
            _ => return None,
        })
    }
}

impl Engine for WebKitGtk {
    fn name(&self) -> String {
        text(unsafe { mesh_wk_version() })
    }

    fn open(&mut self, tab: TabId, url: &str, view: View) -> Result<(), String> {
        let url = c(url);
        if unsafe { mesh_wk_open(self.raw, tab, url.as_ptr(), view.width as c_int, view.height as c_int, view.zoom as f64) } == 0 {
            return Err(format!("tab {tab} is already open"));
        }
        self.views.insert(tab, view);
        Ok(())
    }

    fn close(&mut self, tab: TabId) {
        self.views.remove(&tab);
        unsafe { mesh_wk_close(self.raw, tab) };
    }

    fn load(&mut self, tab: TabId, url: &str) {
        let url = c(url);
        unsafe { mesh_wk_load(self.raw, tab, url.as_ptr()) };
    }

    fn back(&mut self, tab: TabId) {
        unsafe { mesh_wk_back(self.raw, tab) };
    }

    fn forward(&mut self, tab: TabId) {
        unsafe { mesh_wk_forward(self.raw, tab) };
    }

    fn reload(&mut self, tab: TabId) {
        unsafe { mesh_wk_reload(self.raw, tab) };
    }

    fn stop(&mut self, tab: TabId) {
        unsafe { mesh_wk_stop(self.raw, tab) };
    }

    fn resize(&mut self, tab: TabId, view: View) {
        if let Some(v) = self.views.get_mut(&tab) {
            *v = view;
            unsafe { mesh_wk_resize(self.raw, tab, view.width as c_int, view.height as c_int, view.zoom as f64) };
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
        unsafe { mesh_wk_pointer(self.raw, tab, x as f64, y as f64, number, pressed as c_int, modifiers(m)) };
    }

    fn pointer_left(&mut self, tab: TabId) {
        unsafe { mesh_wk_leave(self.raw, tab) };
    }

    fn scroll(&mut self, tab: TabId, x: f32, y: f32, dx: f32, dy: f32, m: Modifiers) {
        unsafe { mesh_wk_scroll(self.raw, tab, x as f64, y as f64, dx as f64, dy as f64, modifiers(m)) };
    }

    fn key(&mut self, tab: TabId, key: &KeyPress) {
        let (number, ch) = spatimesh::wire::key_number(key.key);
        // With Control or the logo key held a key types nothing, whatever the room said.
        let typed = if key.modifiers.control || key.modifiers.logo { "" } else { key.text.as_deref().unwrap_or("") };
        let typed = c(typed);
        unsafe { mesh_wk_key(self.raw, tab, number as c_int, ch, key.code as c_int, key.pressed as c_int, modifiers(key.modifiers), typed.as_ptr()) };
    }

    fn focus(&mut self, tab: TabId, focused: bool) {
        unsafe { mesh_wk_focus(self.raw, tab, focused as c_int) };
    }

    fn set_window(&mut self, _tab: TabId, _window: u32) {
        // Where each tab's sound goes: the window's sink, which needs the compositor's per-window
        // sinks (docs/spatimesh.md, "Sound"). Until then a tab sounds where SpatiMesh does.
    }

    fn set_muted(&mut self, tab: TabId, muted: bool) {
        unsafe { mesh_wk_mute(self.raw, tab, muted as c_int) };
    }

    fn choose(&mut self, tab: TabId, index: Option<usize>) {
        unsafe { mesh_wk_choose(self.raw, tab, index.map_or(-1, |i| i as c_int)) };
    }

    fn answer(&mut self, tab: TabId, accepted: bool, text: Option<String>) {
        let text = text.map(|t| c(&t));
        unsafe { mesh_wk_answer(self.raw, tab, accepted as c_int, text.as_ref().map_or(std::ptr::null(), |t| t.as_ptr())) };
    }

    fn wake_on(&mut self, fd: std::os::fd::RawFd) {
        unsafe { mesh_wk_watch(self.raw, fd) };
    }

    fn pump(&mut self, wait: Duration, canvas: &mut dyn Canvas) -> Vec<Happened> {
        unsafe { mesh_wk_pump(self.raw, wait.as_millis().min(1000) as c_int) };
        let mut out = Vec::new();
        let mut e = Event { kind: 0, tab: 0, number: 0.0, a: 0, b: 0, text: std::ptr::null(), text2: std::ptr::null(), x: 0, y: 0, w: 0, h: 0 };
        while unsafe { mesh_wk_next_event(self.raw, &mut e) } != 0 {
            if let Some(happened) = self.event(&e) {
                out.push(happened);
            }
        }
        for (&tab, view) in &self.views {
            let (mut x, mut y, mut w, mut h) = (0, 0, 0, 0);
            if unsafe { mesh_wk_damage(self.raw, tab, &mut x, &mut y, &mut w, &mut h) } == 0 {
                continue;
            }
            // Both buffers still with the window: the change waits, and is read next time.
            let Some(pixels) = canvas.begin(tab, view.width, view.height, Format::Bgra) else { continue };
            if unsafe { mesh_wk_copy(self.raw, tab, pixels.as_mut_ptr(), view.width as c_int, view.height as c_int) } != 0 {
                canvas.finish(tab, &[Rect { x: x as u32, y: y as u32, width: w as u32, height: h as u32 }]);
                out.push(Happened::Picture { tab });
            }
        }
        out
    }
}
