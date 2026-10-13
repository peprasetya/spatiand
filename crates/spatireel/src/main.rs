//! SpatiReel: Spatiand's video player.
//!
//! The Vibrefy Rust port, moved into Spatiand (docs/spatiand-apps.md): what it knew about
//! films -- their layouts, their names, their sound -- is `spatiand-media`, and what it drew
//! with is replaced by the room's own widgets. This is the application: a library to choose
//! from, and a film being watched.
//!
//! ```text
//! spatireel                 the library, at this machine's folder of films
//! spatireel <file>          that film, at once
//! spatireel <folder>        the library, at that folder
//! ```

mod library;
mod play;

use std::path::{Path, PathBuf};

use spatiand_app::ui::{theme, Button, Grid, Input, Painter, Shape, Tile};
use spatiand_app::{App, Event, Key, WindowId};

use library::{Entry, Kind};
use play::{Outcome, Playing};

const LIBRARY: (u32, u32) = (1280, 800);

/// The library: one folder's folders and films.
struct Library {
    folder: PathBuf,
    entries: Vec<Entry>,
    tiles: Vec<Tile>,
    grid: Grid,
    up: Button,
    /// Where the cursor was in each folder above this one, to come back to.
    trail: Vec<usize>,
    /// What went wrong opening the last film chosen, said until something else is.
    trouble: Option<String>,
}

impl Library {
    fn at(folder: PathBuf) -> Library {
        let mut library = Library {
            folder: PathBuf::new(),
            entries: Vec::new(),
            tiles: Vec::new(),
            grid: Grid::default(),
            up: Button::default(),
            trail: Vec::new(),
            trouble: None,
        };
        library.grid.shape = Shape::Wide;
        library.grid.focused = true;
        library.show(folder, 0);
        library
    }

    fn show(&mut self, folder: PathBuf, cursor: usize) {
        self.entries = library::list(&folder);
        self.tiles = self
            .entries
            .iter()
            .map(|entry| match entry.kind {
                Kind::Folder => Tile { title: entry.name.clone(), detail: Some("Folder".into()), ..Tile::default() },
                Kind::Film => Tile { title: library::title(&entry.name), detail: library::told_by_name(&entry.name), ..Tile::default() },
            })
            .collect();
        self.folder = folder;
        self.grid.cursor = cursor;
    }

    fn go_up(&mut self) -> bool {
        let Some(parent) = self.folder.parent().map(Path::to_path_buf) else { return false };
        let cursor = self.trail.pop().unwrap_or(0);
        self.show(parent, cursor);
        true
    }

    fn draw(&mut self, p: &mut Painter, gl: &spatiand_app::Gl) {
        let mut all = p.bounds().inset(theme::PAD);
        let mut top = all.take_top(Button::HEIGHT);
        if self.folder.parent().is_some() {
            let width = Button::width(p, gl, "\u{2039}  Up");
            self.up.draw(p, gl, top.take_left(width), "\u{2039}  Up");
            top.take_left(20.0);
        }
        let name = self.folder.file_name().map_or_else(|| self.folder.display().to_string(), |n| n.to_string_lossy().into_owned());
        p.label(gl, &name, top, theme::TITLE_EM, theme::INK, 0.0);
        all.take_top(16.0);
        let foot = all.take_bottom(theme::FOOTER_EM * 1.6);
        match self.trouble.as_deref() {
            Some(trouble) => p.label(gl, trouble, foot, theme::FOOTER_EM, [1.0, 0.62, 0.55, 1.0], 0.0),
            None => p.label(gl, &self.folder.display().to_string(), foot, theme::FOOTER_EM, theme::INK_HINT, 0.0),
        };
        if self.tiles.is_empty() {
            p.label(gl, "No films or folders here.", all, theme::ROW_EM, theme::INK_DETAIL, 0.5);
        } else {
            self.grid.draw(p, gl, all, &self.tiles);
        }
    }

    /// Returns the film chosen, if one was.
    fn input(&mut self, input: &Input) -> Option<Entry> {
        if self.up.input(input) || matches!(input, Input::Key { key: Key::Backspace | Key::Escape, .. }) {
            self.go_up();
            return None;
        }
        let chosen = self.grid.input(input)?;
        let entry = self.entries.get(chosen)?.clone();
        match entry.kind {
            Kind::Folder => {
                self.trail.push(chosen);
                self.show(entry.path, 0);
                self.trouble = None;
                None
            }
            Kind::Film => Some(entry),
        }
    }
}

