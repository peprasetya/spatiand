//! SpatiLoupe: Spatiand's viewer.
//!
//! Like Preview on a Mac (docs/spatiand-apps.md): pictures of every common kind, stereo
//! photographs shown to each eye, PDFs, and 3D objects drawn with their materials, in true
//! stereo in the room and flat on a flat screen. A folder's things are tiles to choose from;
//! one is looked at full window, and the folder's others are along its foot.
//!
//! ```text
//! spatiloupe                 the pictures folder
//! spatiloupe <folder>        that folder
//! spatiloupe <file>          that file, with its folder behind it
//! ```

mod folder;
mod model;
mod pdf;
mod picture;
mod stereo;
mod view;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use spatiand_app::ui::{look, theme, Input, Painter, Rect};
use spatiand_app::{App, EyeLayout, Event, Mode, WindowId};

use folder::{Chose, Folder, Kind};
use model::Eye;
use view::{View, Want};

pub const WINDOW: (u32, u32) = (1440, 900);

enum Done {
    Listed { ticket: u64, result: Result<Vec<folder::Entry>, String> },
    Tile { ticket: u64, index: usize, width: u32, height: u32, rgba: Vec<u8>, word: Option<String> },
    Picture { ticket: u64, result: Result<picture::Picture, String> },
    Model { ticket: u64, result: Result<model::Scene, String> },
}

type Job = Box<dyn FnOnce() -> Option<Done> + Send>;

fn worker(name: &str, done: Sender<Done>) -> Sender<Job> {
    let (jobs, queue) = channel::<Job>();
    let _ = std::thread::Builder::new().name(name.into()).spawn(move || {
        for job in queue {
            if let Some(answer) = job() {
                if done.send(answer).is_err() {
                    return;
                }
            }
        }
    });
    jobs
}

/// A tile's picture: a picture made small, or a PDF's first page.
fn tile_picture(path: &Path, kind: Kind) -> Option<(u32, u32, Vec<u8>, Option<String>)> {
    match kind {
        Kind::Picture => {
            let picture = picture::read(path, 360).ok()?;
            let word = picture.eyes.is_stereo().then(|| "3D".to_string());
            // A stereo pair's tile is one eye's view, not two pictures squeezed side by side.
            let (left, _) = picture.eyes.halves();
            let (x0, y0) = ((left[0] * picture.width as f32) as u32, (left[1] * picture.height as f32) as u32);
            let (x1, y1) = ((left[2] * picture.width as f32) as u32, (left[3] * picture.height as f32) as u32);
            let (w, h) = (x1 - x0, y1 - y0);
            let mut rgba = Vec::with_capacity((w * h * 4) as usize);
            for y in y0..y1 {
                let from = ((y * picture.width + x0) * 4) as usize;
                rgba.extend_from_slice(&picture.rgba[from..from + (w * 4) as usize]);
            }
            Some((w, h, rgba, word))
        }
        // A page whole, on a square of the window's ground: a tile shows a square's middle,
        // and a page's middle is not what it looks like.
        Kind::Pdf => pdf::first_page(path, 300).map(|(w, h, rgba)| {
            let side = w.max(h);
            let mut square = [11u8, 13, 18, 255].repeat((side * side) as usize);
            let (x0, y0) = ((side - w) / 2, (side - h) / 2);
            for y in 0..h {
                let to = (((y0 + y) * side + x0) * 4) as usize;
                square[to..to + (w * 4) as usize].copy_from_slice(&rgba[(y * w * 4) as usize..((y + 1) * w * 4) as usize]);
            }
            (side, side, square, None)
        }),
        _ => None,
    }
}

struct Loupe {
    /// The file being opened now: a job for one opened before it is not started at all.
    current: std::sync::Arc<std::sync::atomic::AtomicU64>,
    app: App,
    window: WindowId,
    painter: Option<Painter>,
    calls: Sender<Job>,
    pictures: Sender<Job>,
    done: Receiver<Done>,
    folder: Folder,
    view: Option<View>,
    ticket: u64,
    ready: bool,
    dirty: bool,
    /// Whether the window is drawn as two eyes now.
    two: bool,
    /// What a file opened at the start is, to open once its folder is listed.
    opening: Option<PathBuf>,
    left: Vec<View>,
}

impl Loupe {
    fn ticket(&mut self) -> u64 {
        self.ticket += 1;
        self.ticket
    }

    fn list(&mut self, path: PathBuf, cursor_on: Option<PathBuf>) {
        let ticket = self.ticket();
        let mut folder = Folder::new(path.clone(), ticket);
        folder.cursor_on = cursor_on;
        self.folder = folder;
        let title = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        self.calls.send(Box::new(move || Some(Done::Listed { ticket, result: folder::list(&path) }))).ok();
        self.app.set_title(self.window, &format!("{title} \u{2014} SpatiLoupe"));
        self.dirty = true;
    }

