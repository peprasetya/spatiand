//! What a Spatiand application is written against.
//!
//! An application is a process of its own that asks for windows in the room, is told what the
//! wearer does to them, and draws them with OpenGL ES -- the same program on the Deck, the Mac
//! and the Beam Pro. Underneath is the cut-down Wayland the compositor speaks on all three
//! ([`wire`], private) and one way per platform of handing a picture over without copying it
//! ([`gl`]); neither is in this interface.
//!
//! ```no_run
//! use spatiand_app::{App, Event};
//! let mut app = App::connect("id.prasetya.spatiand.example")?;
//! let window = app.open("Example", (1280, 720));
//! loop {
//!     for event in app.wait(None)? {
//!         match event {
//!             Event::Ready { .. } | Event::Frame { .. } => {
//!                 app.draw(window, |gl, size| { /* GL, into a picture of `size` */ })?;
//!             }
//!             Event::CloseRequested { .. } => return Ok(()),
//!             _ => {}
//!         }
//!     }
//! }
//! # Ok::<(), String>(())
//! ```
//!
//! **One picture or two.** [`App::mode`] says whether the wearer is being drawn a picture for
//! each eye, and [`Event::Mode`] says when that changes -- the glasses unplugged from a Mac.
//! A flat window need not care. Content with depth says how its two eyes are packed
//! ([`App::set_eyes`]) or claims a layer ([`App::set_layer`]), and in [`Mode::Flat`] draws one
//! eye instead.
//!
//! [`ui`] is the room's own look as widgets: what the launcher and the settings are drawn
//! with, for an application's lists, grids and fields.

use std::collections::HashMap;
use std::time::{Duration, Instant};

pub mod config;
pub mod gl;
mod keys;
pub mod ui;
mod wire;

pub use gl::Gl;
pub use glow;

/// One of an application's windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(pub(crate) u32);

/// Whether the wearer is being drawn one picture or a picture for each eye.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// One picture: a Mac's own screen, or glasses showing the same to both eyes.
    Flat,
    /// The room, with a picture drawn for each eye.
    Stereo,
}

/// How a window's two eyes are packed into its picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EyeLayout {
    /// Both eyes see the whole picture.
    #[default]
    Mono,
    /// The left eye's view is the left half. Half-width and full-width are the same thing
    /// here: each eye is given its half stretched across the window.
    SideBySide,
    /// The left eye's view is the top half.
    TopBottom,
}

/// What a window is in the room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layer {
    /// An ordinary panel, moved and resized by the wearer.
    #[default]
    Window,
    /// A panel that follows the view.
    HeadLocked,
    /// The two eye views themselves, drawn by the application from the head's pose.
    Projection,
    /// The sky's front half: an equirectangular picture 180 degrees across.
    Sky180,
    /// All of the sky.
    Sky360,
}

/// Which edge of a window stays where it is when its picture changes shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Anchor {
    #[default]
    Centre,
    Top,
    Bottom,
}

/// A button of the pointer's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
    Other(u32),
}

/// A key, by what it does. Letters and digits are [`Key::Char`], in lower case whatever is
/// held; what was *typed* is the event's `text`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Enter,
    Escape,
    Backspace,
    Delete,
    Tab,
    Space,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Char(char),
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub logo: bool,
}

/// Something the compositor or the wearer did.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The wearer went between one picture and two.
    Mode(Mode),
    /// A window may now be drawn. `suggested` is the size the compositor would give a window
    /// it knows nothing about, for an application with no idea of its own.
    Ready { window: WindowId, suggested: Option<(u32, u32)> },
    /// The wearer resized a window: draw it at this size, or at the nearest the content allows.
    Resized { window: WindowId, width: u32, height: u32 },
    /// The wearer closed a window. It stays until the application closes it.
    CloseRequested { window: WindowId },
    /// The compositor would like the window's next picture: the answer to [`App::ask_frame`].
    Frame { window: WindowId, time_ms: u32 },
    /// Where the pointer is, in the window's own pixels from its top left.
    PointerMoved { window: WindowId, x: f32, y: f32 },
    PointerLeft { window: WindowId },
    Button { window: WindowId, button: Button, pressed: bool },
    /// A wheel or two fingers, in pixels; positive is down and to the right.
    Scroll { window: WindowId, dx: f32, dy: f32 },
    /// `code` is the key's position (Linux's number for it); `text` is what pressing it typed,
    /// if it types anything. `repeat` is a key held down.
    Key { window: WindowId, key: Key, code: u32, pressed: bool, repeat: bool, text: Option<String>, modifiers: Modifiers },
    Focus { window: WindowId, focused: bool },
    /// A layer was asked for and not given; the window is whatever it was before.
    LayerRefused { window: WindowId, layer: Layer },
    /// A layer only one window can have was taken back. The window is a window again.
    LayerLost { window: WindowId, layer: Layer },
}

