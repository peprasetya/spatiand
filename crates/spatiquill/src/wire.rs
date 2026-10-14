//! What a window and a tab say to each other, over the tab's standard input and output.
//!
//! Each message is a byte saying what it is, four bytes saying how long, and that much. The
//! window tells a tab what was typed, how big it is and where the mouse is; the tab tells the
//! window what its screen shows, its title, and when its shell has gone.

use std::io::{self, Read, Write};

/// One cell of a screen: a character, its colours, and how it is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub c: char,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub flags: u16,
}

pub mod flags {
    pub const BOLD: u16 = 1;
    pub const ITALIC: u16 = 2;
    pub const UNDERLINE: u16 = 4;
    pub const STRIKE: u16 = 8;
    /// The first half of a character two cells wide.
    pub const WIDE: u16 = 16;
    /// The second half, which draws nothing of its own.
    pub const SPACER: u16 = 32;
    pub const DIM: u16 = 64;
    pub const HIDDEN: u16 = 128;
    /// The background is the terminal's own, not a colour asked for: drawn as nothing.
    pub const PLAIN_BG: u16 = 256;
    pub const SELECTED: u16 = 512;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Cursor {
    #[default]
    Hidden,
    Block,
    Underline,
    Beam,
}

/// A screen, whole.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Screen {
    pub cols: u16,
    pub rows: u16,
    pub cursor: (u16, u16),
    pub cursor_shape: Cursor,
    /// How many lines up the history the view is, and how many there are.
    pub scrolled: u32,
    pub history: u32,
    /// The program wants the mouse (`TermMode::MOUSE_MODE`), or is on the other screen.
    pub mouse: bool,
    pub alternate: bool,
    pub cells: Vec<Cell>,
}

/// From a window to a tab.
#[derive(Debug, Clone, PartialEq)]
pub enum ToTab {
    /// `key` is spatiand_app's key, by number (see `keys.rs`); `code` the evdev code.
    Key { key: u8, code: u32, text: String, shift: bool, control: bool, alt: bool },
    Paste(String),
    Resize { cols: u16, rows: u16, cell_w: u16, cell_h: u16 },
    /// Move the view up the history by so many lines (down if less than none), or to the
    /// foot with `i32::MIN`.
    Scroll(i32),
    /// A mouse button or wheel at a cell, for a program that asked for the mouse. `button`:
    /// 0 left, 1 middle, 2 right, 64 wheel up, 65 wheel down; `motion` for a drag.
    Mouse { button: u8, pressed: bool, motion: bool, col: u16, row: u16 },
    /// Select from one cell to another, in the screen as shown; `None` clears.
    Select(Option<((u16, u16), (u16, u16))>),
    /// Send back what is selected as [`FromTab::Copied`].
    Copy,
    Focus(bool),
    Quit,
}

/// From a tab to its window.
#[derive(Debug, Clone, PartialEq)]
pub enum FromTab {
    Screen(Screen),
    Title(String),
    Copied(String),
    Bell,
    /// The shell has gone, with this status.
    Exited(i32),
}

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_str(out: &mut Vec<u8>, s: &str) {
    put_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

/// Reading a message's body.
struct Body<'a>(&'a [u8]);

impl Body<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        self.take(n).map(|b| String::from_utf8_lossy(b).into_owned())
    }
}

fn frame(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 5);
    out.push(tag);
    put_u32(&mut out, body.len() as u32);
    out.extend_from_slice(body);
    out
}