    fn open(&mut self, index: usize) {
        let Some(entry) = self.folder.entries.get(index).cloned() else { return };
        let ticket = self.ticket();
        let files: Vec<usize> = self.folder.entries.iter().enumerate().filter(|(_, e)| e.kind != Kind::Folder).map(|(i, _)| i).collect();
        if let Some(old) = self.view.take() {
            self.left.push(old);
        }
        let mut view = View::new(entry.clone(), index, ticket, files);
        let path = entry.path.clone();
        self.current.store(ticket, std::sync::atomic::Ordering::Relaxed);
        let current = self.current.clone();
        let still_wanted = move || current.load(std::sync::atomic::Ordering::Relaxed) == ticket;
        match entry.kind {
            Kind::Picture => {
                self.calls.send(Box::new(move || still_wanted().then(|| Done::Picture { ticket, result: picture::read(&path, picture::LARGEST) }))).ok();
            }
            Kind::Model => {
                self.calls.send(Box::new(move || still_wanted().then(|| Done::Model { ticket, result: model::Scene::read(&path) }))).ok();
            }
            Kind::Pdf => view.open_pdf(),
            Kind::Folder => return,
        }
        self.app.set_title(self.window, &format!("{} \u{2014} SpatiLoupe", entry.name));
        self.view = Some(view);
        self.dirty = true;
    }

    fn leave(&mut self) {
        if let Some(view) = self.view.take() {
            self.folder.point_at(&view.path());
            self.left.push(view);
        }
        let path = self.folder.path.clone();
        let title = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        self.app.set_title(self.window, &format!("{title} \u{2014} SpatiLoupe"));
        self.dirty = true;
    }

    fn answers(&mut self) {
        while let Ok(done) = self.done.try_recv() {
            self.dirty = true;
            match done {
                Done::Listed { ticket, result } => {
                    if ticket != self.folder.ticket {
                        continue;
                    }
                    self.folder.listed(result);
                    if let Some(path) = self.opening.take() {
                        match self.folder.entries.iter().position(|e| e.path == path) {
                            Some(index) => self.open(index),
                            None => {
                                // Not in the list -- a kind it does not know by name: shown anyway.
                                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                                self.folder.entries.push(folder::Entry { name, path, kind: Kind::Picture, size });
                                let index = self.folder.entries.len() - 1;
                                self.open(index);
                            }
                        }
                    }
                }
                Done::Tile { ticket, index, width, height, rgba, word } => self.folder.pictured(ticket, index, width, height, rgba, word),
                Done::Picture { ticket, result } => {
                    if let Some(view) = self.view.as_mut().filter(|v| v.ticket == ticket) {
                        if let Err(e) = &result {
                            log::warn!("{}: {e}", view.entry.path.display());
                        }
                        view.pictured(result.map_err(|e| format!("This picture would not open: {e}")));
                    }
                }
                Done::Model { ticket, result } => {
                    if let Some(view) = self.view.as_mut().filter(|v| v.ticket == ticket) {
                        if let Err(e) = &result {
                            log::warn!("{}: {e}", view.entry.path.display());
                        }
                        view.modelled(result);
                    }
                }
            }
        }
        if let Some(view) = self.view.as_mut() {
            self.dirty |= view.answers();
        }
    }

    fn handle(&mut self, event: Event) -> bool {
        match &event {
            Event::CloseRequested { .. } => return false,
            Event::Ready { .. } => self.ready = true,
            Event::Mode(mode) => log::info!("the wearer now sees {mode:?}"),
            _ => {}
        }
        self.dirty = true;
        let Some(input) = Input::of(&event, 1.0) else { return true };
        match self.view.as_mut() {
            Some(view) => match view.input(&input) {
                Want::Nothing => {}
                Want::Leave => self.leave(),
                Want::Open(index) => self.open(index),
            },
            None => match self.folder.input(&input) {
                Chose::Nothing => {}
                Chose::Folder(path) => {
                    let came_from = self.folder.path.clone();
                    // Up: the cursor on the folder come out of; down: at the start.
                    let cursor = came_from.starts_with(&path).then_some(came_from);
                    self.list(path, cursor);
                }
                Chose::Open(index) => self.open(index),
            },
        }
        true
    }

    /// Whether the window is to be two eyes' pictures, and if that has changed, make it so.
    fn stage(&mut self) {
        let two = self.app.mode() == Mode::Stereo && self.app.in_the_room() && self.view.as_ref().is_some_and(|v| v.has_depth());
        if two == self.two {
            return;
        }
        self.two = two;
        if two {
            self.app.resize(self.window, (WINDOW.0 * 2, WINDOW.1));
            self.app.set_shape(self.window, Some(WINDOW));
            self.app.set_eyes(self.window, EyeLayout::SideBySide, false);
        } else {
            self.app.resize(self.window, WINDOW);
            self.app.set_shape(self.window, None);
            self.app.set_eyes(self.window, EyeLayout::Mono, false);
        }
    }

