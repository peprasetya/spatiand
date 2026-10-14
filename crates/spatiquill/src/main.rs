//! SpatiQuill: Spatiand's terminal.
//!
//! A full terminal (docs/spatiand-apps.md), drawn straight into the room, running the
//! platform's own shell. **Every tab is a process of its own** (`spatiquill --tab`, see
//! `tab.rs`): it holds the shell's pseudo-terminal and the emulator, and tells the window what
//! its screen shows. One process holds all of SpatiQuill's windows, so a tab can be dragged
//! along the bar to reorder it, off the window into a window of its own, or onto another
//! SpatiQuill window -- and starting SpatiQuill again opens a window in the process already
//! running.
//!
//! ```text
//! spatiquill           a window, with a tab
//! spatiquill --tab     a tab, spoken to over its standard input and output
//! ```

mod grid;
mod keys;
mod proc;
mod tab;
mod wire;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use spatiand_app::ui::{look, theme, Painter, Rect};
use spatiand_app::{App, Button as PointerButton, Event, Key, Modifiers, WindowId};

use grid::Grid;
use wire::{FromTab, Gather, Screen, ToTab};

const WINDOW: (u32, u32) = (1280, 820);
const BAR: f32 = 66.0;
const MARGIN: f32 = 18.0;
const TEXT_PX: f32 = 24.0;

enum TabState {
    Running,
    /// The tab's process went without saying its shell had: it crashed.
    Crashed(String),
}

struct Tab {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    gather: Gather,
    screen: Screen,
    title: String,
    state: TabState,
    /// The size it was last told, so that it is told only when that changes.
    told: (u16, u16, u16, u16),
    rang: Option<Instant>,
}

impl Tab {
    fn spawn() -> Result<Tab, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut child = Command::new(exe).arg("--tab").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().map_err(|e| format!("a tab would not start: {e}"))?;
        let stdin = child.stdin.take().ok_or("no way to talk to the tab")?;
        let stdout = child.stdout.take().ok_or("no way to hear the tab")?;
        unsafe {
            let fd = stdout.as_raw_fd();
            libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
        }
        Ok(Tab { child, stdin, stdout, gather: Gather::default(), screen: Screen::default(), title: String::new(), state: TabState::Running, told: (0, 0, 0, 0), rang: None })
    }

    fn tell(&mut self, message: &ToTab) {
        if matches!(self.state, TabState::Running) {
            let _ = wire::send(&mut self.stdin, &message.encode());
        }
    }

    fn label(&self) -> String {
        match &self.state {
            TabState::Crashed(_) => "Stopped".to_string(),
            TabState::Running if self.title.trim().is_empty() => "Shell".to_string(),
            TabState::Running => self.title.trim().to_string(),
        }
    }
}

/// A tab being dragged by its place on the bar.
struct Drag {
    tab: u64,
    from: (f32, f32),
    at: (f32, f32),
    moved: bool,
    /// Off the bar far enough that letting go takes it out of this window.
    torn: bool,
}

struct Win {
    id: WindowId,
    tabs: Vec<u64>,
    active: usize,
    size: (u32, u32),
    ready: bool,
    dirty: bool,
    focused: bool,
    pointer: (f32, f32),
    hover: Hover,
    drag: Option<Drag>,
    /// Selecting text with the pointer: where it started and where it is, in cells.
    selecting: Option<((u16, u16), (u16, u16))>,
    pressed_in_text: bool,
    /// Where each tab is on the bar, and its close mark, as last drawn.
    placed: Vec<(u64, Rect, Rect)>,
    plus: Rect,
    last_click: Option<(Instant, (u16, u16))>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hover {
    Nothing,
    Tab(u64),
    Close(u64),
    Plus,
    Text,
}

struct Quill {
    app: App,
    painter: Option<Painter>,
    grid: Option<Grid>,
    text_px: f32,
    tabs: HashMap<u64, Tab>,
    wins: Vec<Win>,
    next_tab: u64,
    listener: Option<(UnixListener, PathBuf)>,
    /// A tab let go of off its window: where it came from and when. If the pointer arrives
    /// on another SpatiQuill window straight after, it was dropped there; if not, it becomes
    /// a window of its own.
    in_flight: Option<(u64, WindowId, Instant)>,
    clipboard: String,
    typed_at: Instant,
    /// The text's size has changed: the cells are measured again before the next drawing.
    resize_text: bool,
}

/// Where a running SpatiQuill listens for another start: one per room, by its socket's name.
fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    let display = display.rsplit('/').next().unwrap_or("wayland").to_string();
    dir.join(format!("spatiquill-{display}.sock"))
}

/// Whether the machine's clipboard is to be used: `SPATIQUILL_CLIPBOARD=off` keeps a test's
/// selections out of the clipboard of whoever's machine it runs on.
fn clipboard_on() -> bool {
    std::env::var("SPATIQUILL_CLIPBOARD").as_deref() != Ok("off")
}

