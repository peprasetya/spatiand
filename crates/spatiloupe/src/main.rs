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

mod annotate;
mod folder;
mod ink;
mod markup;
mod model;
mod pdf;
mod picture;
mod place;
mod save;
mod stereo;
mod swipe;
mod tools;
mod view;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use spatiand_app::ui::{look, theme, Input, Painter, Rect};
use spatiand_app::{App, EyeLayout, Event, Mode, WindowId};

use folder::{Arrived, Chose, Folder, Kind};
use model::Eye;
use view::{Seen, View, Want};

pub const WINDOW: (u32, u32) = (1440, 900);

enum Done {
    Listed { ticket: u64, result: Result<Vec<folder::Entry>, String> },
    Tile { ticket: u64, tile: Arrived },
    Picture { ticket: u64, result: Result<picture::Picture, String> },
    Model { ticket: u64, result: Result<model::Scene, String> },
    /// A picture marked up, written: where, or why not.
    Saved { ticket: u64, result: Result<PathBuf, String> },
}

/// A picture read before it is looked at: the one the carousel is heading for.
struct Ahead {
    /// Its place in the folder.
    index: usize,
    ticket: u64,
    /// Once it has been read.
    result: Option<Result<picture::Picture, String>>,
}

/// Something for a worker to do, and what to answer if it panics: a picture or an object that
/// would not open says so, rather than its view waiting for ever.
struct Job {
    run: Box<dyn FnOnce() -> Option<Done> + Send>,
    failed: Option<Box<dyn FnOnce(String) -> Done + Send>>,
}

impl Job {
    fn new(run: impl FnOnce() -> Option<Done> + Send + 'static) -> Job {
        Job { run: Box::new(run), failed: None }
    }

    fn or(mut self, failed: impl FnOnce(String) -> Done + Send + 'static) -> Job {
        self.failed = Some(Box::new(failed));
        self
    }
}

/// What a panic said, in words.
fn panic_words(panic: &(dyn std::any::Any + Send)) -> String {
    let said = panic.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| panic.downcast_ref::<String>().cloned());
    format!("SpatiLoupe failed reading it ({})", said.unwrap_or_else(|| "a panic".into()))
}

/// A thread doing jobs one after another. A job that panics -- a decoder's bug on a strange file
/// -- is answered with what its job says, and the thread goes on to the next: one bad file does
/// not leave every later one unread, without a word.
fn worker(name: &str, done: Sender<Done>) -> Sender<Job> {
    let (jobs, queue) = channel::<Job>();
    let _ = std::thread::Builder::new().name(name.into()).spawn(move || {
        for Job { run, failed } in queue {
            let answer = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
                Ok(answer) => answer,
                Err(panic) => {
                    let words = panic_words(&*panic);
                    log::error!("{words}");
                    failed.map(|failed| failed(words))
                }
            };
            if let Some(answer) = answer {
                if done.send(answer).is_err() {
                    return;
                }
            }
        }
    });
    jobs
}

/// A tile's picture: a picture made small, or a PDF's first page.
fn tile_picture(path: &Path, kind: Kind, index: usize) -> Option<Arrived> {
    match kind {
        Kind::Picture => {
            let picture = picture::read_still(path, 360).ok()?;
            let word = picture.eyes.is_stereo().then(|| "3D".to_string());
            // What one eye of the whole picture sees, from its own size rather than the tile's
            // rounded one: the tile stands in for the picture as the carousel brings it in.
            let (w, h) = picture.original;
            let seen = Seen { aspect: view::eye_aspect(w as f32, h as f32, picture.eyes), height: view::eye_height(w as f32, h as f32, picture.eyes), stereo: picture.eyes.is_stereo() };
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
            Some(Arrived { index, width: w, height: h, rgba, word, seen: Some(seen) })
        }
        // A page whole, on a square of the window's ground: a tile shows a square's middle,
        // and a page's middle is not what it looks like.
        Kind::Pdf => pdf::first_page(path, 300).and_then(|(w, h, rgba)| {
            let (side, rgba) = squared(w as usize, h as usize, &rgba)?;
            Some(Arrived { index, width: side as u32, height: side as u32, rgba, word: None, seen: Some(Seen { aspect: 1.0, height: f32::MAX, stereo: false }) })
        }),
        _ => None,
    }
}