struct Reel {
    app: App,
    window: WindowId,
    painter: Option<Painter>,
    library: Library,
    playing: Option<Playing>,
    ready: bool,
    dirty: bool,
}

impl Reel {
    fn play(&mut self, path: &Path) {
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let opened = self.app.gl().and_then(|gl| Playing::open(gl, &path.to_string_lossy(), &name));
        match opened {
            Ok(playing) => {
                self.app.set_title(self.window, &library::title(&name));
                self.playing = Some(playing);
                self.library.trouble = None;
            }
            Err(e) => {
                log::warn!("{e}");
                self.library.trouble = Some(format!("{name} would not play: {e}"));
            }
        }
        self.dirty = true;
    }

    fn stop(&mut self) {
        if let Some(playing) = self.playing.take() {
            playing.close(&mut self.app, self.window);
        }
        self.app.resize(self.window, LIBRARY);
        self.app.set_title(self.window, "SpatiReel");
        self.dirty = true;
    }

    fn handle(&mut self, event: Event) -> bool {
        match &event {
            Event::CloseRequested { window } if *window == self.window => return false,
            // The film's own surface was closed: the film is over.
            Event::CloseRequested { .. } => self.stop(),
            Event::Ready { window, .. } if *window == self.window => self.ready = true,
            Event::Mode(mode) => log::info!("the wearer now sees {mode:?}"),
            Event::LayerRefused { window, .. } | Event::LayerLost { window, .. } => {
                if let Some(playing) = self.playing.as_mut().filter(|p| p.sky() == Some(*window)) {
                    playing.sky_was_refused();
                }
            }
            _ => {}
        }
        self.dirty = true;
        // What the wearer does in the film's own surface is not for the controls.
        if event.window().is_some_and(|window| window != self.window) {
            return true;
        }
        let Some(input) = Input::of(&event, 1.0) else { return true };
        match self.playing.as_mut() {
            Some(playing) => {
                if playing.input(&input) == Outcome::Close {
                    self.stop();
                }
            }
            None => {
                if let Some(film) = self.library.input(&input) {
                    self.play(&film.path);
                }
            }
        }
        true
    }

    fn draw(&mut self) -> Result<(), String> {
        if !self.ready {
            return Ok(());
        }
        if let Some(playing) = self.playing.as_mut() {
            if playing.player().ended() {
                self.stop();
            } else {
                playing.dirty |= self.dirty;
                self.dirty = false;
                playing.draw(&mut self.app, self.window, &mut self.painter)?;
                return Ok(());
            }
        }
        if !std::mem::take(&mut self.dirty) {
            return Ok(());
        }
        let (library, painter) = (&mut self.library, &mut self.painter);
        self.app.draw(self.window, |gl, size| {
            let p = painter.get_or_insert_with(|| Painter::new(gl).expect("the painter"));
            p.begin(gl, size, 1.0, theme::GROUND);
            library.draw(p, gl);
            p.end(gl);
        })?;
        Ok(())
    }

    fn run(mut self) -> Result<(), String> {
        loop {
            // While a film plays there is a next picture to be on time for; otherwise only
            // the wearer can make anything change.
            let wait = self.playing.as_ref().map(Playing::next_wake);
            for event in self.app.wait(wait)? {
                if !self.handle(event) {
                    return Ok(());
                }
            }
            self.draw()?;
        }
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let asked = std::env::args_os().nth(1).map(PathBuf::from);
    let (folder, film) = match asked {
        Some(path) if path.is_dir() => (path, None),
        Some(path) => (path.parent().map(Path::to_path_buf).filter(|p| p.is_dir()).unwrap_or_else(library::home), Some(path)),
        None => (library::home(), None),
    };
    let run = || -> Result<(), String> {
        let mut app = App::connect("id.prasetya.spatiand.spatireel")?;
        let window = app.open("SpatiReel", LIBRARY);
        let mut reel = Reel { app, window, painter: None, library: Library::at(folder), playing: None, ready: false, dirty: true };
        if let Some(film) = film {
            reel.play(&film);
        }
        reel.run()
    };
    if let Err(e) = run() {
        log::error!("{e}");
        std::process::exit(1);
    }
}
