//! A tab: a process of its own holding a shell in a pseudo-terminal, read by alacritty's
//! emulator, and telling its window what the screen shows.
//!
//! Run as `spatiquill --tab`, with the window on the other end of its standard input and
//! output. Whatever happens in here -- a program that floods the terminal, a crash in the
//! emulator -- ends this process and no other.

use std::collections::HashMap;
use std::io;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener, Notify, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};

use crate::wire::{self, flags, Cell, Cursor, FromTab, Screen, ToTab};

/// The sixteen colours, and the terminal's own, in the room's palette: the ground of its
/// cards, its ink, and the Spatiand mark's teal, blue and violet among the usual eight.
pub const PALETTE: [[u8; 3]; 16] = [
    [0x1a, 0x1e, 0x2a], // black
    [0xff, 0x6b, 0x7a], // red
    [0x4c, 0xe0, 0x9a], // green
    [0xf2, 0xc9, 0x5c], // yellow
    [0x37, 0xa6, 0xf7], // blue: the mark's
    [0x8f, 0x6b, 0xf2], // magenta: the mark's violet
    [0x16, 0xe6, 0xc0], // cyan: the mark's teal
    [0xc8, 0xd0, 0xe0], // white
    [0x4a, 0x52, 0x66], // bright black
    [0xff, 0x8f, 0x9b],
    [0x7d, 0xf0, 0xb6],
    [0xff, 0xdc, 0x85],
    [0x6f, 0xc0, 0xff],
    [0xb0, 0x96, 0xff],
    [0x5f, 0xf5, 0xd8],
    [0xf4, 0xf6, 0xfb],
];
pub const FOREGROUND: [u8; 3] = [0xd6, 0xdc, 0xe8];
pub const BACKGROUND: [u8; 3] = [0x0b, 0x0d, 0x13];

#[derive(Clone)]
struct Listener(Sender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// The 256 colours xterm has: the sixteen, a cube of 216, and 24 greys.
fn indexed(i: u8, term: &alacritty_terminal::term::color::Colors) -> [u8; 3] {
    if let Some(rgb) = term[i as usize] {
        return [rgb.r, rgb.g, rgb.b];
    }
    match i {
        0..=15 => PALETTE[i as usize],
        16..=231 => {
            let i = i - 16;
            let step = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            [step(i / 36), step((i / 6) % 6), step(i % 6)]
        }
        _ => {
            let v = 8 + (i - 232) * 10;
            [v, v, v]
        }
    }
}

fn resolve(color: Color, term: &alacritty_terminal::term::color::Colors, bold: bool) -> [u8; 3] {
    match color {
        Color::Spec(Rgb { r, g, b }) => [r, g, b],
        Color::Indexed(i) => indexed(if bold && i < 8 { i + 8 } else { i }, term),
        Color::Named(named) => {
            let index = named as usize;
            if let Some(rgb) = term[index] {
                return [rgb.r, rgb.g, rgb.b];
            }
            match named {
                NamedColor::Foreground | NamedColor::BrightForeground => FOREGROUND,
                NamedColor::DimForeground => [0x9a, 0xa2, 0xb4],
                NamedColor::Background => BACKGROUND,
                NamedColor::Cursor => [0x73, 0xb8, 0xff],
                _ if index < 16 => PALETTE[if bold && index < 8 { index + 8 } else { index }],
                _ if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&index) => {
                    let c = PALETTE[index - NamedColor::DimBlack as usize];
                    [(c[0] as u16 * 2 / 3) as u8, (c[1] as u16 * 2 / 3) as u8, (c[2] as u16 * 2 / 3) as u8]
                }
                _ => FOREGROUND,
            }
        }
    }
}

