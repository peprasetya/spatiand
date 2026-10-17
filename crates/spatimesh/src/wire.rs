//! What a window and an engine host say to each other (docs/spatimesh.md).
//!
//! A Unix socket between the two, made by the window when it starts the host. Each message is
//! a byte saying what it is, four saying how long, and that much. A tab's pictures are in
//! shared memory: the host makes two buffers per tab, hands both over once (their descriptors
//! ride with the [`FromHost::Buffers`] message that names them), and from then on says only
//! which buffer holds the newest picture and what changed in it. The window copies what changed
//! to its texture and gives the buffer back ([`ToHost::Release`]); the host draws the next
//! picture into whichever buffer the window is not reading.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use crate::engine::{Button, Canvas, Choice, Cursor, DialogKind, Format, Happened, KeyPress, Modifiers, Rect, TabId, View};

/// From a window to its engine host.
#[derive(Debug, Clone, PartialEq)]
pub enum ToHost {
    Open { tab: TabId, url: String, view: View },
    Close { tab: TabId },
    Load { tab: TabId, url: String },
    Back { tab: TabId },
    Forward { tab: TabId },
    Reload { tab: TabId },
    Stop { tab: TabId },
    Resize { tab: TabId, view: View },
    Pointer { tab: TabId, x: f32, y: f32, button: Option<(Button, bool)>, modifiers: Modifiers },
    PointerLeft { tab: TabId },
    Scroll { tab: TabId, x: f32, y: f32, dx: f32, dy: f32, modifiers: Modifiers },
    Key { tab: TabId, key: KeyPress },
    Focus { tab: TabId, focused: bool },
    Window { tab: TabId, window: u32 },
    Mute { tab: TabId, muted: bool },
    /// The window has read this buffer of the tab's and is done with it.
    Release { tab: TabId, buffer: u8 },
    Choose { tab: TabId, index: Option<usize> },
    Answer { tab: TabId, accepted: bool, text: Option<String> },
    Quit,
    /// The profile's extensions, as [`crate::extensions::set::ExtensionSet`]'s JSON.
    Extensions { set: String },
    /// Code to run in a tab ([`crate::engine::Engine::run_script`]).
    RunScript { tab: TabId, world: String, all_frames: bool, code: String, request: u64, boot: bool },
    /// SpatiMesh's own code for the start of every frame's every document, in the page's world
    /// ([`crate::engine::Engine::set_page_script`]): its media (`crate::media`).
    PageScript { code: String },
}

/// From an engine host to its window.
#[derive(Debug, Clone, PartialEq)]
pub enum FromHost {
    Hello { engine: String },
    /// A tab's buffers, made anew for a new size: `count` descriptors come with this message.
    Buffers { tab: TabId, width: u32, height: u32, format: Format, count: u8 },
    /// The tab's newest picture is in `buffer`; `changed` is what is new since the last.
    Picture { tab: TabId, buffer: u8, changed: Vec<Rect> },
    /// Anything else a page did (never [`Happened::Picture`], which is the message above).
    Happened(Happened),
}

// ---- encoding ------------------------------------------------------------------------------

struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn f32(&mut self, v: f32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn bool(&mut self, v: bool) -> &mut Self {
        self.u8(v as u8)
    }
    fn str(&mut self, s: &str) -> &mut Self {
        self.u32(s.len() as u32);
        self.0.extend_from_slice(s.as_bytes());
        self
    }
    fn view(&mut self, v: View) -> &mut Self {
        self.u32(v.width).u32(v.height).f32(v.zoom)
    }
    fn modifiers(&mut self, m: Modifiers) -> &mut Self {
        self.u8(m.shift as u8 | (m.control as u8) << 1 | (m.alt as u8) << 2 | (m.logo as u8) << 3)
    }
    fn rect(&mut self, r: Rect) -> &mut Self {
        self.u32(r.x).u32(r.y).u32(r.width).u32(r.height)
    }
}

struct In<'a>(&'a [u8]);

impl In<'_> {
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
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    }
    fn f32(&mut self) -> Option<f32> {
        self.take(4).map(|b| f32::from_le_bytes(b.try_into().unwrap()))
    }
    fn bool(&mut self) -> Option<bool> {
        self.u8().map(|b| b != 0)
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        self.take(n).map(|b| String::from_utf8_lossy(b).into_owned())
    }
    fn view(&mut self) -> Option<View> {
        Some(View::new(self.u32()?, self.u32()?, self.f32()?))
    }
    fn modifiers(&mut self) -> Option<Modifiers> {
        let m = self.u8()?;
        Some(Modifiers { shift: m & 1 != 0, control: m & 2 != 0, alt: m & 4 != 0, logo: m & 8 != 0 })
    }
    fn rect(&mut self) -> Option<Rect> {
        Some(Rect { x: self.u32()?, y: self.u32()?, width: self.u32()?, height: self.u32()? })
    }
}

fn button_number(b: Option<(Button, bool)>) -> (u8, bool) {
    match b {
        None => (0, false),
        Some((button, pressed)) => (
            match button {
                Button::Left => 1,
                Button::Middle => 2,
                Button::Right => 3,
                Button::Back => 4,
                Button::Forward => 5,
            },
            pressed,
        ),
    }
}

fn button_of(n: u8, pressed: bool) -> Option<(Button, bool)> {
    let b = match n {
        1 => Button::Left,
        2 => Button::Middle,
        3 => Button::Right,
        4 => Button::Back,
        5 => Button::Forward,
        _ => return None,
    };
    Some((b, pressed))
}