impl Event {
    /// The window an event is about, if it is about one.
    pub fn window(&self) -> Option<WindowId> {
        match self {
            Event::Mode(_) => None,
            Event::Ready { window, .. }
            | Event::Resized { window, .. }
            | Event::CloseRequested { window }
            | Event::Frame { window, .. }
            | Event::PointerMoved { window, .. }
            | Event::PointerLeft { window }
            | Event::Button { window, .. }
            | Event::Scroll { window, .. }
            | Event::Key { window, .. }
            | Event::Focus { window, .. }
            | Event::LayerRefused { window, .. }
            | Event::LayerLost { window, .. } => Some(*window),
        }
    }
}

/// An application's connection to the room.
pub struct App {
    app_id: String,
    wire: wire::Wire,
    queue: wayland_client::EventQueue<wire::Wire>,
    gl: Option<Gl>,
    /// Each window's size, and what it is drawn into once it has been drawn.
    windows: HashMap<WindowId, ((u32, u32), Option<gl::Target>)>,
    /// Files whose being readable is a reason to wake: see [`App::watch`].
    watched: Vec<i32>,
}

impl App {
    /// Connect to the Spatiand this process was started by. `app_id` names the application to
    /// the room -- its icon, its controller layout, where its sound is placed.
    pub fn connect(app_id: &str) -> Result<App, String> {
        let (wire, queue) = wire::Wire::connect()?;
        let mut app = App { app_id: app_id.to_string(), wire, queue, gl: None, windows: HashMap::new(), watched: Vec::new() };
        // Once round, so the mode is the compositor's word and not this crate's guess by the
        // time anything is decided on it.
        app.queue.roundtrip(&mut app.wire).map_err(|e| format!("lost Spatiand: {e}"))?;
        app.wire.events.retain(|event| !matches!(event, Event::Mode(_)));
        Ok(app)
    }

    /// Shift, Control, Alt and the logo key as they are held now: what a click is to be read
    /// with (Shift-click, Control-click), since a pointer's button says nothing of the keys.
    pub fn modifiers(&self) -> Modifiers {
        self.wire.keyboard.held_modifiers()
    }

    pub fn mode(&self) -> Mode {
        self.wire.mode
    }

    /// Whether this is Spatiand's room at all, and not a desktop the application was started
    /// on for development: layers and eye layouts mean something only in the room.
    pub fn in_the_room(&self) -> bool {
        self.wire.in_the_room()
    }

    /// Open a window whose picture is `size` pixels. It may be drawn once it is [`Event::Ready`].
    pub fn open(&mut self, title: &str, size: (u32, u32)) -> WindowId {
        let id = self.wire.open(&self.app_id, title);
        self.windows.insert(id, (size, None));
        id
    }

    pub fn close(&mut self, window: WindowId) {
        if let Some((_, Some(target))) = self.windows.remove(&window) {
            if let Some(gl) = self.gl.as_ref() {
                target.destroy(gl, &mut self.wire, window);
            }
        }
        self.wire.close(window);
    }

    pub fn size(&self, window: WindowId) -> (u32, u32) {
        self.windows.get(&window).map_or((0, 0), |w| w.0)
    }

    /// Change the size of a window's picture, which takes effect with the next one drawn.
    pub fn resize(&mut self, window: WindowId, size: (u32, u32)) {
        if let Some(entry) = self.windows.get_mut(&window) {
            entry.0 = (size.0.max(1), size.1.max(1));
        }
    }

    pub fn set_title(&mut self, window: WindowId, title: &str) {
        self.wire.set_title(window, title);
    }

    /// How the window's two eyes are packed, from its next picture on. `swapped` is for a
    /// source packed right eye first.
    pub fn set_eyes(&mut self, window: WindowId, eyes: EyeLayout, swapped: bool) {
        self.wire.set_eyes(window, eyes, swapped);
    }

    /// The size the window is, in the pixels the pointer is told in, where that is not its
    /// picture's size: a film with its two eyes side by side is a picture twice as wide as
    /// the window it is shown in, and a picture drawn finer than the window is sharper for it.
    /// `None` goes back to the picture's own size. From the next picture on.
    pub fn set_shape(&mut self, window: WindowId, shape: Option<(u32, u32)>) {
        self.wire.set_shape(window, shape);
    }