/// The machine's own clipboard, where there is a way to reach it from here.
fn system_copy(text: &str) {
    if !clipboard_on() {
        return;
    }
    let program: Option<(&str, &[&str])> = if cfg!(target_os = "macos") {
        Some(("pbcopy", &[]))
    } else if cfg!(target_os = "linux") {
        Some(("wl-copy", &[]))
    } else {
        None
    };
    if let Some((program, args)) = program {
        if let Ok(mut child) = Command::new(program).args(args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    }
}

fn system_paste() -> Option<String> {
    if !clipboard_on() {
        return None;
    }
    let program: (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("pbpaste", &[])
    } else if cfg!(target_os = "linux") {
        ("wl-paste", &["--no-newline"])
    } else {
        return None;
    };
    let out = Command::new(program.0).args(program.1).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

impl Quill {
    fn win(&mut self, id: WindowId) -> Option<&mut Win> {
        self.wins.iter_mut().find(|w| w.id == id)
    }

    fn win_index(&self, id: WindowId) -> Option<usize> {
        self.wins.iter().position(|w| w.id == id)
    }

    fn new_tab(&mut self) -> Option<u64> {
        match Tab::spawn() {
            Ok(tab) => {
                self.app.watch(tab.stdout.as_raw_fd());
                let id = self.next_tab;
                self.next_tab += 1;
                self.tabs.insert(id, tab);
                Some(id)
            }
            Err(e) => {
                log::error!("{e}");
                None
            }
        }
    }

    fn open_window(&mut self, tabs: Vec<u64>) -> WindowId {
        let id = self.app.open("SpatiQuill", WINDOW);
        self.wins.push(Win {
            id,
            tabs,
            active: 0,
            size: WINDOW,
            ready: false,
            dirty: true,
            focused: false,
            pointer: (0.0, 0.0),
            hover: Hover::Nothing,
            drag: None,
            selecting: None,
            pressed_in_text: false,
            placed: Vec::new(),
            plus: Rect::default(),
            last_click: None,
        });
        id
    }

    fn new_window(&mut self) {
        if let Some(tab) = self.new_tab() {
            self.open_window(vec![tab]);
        }
    }

    /// The terminal's place in a window, and how many cells fit in it.
    fn text_area(&self, size: (u32, u32)) -> (Rect, u16, u16) {
        let area = Rect::new(MARGIN, BAR + 10.0, size.0 as f32 - MARGIN * 2.0, size.1 as f32 - BAR - 10.0 - MARGIN);
        let (cw, ch) = self.grid.as_ref().map_or((TEXT_PX * 0.6, TEXT_PX * 1.28), |g| g.cell);
        let cols = ((area.w / cw).floor() as u16).max(2);
        let rows = ((area.h / ch).floor() as u16).max(1);
        (area, cols, rows)
    }

    /// Tell every tab of every window how big it is, where that has changed.
    fn size_tabs(&mut self) {
        let cell = self.grid.as_ref().map(|g| (g.cell.0 as u16, g.cell.1 as u16));
        let Some((cw, ch)) = cell else { return };
        let sizes: Vec<(Vec<u64>, u16, u16)> = self.wins.iter().map(|w| {
            let (_, cols, rows) = self.text_area(w.size);
            (w.tabs.clone(), cols, rows)
        }).collect();
        for (tabs, cols, rows) in sizes {
            for id in tabs {
                if let Some(tab) = self.tabs.get_mut(&id) {
                    let size = (cols, rows, cw, ch);
                    if tab.told != size {
                        tab.told = size;
                        tab.tell(&ToTab::Resize { cols, rows, cell_w: cw, cell_h: ch });
                    }
                }
            }
        }
    }

    fn close_tab(&mut self, id: u64) {
        if let Some(mut tab) = self.tabs.remove(&id) {
            self.app.unwatch(tab.stdout.as_raw_fd());
            tab.tell(&ToTab::Quit);
            // Given a moment to go on its own, then made to.
            let deadline = Instant::now() + Duration::from_millis(300);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = tab.child.try_wait() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = tab.child.kill();
            let _ = tab.child.wait();
        }
        for win in &mut self.wins {
            if let Some(at) = win.tabs.iter().position(|&t| t == id) {
                win.tabs.remove(at);
                if win.active > at || win.active >= win.tabs.len() {
                    win.active = win.active.saturating_sub(1);
                }
                win.dirty = true;
            }
        }
        self.close_empty_windows();
    }

    fn close_empty_windows(&mut self) {
        let empty: Vec<WindowId> = self.wins.iter().filter(|w| w.tabs.is_empty()).map(|w| w.id).collect();
        for id in empty {
            self.app.close(id);
            self.wins.retain(|w| w.id != id);
        }
    }

    fn close_window(&mut self, id: WindowId) {
        let tabs = self.win(id).map(|w| w.tabs.clone()).unwrap_or_default();
        for tab in tabs {
            self.close_tab(tab);
        }
        if self.win_index(id).is_some() {
            self.app.close(id);
            self.wins.retain(|w| w.id != id);
        }
    }

    /// Move a tab from wherever it is to `to`, at `place` on its bar.
    fn move_tab(&mut self, tab: u64, to: WindowId, place: Option<usize>) {
        for win in &mut self.wins {
            if let Some(at) = win.tabs.iter().position(|&t| t == tab) {
                win.tabs.remove(at);
                win.active = win.active.min(win.tabs.len().saturating_sub(1));
                win.dirty = true;
            }
        }
        if let Some(win) = self.win(to) {
            let at = place.unwrap_or(win.tabs.len()).min(win.tabs.len());
            win.tabs.insert(at, tab);
            win.active = at;
            win.dirty = true;
        }
        self.close_empty_windows();
        self.size_tabs();
    }

    /// What the tabs have said.
    fn hear(&mut self) {
        let mut ended = Vec::new();
        let mut copied = None;
        for (&id, tab) in self.tabs.iter_mut() {
            if !matches!(tab.state, TabState::Running) {
                continue;
            }
            let mut buffer = [0u8; 65536];
            let mut gone = false;
            loop {
                match tab.stdout.read(&mut buffer) {
                    Ok(0) => {
                        gone = true;
                        break;
                    }
                    Ok(n) => tab.gather.push(&buffer[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        gone = true;
                        break;
                    }
                }
            }
            let mut exited = false;
            while let Some(message) = tab.gather.next() {
                match message {
                    FromTab::Screen(screen) => tab.screen = screen,
                    FromTab::Title(title) => tab.title = title,
                    FromTab::Copied(text) => copied = Some(text),
                    FromTab::Bell => tab.rang = Some(Instant::now()),
                    FromTab::Exited(_) => exited = true,
                }
            }
            if exited {
                ended.push(id);
            } else if gone {
                let why = match tab.child.wait() {
                    Ok(status) => {
                        use std::os::unix::process::ExitStatusExt;
                        match status.signal() {
                            Some(signal) => format!("The tab's process was stopped by signal {signal}."),
                            None => format!("The tab's process ended unexpectedly (status {}).", status.code().unwrap_or(-1)),
                        }
                    }
                    Err(e) => format!("The tab's process is gone: {e}"),
                };
                log::warn!("tab {id}: {why}");
                tab.state = TabState::Crashed(why);
            }
        }
        for win in &mut self.wins {
            win.dirty = true;
        }
        if let Some(text) = copied {
            if !text.is_empty() {
                system_copy(&text);
                self.clipboard = text;
            }
        }
        // A shell that has finished takes its tab with it, as in every terminal.
        for id in ended {
            self.close_tab(id);
        }
    }

    fn restart(&mut self, id: u64) {
        let Some(new) = self.new_tab() else { return };
        for win in &mut self.wins {
            for t in win.tabs.iter_mut() {
                if *t == id {
                    *t = new;
                }
            }
        }
        if let Some(tab) = self.tabs.remove(&id) {
            self.app.unwatch(tab.stdout.as_raw_fd());
        }
        self.size_tabs();
    }

    fn active_tab(&self, win: usize) -> Option<u64> {
        let w = &self.wins[win];
        w.tabs.get(w.active).copied()
    }

    /// The cell at a point of a window, if the point is on the terminal.
    fn cell_at(&self, win: usize, x: f32, y: f32) -> Option<(u16, u16)> {
        let (area, cols, rows) = self.text_area(self.wins[win].size);
        let (cw, ch) = self.grid.as_ref()?.cell;
        if !area.inset(-6.0).contains(x, y) {
            return None;
        }
        let col = (((x - area.x) / cw).floor().max(0.0) as u16).min(cols - 1);
        let row = (((y - area.y) / ch).floor().max(0.0) as u16).min(rows - 1);
        Some((col, row))
    }

    fn handle(&mut self, event: Event) -> bool {
        let Some(id) = event.window() else { return true };
        let Some(w) = self.win_index(id) else { return true };
        self.wins[w].dirty = true;
        match event {
            Event::Ready { .. } => {
                self.wins[w].ready = true;
                self.size_tabs();
            }
            Event::Resized { width, height, .. } => {
                let size = (width.max(400), height.max(240));
                self.wins[w].size = size;
                self.app.resize(id, size);
                self.size_tabs();
            }
            Event::CloseRequested { .. } => {
                self.close_window(id);
                if self.wins.is_empty() {
                    return false;
                }
            }
            Event::Focus { focused, .. } => {
                self.wins[w].focused = focused;
                if let Some(tab) = self.active_tab(w).and_then(|t| self.tabs.get_mut(&t)) {
                    tab.tell(&ToTab::Focus(focused));
                }
            }
            Event::PointerMoved { x, y, .. } => self.pointer(w, x, y),
            Event::PointerLeft { .. } => self.wins[w].hover = Hover::Nothing,
            Event::Button { button, pressed, .. } => self.button(w, button, pressed),
            Event::Scroll { dy, .. } => self.scroll(w, dy),
            Event::Key { key, code, pressed: true, text, modifiers, .. } => {
                self.typed_at = Instant::now();
                self.key(w, key, code, text.unwrap_or_default(), modifiers);
            }
            _ => {}
        }
        true
    }

    fn pointer(&mut self, w: usize, x: f32, y: f32) {
        // A tab let go of over nothing, now over this window: it was dropped here.
        if let Some((tab, from, at)) = self.in_flight {
            if from != self.wins[w].id && at.elapsed() < Duration::from_millis(900) {
                self.in_flight = None;
                let place = self.wins[w].placed.iter().position(|(_, r, _)| x < r.x + r.w * 0.5);
                let to = self.wins[w].id;
                self.move_tab(tab, to, place);
                return;
            }
        }
        let win = &mut self.wins[w];
        win.pointer = (x, y);
        if let Some(drag) = win.drag.as_mut() {
            drag.at = (x, y);
            if (x - drag.from.0).abs() > 10.0 || (y - drag.from.1).abs() > 10.0 {
                drag.moved = true;
            }
            drag.torn = y > BAR + 90.0 || y < -40.0 || x < -40.0 || x > win.size.0 as f32 + 40.0;
            if drag.moved && !drag.torn {
                // Along the bar: the tab goes where the pointer is.
                let tab = drag.tab;
                let target = win.placed.iter().position(|(_, r, _)| x < r.x + r.w * 0.5).unwrap_or(win.tabs.len()).min(win.tabs.len().saturating_sub(1));
                if let Some(from) = win.tabs.iter().position(|&t| t == tab) {
                    if from != target {
                        win.tabs.remove(from);
                        win.tabs.insert(target, tab);
                        win.active = target;
                    }
                }
            }
            return;
        }
        win.hover = if let Some((id, _, close)) = win.placed.iter().find(|(_, r, _)| r.contains(x, y)) {
            if close.inset(-6.0).contains(x, y) { Hover::Close(*id) } else { Hover::Tab(*id) }
        } else if win.plus.contains(x, y) {
            Hover::Plus
        } else if y > BAR {
            Hover::Text
        } else {
            Hover::Nothing
        };
        // Dragging across the terminal: a selection, or the program's own mouse.
        if win.pressed_in_text {
            let tab = self.active_tab(w);
            let cell = self.cell_at(w, x, y);
            let mouse = tab.and_then(|t| self.tabs.get(&t)).is_some_and(|t| t.screen.mouse);
            if let (Some(tab), Some(cell)) = (tab, cell) {
                if mouse {
                    if let Some(t) = self.tabs.get_mut(&tab) {
                        t.tell(&ToTab::Mouse { button: 0, pressed: true, motion: true, col: cell.0, row: cell.1 });
                    }
                } else if let Some((start, _)) = self.wins[w].selecting {
                    self.wins[w].selecting = Some((start, cell));
                    if let Some(t) = self.tabs.get_mut(&tab) {
                        t.tell(&ToTab::Select(Some((start, cell))));
                    }
                }
            }
        }
    }

    fn button(&mut self, w: usize, button: PointerButton, pressed: bool) {
        let (x, y) = self.wins[w].pointer;
        let hover = self.wins[w].hover;
        if button != PointerButton::Left {
            // The other buttons, to a program that asked for the mouse; the right pastes for
            // one that did not.
            let Some(tab) = self.active_tab(w) else { return };
            let cell = self.cell_at(w, x, y);
            let mouse = self.tabs.get(&tab).is_some_and(|t| t.screen.mouse);
            if let (true, Some(cell)) = (mouse, cell) {
                let b = if button == PointerButton::Middle { 1 } else { 2 };
                if let Some(t) = self.tabs.get_mut(&tab) {
                    t.tell(&ToTab::Mouse { button: b, pressed, motion: false, col: cell.0, row: cell.1 });
                }
            } else if pressed && button == PointerButton::Right {
                self.paste(w);
            }
            return;
        }
        if pressed {
            match hover {
                Hover::Tab(tab) => {
                    if let Some(at) = self.wins[w].tabs.iter().position(|&t| t == tab) {
                        self.wins[w].active = at;
                    }
                    self.wins[w].drag = Some(Drag { tab, from: (x, y), at: (x, y), moved: false, torn: false });
                }
                Hover::Close(tab) => self.close_tab(tab),
                Hover::Plus => {
                    if let Some(tab) = self.new_tab() {
                        let win = &mut self.wins[w];
                        win.tabs.push(tab);
                        win.active = win.tabs.len() - 1;
                        self.size_tabs();
                    }
                }
                Hover::Text => {
                    let Some(tab) = self.active_tab(w) else { return };
                    if let Some(TabState::Crashed(_)) = self.tabs.get(&tab).map(|t| &t.state) {
                        return;
                    }
                    let Some(cell) = self.cell_at(w, x, y) else { return };
                    self.wins[w].pressed_in_text = true;
                    let mouse = self.tabs.get(&tab).is_some_and(|t| t.screen.mouse);
                    let t = self.tabs.get_mut(&tab).unwrap();
                    if mouse {
                        t.tell(&ToTab::Mouse { button: 0, pressed: true, motion: false, col: cell.0, row: cell.1 });
                    } else {
                        self.wins[w].selecting = Some((cell, cell));
                        t.tell(&ToTab::Select(None));
                    }
                }
                Hover::Nothing => {}
            }
            return;
        }
        // Let go.
        if let Some(drag) = self.wins[w].drag.take() {
            if drag.torn {
                let alone = self.wins[w].tabs.len() == 1;
                if alone && self.wins.len() == 1 {
                    // The only tab of the only window: there is nowhere for it to go.
                    return;
                }
                // Dropped on another window if the pointer arrives on one straight away;
                // its own window otherwise. See `in_flight`.
                self.in_flight = Some((drag.tab, self.wins[w].id, Instant::now()));
            }
            return;
        }
        if std::mem::take(&mut self.wins[w].pressed_in_text) {
            let Some(tab) = self.active_tab(w) else { return };
            let cell = self.cell_at(w, x, y).unwrap_or((0, 0));
            let mouse = self.tabs.get(&tab).is_some_and(|t| t.screen.mouse);
            let t = self.tabs.get_mut(&tab).unwrap();
            if mouse {
                t.tell(&ToTab::Mouse { button: 0, pressed: false, motion: false, col: cell.0, row: cell.1 });
            } else if let Some((start, end)) = self.wins[w].selecting.take() {
                if start == end {
                    // A click, not a drag: twice on one place selects the line's word there.
                    let double = self.wins[w].last_click.is_some_and(|(at, c)| at.elapsed() < Duration::from_millis(400) && c == start);
                    self.wins[w].last_click = Some((Instant::now(), start));
                    if double {
                        let (from, to) = word_at(&t.screen, start);
                        t.tell(&ToTab::Select(Some((from, to))));
                        t.tell(&ToTab::Copy);
                    } else {
                        t.tell(&ToTab::Select(None));
                    }
                } else {
                    // What is selected is copied, as on every Linux desktop.
                    t.tell(&ToTab::Copy);
                }
            }
        }
    }

    fn scroll(&mut self, w: usize, dy: f32) {
        let Some(tab) = self.active_tab(w) else { return };
        let (x, y) = self.wins[w].pointer;
        let cell = self.cell_at(w, x, y).unwrap_or((0, 0));
        let ch = self.grid.as_ref().map_or(30.0, |g| g.cell.1);
        let Some(t) = self.tabs.get_mut(&tab) else { return };
        let lines = (dy / ch).round() as i32;
        let lines = if lines == 0 { dy.signum() as i32 } else { lines };
        if t.screen.mouse || t.screen.alternate {
            let button = if dy < 0.0 { 64 } else { 65 };
            for _ in 0..lines.unsigned_abs().min(10) {
                t.tell(&ToTab::Mouse { button, pressed: true, motion: false, col: cell.0, row: cell.1 });
            }
        } else {
            t.tell(&ToTab::Scroll(-lines));
        }
    }

    fn paste(&mut self, w: usize) {
        let text = system_paste().filter(|t| !t.is_empty()).unwrap_or_else(|| self.clipboard.clone());
        if text.is_empty() {
            return;
        }
        if let Some(t) = self.active_tab(w).and_then(|t| self.tabs.get_mut(&t)) {
            t.tell(&ToTab::Paste(text));
        }
    }

    fn key(&mut self, w: usize, key: Key, code: u32, text: String, m: Modifiers) {
        // SpatiQuill's own keys: the Command key on a Mac, Control and Shift together elsewhere.
        let command = m.logo || (m.control && m.shift);
        let letter = match key {
            Key::Char(c) => Some(c.to_ascii_lowercase()),
            _ => None,
        };
        let id = self.wins[w].id;
        if command {
            match (letter, key) {
                (Some('t'), _) => {
                    if let Some(tab) = self.new_tab() {
                        let win = &mut self.wins[w];
                        win.tabs.insert(win.active + 1, tab);
                        win.active += 1;
                        self.size_tabs();
                    }
                    return;
                }
                (Some('w'), _) => {
                    if let Some(tab) = self.active_tab(w) {
                        self.close_tab(tab);
                    }
                    return;
                }
                (Some('n'), _) => {
                    self.new_window();
                    return;
                }
                (Some('c'), _) => {
                    if let Some(t) = self.active_tab(w).and_then(|t| self.tabs.get_mut(&t)) {
                        t.tell(&ToTab::Copy);
                    }
                    return;
                }
                (Some('v'), _) => {
                    self.paste(w);
                    return;
                }
                (Some('=') | Some('+'), _) => return self.zoom(1.1),
                (Some('-'), _) => return self.zoom(1.0 / 1.1),
                (Some('0'), _) => return self.zoom(0.0),
                (Some(d @ '1'..='9'), _) => {
                    let win = &mut self.wins[w];
                    let at = if d == '9' { win.tabs.len().saturating_sub(1) } else { (d as usize - '1' as usize).min(win.tabs.len().saturating_sub(1)) };
                    win.active = at;
                    return;
                }
                // Moved along the bar.
                (_, Key::PageUp) | (_, Key::PageDown) if m.control && m.shift => {
                    let win = &mut self.wins[w];
                    let to = if key == Key::PageUp { win.active.saturating_sub(1) } else { (win.active + 1).min(win.tabs.len() - 1) };
                    win.tabs.swap(win.active, to);
                    win.active = to;
                    return;
                }
                // To the next SpatiQuill window, or a new one.
                (Some('m'), _) => {
                    let Some(tab) = self.active_tab(w) else { return };
                    let next = self.wins.get((w + 1) % self.wins.len()).map(|w| w.id).filter(|&n| n != id);
                    match next {
                        Some(next) => self.move_tab(tab, next, None),
                        None if self.wins[w].tabs.len() > 1 => {
                            let new = self.open_window(Vec::new());
                            self.move_tab(tab, new, None);
                        }
                        None => {}
                    }
                    return;
                }
                _ => {}
            }
        }
        if m.control && !m.shift && matches!(key, Key::PageUp | Key::PageDown) || m.control && key == Key::Tab {
            let win = &mut self.wins[w];
            let n = win.tabs.len().max(1);
            let back = key == Key::PageUp || (key == Key::Tab && m.shift);
            win.active = if back { (win.active + n - 1) % n } else { (win.active + 1) % n };
            return;
        }
        let Some(tab) = self.active_tab(w) else { return };
        let Some(t) = self.tabs.get_mut(&tab) else { return };
        if let TabState::Crashed(_) = t.state {
            if key == Key::Enter {
                self.restart(tab);
            }
            return;
        }
        if m.shift && matches!(key, Key::PageUp | Key::PageDown) && !t.screen.alternate {
            let page = t.screen.rows as i32 - 1;
            t.tell(&ToTab::Scroll(if key == Key::PageUp { page } else { -page }));
            return;
        }
        if m.shift && key == Key::Home && !t.screen.alternate {
            t.tell(&ToTab::Scroll(i32::MAX / 2));
            return;
        }
        if m.shift && key == Key::End && !t.screen.alternate {
            t.tell(&ToTab::Scroll(i32::MIN));
            return;
        }
        if m.logo {
            return;
        }
        self.wins[w].selecting = None;
        t.tell(&ToTab::Key { key: keys::number(key), code, text, shift: m.shift, control: m.control, alt: m.alt });
    }

    fn zoom(&mut self, by: f32) {
        self.text_px = if by == 0.0 { TEXT_PX } else { (self.text_px * by).clamp(12.0, 64.0) };
        self.resize_text = true;
    }

    fn draw(&mut self) -> Result<(), String> {
        let blink_on = self.typed_at.elapsed() < Duration::from_millis(900) || (self.typed_at.elapsed().as_millis() / 530) % 2 == 0;
        for w in 0..self.wins.len() {
            if !self.wins[w].ready {
                continue;
            }
            let id = self.wins[w].id;
            // Text first: the cells it makes decide how many fit.
            if self.grid.is_none() || self.resize_text {
                let px = self.text_px;
                let gl = self.app.gl()?;
                match self.grid.as_mut() {
                    Some(grid) => grid.set_size(gl, px),
                    None => self.grid = Some(Grid::new(gl, px)?),
                }
                self.resize_text = false;
                for win in &mut self.wins {
                    win.dirty = true;
                }
                self.size_tabs();
            }
            let size = self.wins[w].size;
            let (area, _, _) = self.text_area(size);
            let title = self.active_tab(w).and_then(|t| self.tabs.get(&t)).map(|t| t.label()).unwrap_or_default();
            self.app.set_title(id, &format!("{title} \u{2014} SpatiQuill"));
            let (painter, grid, wins, tabs) = (&mut self.painter, &mut self.grid, &mut self.wins, &self.tabs);
            let win = &mut wins[w];
            win.dirty = false;
            self.app.draw(id, |gl, picture| {
                let p = painter.get_or_insert_with(|| Painter::new(gl).expect("the painter"));
                p.begin(gl, picture, picture.1 as f32 / size.1 as f32, theme::GROUND);
                look::backdrop(p, gl);
                let all = p.bounds();
                // The terminal's own ground, exactly the colour a cell with no colour of its
                // own is, so that what is printed and what is not cannot be told apart by it.
                let bg = tab::BACKGROUND;
                let ground = Rect::new(area.x - 10.0, area.y - 6.0, all.w - (area.x - 10.0) * 2.0, all.h - area.y + 6.0 - 8.0);
                p.fill(gl, ground, 16.0, [bg[0] as f32 / 255.0, bg[1] as f32 / 255.0, bg[2] as f32 / 255.0, 1.0]);
                draw_bar(p, gl, win, tabs, all);
                p.end(gl);
                let active = win.tabs.get(win.active).and_then(|t| tabs.get(t));
                if let (Some(grid), Some(tab)) = (grid.as_mut(), active) {
                    if !tab.screen.cells.is_empty() {
                        let scale = picture.1 as f32 / size.1 as f32;
                        let _ = scale;
                        grid.draw(gl, picture, area.x.round(), area.y.round(), &tab.screen, win.focused, blink_on, [0.45, 0.72, 1.0, 1.0], [0.216, 0.651, 0.969, 0.40]);
                    }
                }
                // Over the text: a tab that stopped, how far up the history the view is, and
                // a tab on its way somewhere.
                p.begin_over(gl, picture, Rect::new(0.0, 0.0, picture.0 as f32, picture.1 as f32), (size.0 as f32, size.1 as f32));
                if let Some(tab) = active {
                    if let TabState::Crashed(why) = &tab.state {
                        let pane = Rect::new(all.w * 0.5 - 360.0, all.h * 0.5 - 110.0, 720.0, 220.0);
                        look::glass(p, gl, pane, 24.0, 1.0);
                        let mut inner = pane.inset(28.0);
                        p.label(gl, "This tab stopped", inner.take_top(52.0), theme::TITLE_EM * 0.8, look::TROUBLE, 0.0);
                        p.label(gl, why, inner.take_top(44.0), theme::DETAIL_EM, theme::INK_DETAIL, 0.0);
                        inner.take_top(16.0);
                        p.label(gl, "Enter starts a new shell here.    The other tabs are as they were.", inner.take_top(36.0), theme::FOOTER_EM, theme::INK_HINT, 0.0);
                    }
                    if tab.screen.scrolled > 0 && tab.screen.history > 0 {
                        let track = Rect::new(all.w - 12.0, area.y, 5.0, area.h);
                        p.fill(gl, track, 2.5, theme::SCROLL_TRACK);
                        let total = tab.screen.history as f32 + tab.screen.rows as f32;
                        let h = (track.h * tab.screen.rows as f32 / total).max(30.0);
                        let top = track.y + (track.h - h) * (1.0 - tab.screen.scrolled as f32 / tab.screen.history as f32);
                        p.fill(gl, Rect { y: top, h, ..track }, 2.5, theme::SCROLL_THUMB);
                        let note = format!("{} lines up    Shift-End goes to the foot", tab.screen.scrolled);
                        let width = p.measure(gl, &note, theme::FOOTER_EM * 0.85, 8192.0).0 + 36.0;
                        let pill = Rect::new(all.w - width - 30.0, all.h - 64.0, width, 40.0);
                        look::glass(p, gl, pill, 20.0, 1.0);
                        p.label(gl, &note, pill, theme::FOOTER_EM * 0.85, theme::INK_DETAIL, 0.5);
                    }
                }
                if let Some(drag) = win.drag.as_ref().filter(|d| d.torn) {
                    let label = tabs.get(&drag.tab).map(|t| t.label()).unwrap_or_default();
                    let ghost = Rect::new(drag.at.0 - 140.0, drag.at.1 - 26.0, 280.0, 52.0);
                    p.glow(gl, ghost, 26.0, [0.30, 0.62, 1.0, 0.45], 26.0);
                    look::glass(p, gl, ghost, 26.0, 1.0);
                    p.label(gl, &label, ghost.inset(16.0), theme::ROW_EM * 0.8, theme::INK, 0.5);
                    let hint = if win.tabs.len() == 1 && true { "Let go over another SpatiQuill window to move it there" } else { "Let go to open it in a window of its own, or over another SpatiQuill window" };
                    let width = p.measure(gl, hint, theme::FOOTER_EM * 0.85, 8192.0).0 + 36.0;
                    let pill = Rect::new((all.w - width) * 0.5, all.h - 64.0, width, 40.0);
                    look::glass(p, gl, pill, 20.0, 1.0);
                    p.label(gl, hint, pill, theme::FOOTER_EM * 0.85, theme::INK_DETAIL, 0.5);
                }
                p.end(gl);
            })?;
        }
        Ok(())
    }

    fn run(&mut self) -> Result<(), String> {
        loop {
            let blinking = self.wins.iter().any(|w| w.focused);
            let wait = if self.in_flight.is_some() { 30 } else if blinking { 265 } else { 1000 };
            for event in self.app.wait(Some(Duration::from_millis(wait)))? {
                if !self.handle(event) {
                    return Ok(());
                }
            }
            self.hear();
            // A tab let go of that no window claimed becomes a window of its own.
            if let Some((tab, from, at)) = self.in_flight {
                if at.elapsed() >= Duration::from_millis(600) {
                    self.in_flight = None;
                    let alone = self.win(from).is_some_and(|w| w.tabs.len() <= 1);
                    if !alone {
                        let new = self.open_window(Vec::new());
                        self.move_tab(tab, new, None);
                    }
                }
            }
            // Another start of SpatiQuill, asking for a window.
            let mut asked_windows = 0;
            if let Some((listener, _)) = self.listener.as_ref() {
                while let Ok((mut stream, _)) = listener.accept() {
                    let mut asked = String::new();
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                    let _ = stream.read_to_string(&mut asked);
                    if asked.trim() == "window" {
                        asked_windows += 1;
                    }
                }
            }
            for _ in 0..asked_windows {
                self.new_window();
            }
            if self.wins.is_empty() {
                return Ok(());
            }
            self.draw()?;
        }
    }
}

/// The word around a cell: the run of characters that are not spaces or brackets.
fn word_at(screen: &Screen, at: (u16, u16)) -> ((u16, u16), (u16, u16)) {
    let cols = screen.cols as usize;
    let row = at.1 as usize;
    let line = &screen.cells[row * cols..(row + 1) * cols];
    let is_word = |c: char| !c.is_whitespace() && !"()[]{}<>'\"`|;,".contains(c);
    let (mut a, mut b) = (at.0 as usize, at.0 as usize);
    if !is_word(line[a].c) {
        return (at, at);
    }
    while a > 0 && is_word(line[a - 1].c) {
        a -= 1;
    }
    while b + 1 < cols && is_word(line[b + 1].c) {
        b += 1;
    }
    ((a as u16, at.1), (b as u16, at.1))
}

fn draw_bar(p: &mut Painter, gl: &spatiand_app::Gl, win: &mut Win, tabs: &HashMap<u64, Tab>, all: Rect) {
    let bar = Rect::new(all.x + 12.0, all.y + 10.0, all.w - 24.0, BAR - 16.0);
    let plus_w = 50.0;
    let n = win.tabs.len().max(1);
    let width = ((bar.w - plus_w - 16.0) / n as f32).clamp(150.0, 330.0);
    win.placed.clear();
    let mut x = bar.x;
    for (index, &id) in win.tabs.iter().enumerate() {
        let Some(tab) = tabs.get(&id) else { continue };
        let rect = Rect::new(x, bar.y, width - 8.0, bar.h);
        x += width;
        if rect.right() > bar.right() - plus_w {
            break;
        }
        let active = index == win.active;
        let hover = matches!(win.hover, Hover::Tab(t) | Hover::Close(t) if t == id);
        let dragged = win.drag.as_ref().is_some_and(|d| d.tab == id && d.moved);
        let rect = if dragged && !win.drag.as_ref().unwrap().torn {
            let d = win.drag.as_ref().unwrap();
            Rect { x: (d.at.0 - rect.w * 0.5).clamp(bar.x, bar.right() - plus_w - rect.w), ..rect }
        } else {
            rect
        };
        if active {
            p.glow(gl, rect, 14.0, [0.30, 0.62, 1.0, if win.focused { 0.18 } else { 0.06 }], 16.0);
            p.gradient(gl, rect, 14.0, [1.0, 1.0, 1.0, 0.11], [1.0, 1.0, 1.0, 0.05]);
            look::rule(p, gl, Rect::new(rect.x + 16.0, rect.bottom() - 3.0, rect.w - 32.0, 2.5));
        } else if hover {
            p.fill(gl, rect, 14.0, theme::HOVER);
        }
        // A mark for how the tab is: running, rung, stopped.
        let colour = match tab.state {
            TabState::Crashed(_) => look::TROUBLE,
            TabState::Running if tab.rang.is_some_and(|t| t.elapsed() < Duration::from_secs(3)) => [1.0, 0.85, 0.4, 1.0],
            TabState::Running => if active { look::TEAL } else { theme::INK_HINT },
        };
        p.fill(gl, Rect::new(rect.x + 16.0, rect.y + rect.h * 0.5 - 4.0, 8.0, 8.0), 4.0, colour);
        let close = Rect::new(rect.right() - 38.0, rect.y + rect.h * 0.5 - 13.0, 26.0, 26.0);
        let label = Rect { x: rect.x + 34.0, w: rect.w - 34.0 - if active || hover { 44.0 } else { 14.0 }, ..rect };
        p.label(gl, &tab.label(), label, theme::ROW_EM * 0.72, if active { theme::INK } else { theme::INK_ROW }, 0.0);
        if active || hover {
            if win.hover == Hover::Close(id) {
                p.fill(gl, close, 13.0, [1.0, 1.0, 1.0, 0.14]);
            }
            p.label(gl, "\u{2715}", close, theme::FOOTER_EM * 0.8, theme::INK_DETAIL, 0.5);
        }
        win.placed.push((id, rect, close));
    }
    win.plus = Rect::new(x.min(bar.right() - plus_w), bar.y + (bar.h - 44.0) * 0.5, 44.0, 44.0);
    if win.hover == Hover::Plus {
        p.fill(gl, win.plus, 22.0, theme::HOVER);
    }
    p.label(gl, "+", win.plus, theme::ROW_EM, theme::INK_DETAIL, 0.5);
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--tab") {
        // A tab's log is its window's, which is the program that started it.
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
        if let Err(e) = tab::run() {
            eprintln!("spatiquill tab: {e}");
            std::process::exit(1);
        }
        return;
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    // Already running in this room: that SpatiQuill opens the window.
    let path = socket_path();
    if let Ok(mut stream) = UnixStream::connect(&path) {
        if stream.write_all(b"window").is_ok() {
            log::info!("SpatiQuill is already running; it opens the window");
            return;
        }
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).ok().map(|l| {
        let _ = l.set_nonblocking(true);
        (l, path.clone())
    });
    let run = || -> Result<(), String> {
        let app = App::connect("id.prasetya.spatiand.spatiquill")?;
        let mut quill = Quill {
            app,
            painter: None,
            grid: None,
            text_px: TEXT_PX,
            tabs: HashMap::new(),
            wins: Vec::new(),
            next_tab: 1,
            listener,
            in_flight: None,
            clipboard: String::new(),
            typed_at: Instant::now(),
            resize_text: false,
        };
        if let Some((l, _)) = quill.listener.as_ref() {
            let fd = l.as_raw_fd();
            quill.app.watch(fd);
        }
        quill.new_window();
        if quill.wins.is_empty() {
            return Err("no tab would start".into());
        }
        let ended = quill.run();
        let all: Vec<u64> = quill.tabs.keys().copied().collect();
        for tab in all {
            quill.close_tab(tab);
        }
        ended
    };
    let ended = run();
    let _ = std::fs::remove_file(&path);
    if let Err(e) = ended {
        log::error!("{e}");
        std::process::exit(1);
    }
}