impl ToTab {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::new();
        let tag = match self {
            ToTab::Key { key, code, text, shift, control, alt } => {
                b.push(*key);
                put_u32(&mut b, *code);
                b.push(*shift as u8 | (*control as u8) << 1 | (*alt as u8) << 2);
                put_str(&mut b, text);
                b'K'
            }
            ToTab::Paste(text) => {
                put_str(&mut b, text);
                b'P'
            }
            ToTab::Resize { cols, rows, cell_w, cell_h } => {
                for v in [*cols, *rows, *cell_w, *cell_h] {
                    put_u16(&mut b, v);
                }
                b'R'
            }
            ToTab::Scroll(lines) => {
                put_u32(&mut b, *lines as u32);
                b'S'
            }
            ToTab::Mouse { button, pressed, motion, col, row } => {
                b.push(*button);
                b.push(*pressed as u8 | (*motion as u8) << 1);
                put_u16(&mut b, *col);
                put_u16(&mut b, *row);
                b'M'
            }
            ToTab::Select(range) => {
                if let Some(((a, b2), (c, d))) = range {
                    for v in [*a, *b2, *c, *d] {
                        put_u16(&mut b, v);
                    }
                }
                b'X'
            }
            ToTab::Copy => b'C',
            ToTab::Focus(on) => {
                b.push(*on as u8);
                b'F'
            }
            ToTab::Quit => b'Q',
        };
        frame(tag, &b)
    }

    fn decode(tag: u8, body: &[u8]) -> Option<ToTab> {
        let mut b = Body(body);
        Some(match tag {
            b'K' => {
                let key = b.u8()?;
                let code = b.u32()?;
                let m = b.u8()?;
                ToTab::Key { key, code, shift: m & 1 != 0, control: m & 2 != 0, alt: m & 4 != 0, text: b.str()? }
            }
            b'P' => ToTab::Paste(b.str()?),
            b'R' => ToTab::Resize { cols: b.u16()?, rows: b.u16()?, cell_w: b.u16()?, cell_h: b.u16()? },
            b'S' => ToTab::Scroll(b.u32()? as i32),
            b'M' => {
                let button = b.u8()?;
                let m = b.u8()?;
                ToTab::Mouse { button, pressed: m & 1 != 0, motion: m & 2 != 0, col: b.u16()?, row: b.u16()? }
            }
            b'X' => {
                if body.is_empty() {
                    ToTab::Select(None)
                } else {
                    ToTab::Select(Some(((b.u16()?, b.u16()?), (b.u16()?, b.u16()?))))
                }
            }
            b'C' => ToTab::Copy,
            b'F' => ToTab::Focus(b.u8()? != 0),
            b'Q' => ToTab::Quit,
            _ => return None,
        })
    }
}

impl FromTab {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::new();
        let tag = match self {
            FromTab::Screen(s) => {
                for v in [s.cols, s.rows, s.cursor.0, s.cursor.1] {
                    put_u16(&mut b, v);
                }
                b.push(match s.cursor_shape {
                    Cursor::Hidden => 0,
                    Cursor::Block => 1,
                    Cursor::Underline => 2,
                    Cursor::Beam => 3,
                });
                put_u32(&mut b, s.scrolled);
                put_u32(&mut b, s.history);
                b.push(s.mouse as u8 | (s.alternate as u8) << 1);
                b.reserve(s.cells.len() * 12);
                for c in &s.cells {
                    put_u32(&mut b, c.c as u32);
                    put_u16(&mut b, c.flags);
                    b.extend_from_slice(&c.fg);
                    b.extend_from_slice(&c.bg);
                }
                b'F'
            }
            FromTab::Title(t) => {
                put_str(&mut b, t);
                b'T'
            }
            FromTab::Copied(t) => {
                put_str(&mut b, t);
                b'C'
            }
            FromTab::Bell => b'B',
            FromTab::Exited(code) => {
                put_u32(&mut b, *code as u32);
                b'E'
            }
        };
        frame(tag, &b)
    }

    fn decode(tag: u8, body: &[u8]) -> Option<FromTab> {
        let mut b = Body(body);
        Some(match tag {
            b'F' => {
                let (cols, rows, cx, cy) = (b.u16()?, b.u16()?, b.u16()?, b.u16()?);
                let cursor_shape = match b.u8()? {
                    1 => Cursor::Block,
                    2 => Cursor::Underline,
                    3 => Cursor::Beam,
                    _ => Cursor::Hidden,
                };
                let (scrolled, history) = (b.u32()?, b.u32()?);
                let m = b.u8()?;
                let n = cols as usize * rows as usize;
                let mut cells = Vec::with_capacity(n);
                for _ in 0..n {
                    let c = char::from_u32(b.u32()?).unwrap_or(' ');
                    let flags = b.u16()?;
                    let fg = b.take(3)?;
                    let fg = [fg[0], fg[1], fg[2]];
                    let bg = b.take(3)?;
                    let bg = [bg[0], bg[1], bg[2]];
                    cells.push(Cell { c, fg, bg, flags });
                }
                FromTab::Screen(Screen { cols, rows, cursor: (cx, cy), cursor_shape, scrolled, history, mouse: m & 1 != 0, alternate: m & 2 != 0, cells })
            }
            b'T' => FromTab::Title(b.str()?),
            b'C' => FromTab::Copied(b.str()?),
            b'B' => FromTab::Bell,
            b'E' => FromTab::Exited(b.u32()? as i32),
            _ => return None,
        })
    }
}