/// A key as a number and a character, to cross the wire. The same numbering as SpatiQuill's.
pub fn key_number(key: spatiand_app::Key) -> (u8, u32) {
    use spatiand_app::Key::*;
    match key {
        Enter => (1, 0),
        Escape => (2, 0),
        Backspace => (3, 0),
        Delete => (4, 0),
        Tab => (5, 0),
        Space => (6, 0),
        Left => (7, 0),
        Right => (8, 0),
        Up => (9, 0),
        Down => (10, 0),
        Home => (11, 0),
        End => (12, 0),
        PageUp => (13, 0),
        PageDown => (14, 0),
        Char(c) => (15, c as u32),
        Other => (0, 0),
    }
}

pub fn key_of(n: u8, c: u32) -> spatiand_app::Key {
    use spatiand_app::Key::*;
    match n {
        1 => Enter,
        2 => Escape,
        3 => Backspace,
        4 => Delete,
        5 => Tab,
        6 => Space,
        7 => Left,
        8 => Right,
        9 => Up,
        10 => Down,
        11 => Home,
        12 => End,
        13 => PageUp,
        14 => PageDown,
        15 => char::from_u32(c).map_or(Other, Char),
        _ => Other,
    }
}

fn cursor_number(c: Cursor) -> u8 {
    match c {
        Cursor::Arrow => 0,
        Cursor::Hand => 1,
        Cursor::Text => 2,
        Cursor::Wait => 3,
        Cursor::Crosshair => 4,
        Cursor::Move => 5,
        Cursor::ResizeHorizontal => 6,
        Cursor::ResizeVertical => 7,
        Cursor::NotAllowed => 8,
        Cursor::Grab => 9,
        Cursor::Grabbing => 10,
        Cursor::Hidden => 11,
    }
}

fn cursor_of(n: u8) -> Cursor {
    match n {
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
    }
}

fn frame(tag: u8, body: Out) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.0.len() + 5);
    out.push(tag);
    out.extend_from_slice(&(body.0.len() as u32).to_le_bytes());
    out.extend_from_slice(&body.0);
    out
}

impl ToHost {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Out(Vec::new());
        let tag = match self {
            ToHost::Open { tab, url, view } => {
                b.u64(*tab).str(url).view(*view);
                1
            }
            ToHost::Close { tab } => {
                b.u64(*tab);
                2
            }
            ToHost::Load { tab, url } => {
                b.u64(*tab).str(url);
                3
            }
            ToHost::Back { tab } => {
                b.u64(*tab);
                4
            }
            ToHost::Forward { tab } => {
                b.u64(*tab);
                5
            }
            ToHost::Reload { tab } => {
                b.u64(*tab);
                6
            }
            ToHost::Stop { tab } => {
                b.u64(*tab);
                7
            }
            ToHost::Resize { tab, view } => {
                b.u64(*tab).view(*view);
                8
            }
            ToHost::Pointer { tab, x, y, button, modifiers } => {
                let (n, pressed) = button_number(*button);
                b.u64(*tab).f32(*x).f32(*y).u8(n).bool(pressed).modifiers(*modifiers);
                9
            }
            ToHost::PointerLeft { tab } => {
                b.u64(*tab);
                10
            }
            ToHost::Scroll { tab, x, y, dx, dy, modifiers } => {
                b.u64(*tab).f32(*x).f32(*y).f32(*dx).f32(*dy).modifiers(*modifiers);
                11
            }
            ToHost::Key { tab, key } => {
                let (n, c) = key_number(key.key);
                b.u64(*tab).u32(key.code).u8(n).u32(c).str(key.text.as_deref().unwrap_or("")).bool(key.text.is_some());
                b.bool(key.pressed).bool(key.repeat).modifiers(key.modifiers);
                12
            }
            ToHost::Focus { tab, focused } => {
                b.u64(*tab).bool(*focused);
                13
            }
            ToHost::Window { tab, window } => {
                b.u64(*tab).u32(*window);
                14
            }
            ToHost::Mute { tab, muted } => {
                b.u64(*tab).bool(*muted);
                15
            }
            ToHost::Release { tab, buffer } => {
                b.u64(*tab).u8(*buffer);
                16
            }
            ToHost::Quit => 17,
            ToHost::Choose { tab, index } => {
                b.u64(*tab).u32(index.map_or(u32::MAX, |i| i as u32));
                18
            }
            ToHost::Answer { tab, accepted, text } => {
                b.u64(*tab).bool(*accepted).bool(text.is_some()).str(text.as_deref().unwrap_or(""));
                19
            }
            ToHost::Extensions { set } => {
                b.str(set);
                20
            }
            ToHost::RunScript { tab, world, all_frames, code, request, boot } => {
                b.u64(*tab).str(world).bool(*all_frames).str(code).u64(*request).bool(*boot);
                21
            }
            ToHost::PageScript { code } => {
                b.str(code);
                22
            }
        };
        frame(tag, b)
    }

    fn decode(tag: u8, body: &[u8]) -> Option<ToHost> {
        let mut i = In(body);
        Some(match tag {
            1 => ToHost::Open { tab: i.u64()?, url: i.str()?, view: i.view()? },
            2 => ToHost::Close { tab: i.u64()? },
            3 => ToHost::Load { tab: i.u64()?, url: i.str()? },
            4 => ToHost::Back { tab: i.u64()? },
            5 => ToHost::Forward { tab: i.u64()? },
            6 => ToHost::Reload { tab: i.u64()? },
            7 => ToHost::Stop { tab: i.u64()? },
            8 => ToHost::Resize { tab: i.u64()?, view: i.view()? },
            9 => {
                let (tab, x, y, n, pressed, modifiers) = (i.u64()?, i.f32()?, i.f32()?, i.u8()?, i.bool()?, i.modifiers()?);
                ToHost::Pointer { tab, x, y, button: button_of(n, pressed), modifiers }
            }
            10 => ToHost::PointerLeft { tab: i.u64()? },
            11 => ToHost::Scroll { tab: i.u64()?, x: i.f32()?, y: i.f32()?, dx: i.f32()?, dy: i.f32()?, modifiers: i.modifiers()? },
            12 => {
                let (tab, code, n, c, text, has_text) = (i.u64()?, i.u32()?, i.u8()?, i.u32()?, i.str()?, i.bool()?);
                let (pressed, repeat, modifiers) = (i.bool()?, i.bool()?, i.modifiers()?);
                ToHost::Key { tab, key: KeyPress { code, key: key_of(n, c), text: has_text.then_some(text), pressed, repeat, modifiers } }
            }
            13 => ToHost::Focus { tab: i.u64()?, focused: i.bool()? },
            14 => ToHost::Window { tab: i.u64()?, window: i.u32()? },
            15 => ToHost::Mute { tab: i.u64()?, muted: i.bool()? },
            16 => ToHost::Release { tab: i.u64()?, buffer: i.u8()? },
            17 => ToHost::Quit,
            18 => {
                let (tab, index) = (i.u64()?, i.u32()?);
                ToHost::Choose { tab, index: (index != u32::MAX).then_some(index as usize) }
            }
            19 => {
                let (tab, accepted, has_text, text) = (i.u64()?, i.bool()?, i.bool()?, i.str()?);
                ToHost::Answer { tab, accepted, text: has_text.then_some(text) }
            }
            20 => ToHost::Extensions { set: i.str()? },
            21 => ToHost::RunScript { tab: i.u64()?, world: i.str()?, all_frames: i.bool()?, code: i.str()?, request: i.u64()?, boot: i.bool()? },
            22 => ToHost::PageScript { code: i.str()? },
            _ => return None,
        })
    }
}