    /// What the window is in the room, from its next picture on. Silence is the yes: a claim
    /// that cannot be honoured comes back as [`Event::LayerRefused`].
    pub fn set_layer(&mut self, window: WindowId, layer: Layer) {
        self.wire.set_layer(window, layer);
    }

    /// Turn a sky about the vertical, to bring a panorama's interesting part round to the front.
    pub fn set_yaw(&mut self, window: WindowId, radians: f32) {
        self.wire.set_yaw(window, radians);
    }

    /// Let the room fade this window out after the wearer has done nothing for `after`, and
    /// bring it back when they stir: a transport bar over a film. `None` turns it off.
    pub fn set_idle_fade(&mut self, window: WindowId, after: Option<Duration>) {
        self.wire.set_idle_fade(window, after);
    }

    pub fn set_anchor(&mut self, window: WindowId, anchor: Anchor) {
        self.wire.set_anchor(window, anchor);
    }

    /// The window draws the pointer itself while it is over it: for content with depth, where
    /// only the application knows how far away what is pointed at is.
    pub fn set_cursor_drawn(&mut self, window: WindowId, drawn: bool) {
        self.wire.set_cursor_drawn(window, drawn);
    }

    /// Ask for an [`Event::Frame`] when the compositor next wants this window drawn. Asked
    /// before drawing, it paces an animation to the display.
    pub fn ask_frame(&mut self, window: WindowId) {
        self.wire.ask_frame(window);
    }

    /// Put into effect what has been said about a window without drawing it again.
    pub fn commit(&mut self, window: WindowId) {
        self.wire.commit(window);
    }

    /// The application's GL context, made the first time it is wanted.
    pub fn gl(&mut self) -> Result<&Gl, String> {
        if self.gl.is_none() {
            self.gl = Some(Gl::new()?);
        }
        Ok(self.gl.as_ref().unwrap())
    }

    /// Draw a window. `paint` is given GL, with the place to draw already bound and the
    /// viewport set, and the picture's size. **Row 0 is the top**: use a projection with y
    /// downwards, as [`ui::Painter`] does.
    ///
    /// Does nothing, and says so with `false`, before the window is [`Event::Ready`].
    pub fn draw(&mut self, window: WindowId, paint: impl FnOnce(&Gl, (u32, u32))) -> Result<bool, String> {
        if !self.wire.is_ready(window) {
            return Ok(false);
        }
        self.gl()?;
        let gl = self.gl.as_ref().unwrap();
        let Some((size, target)) = self.windows.get_mut(&window) else { return Ok(false) };
        if target.as_ref().is_some_and(|t| t.size() != *size) {
            target.take().unwrap().destroy(gl, &mut self.wire, window);
        }
        if target.is_none() {
            *target = Some(gl::Target::new(gl, *size)?);
        }
        let target = target.as_mut().unwrap();
        target.begin(gl, &mut self.wire, window)?;
        paint(gl, *size);
        target.present(gl, &mut self.wire, window)?;
        Ok(true)
    }

    /// Show a picture drawn in ordinary memory: `fill` is given `size.0 * size.1 * 4` bytes,
    /// blue, green, red, alpha, colour already multiplied by alpha, top row first.
    pub fn show_pixels(&mut self, window: WindowId, fill: impl FnOnce(&mut [u8])) -> Result<(), String> {
        let size = self.size(window);
        self.wire.show_pixels(window, size, fill)
    }

    /// Wake [`App::wait`] when `fd` has something to read -- a child process's output, a
    /// socket -- as well as for the room. What is read, and when, is the application's.
    pub fn watch(&mut self, fd: i32) {
        if !self.watched.contains(&fd) {
            self.watched.push(fd);
        }
    }

    pub fn unwatch(&mut self, fd: i32) {
        self.watched.retain(|&w| w != fd);
    }

    /// Wait for something to happen, for at most `timeout`, and say what did. Sends whatever
    /// the application has said since the last call, so nothing reaches the room until this is
    /// called.
    pub fn wait(&mut self, timeout: Option<Duration>) -> Result<Vec<Event>, String> {
        let now = Instant::now();
        let deadline = timeout.map(|t| now + t);
        // A held key repeats on this side of the wire, so its next repeat is a reason to wake.
        let wake = match (deadline, self.wire.next_repeat()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        wire::pump(&mut self.wire, &mut self.queue, wake.map(|at| at.saturating_duration_since(now)), &self.watched)?;
        self.wire.repeat(Instant::now());
        Ok(std::mem::take(&mut self.wire.events))
    }
}