/// Write a message whole.
pub fn send(to: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    to.write_all(bytes)?;
    to.flush()
}

/// Read the next message, waiting for it; `None` at the end of the stream.
pub fn read_to_tab(from: &mut impl Read) -> io::Result<Option<ToTab>> {
    loop {
        let mut head = [0u8; 5];
        match from.read_exact(&mut head) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        }
        let n = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
        let mut body = vec![0u8; n];
        from.read_exact(&mut body)?;
        if let Some(message) = ToTab::decode(head[0], &body) {
            return Ok(Some(message));
        }
    }
}

/// Messages from a tab, gathered from whatever has arrived so far: a window reads its tabs
/// without waiting on any of them.
#[derive(Default)]
pub struct Gather {
    pending: Vec<u8>,
}

impl Gather {
    pub fn push(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
    }

    pub fn next(&mut self) -> Option<FromTab> {
        loop {
            if self.pending.len() < 5 {
                return None;
            }
            let n = u32::from_le_bytes([self.pending[1], self.pending[2], self.pending[3], self.pending[4]]) as usize;
            if self.pending.len() < 5 + n {
                return None;
            }
            let message = FromTab::decode(self.pending[0], &self.pending[5..5 + n]);
            self.pending.drain(..5 + n);
            if message.is_some() {
                return message;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_goes_in_comes_out() {
        let screen = Screen {
            cols: 2,
            rows: 1,
            cursor: (1, 0),
            cursor_shape: Cursor::Beam,
            scrolled: 3,
            history: 40,
            mouse: true,
            alternate: false,
            cells: vec![Cell { c: '\u{1f600}', fg: [1, 2, 3], bg: [4, 5, 6], flags: flags::WIDE }, Cell { c: ' ', fg: [0; 3], bg: [0; 3], flags: flags::SPACER }],
        };
        let mut gather = Gather::default();
        let bytes = [FromTab::Screen(screen.clone()).encode(), FromTab::Title("vim".into()).encode()].concat();
        // Arriving in pieces, as a pipe gives them.
        for piece in bytes.chunks(7) {
            gather.push(piece);
        }
        assert_eq!(gather.next(), Some(FromTab::Screen(screen)));
        assert_eq!(gather.next(), Some(FromTab::Title("vim".into())));
        assert_eq!(gather.next(), None);

        let asked = [ToTab::Key { key: 3, code: 30, text: "é".into(), shift: true, control: false, alt: true }, ToTab::Scroll(-5), ToTab::Select(None), ToTab::Resize { cols: 80, rows: 24, cell_w: 12, cell_h: 26 }];
        let bytes: Vec<u8> = asked.iter().flat_map(|m| m.encode()).collect();
        let mut reader = &bytes[..];
        for m in &asked {
            assert_eq!(read_to_tab(&mut reader).unwrap().as_ref(), Some(m));
        }
        assert_eq!(read_to_tab(&mut reader).unwrap(), None);
    }
}