impl FromHost {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Out(Vec::new());
        let tag = match self {
            FromHost::Hello { engine } => {
                b.str(engine);
                1
            }
            FromHost::Buffers { tab, width, height, format, count } => {
                b.u64(*tab).u32(*width).u32(*height).u8(matches!(format, Format::Rgba) as u8).u8(*count);
                2
            }
            FromHost::Picture { tab, buffer, changed } => {
                b.u64(*tab).u8(*buffer).u32(changed.len() as u32);
                for r in changed {
                    b.rect(*r);
                }
                3
            }
            FromHost::Happened(h) => match h {
                Happened::Title { tab, title } => {
                    b.u64(*tab).str(title);
                    10
                }
                Happened::Url { tab, url } => {
                    b.u64(*tab).str(url);
                    11
                }
                Happened::Progress { tab, progress, loading } => {
                    b.u64(*tab).f32(*progress).bool(*loading);
                    12
                }
                Happened::History { tab, back, forward } => {
                    b.u64(*tab).bool(*back).bool(*forward);
                    13
                }
                Happened::Picture { tab } => {
                    b.u64(*tab);
                    14
                }
                Happened::Cursor { tab, cursor } => {
                    b.u64(*tab).u8(cursor_number(*cursor));
                    15
                }
                Happened::Sound { tab, playing } => {
                    b.u64(*tab).bool(*playing);
                    16
                }
                Happened::OpenTab { opener, url, background } => {
                    b.u64(*opener).str(url).bool(*background);
                    17
                }
                Happened::CloseTab { tab } => {
                    b.u64(*tab);
                    18
                }
                Happened::Crashed { tab, why } => {
                    b.u64(*tab).str(why);
                    19
                }
                Happened::Failed { tab, url, why } => {
                    b.u64(*tab).str(url).str(why);
                    20
                }
                Happened::Choose { tab, choices, at } => {
                    b.u64(*tab).rect(*at).u32(choices.len() as u32);
                    for c in choices {
                        b.str(&c.label).bool(c.enabled).bool(c.selected).bool(c.heading);
                    }
                    21
                }
                Happened::Dialog { tab, kind, message, text } => {
                    let k = match kind {
                        DialogKind::Alert => 0,
                        DialogKind::Confirm => 1,
                        DialogKind::Prompt => 2,
                        DialogKind::Leave => 3,
                    };
                    b.u64(*tab).u8(k).str(message).str(text);
                    22
                }
                Happened::Extension { tab, json } => {
                    b.u64(*tab).str(json);
                    23
                }
                Happened::ScriptResult { tab, request, result } => {
                    b.u64(*tab).u64(*request);
                    match result {
                        Ok(v) => b.bool(true).str(v),
                        Err(e) => b.bool(false).str(e),
                    };
                    24
                }
                Happened::Media { tab, json } => {
                    b.u64(*tab).str(json);
                    25
                }
            },
        };
        frame(tag, b)
    }

    fn decode(tag: u8, body: &[u8]) -> Option<FromHost> {
        let mut i = In(body);
        Some(match tag {
            1 => FromHost::Hello { engine: i.str()? },
            2 => {
                let (tab, width, height, rgba, count) = (i.u64()?, i.u32()?, i.u32()?, i.u8()?, i.u8()?);
                FromHost::Buffers { tab, width, height, format: if rgba == 1 { Format::Rgba } else { Format::Bgra }, count }
            }
            3 => {
                let (tab, buffer, n) = (i.u64()?, i.u8()?, i.u32()?);
                let mut changed = Vec::with_capacity(n.min(1024) as usize);
                for _ in 0..n {
                    changed.push(i.rect()?);
                }
                FromHost::Picture { tab, buffer, changed }
            }
            10 => FromHost::Happened(Happened::Title { tab: i.u64()?, title: i.str()? }),
            11 => FromHost::Happened(Happened::Url { tab: i.u64()?, url: i.str()? }),
            12 => FromHost::Happened(Happened::Progress { tab: i.u64()?, progress: i.f32()?, loading: i.bool()? }),
            13 => FromHost::Happened(Happened::History { tab: i.u64()?, back: i.bool()?, forward: i.bool()? }),
            14 => FromHost::Happened(Happened::Picture { tab: i.u64()? }),
            15 => FromHost::Happened(Happened::Cursor { tab: i.u64()?, cursor: cursor_of(i.u8()?) }),
            16 => FromHost::Happened(Happened::Sound { tab: i.u64()?, playing: i.bool()? }),
            17 => FromHost::Happened(Happened::OpenTab { opener: i.u64()?, url: i.str()?, background: i.bool()? }),
            18 => FromHost::Happened(Happened::CloseTab { tab: i.u64()? }),
            19 => FromHost::Happened(Happened::Crashed { tab: i.u64()?, why: i.str()? }),
            20 => FromHost::Happened(Happened::Failed { tab: i.u64()?, url: i.str()?, why: i.str()? }),
            21 => {
                let (tab, at, n) = (i.u64()?, i.rect()?, i.u32()?);
                let mut choices = Vec::with_capacity(n.min(4096) as usize);
                for _ in 0..n {
                    choices.push(Choice { label: i.str()?, enabled: i.bool()?, selected: i.bool()?, heading: i.bool()? });
                }
                FromHost::Happened(Happened::Choose { tab, choices, at })
            }
            22 => {
                let (tab, k, message, text) = (i.u64()?, i.u8()?, i.str()?, i.str()?);
                let kind = match k {
                    1 => DialogKind::Confirm,
                    2 => DialogKind::Prompt,
                    3 => DialogKind::Leave,
                    _ => DialogKind::Alert,
                };
                FromHost::Happened(Happened::Dialog { tab, kind, message, text })
            }
            23 => FromHost::Happened(Happened::Extension { tab: i.u64()?, json: i.str()? }),
            25 => FromHost::Happened(Happened::Media { tab: i.u64()?, json: i.str()? }),
            24 => {
                let (tab, request, ok, text) = (i.u64()?, i.u64()?, i.bool()?, i.str()?);
                FromHost::Happened(Happened::ScriptResult { tab, request, result: if ok { Ok(text) } else { Err(text) } })
            }
            _ => return None,
        })
    }
}