/// A page `w` by `h` in the middle of a square of the window's ground, as wide as its longer side:
/// `None` if that is more than a tile could ever want, or the page is not what it says.
fn squared(w: usize, h: usize, rgba: &[u8]) -> Option<(usize, Vec<u8>)> {
    let side = w.max(h);
    if side == 0 || side > 2048 || rgba.len() < w * h * 4 {
        return None;
    }
    let mut square = [11u8, 13, 18, 255].repeat(side * side);
    let (x0, y0) = ((side - w) / 2, (side - h) / 2);
    for y in 0..h {
        let to = ((y0 + y) * side + x0) * 4;
        square[to..to + w * 4].copy_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
    }
    Some((side, square))
}

struct Loupe {
    /// The file being opened now: a job for one opened before it is not started at all.
    current: Arc<AtomicU64>,
    /// The picture being read ahead, as `current` is for the one being opened.
    ahead_ticket: Arc<AtomicU64>,
    ahead: Option<Ahead>,
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
    /// Whether the window is drawn as two eyes now, and at what size it was staged.
    two: bool,
    staged: Option<(bool, (u32, u32))>,
    /// Whether the window has been told its ground is clear.
    clear: bool,
    /// The window's size as the wearer last made it: free, not held to a picture's shape.
    size: (u32, u32),
    /// What a file opened at the start is, to open once its folder is listed.
    opening: Option<PathBuf>,
    /// A locked PDF just written (or its copy), and the password it was opened with: given to it
    /// when it is opened again, and let go of then whatever is opened.
    unlocking: Option<(PathBuf, pdf::Secret)>,
    left: Vec<View>,
    /// Which of the folder's tiles the viewer's strip was last made from (`Folder::generation`).
    strip: Option<u64>,
    /// Word that the folder on show has changed, from its watcher, kept while it is on show.
    changed: (Sender<()>, Receiver<()>),
    watched: Option<notify::RecommendedWatcher>,
    change_due: bool,
    relisted: std::time::Instant,
    /// What was done, to be said by the next file's view: a save, whose file is shown afresh.
    saying: Option<String>,
}