fn snapshot(term: &Term<Listener>) -> Screen {
    let content = term.renderable_content();
    let (cols, rows) = (term.columns(), term.screen_lines());
    let mut cells = vec![Cell { c: ' ', fg: FOREGROUND, bg: BACKGROUND, flags: flags::PLAIN_BG }; cols * rows];
    let offset = content.display_offset as i32;
    let selection = content.selection;
    for indexed in content.display_iter {
        let point = indexed.point;
        let row = point.line.0 + offset;
        if row < 0 || row as usize >= rows || point.column.0 >= cols {
            continue;
        }
        let cell = indexed.cell;
        let f = cell.flags;
        let bold = f.contains(Flags::BOLD);
        let mut fg = resolve(cell.fg, content.colors, bold);
        let mut bg = resolve(cell.bg, content.colors, false);
        let mut out = 0u16;
        let plain_bg = cell.bg == Color::Named(NamedColor::Background);
        if f.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        } else if plain_bg {
            out |= flags::PLAIN_BG;
        }
        for (from, to) in [
            (Flags::BOLD, flags::BOLD),
            (Flags::ITALIC, flags::ITALIC),
            (Flags::ALL_UNDERLINES, flags::UNDERLINE),
            (Flags::STRIKEOUT, flags::STRIKE),
            (Flags::WIDE_CHAR, flags::WIDE),
            (Flags::WIDE_CHAR_SPACER, flags::SPACER),
            (Flags::DIM, flags::DIM),
            (Flags::HIDDEN, flags::HIDDEN),
        ] {
            if f.intersects(from) {
                out |= to;
            }
        }
        if selection.is_some_and(|s| s.contains(point)) {
            out |= flags::SELECTED;
        }
        cells[row as usize * cols + point.column.0] = Cell { c: cell.c, fg, bg, flags: out };
    }
    let cursor_row = content.cursor.point.line.0 + offset;
    let shape = if !content.mode.contains(TermMode::SHOW_CURSOR) || cursor_row < 0 || cursor_row as usize >= rows {
        Cursor::Hidden
    } else {
        match content.cursor.shape {
            CursorShape::Hidden => Cursor::Hidden,
            CursorShape::Underline => Cursor::Underline,
            CursorShape::Beam => Cursor::Beam,
            _ => Cursor::Block,
        }
    };
    Screen {
        cols: cols as u16,
        rows: rows as u16,
        cursor: (content.cursor.point.column.0 as u16, cursor_row.max(0) as u16),
        cursor_shape: shape,
        scrolled: content.display_offset as u32,
        history: term.grid().history_size() as u32,
        mouse: content.mode.intersects(TermMode::MOUSE_MODE),
        alternate: content.mode.contains(TermMode::ALT_SCREEN),
        cells,
    }
}

/// The shell this platform's people use: theirs, by `$SHELL`, or the system's.
fn shell() -> Option<tty::Shell> {
    if cfg!(target_os = "android") {
        return Some(tty::Shell::new("/system/bin/sh".into(), vec!["-l".into()]));
    }
    // On a Mac alacritty logs the shell in through `login`, which is what Terminal does.
    None
}

/// A mouse report, as a program that asked for the mouse wants it: SGR's if it asked for
/// that, the old six-byte form if not.
fn mouse_report(mode: TermMode, button: u8, pressed: bool, motion: bool, col: u16, row: u16) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    if motion && !mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) {
        return None;
    }
    let code = button + if motion { 32 } else { 0 };
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if pressed || button >= 64 { 'M' } else { 'm' };
        return Some(format!("\x1b[<{code};{};{}{end}", col + 1, row + 1).into_bytes());
    }
    let code = if pressed || button >= 64 { code } else { 3 };
    let (x, y) = ((col as u32 + 33).min(255) as u8, (row as u32 + 33).min(255) as u8);
    Some(vec![0x1b, b'[', b'M', 32 + code, x, y])
}