// ---- the socket ----------------------------------------------------------------------------

/// The most descriptors one message carries.
const MOST_FDS: usize = 4;

/// One end of the socket between a window and an engine host.
pub struct Link {
    fd: OwnedFd,
    read: Vec<u8>,
    fds: VecDeque<OwnedFd>,
    closed: bool,
}

impl Link {
    /// A socket pair: one end for this process, one to hand to the other.
    pub fn pair() -> io::Result<(Link, OwnedFd)> {
        let mut fds = [0 as RawFd; 2];
        // Neither end is inherited by anything started later; the one handed to the host is
        // put in its place by the launch itself.
        let flags = libc::SOCK_STREAM;
        #[cfg(not(target_os = "macos"))]
        let flags = flags | libc::SOCK_CLOEXEC;
        if unsafe { libc::socketpair(libc::AF_UNIX, flags, 0, fds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        #[cfg(target_os = "macos")]
        for fd in fds {
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        }
        let (ours, theirs) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        Ok((Link::from_fd(ours), theirs))
    }

    pub fn from_fd(fd: OwnedFd) -> Link {
        unsafe {
            let raw = fd.as_raw_fd();
            libc::fcntl(raw, libc::F_SETFL, libc::fcntl(raw, libc::F_GETFL) | libc::O_NONBLOCK);
        }
        // A window that has gone must not kill its host with SIGPIPE: the write fails instead.
        #[cfg(target_os = "macos")]
        unsafe {
            let one: libc::c_int = 1;
            libc::setsockopt(fd.as_raw_fd(), libc::SOL_SOCKET, libc::SO_NOSIGPIPE, &one as *const _ as *const _, 4);
        }
        Link { fd, read: Vec::new(), fds: VecDeque::new(), closed: false }
    }

    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Send one message whole, with descriptors if any. Waits while the other end is behind.
    pub fn send(&mut self, bytes: &[u8], fds: &[RawFd]) -> io::Result<()> {
        let mut sent = 0;
        let mut first = true;
        while sent < bytes.len() {
            let n = self.send_some(&bytes[sent..], if first { fds } else { &[] })?;
            first = false;
            match n {
                Some(n) => sent += n,
                None => self.wait(libc::POLLOUT, 1000)?,
            }
        }
        Ok(())
    }

    fn send_some(&self, bytes: &[u8], fds: &[RawFd]) -> io::Result<Option<usize>> {
        let mut iov = libc::iovec { iov_base: bytes.as_ptr() as *mut _, iov_len: bytes.len() };
        let mut space = [0u8; 128];
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        if !fds.is_empty() {
            let size = std::mem::size_of_val(fds) as u32;
            msg.msg_control = space.as_mut_ptr() as *mut _;
            msg.msg_controllen = unsafe { libc::CMSG_SPACE(size) } as _;
            unsafe {
                let cmsg = libc::CMSG_FIRSTHDR(&msg);
                (*cmsg).cmsg_level = libc::SOL_SOCKET;
                (*cmsg).cmsg_type = libc::SCM_RIGHTS;
                (*cmsg).cmsg_len = libc::CMSG_LEN(size) as _;
                std::ptr::copy_nonoverlapping(fds.as_ptr(), libc::CMSG_DATA(cmsg) as *mut RawFd, fds.len());
            }
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let flags = libc::MSG_NOSIGNAL;
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        let flags = 0;
        let n = unsafe { libc::sendmsg(self.fd.as_raw_fd(), &msg, flags) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::Interrupted {
                return Ok(None);
            }
            return Err(e);
        }
        Ok(Some(n as usize))
    }

    fn wait(&self, events: i16, ms: i32) -> io::Result<()> {
        let mut p = libc::pollfd { fd: self.fd.as_raw_fd(), events, revents: 0 };
        let n = unsafe { libc::poll(&mut p, 1, ms) };
        if n < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return Err(io::Error::last_os_error());
        }
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "the other end is not reading"));
        }
        Ok(())
    }

    /// Wait for something to read, for at most `ms`.
    pub fn wait_readable(&self, ms: i32) -> bool {
        let mut p = libc::pollfd { fd: self.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        unsafe { libc::poll(&mut p, 1, ms) > 0 }
    }

    /// Read whatever has come, without waiting. After this, [`Link::is_closed`] says whether
    /// the other end has gone.
    fn fill(&mut self) {
        loop {
            let mut buffer = [0u8; 65536];
            let mut iov = libc::iovec { iov_base: buffer.as_mut_ptr() as *mut _, iov_len: buffer.len() };
            let mut space = [0u8; 256];
            let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = space.as_mut_ptr() as *mut _;
            msg.msg_controllen = space.len() as _;
            #[cfg(any(target_os = "linux", target_os = "android"))]
            let flags = libc::MSG_CMSG_CLOEXEC;
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            let flags = 0;
            let n = unsafe { libc::recvmsg(self.fd.as_raw_fd(), &mut msg, flags) };
            if n < 0 {
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::WouldBlock && e.kind() != io::ErrorKind::Interrupted {
                    self.closed = true;
                }
                return;
            }
            unsafe {
                let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
                while !cmsg.is_null() {
                    if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                        let data = libc::CMSG_DATA(cmsg) as *const RawFd;
                        let count = ((*cmsg).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / std::mem::size_of::<RawFd>();
                        for k in 0..count.min(MOST_FDS * 4) {
                            let fd = std::ptr::read_unaligned(data.add(k));
                            #[cfg(target_os = "macos")]
                            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                            self.fds.push_back(OwnedFd::from_raw_fd(fd));
                        }
                    }
                    cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
                }
            }
            if n == 0 {
                self.closed = true;
                return;
            }
            self.read.extend_from_slice(&buffer[..n as usize]);
        }
    }

    /// The next whole message's tag and body, if one has come.
    fn next_frame(&mut self) -> Option<(u8, Vec<u8>)> {
        if self.read.len() < 5 {
            return None;
        }
        let length = u32::from_le_bytes(self.read[1..5].try_into().unwrap()) as usize;
        if self.read.len() < 5 + length {
            return None;
        }
        let tag = self.read[0];
        let body = self.read[5..5 + length].to_vec();
        self.read.drain(..5 + length);
        Some((tag, body))
    }

    /// Everything that has come for a host.
    pub fn receive_to_host(&mut self) -> Vec<ToHost> {
        self.fill();
        let mut out = Vec::new();
        while let Some((tag, body)) = self.next_frame() {
            match ToHost::decode(tag, &body) {
                Some(message) => out.push(message),
                None => log::warn!("a message from the window could not be read ({tag})"),
            }
        }
        out
    }

    /// Everything that has come for a window, each [`FromHost::Buffers`] with its descriptors.
    pub fn receive_from_host(&mut self) -> Vec<(FromHost, Vec<OwnedFd>)> {
        self.fill();
        let mut out = Vec::new();
        while let Some((tag, body)) = self.next_frame() {
            match FromHost::decode(tag, &body) {
                Some(message) => {
                    let fds = match &message {
                        FromHost::Buffers { count, .. } => (0..*count).filter_map(|_| self.fds.pop_front()).collect(),
                        _ => Vec::new(),
                    };
                    out.push((message, fds));
                }
                None => log::warn!("a message from the engine could not be read ({tag})"),
            }
        }
        out
    }
}