    fn draw(&mut self) -> Result<(), String> {
        if !self.ready {
            return Ok(());
        }
        // The tiles wanted: the folder's on show, or the strip's around what is being looked at.
        let wanted = match self.view.as_ref() {
            Some(view) => self.folder.wanted_near(view.index, 14),
            None => self.folder.wanted(),
        };
        for (index, path, kind) in wanted {
            let ticket = self.folder.ticket;
            self.pictures
                .send(Box::new(move || {
                    let (width, height, rgba, word) = tile_picture(&path, kind)?;
                    Some(Done::Tile { ticket, index, width, height, rgba, word })
                }))
                .ok();
        }
        if let Some(view) = self.view.as_mut() {
            self.dirty |= view.tick();
        }
        let animating = self.view.as_ref().map_or(self.folder.animating(), |v| v.animating());
        if !std::mem::take(&mut self.dirty) && !animating {
            return Ok(());
        }
        self.stage();
        let two = self.two;
        let (view, folder, painter, left) = (&mut self.view, &mut self.folder, &mut self.painter, &mut self.left);
        self.app.draw(self.window, |gl, size| {
            let p = painter.get_or_insert_with(|| Painter::new(gl).expect("the painter"));
            for old in left.drain(..) {
                old.forget(gl);
            }
            match view.as_mut() {
                Some(view) => {
                    // The strip's tiles are the folder's: the files only, in order.
                    folder.take_arrived(p, gl);
                    view::TILES.with(|t| *t.borrow_mut() = folder.file_tiles());
                    view.settle_turns(gl);
                    p.begin(gl, size, size.1 as f32 / WINDOW.1 as f32, theme::GROUND);
                    if two {
                        let half = size.0 as f32 * 0.5;
                        view.paint(p, gl, size, Rect::new(0.0, 0.0, half, size.1 as f32), Eye::Left);
                        view.paint(p, gl, size, Rect::new(half, 0.0, half, size.1 as f32), Eye::Right);
                    } else {
                        view.paint(p, gl, size, Rect::new(0.0, 0.0, size.0 as f32, size.1 as f32), Eye::Both);
                    }
                }
                None => {
                    p.begin(gl, size, 1.0, theme::GROUND);
                    look::backdrop(p, gl);
                    folder.draw(p, gl);
                    p.end(gl);
                }
            }
        })?;
        Ok(())
    }

    fn run(&mut self) -> Result<(), String> {
        loop {
            let animating = self.view.as_ref().map_or(self.folder.animating(), |v| v.animating());
            let mut wait = Duration::from_millis(if animating { 16 } else { 120 });
            if let Some(next) = self.view.as_ref().and_then(|v| v.next_wake()) {
                wait = wait.min(next.max(Duration::from_millis(4)));
            }
            for event in self.app.wait(Some(wait))? {
                if !self.handle(event) {
                    return Ok(());
                }
            }
            self.answers();
            self.draw()?;
        }
    }
}

impl Folder {
    /// The tiles of the files, without the folders, for the viewer's strip.
    pub fn file_tiles(&self) -> Vec<spatiand_app::ui::Tile> {
        // Names only: the strip is short, and what each is is in the bar when it is looked at.
        self.entries
            .iter()
            .zip(self.tiles())
            .filter(|(e, _)| e.kind != Kind::Folder)
            .map(|(_, t)| spatiand_app::ui::Tile { detail: None, ..t.clone() })
            .collect()
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let asked = std::env::args_os().nth(1).map(PathBuf::from);
    let run = || -> Result<(), String> {
        let mut app = App::connect("id.prasetya.spatiand.spatiloupe")?;
        let window = app.open("SpatiLoupe", WINDOW);
        let (done, answers) = channel();
        let mut loupe = Loupe {
            current: Default::default(),
            app,
            window,
            painter: None,
            calls: worker("calls", done.clone()),
            pictures: worker("pictures", done),
            done: answers,
            folder: Folder::new(PathBuf::new(), 0),
            view: None,
            ticket: 0,
            ready: false,
            dirty: true,
            two: false,
            opening: None,
            left: Vec::new(),
        };
        match asked {
            Some(path) if path.is_dir() => loupe.list(path, None),
            Some(path) => {
                let path = std::fs::canonicalize(&path).unwrap_or(path);
                let folder = path.parent().map(Path::to_path_buf).unwrap_or_else(folder::home);
                loupe.opening = Some(path.clone());
                loupe.list(folder, Some(path));
            }
            None => loupe.list(folder::home(), None),
        }
        loupe.run()
    };
    if let Err(e) = run() {
        log::error!("{e}");
        std::process::exit(1);
    }
}