/// Something to say when the folder at `path`, on this machine, changes: what is in it, not what
/// is in the folders in it. `None` for a remote's, or where it cannot be watched.
fn watch(path: &Path, changed: Sender<()>) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher;
    if place::remote(path).is_some() {
        return None;
    }
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok_and(|e| !matches!(e.kind, notify::EventKind::Access(_))) {
            let _ = changed.send(());
        }
    })
    .map_err(|e| log::warn!("{}: not watched: {e}", path.display()))
    .ok()?;
    watcher.watch(path, notify::RecursiveMode::NonRecursive).map_err(|e| log::warn!("{}: not watched: {e}", path.display())).ok()?;
    Some(watcher)
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
        self.folder.give_way(&mut folder);
        self.folder = folder;
        self.watched = watch(&path, self.changed.0.clone());
        let title = place::name(&path);
        self.ask_listing(ticket, path);
        self.app.set_title(self.window, &format!("{title} \u{2014} SpatiLoupe"));
        self.dirty = true;
    }

    fn ask_listing(&mut self, ticket: u64, path: PathBuf) {
        self.calls.send(Job::new(move || Some(Done::Listed { ticket, result: folder::list(&path) })).or(move |e| Done::Listed { ticket, result: Err(e) })).ok();
    }

    /// List the folder on show again, keeping what has not changed: coming back to it from a file,
    /// and when it changes. Tiles asked for under the listing before are not this one's.
    fn relist(&mut self) {
        let ticket = self.ticket();
        self.folder.ticket = ticket;
        self.relisted = std::time::Instant::now();
        self.ask_listing(ticket, self.folder.path.clone());
    }

    fn open(&mut self, index: usize) {
        let Some(entry) = self.folder.entries.get(index).cloned() else { return };
        // A picture read ahead for this, or being read: that reading, not a second.
        let ahead = self.ahead.take().filter(|a| a.index == index);
        let ticket = match &ahead {
            Some(ahead) => ahead.ticket,
            None => self.ticket(),
        };
        let files: Vec<usize> = self.folder.entries.iter().enumerate().filter(|(_, e)| e.kind != Kind::Folder).map(|(i, _)| i).collect();
        // A throw's glide outlasts the settle that brought this file in: it is let go by here too.
        let glide = self.view.as_ref().and_then(|v| v.hand_on());
        if let Some(old) = self.view.take() {
            self.left.push(old);
        }
        let mut view = View::new(entry.clone(), index, ticket, files);
        // The owner, 2026-11-19: "sometimes the list not go away, even I wait long enough".
        // Each file's view began not knowing that the room says the hands, and took the head's
        // sweep of the pointer for them until the wearer next touched something.
        view.hands_said(self.app.says_hands());
        if let Some(said) = self.saying.take() {
            view.say(said);
        }
        view.set_size(self.size);
        view.handed(glide);
        let path = entry.path.clone();
        // This before the reading ahead is forgotten, so that a reading of this ticket that has
        // not begun is wanted all the while.
        self.current.store(ticket, Ordering::Relaxed);
        self.ahead_ticket.store(0, Ordering::Relaxed);
        let current = self.current.clone();
        let still_wanted = move || current.load(Ordering::Relaxed) == ticket;
        match entry.kind {
            Kind::Picture => match ahead {
                Some(Ahead { result: Some(result), .. }) => {
                    if let Err(e) = &result {
                        log::warn!("{}: {e}", entry.path.display());
                    }
                    view.pictured(result.map_err(|e| format!("This picture would not open: {e}")));
                }
                // Still being read, and what it comes to is this view's: the ticket is the same.
                Some(_) => {}
                None => {
                    let job = Job::new(move || still_wanted().then(|| Done::Picture { ticket, result: picture::read(&path, picture::LARGEST) }));
                    self.calls.send(job.or(move |e| Done::Picture { ticket, result: Err(e) })).ok();
                }
            },
            Kind::Model => {
                let job = Job::new(move || still_wanted().then(|| Done::Model { ticket, result: model::Scene::read(&path) }));
                self.calls.send(job.or(move |e| Done::Model { ticket, result: Err(e) })).ok();
            }
            Kind::Pdf => {
                let password = self.unlocking.take().filter(|(written, _)| *written == entry.path).map(|(_, password)| password);
                view.open_pdf(password);
            }
            Kind::Folder => return,
        }
        self.app.set_title(self.window, &format!("{} \u{2014} SpatiLoupe", entry.name));
        self.view = Some(view);
        self.dirty = true;
    }

    /// Read the picture the carousel is heading for before it gets there, so that it is there,
    /// or nearly, when it arrives, rather than its tile for as long as it takes to read. One at a
    /// time, and only a picture: a document or an object is opened as it always is.
    fn read_ahead(&mut self) {
        let Some(index) = self.view.as_ref().and_then(|v| v.heading()) else { return };
        if self.ahead.as_ref().is_some_and(|a| a.index == index) {
            return;
        }
        let Some(entry) = self.folder.entries.get(index).filter(|e| e.kind == Kind::Picture) else { return };
        let path = entry.path.clone();
        let ticket = self.ticket();
        self.ahead_ticket.store(ticket, Ordering::Relaxed);
        let (current, ahead) = (self.current.clone(), self.ahead_ticket.clone());
        // Not begun if the carousel has turned for another by then; wanted still if it arrived.
        let wanted = move || current.load(Ordering::Relaxed) == ticket || ahead.load(Ordering::Relaxed) == ticket;
        let job = Job::new(move || wanted().then(|| Done::Picture { ticket, result: picture::read(&path, picture::LARGEST) }));
        self.calls.send(job.or(move |e| Done::Picture { ticket, result: Err(e) })).ok();
        self.ahead = Some(Ahead { index, ticket, result: None });
    }

    fn leave(&mut self) {
        self.ahead = None;
        self.ahead_ticket.store(0, Ordering::Relaxed);
        if let Some(view) = self.view.take() {
            self.folder.point_at(&view.path());
            self.left.push(view);
        }
        let path = self.folder.path.clone();
        let title = place::name(&path);
        self.app.set_title(self.window, &format!("{title} \u{2014} SpatiLoupe"));
        // What has come or gone while the file was looked at.
        self.relist();
        self.dirty = true;
    }

    fn answers(&mut self) {
        // The folder on show changed: listed again, unless a file is being looked at, whose place
        // among the folder's would move under it -- it is listed again on coming back.
        // At most once a second: a folder being copied into says so with every file.
        self.change_due |= self.changed.1.try_iter().count() > 0;
        if self.change_due && self.view.is_none() && matches!(self.folder.state, folder::State::Ready) && self.relisted.elapsed() > Duration::from_secs(1) {
            self.change_due = false;
            self.relist();
        }
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
                                // Not in the list -- a hidden file, or a kind it does not know by
                                // name (then tried as a picture): shown anyway, as what it is.
                                let meta = std::fs::metadata(&path).ok();
                                let size = meta.as_ref().map_or(0, |m| m.len());
                                let modified = meta.as_ref().map(place::modified).unwrap_or_default();
                                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                                let kind = folder::kind_of(&path).unwrap_or(Kind::Picture);
                                let index = self.folder.add(folder::Entry { name, path, kind, size, modified });
                                self.open(index);
                            }
                        }
                    }
                    // Given to the file written if that is what opened; let go of either way.
                    self.unlocking = None;
                }
                Done::Tile { ticket, tile } => self.folder.pictured(ticket, tile),
                Done::Picture { ticket, result } => {
                    if let Some(view) = self.view.as_mut().filter(|v| v.ticket == ticket) {
                        if let Err(e) = &result {
                            log::warn!("{}: {e}", view.entry.path.display());
                        }
                        view.pictured(result.map_err(|e| format!("This picture would not open: {e}")));
                    } else if let Some(ahead) = self.ahead.as_mut().filter(|a| a.ticket == ticket) {
                        ahead.result = Some(result);
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
                Done::Saved { ticket, result } => {
                    if let Some(view) = self.view.as_mut().filter(|v| v.ticket == ticket) {
                        view.saved(result, None);
                    }
                }
            }
        }
        if let Some(view) = self.view.as_mut() {
            self.dirty |= view.answers();
        }
        // A file written -- the one looked at, or its copy -- shown afresh as it now is: listed
        // again, the folder perhaps holding a new file, and opened as it comes.
        if let Some(view::Want::Saved { path, said, password }) = self.view.as_mut().and_then(|v| v.take_wanted()) {
            self.saying = Some(said);
            self.unlocking = password.map(|password| (path.clone(), password));
            self.opening = Some(path);
            self.relist();
            self.dirty = true;
        }
    }

    /// A marked-up picture written by a worker: it is read again whole and drawn on, which is
    /// not for the thread that draws the window.
    fn save(&mut self, saving: Box<save::Saving>) {
        let Some(ticket) = self.view.as_ref().map(|v| v.ticket) else { return };
        let job = Job::new(move || Some(Done::Saved { ticket, result: save::save(&saving) }));
        self.calls.send(job.or(move |e| Done::Saved { ticket, result: Err(e) })).ok();
    }

    fn handle(&mut self, event: Event) -> bool {
        match &event {
            // Closed, or Command-Q or Control-Q (the SDK's `Event::Quit`): SpatiLoupe goes, but
            // for markup not saved, which is asked about first, in the window.
            Event::CloseRequested { .. } | Event::Quit { .. } => {
                if let Some(view) = self.view.as_mut().filter(|v| v.unsaved()) {
                    view.ask_quit();
                    self.dirty = true;
                    return true;
                }
                return false;
            }
            Event::Ready { .. } => {
                self.ready = true;
                // A picture, a page or an object zooms when pinched; the window stays as it is.
                self.app.set_pinch_taken(self.window, true);
                // And goes both ways under a thumb: sideways to the next picture, as readily as
                // up or down (swipe.rs decides which a gesture is). Left to the room's pad, a
                // sideways swipe waited a third of the pad and went up and down a little off level.
                self.app.set_scroll_free(self.window, true);
            }
            Event::Resized { width, height, .. } => {
                self.size = ((*width).max(480), (*height).max(320));
                if let Some(view) = self.view.as_mut() {
                    view.set_size(self.size);
                }
                self.dirty = true;
                return true;
            }
            Event::Describe { window } => {
                let description = self.description();
                self.app.describe(*window, &description);
                return true;
            }
            Event::Mode(mode) => log::info!("the wearer now sees {mode:?}"),
            // Dark or light: drawn again in the theme's new colours.
            Event::Look { .. } => self.dirty = true,
            Event::Hands => {
                if let Some(view) = self.view.as_mut() {
                    view.hands();
                    self.dirty = true;
                }
                return true;
            }
            _ => {}
        }
        self.dirty = true;
        let Some(input) = Input::of(&event, 1.0) else { return true };
        match self.view.as_mut() {
            Some(view) => match view.input(&input) {
                Want::Nothing => {}
                Want::Leave => self.leave(),
                Want::Open(index) => self.open(index),
                Want::Quit => return false,
                Want::Save(saving) => self.save(saving),
                // Only from what was done between inputs (`answers`).
                Want::Saved { .. } => {}
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

    /// What the window shows, for the room's speech: the file looked at and what is read on it,
    /// or the folder; the folder's names either way, as the words likely to be said.
    fn description(&mut self) -> spatiand_app::Description {
        let names: Vec<String> = self.folder.entries.iter().map(|e| e.name.clone()).take(150).collect();
        let (title, text) = match self.view.as_mut() {
            Some(view) => view.describe(),
            None => (place::name(&self.folder.path), names.iter().take(200).cloned().collect::<Vec<_>>().join("\n")),
        };
        spatiand_app::Description {
            kind: "viewer".into(),
            app: "SpatiLoupe".into(),
            title,
            text,
            vocabulary: names,
            directory: self.folder.path.display().to_string(),
            commands: Vec::new(),
        }
    }

    /// Whether the window is to be two eyes' pictures, and if that has changed, make it so.
    fn stage(&mut self) {
        let two = self.app.mode() == Mode::Stereo && self.app.in_the_room() && self.view.as_ref().is_some_and(|v| v.has_depth());
        if self.staged == Some((two, self.size)) {
            return;
        }
        self.two = two;
        self.staged = Some((two, self.size));
        let size = self.size;
        if two {
            self.app.resize(self.window, (size.0 * 2, size.1));
            self.app.set_shape(self.window, Some(size));
            self.app.set_eyes(self.window, EyeLayout::SideBySide, false);
        } else {
            self.app.resize(self.window, size);
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
        // Tiles far from those on show let go of: a folder of ten thousand pictures is not ten
        // thousand textures.
        match self.view.as_ref() {
            Some(view) => self.folder.let_go_far(view.index.saturating_sub(14)..view.index + 15),
            None => {
                let near = self.folder.in_view();
                self.folder.let_go_far(near);
            }
        }
        for (index, path, kind) in wanted {
            let ticket = self.folder.ticket;
            // A tile that panics is left without its picture.
            self.pictures.send(Job::new(move || Some(Done::Tile { ticket, tile: tile_picture(&path, kind, index)? }))).ok();
        }
        if let Some(view) = self.view.as_mut() {
            self.dirty |= view.tick();
        }
        // The carousel has come to rest on another file: that is the one looked at now.
        if let Some(Want::Open(index)) = self.view.as_mut().map(|v| v.arrival()) {
            self.open(index);
        }
        let animating = self.view.as_ref().map_or(self.folder.animating(), |v| v.animating());
        if !std::mem::take(&mut self.dirty) && !animating {
            return Ok(());
        }
        self.stage();
        let clear = self.view.as_ref().is_some_and(|v| v.see_through());
        if self.clear != clear {
            self.clear = clear;
            self.app.set_see_through(self.window, clear);
        }
        let two = self.two;
        let (view, folder, painter, left, strip) = (&mut self.view, &mut self.folder, &mut self.painter, &mut self.left, &mut self.strip);
        self.app.draw(self.window, |gl, size| {
            let p = painter.get_or_insert_with(|| Painter::new(gl).expect("the painter"));
            for old in left.drain(..) {
                old.forget(gl);
            }
            match view.as_mut() {
                Some(view) => {
                    // The strip's tiles are the folder's: the files only, in order.
                    folder.take_arrived(p, gl);
                    // Made again only when a tile has changed, not on every frame.
                    if *strip != Some(folder.generation) {
                        *strip = Some(folder.generation);
                        view::TILES.with(|t| *t.borrow_mut() = folder.file_tiles());
                        view::SEEN.with(|s| *s.borrow_mut() = folder.file_seen());
                    }
                    view.settle_turns(gl);
                    // A picture is shown on nothing; the room is its ground.
                    let ground = if view.see_through() { [0.0, 0.0, 0.0, 0.0] } else { theme::ground() };
                    p.begin(gl, size, 1.0, ground);
                    if two {
                        let half = size.0 as f32 * 0.5;
                        view.paint(p, gl, size, Rect::new(0.0, 0.0, half, size.1 as f32), Eye::Left);
                        view.paint(p, gl, size, Rect::new(half, 0.0, half, size.1 as f32), Eye::Right);
                    } else {
                        view.paint(p, gl, size, Rect::new(0.0, 0.0, size.0 as f32, size.1 as f32), Eye::Both);
                    }
                }
                None => {
                    p.begin(gl, size, 1.0, theme::ground());
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
            self.read_ahead();
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

/// What SpatiLoupe was started on.
#[derive(Debug, PartialEq)]
enum Asked {
    Folder(PathBuf),
    File(PathBuf),
}

/// What a place named on the command line is. A place on this machine wins over a name that
/// reads as a remote's (`a:b.jpg` here), and is made whole -- from the working folder, `..` taken
/// away, but a link left a link, so that a linked file opens among its link's neighbours -- so
/// that its folder has a name and an Up. A remote's `remote:folder/` is its folder, without the
/// trailing `/` that left it with no name and an Up to itself.
fn asked_place(raw: PathBuf) -> Asked {
    let remote = place::remote(&raw);
    if raw.exists() || remote.is_none() {
        let whole = std::path::absolute(&raw).unwrap_or(raw);
        let mut path = PathBuf::new();
        for part in whole.components() {
            match part {
                std::path::Component::ParentDir => {
                    path.pop();
                }
                std::path::Component::CurDir => {}
                part => path.push(part),
            }
        }
        return if path.is_dir() { Asked::Folder(path) } else { Asked::File(path) };
    }
    let (r, p) = remote.unwrap_or_default();
    let folderish = p.is_empty() || p.ends_with('/') || folder::kind_of(&raw).is_none();
    let path = PathBuf::from(format!("{r}:{}", p.trim_end_matches('/')));
    if folderish { Asked::Folder(path) } else { Asked::File(path) }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let asked = std::env::args_os().nth(1).map(PathBuf::from).map(asked_place);
    let run = || -> Result<(), String> {
        let mut app = App::connect("id.prasetya.spatiand.spatiloupe")?;
        let window = app.open("SpatiLoupe", WINDOW);
        let (done, answers) = channel();
        let mut loupe = Loupe {
            current: Default::default(),
            ahead_ticket: Default::default(),
            ahead: None,
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
            staged: None,
            clear: false,
            size: WINDOW,
            opening: None,
            unlocking: None,
            left: Vec::new(),
            strip: None,
            changed: channel(),
            watched: None,
            change_due: false,
            relisted: std::time::Instant::now(),
            saying: None,
        };
        match asked {
            Some(Asked::Folder(path)) => loupe.list(path, None),
            Some(Asked::File(path)) => {
                let folder = place::parent(&path).unwrap_or_else(folder::home);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_that_panics_is_answered_and_the_worker_goes_on() {
        let (done, answers) = channel();
        let jobs = worker("test", done);
        jobs.send(Job::new(|| panic!("a decoder's bug")).or(|e| Done::Listed { ticket: 1, result: Err(e) })).unwrap();
        jobs.send(Job::new(|| Some(Done::Listed { ticket: 2, result: Ok(Vec::new()) }))).unwrap();
        let answer = |answers: &Receiver<Done>| match answers.recv_timeout(Duration::from_secs(5)).expect("an answer") {
            Done::Listed { ticket, result } => (ticket, result.map(|r| r.len())),
            _ => unreachable!(),
        };
        let (ticket, result) = answer(&answers);
        assert_eq!(ticket, 1);
        assert!(result.is_err_and(|e| e.contains("a decoder's bug")));
        assert_eq!(answer(&answers), (2, Ok(0)));
    }

    #[test]
    fn a_place_named_on_the_command_line_is_what_it_is() {
        // A remote's folder, with and without its trailing slash, its top, and a file in it.
        assert_eq!(asked_place("Drive:Photos/".into()), Asked::Folder("Drive:Photos".into()));
        assert_eq!(asked_place("Drive:Photos".into()), Asked::Folder("Drive:Photos".into()));
        assert_eq!(asked_place("Drive:".into()), Asked::Folder("Drive:".into()));
        assert_eq!(asked_place("Drive:Photos/a.jpg".into()), Asked::File("Drive:Photos/a.jpg".into()));
        let dir = std::env::temp_dir().join(format!("spatiloupe-asked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("trip/real")).unwrap();
        std::fs::write(dir.join("trip/real/a.jpg"), b"x").unwrap();
        // A link to a picture in another folder opens among the link's neighbours.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("trip/real/a.jpg"), dir.join("trip/b.jpg")).unwrap();
            assert_eq!(asked_place(dir.join("trip/b.jpg")), Asked::File(dir.join("trip/b.jpg")));
        }
        assert_eq!(asked_place(dir.join("trip/real/../real")), Asked::Folder(dir.join("trip/real")));
        // Relative names, and one with a colon that is a file here, from the working folder (no
        // other test reads a relative path).
        std::fs::write(dir.join("trip/x:y.jpg"), b"x").unwrap();
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.join("trip")).unwrap();
        let relative = (asked_place("x:y.jpg".into()), asked_place("real".into()), asked_place(".".into()));
        std::env::set_current_dir(cwd).unwrap();
        assert_eq!(relative, (Asked::File(dir.join("trip/x:y.jpg")), Asked::Folder(dir.join("trip/real")), Asked::Folder(dir.join("trip"))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_page_is_put_on_its_square_and_a_huge_one_is_not() {
        let (side, square) = squared(2, 4, &[255u8; 2 * 4 * 4]).unwrap();
        assert_eq!(side, 4);
        // A column of ground either side of the page.
        assert_eq!(&square[..4], &[11, 13, 18, 255]);
        assert_eq!(&square[4..8], &[255, 255, 255, 255]);
        assert_eq!(squared(10, 432_000, &[]), None);
    }
}