pub fn run() -> io::Result<()> {
    let mut input = io::stdin();
    let output = Arc::new(std::sync::Mutex::new(io::stdout()));
    // The first thing a window says is how big the tab is.
    let (cols, rows, cell_w, cell_h) = loop {
        match wire::read_to_tab(&mut input)? {
            Some(ToTab::Resize { cols, rows, cell_w, cell_h }) => break (cols.max(2), rows.max(1), cell_w, cell_h),
            Some(_) => continue,
            None => return Ok(()),
        }
    };
    let (events, heard) = channel();
    let listener = Listener(events);
    let size = Size { cols: cols as usize, rows: rows as usize };
    let term = Arc::new(FairMutex::new(Term::new(Config::default(), &size, listener.clone())));
    let home = std::env::var_os("SPATIQUILL_FOLDER").or_else(|| std::env::var_os("HOME")).map(std::path::PathBuf::from);
    let mut env = HashMap::new();
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env.insert("COLORTERM".to_string(), "truecolor".to_string());
    env.insert("TERM_PROGRAM".to_string(), "SpatiQuill".to_string());
    let options = tty::Options { shell: shell(), working_directory: home, drain_on_exit: true, env };
    let window_size = WindowSize { num_lines: rows, num_cols: cols, cell_width: cell_w, cell_height: cell_h };
    let pty = tty::new(&options, window_size, 0)?;
    let (master, shell_pid) = (std::os::fd::AsRawFd::as_raw_fd(pty.file()), pty.child().id() as i32);
    let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)?;
    let loop_tx = event_loop.channel();
    let notifier = Notifier(loop_tx.clone());
    let _io = event_loop.spawn();

    // What the window says, on a thread of its own: typing, resizing, the mouse.
    let reading_term = term.clone();
    let reading_output = output.clone();
    let (asked_tx, asked) = channel::<ToTab>();
    std::thread::spawn(move || {
        while let Ok(Some(message)) = wire::read_to_tab(&mut input) {
            if asked_tx.send(message).is_err() {
                return;
            }
        }
        let _ = asked_tx.send(ToTab::Quit);
    });

    let mut dirty = true;
    // The title the program set, if it set one; the tab is named by what is running otherwise.
    let mut set_title = false;
    let mut named = String::new();
    let mut looked = Instant::now() - Duration::from_secs(5);
    let mut last_sent = Instant::now() - Duration::from_secs(1);
    let mut last_screen: Option<Screen> = None;
    let send = |message: FromTab| -> bool {
        let mut out = reading_output.lock().unwrap();
        wire::send(&mut *out, &message.encode()).is_ok()
    };
    loop {
        // Whatever the window has asked, then whatever the terminal has said.
        while let Ok(message) = asked.try_recv() {
            dirty = true;
            match message {
                ToTab::Key { key, code, text, shift, control, alt } => {
                    let mut term = reading_term.lock();
                    let app_cursor = term.mode().contains(TermMode::APP_CURSOR);
                    // Typing brings the view back down to where the typing is.
                    if term.grid().display_offset() != 0 {
                        term.scroll_display(Scroll::Bottom);
                    }
                    term.selection = None;
                    drop(term);
                    notifier.notify(crate::keys::bytes(key, code, &text, shift, control, alt, app_cursor));
                }
                ToTab::Paste(text) => {
                    let bracketed = reading_term.lock().mode().contains(TermMode::BRACKETED_PASTE);
                    let text = text.replace("\r\n", "\r").replace('\n', "\r");
                    if bracketed {
                        notifier.notify(format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes());
                    } else {
                        notifier.notify(text.into_bytes());
                    }
                }
                ToTab::Resize { cols, rows, cell_w, cell_h } => {
                    let size = WindowSize { num_lines: rows.max(1), num_cols: cols.max(2), cell_width: cell_w, cell_height: cell_h };
                    reading_term.lock().resize(Size { cols: size.num_cols as usize, rows: size.num_lines as usize });
                    let _ = loop_tx.send(Msg::Resize(size));
                }
                ToTab::Scroll(lines) => {
                    let mut term = reading_term.lock();
                    if lines == i32::MIN {
                        term.scroll_display(Scroll::Bottom);
                    } else {
                        term.scroll_display(Scroll::Delta(lines));
                    }
                }
                ToTab::Mouse { button, pressed, motion, col, row } => {
                    let mode = *reading_term.lock().mode();
                    if let Some(bytes) = mouse_report(mode, button, pressed, motion, col, row) {
                        notifier.notify(bytes);
                    } else if button >= 64 && mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
                        // A full-screen program that did not ask for the mouse is given the
                        // wheel as the arrow keys, which is what less and man want.
                        let arrow = if button == 64 { "A" } else { "B" };
                        let app = mode.contains(TermMode::APP_CURSOR);
                        let seq = if app { format!("\x1bO{arrow}") } else { format!("\x1b[{arrow}") };
                        notifier.notify(seq.repeat(3).into_bytes());
                    }
                }
                ToTab::Select(range) => {
                    let mut term = reading_term.lock();
                    let offset = term.grid().display_offset() as i32;
                    term.selection = range.map(|((c0, r0), (c1, r1))| {
                        let at = |c: u16, r: u16| Point::new(Line(r as i32 - offset), Column(c as usize));
                        let (a, b) = if (r0, c0) <= (r1, c1) { (at(c0, r0), at(c1, r1)) } else { (at(c1, r1), at(c0, r0)) };
                        let mut selection = Selection::new(SelectionType::Simple, a, Side::Left);
                        selection.update(b, Side::Right);
                        selection
                    });
                }
                ToTab::Copy => {
                    let text = reading_term.lock().selection_to_string().unwrap_or_default();
                    if !send(FromTab::Copied(text)) {
                        return Ok(());
                    }
                }
                ToTab::Focus(on) => {
                    let mode = *reading_term.lock().mode();
                    if mode.contains(TermMode::FOCUS_IN_OUT) {
                        notifier.notify(if on { b"\x1b[I".to_vec() } else { b"\x1b[O".to_vec() });
                    }
                }
                ToTab::Quit => {
                    let _ = loop_tx.send(Msg::Shutdown);
                    return Ok(());
                }
            }
        }
        // A screen at most every eight milliseconds, however fast the program writes.
        let wait = if dirty { Duration::from_millis(8).saturating_sub(last_sent.elapsed()) } else { Duration::from_millis(50) };
        match heard.recv_timeout(wait) {
            Ok(event) => match event {
                Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange => dirty = true,
                Event::Title(title) => {
                    set_title = !title.trim().is_empty();
                    if !send(FromTab::Title(title)) {
                        return Ok(());
                    }
                }
                Event::ResetTitle => {
                    set_title = false;
                    named.clear();
                }
                Event::Bell => {
                    let _ = send(FromTab::Bell);
                }
                Event::PtyWrite(text) => notifier.notify(text.into_bytes()),
                Event::TextAreaSizeRequest(answer) => {
                    let term = reading_term.lock();
                    let size = WindowSize { num_lines: term.screen_lines() as u16, num_cols: term.columns() as u16, cell_width: cell_w, cell_height: cell_h };
                    drop(term);
                    notifier.notify(answer(size).into_bytes());
                }
                Event::ColorRequest(index, answer) => {
                    let term = reading_term.lock();
                    let c = match index {
                        256 => FOREGROUND,
                        257 => BACKGROUND,
                        i if i < 256 => indexed(i as u8, term.colors()),
                        _ => FOREGROUND,
                    };
                    drop(term);
                    notifier.notify(answer(Rgb { r: c[0], g: c[1], b: c[2] }).into_bytes());
                }
                Event::ClipboardStore(_, text) => {
                    let _ = send(FromTab::Copied(text));
                }
                Event::ClipboardLoad(..) => {}
                Event::ChildExit(status) => {
                    // What was last on the screen is sent first, so that a shell's last words
                    // are seen even though the tab is going.
                    let screen = snapshot(&reading_term.lock());
                    let _ = send(FromTab::Screen(screen));
                    let _ = send(FromTab::Exited(status.code().unwrap_or(-1)));
                    return Ok(());
                }
                Event::Exit => {
                    let _ = send(FromTab::Exited(0));
                    return Ok(());
                }
            },
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        // Once a second, what is running in front and where, for a tab no program has named.
        if !set_title && looked.elapsed() >= Duration::from_secs(1) {
            looked = Instant::now();
            let name = crate::proc::describe(master, shell_pid);
            if !name.is_empty() && name != named {
                named = name.clone();
                if !send(FromTab::Title(name)) {
                    return Ok(());
                }
            }
        }
        if dirty && last_sent.elapsed() >= Duration::from_millis(8) {
            let screen = snapshot(&reading_term.lock());
            dirty = false;
            if last_screen.as_ref() != Some(&screen) {
                last_sent = Instant::now();
                if !send(FromTab::Screen(screen.clone())) {
                    return Ok(());
                }
                last_screen = Some(screen);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cube_and_the_greys_are_xterms() {
        let colours = alacritty_terminal::term::color::Colors::default();
        assert_eq!(indexed(16, &colours), [0, 0, 0]);
        assert_eq!(indexed(231, &colours), [255, 255, 255]);
        assert_eq!(indexed(196, &colours), [255, 0, 0]);
        assert_eq!(indexed(232, &colours), [8, 8, 8]);
    }

    #[test]
    fn a_program_that_asked_for_the_mouse_hears_it_in_its_own_form() {
        let sgr = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(mouse_report(sgr, 0, true, false, 4, 2).unwrap(), b"\x1b[<0;5;3M");
        assert_eq!(mouse_report(sgr, 0, false, false, 4, 2).unwrap(), b"\x1b[<0;5;3m");
        assert_eq!(mouse_report(TermMode::MOUSE_REPORT_CLICK, 0, true, false, 0, 0).unwrap(), vec![0x1b, b'[', b'M', 32, 33, 33]);
        assert!(mouse_report(TermMode::empty(), 0, true, false, 0, 0).is_none());
    }
}