// ---- shared memory -------------------------------------------------------------------------

/// Memory shared between the two processes, mapped into this one.
pub struct Shared {
    fd: OwnedFd,
    ptr: *mut u8,
    len: usize,
}

// The mapping is plain memory; who writes it when is the protocol's business.
unsafe impl Send for Shared {}

impl Shared {
    /// New memory of `len` bytes, all zero.
    pub fn new(len: usize) -> io::Result<Shared> {
        let fd = anonymous_file()?;
        if unsafe { libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Shared::map(fd, len, true)
    }

    /// Memory another process made, from its descriptor.
    pub fn map(fd: OwnedFd, len: usize, writable: bool) -> io::Result<Shared> {
        let protection = if writable { libc::PROT_READ | libc::PROT_WRITE } else { libc::PROT_READ };
        let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len.max(1), protection, libc::MAP_SHARED, fd.as_raw_fd(), 0) };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Shared { fd, ptr: ptr as *mut u8, len })
    }

    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    pub fn bytes_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.ptr as *mut _, self.len.max(1)) };
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn anonymous_file() -> io::Result<OwnedFd> {
    let fd = unsafe { libc::memfd_create(c"spatimesh-picture".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn anonymous_file() -> io::Result<OwnedFd> {
    // A POSIX shared memory object, unlinked the moment it is open: only its descriptor names
    // it, which is what is handed over. The Mac takes no name longer than 31 bytes (PSHMNAMLEN).
    for _ in 0..8 {
        let name = std::ffi::CString::new(format!("/smesh.{:x}.{}", std::process::id(), &crate::store::new_id()[..10])).unwrap();
        let fd = unsafe { libc::shm_open(name.as_ptr(), libc::O_CREAT | libc::O_EXCL | libc::O_RDWR, 0o600 as libc::c_uint) };
        if fd >= 0 {
            unsafe {
                libc::shm_unlink(name.as_ptr());
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
            return Ok(unsafe { OwnedFd::from_raw_fd(fd) });
        }
    }
    Err(io::Error::last_os_error())
}

// ---- the host's canvas ---------------------------------------------------------------------

fn union(a: Option<Rect>, b: Rect) -> Rect {
    match a {
        None => b,
        Some(a) => {
            let x = a.x.min(b.x);
            let y = a.y.min(b.y);
            let right = (a.x + a.width).max(b.x + b.width);
            let bottom = (a.y + a.height).max(b.y + b.height);
            Rect { x, y, width: right - x, height: bottom - y }
        }
    }
}

/// Two buffers of one tab's pictures.
struct Pair {
    width: u32,
    height: u32,
    format: Format,
    buffers: [Shared; 2],
    /// The window has this one and has not given it back.
    held: [bool; 2],
    /// What of each is older than the newest picture.
    stale: [Option<Rect>; 2],
    newest: Option<usize>,
    /// The buffer handed out by `begin` and not yet finished.
    drawing: Option<usize>,
}

/// The canvas an engine host's engine draws into: each tab's two buffers, and the messages that
/// tell the window about them.
#[derive(Default)]
pub struct SharedCanvas {
    pairs: HashMap<TabId, Pair>,
    /// Messages for the window, with the descriptors that go with them.
    outbox: Vec<(FromHost, Vec<RawFd>)>,
}

impl SharedCanvas {
    pub fn new() -> SharedCanvas {
        SharedCanvas::default()
    }

    /// The window is done with a buffer.
    pub fn release(&mut self, tab: TabId, buffer: u8) {
        if let Some(pair) = self.pairs.get_mut(&tab) {
            if let Some(held) = pair.held.get_mut(buffer as usize) {
                *held = false;
            }
        }
    }

    pub fn forget(&mut self, tab: TabId) {
        self.pairs.remove(&tab);
    }

    /// Messages waiting for the window. The descriptors stay open as long as the canvas has
    /// the buffers, which is longer than it takes to send them.
    pub fn take_outbox(&mut self) -> Vec<(FromHost, Vec<RawFd>)> {
        std::mem::take(&mut self.outbox)
    }
}

impl Canvas for SharedCanvas {
    fn begin(&mut self, tab: TabId, width: u32, height: u32, format: Format) -> Option<&mut [u8]> {
        let (width, height) = (width.max(1), height.max(1));
        let fits = self.pairs.get(&tab).is_some_and(|p| p.width == width && p.height == height && p.format == format);
        if !fits {
            let len = width as usize * height as usize * 4;
            let made = (Shared::new(len), Shared::new(len));
            let (Ok(a), Ok(b)) = made else {
                log::warn!("no memory for a {width}x{height} picture");
                return None;
            };
            let fds = vec![a.raw_fd(), b.raw_fd()];
            self.pairs.insert(tab, Pair { width, height, format, buffers: [a, b], held: [false; 2], stale: [None; 2], newest: None, drawing: None });
            self.outbox.push((FromHost::Buffers { tab, width, height, format, count: 2 }, fds));
        }
        let pair = self.pairs.get_mut(&tab)?;
        // The one the window is not reading, and not the newest if both are free: the newest
        // stays whole for the window to read.
        let choice = match pair.newest {
            Some(n) if !pair.held[1 - n] => 1 - n,
            Some(n) if !pair.held[n] => n,
            None if !pair.held[0] => 0,
            _ => return None,
        };
        // Bring it up to date with the newest picture where it is behind, so the engine need
        // only draw what changed since then.
        if let (Some(newest), Some(stale)) = (pair.newest.filter(|&n| n != choice), pair.stale[choice].take()) {
            let stride = pair.width as usize * 4;
            let (a, b) = pair.buffers.split_at_mut(1);
            let (from, to) = if newest == 0 { (&a[0], &mut b[0]) } else { (&b[0], &mut a[0]) };
            let (src, dst) = (from.bytes(), to.bytes_mut());
            if let Some(stale) = stale.clip(pair.width, pair.height) {
                for row in stale.y..stale.y + stale.height {
                    let start = row as usize * stride + stale.x as usize * 4;
                    let end = start + stale.width as usize * 4;
                    dst[start..end].copy_from_slice(&src[start..end]);
                }
            }
        }
        pair.drawing = Some(choice);
        Some(pair.buffers[choice].bytes_mut())
    }

    fn finish(&mut self, tab: TabId, changed: &[Rect]) {
        let Some(pair) = self.pairs.get_mut(&tab) else { return };
        let Some(drawn) = pair.drawing.take() else { return };
        let changed: Vec<Rect> = changed.iter().filter_map(|r| r.clip(pair.width, pair.height)).collect();
        // A picture never shown to the window is all new to it.
        let changed = if pair.newest.is_none() { vec![Rect::whole(pair.width, pair.height)] } else { changed };
        for r in &changed {
            let other = 1 - drawn;
            pair.stale[other] = Some(union(pair.stale[other], *r));
        }
        pair.stale[drawn] = None;
        pair.newest = Some(drawn);
        pair.held[drawn] = true;
        self.outbox.push((FromHost::Picture { tab, buffer: drawn as u8, changed }, Vec::new()));
    }
}

/// A tab's pictures as the window sees them: the host's buffers, mapped.
pub struct Pictures {
    pub width: u32,
    pub height: u32,
    pub format: Format,
    buffers: Vec<Shared>,
}

impl Pictures {
    pub fn map(width: u32, height: u32, format: Format, fds: Vec<OwnedFd>) -> io::Result<Pictures> {
        let len = width as usize * height as usize * 4;
        let buffers = fds.into_iter().map(|fd| Shared::map(fd, len, false)).collect::<io::Result<Vec<_>>>()?;
        Ok(Pictures { width, height, format, buffers })
    }

    pub fn buffer(&self, index: u8) -> Option<&[u8]> {
        self.buffers.get(index as usize).map(Shared::bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{fake::Fake, Engine};
    use std::time::Duration;

    /// Memory for pictures, as every platform makes it (the Mac's shared memory objects take no
    /// name past 31 bytes), many times over, written and read back through a second mapping.
    #[test]
    fn memory_for_pictures_is_made() {
        for _ in 0..32 {
            let mut a = Shared::new(400 * 300 * 4).expect("memory for a picture");
            a.bytes_mut()[1234] = 42;
            let fd: OwnedFd = unsafe { OwnedFd::from_raw_fd(libc::dup(a.raw_fd())) };
            let b = Shared::map(fd, a.len(), false).unwrap();
            assert_eq!(b.bytes()[1234], 42);
        }
    }

    #[test]
    fn every_message_comes_back_the_same() {
        let to_host = vec![
            ToHost::Open { tab: 1, url: "https://example.com/ü".into(), view: View::new(800, 600, 1.25) },
            ToHost::Pointer { tab: 2, x: 1.5, y: 2.5, button: Some((Button::Right, true)), modifiers: Modifiers { shift: true, logo: true, ..Default::default() } },
            ToHost::Pointer { tab: 2, x: 1.0, y: 2.0, button: None, modifiers: Modifiers::default() },
            ToHost::Key { tab: 3, key: KeyPress { code: 30, key: spatiand_app::Key::Char('ä'), text: Some("ä".into()), pressed: true, repeat: false, modifiers: Modifiers { control: true, ..Default::default() } } },
            ToHost::Key { tab: 3, key: KeyPress { code: 1, key: spatiand_app::Key::Escape, text: None, pressed: false, repeat: true, modifiers: Modifiers::default() } },
            ToHost::Scroll { tab: 4, x: 3.0, y: 4.0, dx: -1.0, dy: 120.0, modifiers: Modifiers::default() },
            ToHost::Release { tab: 5, buffer: 1 },
            ToHost::Choose { tab: 6, index: Some(3) },
            ToHost::Choose { tab: 6, index: None },
            ToHost::Answer { tab: 7, accepted: true, text: Some("typed".into()) },
            ToHost::Answer { tab: 7, accepted: false, text: None },
            ToHost::Quit,
            ToHost::Extensions { set: "{\"extensions\":[]}".into() },
            ToHost::RunScript { tab: 8, world: "spatimesh-abc".into(), all_frames: true, code: "1+1".into(), request: 42, boot: true },
        ];
        for message in to_host {
            let bytes = message.encode();
            assert_eq!(ToHost::decode(bytes[0], &bytes[5..]), Some(message));
        }
        let from_host = vec![
            FromHost::Hello { engine: "x".into() },
            FromHost::Picture { tab: 9, buffer: 1, changed: vec![Rect { x: 1, y: 2, width: 3, height: 4 }] },
            FromHost::Happened(Happened::OpenTab { opener: 1, url: "https://a/".into(), background: true }),
            FromHost::Happened(Happened::Failed { tab: 1, url: "https://nowhere/".into(), why: "no such host".into() }),
            FromHost::Happened(Happened::Cursor { tab: 1, cursor: Cursor::Hand }),
            FromHost::Happened(Happened::Choose {
                tab: 2,
                choices: vec![Choice { label: "Fruit".into(), enabled: false, selected: false, heading: true }, Choice { label: "Apple".into(), enabled: true, selected: true, heading: false }],
                at: Rect { x: 10, y: 20, width: 100, height: 24 },
            }),
            FromHost::Happened(Happened::Dialog { tab: 3, kind: DialogKind::Prompt, message: "Name?".into(), text: "default".into() }),
            FromHost::Happened(Happened::Extension { tab: 4, json: "{\"c\":\"ready\"}".into() }),
            FromHost::Happened(Happened::ScriptResult { tab: 4, request: 42, result: Ok("2".into()) }),
            FromHost::Happened(Happened::ScriptResult { tab: 4, request: 43, result: Err("no".into()) }),
        ];
        for message in from_host {
            let bytes = message.encode();
            assert_eq!(FromHost::decode(bytes[0], &bytes[5..]), Some(message));
        }
    }

    /// A host in a thread, with the flat-colour engine, over a real socket and real shared
    /// memory: the window side opens a tab, receives its buffers and picture, reads the colour.
    #[test]
    fn a_picture_crosses() {
        let (mut window, host_end) = Link::pair().unwrap();
        let host = std::thread::spawn(move || crate::host::serve(Box::new(Fake::new()), Link::from_fd(host_end)));
        window.send(&ToHost::Open { tab: 1, url: "https://a.example/".into(), view: View::new(32, 16, 1.0) }.encode(), &[]).unwrap();
        let mut pictures: Option<Pictures> = None;
        let mut colour = None;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while colour.is_none() && std::time::Instant::now() < deadline {
            window.wait_readable(100);
            for (message, fds) in window.receive_from_host() {
                match message {
                    FromHost::Buffers { width, height, format, .. } => pictures = Some(Pictures::map(width, height, format, fds).unwrap()),
                    FromHost::Picture { tab, buffer, changed } => {
                        assert_eq!(changed, vec![Rect::whole(32, 16)]);
                        let bytes = pictures.as_ref().unwrap().buffer(buffer).unwrap();
                        colour = Some([bytes[2], bytes[1], bytes[0]]);
                        window.send(&ToHost::Release { tab, buffer }.encode(), &[]).unwrap();
                    }
                    _ => {}
                }
            }
        }
        assert_eq!(colour, Some(crate::engine::fake::colour_of("https://a.example/")));
        window.send(&ToHost::Quit.encode(), &[]).unwrap();
        host.join().unwrap();
    }

    #[test]
    fn the_canvas_brings_a_buffer_up_to_date() {
        let mut canvas = SharedCanvas::new();
        // First picture: all red, all new.
        canvas.begin(1, 4, 4, Format::Bgra).unwrap().fill(0x11);
        canvas.finish(1, &[Rect { x: 0, y: 0, width: 1, height: 1 }]);
        let first = canvas.take_outbox();
        assert!(matches!(first[1].0, FromHost::Picture { buffer: 0, ref changed, .. } if changed == &vec![Rect::whole(4, 4)]));
        // The window still has buffer 0; the next picture goes into 1, brought up to date
        // first, with only the changed corner drawn.
        let next = canvas.begin(1, 4, 4, Format::Bgra).unwrap();
        assert!(next.iter().all(|&b| b == 0x11), "the stale part was copied from the newest");
        next[0] = 0x22;
        canvas.finish(1, &[Rect { x: 0, y: 0, width: 1, height: 1 }]);
        // Both held now: nothing to draw into until one comes back.
        assert!(canvas.begin(1, 4, 4, Format::Bgra).is_none());
        canvas.release(1, 0);
        let again = canvas.begin(1, 4, 4, Format::Bgra).unwrap();
        assert_eq!(again[0], 0x22, "buffer 0 brought up to date with buffer 1's corner");
        // The engine with nothing new to draw.
        let _ = Fake::new().pump(Duration::ZERO, &mut canvas);
    }
}

#[cfg(test)]
mod golden {
    //! The bytes of a few messages, as Android's engine (android/src/.../mesh/Wire.java) must
    //! read and write them. android/test/WireTest.java checks the Java side against the same
    //! bytes; a change to the wire here fails this test until both are brought up to date
    //! (`cargo test -p spatimesh golden -- --nocapture` prints the new bytes).
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn to_host() -> Vec<(ToHost, &'static str)> {
        vec![
            (
                ToHost::Open { tab: 7, url: "https://example.com/ä".into(), view: View::new(1580, 868, 1.25) },
                "012e00000007000000000000001600000068747470733a2f2f6578616d706c652e636f6d2fc3a42c060000640300000000a03f",
            ),
            (
                ToHost::Pointer { tab: 7, x: 12.5, y: 40.0, button: Some((Button::Left, true)), modifiers: Modifiers { shift: true, ..Default::default() } },
                "091300000007000000000000000000484100002042010101",
            ),
            (
                ToHost::Scroll { tab: 7, x: 1.0, y: 2.0, dx: 0.0, dy: -120.0, modifiers: Modifiers::default() },
                "0b1900000007000000000000000000803f00000040000000000000f0c200",
            ),
            (
                ToHost::Key {
                    tab: 7,
                    key: KeyPress {
                        code: 30,
                        key: spatiand_app::Key::Char('a'),
                        text: Some("a".into()),
                        pressed: true,
                        repeat: false,
                        modifiers: Modifiers { control: true, ..Default::default() },
                    },
                },
                "0c1a00000007000000000000001e0000000f61000000010000006101010002",
            ),
            (ToHost::Choose { tab: 7, index: Some(2) }, "120c000000070000000000000002000000"),
            (ToHost::Answer { tab: 7, accepted: true, text: Some("yes".into()) }, "13110000000700000000000000010103000000796573"),
            (ToHost::Release { tab: 7, buffer: 1 }, "1009000000070000000000000001"),
            (ToHost::Quit, "1100000000"),
            (ToHost::Extensions { set: "{\"extensions\":[]}".into() }, "1415000000110000007b22657874656e73696f6e73223a5b5d7d"),
            (
                ToHost::RunScript { tab: 7, world: "spatimesh-abc".into(), all_frames: true, code: "1+1".into(), request: 42, boot: true },
                "152a00000007000000000000000d00000073706174696d6573682d6162630103000000312b312a0000000000000001",
            ),
        ]
    }

    fn from_host() -> Vec<(FromHost, &'static str)> {
        vec![
            (
                FromHost::Hello { engine: "Android System WebView 140".into() },
                "011e0000001a000000416e64726f69642053797374656d205765625669657720313430",
            ),
            (
                FromHost::Buffers { tab: 7, width: 1580, height: 868, format: Format::Rgba, count: 2 },
                "021200000007000000000000002c060000640300000102",
            ),
            (
                FromHost::Picture { tab: 7, buffer: 0, changed: vec![Rect::whole(1580, 868)] },
                "031d0000000700000000000000000100000000000000000000002c06000064030000",
            ),
            (FromHost::Happened(Happened::Title { tab: 7, title: "Ä title".into() }), "0a14000000070000000000000008000000c384207469746c65"),
            (FromHost::Happened(Happened::Progress { tab: 7, progress: 0.5, loading: true }), "0c0d00000007000000000000000000003f01"),
            (
                FromHost::Happened(Happened::OpenTab { opener: 7, url: "https://a/".into(), background: false }),
                "111700000007000000000000000a00000068747470733a2f2f612f00",
            ),
            (
                FromHost::Happened(Happened::Dialog { tab: 7, kind: DialogKind::Confirm, message: "Sure?".into(), text: String::new() }),
                "161600000007000000000000000105000000537572653f00000000",
            ),
            (FromHost::Happened(Happened::Extension { tab: 7, json: "{\"c\":\"ready\"}".into() }), "171900000007000000000000000d0000007b2263223a227265616479227d"),
            (FromHost::Happened(Happened::ScriptResult { tab: 7, request: 42, result: Ok("2".into()) }), "181600000007000000000000002a00000000000000010100000032"),
            (FromHost::Happened(Happened::ScriptResult { tab: 7, request: 43, result: Err("no".into()) }), "181700000007000000000000002b0000000000000000020000006e6f"),
        ]
    }

    #[test]
    fn the_bytes_android_reads_and_writes() {
        let mut changed = 0;
        for (message, bytes) in to_host() {
            let made = hex(&message.encode());
            println!("TO   {made}");
            changed += (made != bytes) as usize;
        }
        for (message, bytes) in from_host() {
            let made = hex(&message.encode());
            println!("FROM {made}");
            changed += (made != bytes) as usize;
        }
        assert_eq!(changed, 0, "the wire changed: bring android/test/WireTest.java and Wire.java up to date");
    }
}
