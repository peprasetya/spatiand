//! SpatiShelf: Spatiand's file manager.
//!
//! Like Finder or Explorer (docs/spatiand-apps.md), with tabs, **each tab a process of its
//! own** (`spatishelf --tab`, see `tab.rs`): listing, pictures and copying happen there, so a
//! folder that hangs or a file that crashes a decoder takes one tab and no more. Files open in
//! a Spatiand application or in the platform's own. Remotes -- rclone's, held by the storage
//! service -- are places like any folder, with nothing mounted; native applications reach
//! them through the service's WebDAV.
//!
//! ```text
//! spatishelf               the home folder
//! spatishelf <place>       that folder, or a remote's (`remote:folder`)
//! spatishelf --tab         a tab, spoken to over its standard input and output
//! ```

mod look;
mod proto;
mod setup;
mod tab;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use serde_json::json;
use spatiand_app::ui::{look as room, theme, Button, Field, Input, List, Painter, Rect, Row, Texture, Typed};
use spatiand_app::{App, Button as PointerButton, Event, Key, Modifiers, WindowId};
use spatiand_storage::{Remote, Storage};

use proto::{FromTab, Item, Kind, Op, Place, ToTab};
use setup::Setup;

const WINDOW: (u32, u32) = (1440, 900);
const BAR: f32 = 66.0;
const SIDE: f32 = 330.0;
const TILE: f32 = 176.0;
const ROW: f32 = 54.0;

#[derive(Clone, Copy, PartialEq)]
enum View {
    Grid,
    List,
}

enum State {
    Loading(Instant),
    Failed(String),
    Ready,
}

/// A tab: its process, and what the window knows of what it showed.
struct ShelfTab {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    gather: proto::Gather,
    crashed: Option<String>,
    place: Place,
    back: Vec<Place>,
    forward: Vec<Place>,
    state: State,
    items: Vec<Item>,
    free: Option<u64>,
    /// The items on show, after hiding and filtering: indices into `items`.
    shown: Vec<usize>,
    pictures: HashMap<usize, Texture>,
    arrived: Vec<(usize, u32, u32, Vec<u8>)>,
    asked: HashSet<usize>,
    selected: BTreeSet<usize>,
    /// The cursor, as a place in `shown`.
    cursor: usize,
    anchor: usize,
    scroll: f32,
    filter: String,
    /// What the cursor goes to once the listing comes: a folder come out of, a file made.
    land_on: Option<String>,
    /// Things being done, by their number: what, how far.
    doing: HashMap<u64, (String, u64, u64)>,
    stale: Vec<Texture>,
}

impl ShelfTab {
    fn spawn(place: Place) -> Result<ShelfTab, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut child = Command::new(exe).arg("--tab").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().map_err(|e| format!("a tab would not start: {e}"))?;
        let stdin = child.stdin.take().ok_or("no way to talk to the tab")?;
        let stdout = child.stdout.take().ok_or("no way to hear the tab")?;
        unsafe {
            let fd = stdout.as_raw_fd();
            libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
        }
        let mut tab = ShelfTab {
            child,
            stdin,
            stdout,
            gather: proto::Gather::default(),
            crashed: None,
            place: place.clone(),
            back: Vec::new(),
            forward: Vec::new(),
            state: State::Loading(Instant::now()),
            items: Vec::new(),
            free: None,
            shown: Vec::new(),
            pictures: HashMap::new(),
            arrived: Vec::new(),
            asked: HashSet::new(),
            selected: BTreeSet::new(),
            cursor: 0,
            anchor: 0,
            scroll: 0.0,
            filter: String::new(),
            land_on: None,
            doing: HashMap::new(),
            stale: Vec::new(),
        };
        tab.tell(&ToTab::Go(place));
        Ok(tab)
    }

    fn tell(&mut self, m: &ToTab) {
        if self.crashed.is_none() {
            let _ = proto::send(&mut self.stdin, &proto::encode_to_tab(m));
        }
    }

    fn go(&mut self, place: Place, remember: bool) {
        if remember && place != self.place {
            self.back.push(self.place.clone());
            self.forward.clear();
        }
        // Going up: the cursor lands on the folder come out of.
        if self.place.parent().as_ref() == Some(&place) {
            self.land_on = Some(self.place.name());
        }
        self.place = place.clone();
        self.state = State::Loading(Instant::now());
        self.filter.clear();
        self.tell(&ToTab::Go(place));
    }

    fn refilter(&mut self, hidden: bool) {
        let words = self.filter.to_lowercase();
        self.shown = self.items.iter().enumerate().filter(|(_, i)| (hidden || !i.hidden) && (words.is_empty() || i.name.to_lowercase().contains(&words))).map(|(n, _)| n).collect();
        self.cursor = self.cursor.min(self.shown.len().saturating_sub(1));
        self.selected.retain(|s| self.shown.contains(s));
    }

    fn item_at_cursor(&self) -> Option<(usize, &Item)> {
        let index = *self.shown.get(self.cursor)?;
        Some((index, &self.items[index]))
    }

    /// What the selection is, as places; the cursor's item if nothing is selected.
    fn chosen(&self) -> Vec<Place> {
        let mut out: Vec<Place> = self.selected.iter().filter_map(|&i| self.items.get(i)).map(|i| self.place.join(&i.name)).collect();
        if out.is_empty() {
            if let Some((_, item)) = self.item_at_cursor() {
                out.push(self.place.join(&item.name));
            }
        }
        out
    }

    fn label(&self) -> String {
        if self.crashed.is_some() {
            return "Stopped".into();
        }
        match &self.place {
            Place::Local(p) if Some(PathBuf::from(p)) == home() => "Home".into(),
            place => place.name(),
        }
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The places along the side, for this kind of machine.
fn places() -> Vec<(String, Place)> {
    let mut out = Vec::new();
    if cfg!(target_os = "android") {
        for (name, path) in [("Phone", "/sdcard"), ("Camera", "/sdcard/DCIM"), ("Downloads", "/sdcard/Download"), ("Pictures", "/sdcard/Pictures"), ("Films", "/sdcard/Movies"), ("Music", "/sdcard/Music"), ("Documents", "/sdcard/Documents")] {
            if std::path::Path::new(path).is_dir() {
                out.push((name.to_string(), Place::Local(path.to_string())));
            }
        }
        return out;
    }
    let Some(home) = home() else { return vec![("Computer".into(), Place::Local("/".into()))] };
    out.push(("Home".to_string(), Place::Local(home.to_string_lossy().into_owned())));
    let films = if cfg!(target_os = "macos") { "Movies" } else { "Videos" };
    for (name, folder) in [("Desktop", "Desktop"), ("Documents", "Documents"), ("Downloads", "Downloads"), ("Pictures", "Pictures"), ("Films", films), ("Music", "Music")] {
        let path = home.join(folder);
        if path.is_dir() {
            out.push((name.to_string(), Place::Local(path.to_string_lossy().into_owned())));
        }
    }
    out.push(("Computer".to_string(), Place::Local("/".into())));
    out
}

/// Something wanted of the storage service, done on a thread.
enum Done {
    Connected(Result<Storage, String>),
    Remotes(Result<Vec<Remote>, String>),
    Providers(Result<Vec<setup::Provider>, String>),
    /// What rclone said to a creation or an answer: done, a question, or a refusal.
    Configured(Result<Option<(String, setup::Question)>, String>),
    Imported(Result<Vec<String>, String>),
}

enum Overlay {
    None,
    Menu { at: (f32, f32), list: List, entries: Vec<(String, Action)> },
    Ask { what: Asking, field: Field, ok: Button, cancel: Button },
    Confirm { text: String, action: Action, ok: Button, cancel: Button },
    Network { connect: Button, close: Button },
    Setup(Box<Setup>),
}

#[derive(Clone, PartialEq)]
enum Asking {
    Rename(Place),
    NewFolder,
}

/// What a menu or a key does.
#[derive(Clone, PartialEq, Debug)]
enum Action {
    Open,
    OpenWith(Opener),
    Copy,
    Cut,
    Paste,
    Rename,
    NewFolder,
    Trash,
    Delete,
    CopyPath,
    Refresh,
    ToggleHidden,
    Forget(String),
}

#[derive(Clone, PartialEq, Debug)]
enum Opener {
    Own(&'static str),
    Native,
}

struct Shelf {
    /// Taken out while a picture is drawn, which borrows everything else.
    app: Option<App>,
    window: WindowId,
    painter: Option<Painter>,
    tabs: Vec<ShelfTab>,
    active: usize,
    view: View,
    hidden: bool,
    ready: bool,
    dirty: bool,
    size: (u32, u32),
    pointer: (f32, f32),
    modifiers: Modifiers,
    storage: Option<Storage>,
    remotes: Vec<Remote>,
    jobs: Sender<Box<dyn FnOnce() -> Done + Send>>,
    done: Receiver<Done>,
    side: List,
    side_rows: Vec<SideRow>,
    overlay: Overlay,
    /// Files copied or cut, for pasting in another folder or tab.
    clipboard: Option<(Vec<Place>, bool)>,
    next_op: u64,
    trouble: Option<(String, Instant)>,
    note: Option<(String, Instant)>,
    last_click: Option<(Instant, usize)>,
    filter_field: Field,
    filtering: bool,
    up: Button,
    backward: Button,
    forward: Button,
    view_button: Button,
    placed_tabs: Vec<(Rect, Rect)>,
    plus: Rect,
    trail: Vec<(Rect, Place)>,
    content: Rect,
    typed: (String, Instant),
}

#[derive(Clone)]
enum SideRow {
    Heading,
    Go(Place),
    AddRemote,
    Import,
    Network,
}

fn worker() -> (Sender<Box<dyn FnOnce() -> Done + Send>>, Receiver<Done>) {
    let (jobs, queue) = channel::<Box<dyn FnOnce() -> Done + Send>>();
    let (done, answers) = channel();
    std::thread::spawn(move || {
        for job in queue {
            if done.send(job()).is_err() {
                return;
            }
        }
    });
    (jobs, answers)
}

/// A program of Spatiand's own, beside this one.
fn own_program(name: &str) -> Option<PathBuf> {
    let file = if cfg!(target_os = "android") { format!("lib{name}.so") } else { name.to_string() };
    let mut dirs = Vec::new();
    if let Some(d) = std::env::var_os("SPATIAND_APPS_DIR") {
        dirs.push(PathBuf::from(d));
    }
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        dirs.push(d);
    }
    dirs.into_iter().map(|d| d.join(&file)).find(|p| p.is_file())
}

fn detached(mut command: Command) -> Result<(), String> {
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    command.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Which Spatiand application opens a kind of file, if one does.
fn opener_for(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Picture | Kind::Pdf | Kind::Model => Some("spatiloupe"),
        Kind::Film | Kind::Music => Some("spatireel"),
        _ => None,
    }
}

impl Shelf {
    fn app(&mut self) -> &mut App {
        self.app.as_mut().expect("the app")
    }

    fn tab(&mut self) -> &mut ShelfTab {
        &mut self.tabs[self.active]
    }

    fn connect(&mut self) {
        self.jobs.send(Box::new(|| Done::Connected(Storage::connect()))).ok();
    }

    fn refresh_remotes(&mut self) {
        if let Some(storage) = self.storage.clone() {
            self.jobs.send(Box::new(move || Done::Remotes(storage.remotes()))).ok();
        }
    }

    fn rebuild_side(&mut self) {
        let mut rows = vec![(Row::heading("PLACES"), SideRow::Heading)];
        for (name, place) in places() {
            rows.push((Row::new(name), SideRow::Go(place)));
        }
        rows.push((Row::heading("REMOTES"), SideRow::Heading));
        for remote in &self.remotes {
            // The kind is said in the foot when it is open; here there is room for the name.
            rows.push((Row::new(remote.name.clone()), SideRow::Go(Place::Remote(remote.name.clone(), String::new()))));
        }
        rows.push((Row::new("+  New remote"), SideRow::AddRemote));
        if spatiand_storage::own_rclone_config().is_some() {
            rows.push((Row::new("From rclone"), SideRow::Import));
        }
        rows.push((Row::new("For other apps"), SideRow::Network));
        self.side_rows = rows.iter().map(|(_, r)| r.clone()).collect();
        SIDE_LABELS.with(|l| *l.borrow_mut() = rows.into_iter().map(|(r, _)| r).collect());
    }

    fn new_tab(&mut self, place: Place) {
        match ShelfTab::spawn(place) {
            Ok(tab) => {
                self.app().watch(tab.stdout.as_raw_fd());
                self.tabs.insert((self.active + 1).min(self.tabs.len()), tab);
                self.active = (self.active + 1).min(self.tabs.len() - 1);
            }
            Err(e) => self.fail(e),
        }
    }

    fn close_tab(&mut self, at: usize) {
        if self.tabs.len() <= 1 {
            return;
        }
        let mut tab = self.tabs.remove(at);
        self.app().unwatch(tab.stdout.as_raw_fd());
        tab.tell(&ToTab::Quit);
        let _ = tab.child.kill();
        let _ = tab.child.wait();
        self.active = self.active.min(self.tabs.len() - 1);
    }

    fn fail(&mut self, what: impl Into<String>) {
        let what = what.into();
        log::warn!("{what}");
        self.trouble = Some((what, Instant::now()));
    }

    fn say(&mut self, what: impl Into<String>) {
        self.note = Some((what.into(), Instant::now()));
    }

    fn hear(&mut self) {
        let hidden = self.hidden;
        let mut finished = Vec::new();
        for (t, tab) in self.tabs.iter_mut().enumerate() {
            if tab.crashed.is_some() {
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
            while let Some(m) = tab.gather.next() {
                match m {
                    FromTab::Listed { place, items, free } if place == tab.place => {
                        for (_, t) in tab.pictures.drain() {
                            tab.stale.push(t);
                        }
                        tab.asked.clear();
                        tab.arrived.clear();
                        tab.items = items;
                        tab.free = free;
                        tab.selected.clear();
                        tab.refilter(hidden);
                        let land = tab.land_on.take();
                        tab.cursor = land.and_then(|name| tab.shown.iter().position(|&i| tab.items[i].name == name)).unwrap_or(0);
                        tab.anchor = tab.cursor;
                        tab.scroll = f32::NAN;
                        tab.state = State::Ready;
                    }
                    FromTab::Failed { place, error } if place == tab.place => tab.state = State::Failed(error),
                    FromTab::Picture { place, index, width, height, rgba } if place == tab.place => tab.arrived.push((index, width, height, rgba)),
                    FromTab::Progress { id, done, total, what } => {
                        tab.doing.insert(id, (what, done, total));
                    }
                    FromTab::Done { id, error } => {
                        tab.doing.remove(&id);
                        finished.push((t, error));
                    }
                    _ => {}
                }
            }
            if gone {
                use std::os::unix::process::ExitStatusExt;
                let why = match tab.child.wait() {
                    Ok(s) => match s.signal() {
                        Some(sig) => format!("The tab's process was stopped by signal {sig}."),
                        None => format!("The tab's process ended unexpectedly (status {}).", s.code().unwrap_or(-1)),
                    },
                    Err(e) => e.to_string(),
                };
                tab.crashed = Some(why);
            }
        }
        for (t, error) in finished {
            if let Some(e) = error {
                self.fail(e);
            }
            // Whatever was done, every tab showing the places it touched is listed again: the
            // simplest true thing, and listing is cheap next to copying.
            let _ = t;
            for tab in &mut self.tabs {
                tab.tell(&ToTab::Refresh);
            }
        }
        self.dirty = true;
    }

    fn answers(&mut self) {
        while let Ok(done) = self.done.try_recv() {
            self.dirty = true;
            match done {
                Done::Connected(Ok(storage)) => {
                    self.storage = Some(storage);
                    self.refresh_remotes();
                }
                Done::Connected(Err(e)) => log::warn!("no storage service: {e}"),
                Done::Remotes(Ok(remotes)) => {
                    self.remotes = remotes;
                    self.rebuild_side();
                }
                Done::Remotes(Err(e)) => self.fail(e),
                Done::Providers(result) => {
                    if let Overlay::Setup(s) = &mut self.overlay {
                        match result {
                            Ok(p) => s.loaded(p),
                            Err(e) => {
                                s.trouble = Some(e);
                            }
                        }
                    }
                }
                Done::Configured(result) => {
                    let name = if let Overlay::Setup(s) = &self.overlay { s.remote_name() } else { String::new() };
                    match result {
                        Ok(None) => {
                            self.overlay = Overlay::None;
                            self.refresh_remotes();
                            self.say(format!("{name} is ready."));
                            if !name.is_empty() {
                                self.tab().go(Place::Remote(name, String::new()), true);
                            }
                        }
                        Ok(Some((state, question))) => {
                            if let Overlay::Setup(s) = &mut self.overlay {
                                s.asked(state, question);
                            }
                        }
                        Err(e) => {
                            if let Overlay::Setup(s) = &mut self.overlay {
                                s.trouble = Some(e);
                                s.step = setup::Step::Form;
                            }
                        }
                    }
                }
                Done::Imported(Ok(names)) => {
                    self.refresh_remotes();
                    if names.is_empty() {
                        self.say("Your rclone remotes are here already.");
                    } else {
                        self.say(format!("Brought in {}.", names.join(", ")));
                    }
                }
                Done::Imported(Err(e)) => self.fail(e),
            }
        }
    }

    fn act(&mut self, action: Action) {
        let hidden = self.hidden;
        match action {
            Action::Open => self.open(None),
            Action::OpenWith(opener) => self.open(Some(opener)),
            Action::Copy | Action::Cut => {
                let chosen = self.tab().chosen();
                if !chosen.is_empty() {
                    let n = chosen.len();
                    self.clipboard = Some((chosen, action == Action::Cut));
                    self.say(format!("{} {} \u{2014} paste where it is to go", if action == Action::Cut { "Cut" } else { "Copied" }, if n == 1 { "1 item".to_string() } else { format!("{n} items") }));
                }
            }
            Action::Paste => {
                let Some((from, moving)) = self.clipboard.clone() else { return };
                let to = self.tab().place.clone();
                let id = self.next_op;
                self.next_op += 1;
                let op = if moving { Op::Move { from, to } } else { Op::Copy { from, to } };
                self.tab().tell(&ToTab::Do { id, op });
                if moving {
                    self.clipboard = None;
                }
            }
            Action::Rename => {
                let Some((_, item)) = self.tab().item_at_cursor() else { return };
                let name = item.name.clone();
                let place = self.tab().place.join(&name);
                let mut field = Field::default();
                field.focused = true;
                field.set(name);
                self.overlay = Overlay::Ask { what: Asking::Rename(place), field, ok: Button::default(), cancel: Button::default() };
            }
            Action::NewFolder => {
                let mut field = Field::default();
                field.focused = true;
                field.set("New folder");
                self.overlay = Overlay::Ask { what: Asking::NewFolder, field, ok: Button::default(), cancel: Button::default() };
            }
            Action::Trash => {
                let chosen = self.tab().chosen();
                if chosen.is_empty() {
                    return;
                }
                // A remote has no trash here, and neither has a phone: those are asked about.
                let local = chosen.iter().all(|p| matches!(p, Place::Local(_))) && !cfg!(target_os = "android");
                if local {
                    let id = self.next_op;
                    self.next_op += 1;
                    self.tab().tell(&ToTab::Do { id, op: Op::Trash(chosen) });
                } else {
                    self.act(Action::Delete);
                }
            }
            Action::Delete => {
                let chosen = self.tab().chosen();
                if chosen.is_empty() {
                    return;
                }
                let what = if chosen.len() == 1 { chosen[0].name() } else { format!("{} items", chosen.len()) };
                self.overlay = Overlay::Confirm { text: format!("Delete {what} for good? There is no trash to take it back from."), action: Action::Delete, ok: Button::default(), cancel: Button::default() };
            }
            Action::CopyPath => {
                let text = self.tab().chosen().iter().map(|p| p.text()).collect::<Vec<_>>().join("\n");
                copy_text(&text);
                self.say("The path is copied.");
            }
            Action::Refresh => self.tab().tell(&ToTab::Refresh),
            Action::ToggleHidden => {
                self.hidden = !self.hidden;
                for tab in &mut self.tabs {
                    tab.refilter(!hidden);
                }
            }
            Action::Forget(name) => {
                self.overlay = Overlay::Confirm { text: format!("Forget the remote {name}? Its files are not touched; only SpatiShelf's way to them goes."), action: Action::Forget(name), ok: Button::default(), cancel: Button::default() };
            }
        }
    }

    /// Something confirmed.
    fn confirmed(&mut self, action: Action) {
        match action {
            Action::Delete => {
                let chosen = self.tab().chosen();
                let id = self.next_op;
                self.next_op += 1;
                self.tab().tell(&ToTab::Do { id, op: Op::Delete(chosen) });
            }
            Action::Forget(name) => {
                if let Some(storage) = self.storage.clone() {
                    self.jobs.send(Box::new(move || {
                        let _ = storage.rc("config/delete", json!({"name": name}));
                        Done::Remotes(storage.remotes())
                    })).ok();
                }
            }
            _ => {}
        }
    }

    fn open(&mut self, with: Option<Opener>) {
        let Some((_, item)) = self.tab().item_at_cursor() else { return };
        let item = item.clone();
        let place = self.tab().place.join(&item.name);
        if item.kind == Kind::Folder && with.is_none() {
            self.tab().go(place, true);
            return;
        }
        let opener = with.unwrap_or_else(|| match opener_for(item.kind) {
            Some(own) => Opener::Own(own),
            None => Opener::Native,
        });
        let result = match (&opener, &place) {
            (Opener::Own(program), _) => {
                let Some(path) = own_program(program) else {
                    self.fail(format!("{program} is not installed here."));
                    return;
                };
                let mut command = Command::new(path);
                match &place {
                    Place::Local(p) => {
                        command.arg(p);
                    }
                    // SpatiLoupe knows remotes itself; SpatiReel is given the stream.
                    Place::Remote(r, p) if *program == "spatireel" => {
                        if let Some(storage) = &self.storage {
                            command.arg(storage.url(r, p));
                        }
                    }
                    Place::Remote(..) => {
                        command.arg(place.text());
                    }
                }
                if *program == "spatiquill" {
                    command.env("SPATIQUILL_FOLDER", match &place {
                        Place::Local(p) => p.clone(),
                        _ => String::new(),
                    });
                }
                detached(command)
            }
            (Opener::Native, Place::Local(p)) => native_open(p),
            (Opener::Native, Place::Remote(r, p)) => match &self.storage {
                Some(storage) => native_open(&storage.url(r, p)),
                None => Err("the storage service is not running".into()),
            },
        };
        match result {
            Ok(()) => self.say(format!("Opening {}\u{2026}", item.name)),
            Err(e) => self.fail(format!("{} would not open: {e}", item.name)),
        }
    }

    fn menu(&mut self) {
        let mut entries: Vec<(String, Action)> = Vec::new();
        let item = self.tab().item_at_cursor().map(|(_, i)| i.clone());
        if let Some(item) = &item {
            entries.push(("Open".into(), Action::Open));
            if item.kind == Kind::Folder {
                if matches!(self.tab().place, Place::Local(_)) && own_program("spatiquill").is_some() {
                    entries.push(("Open in SpatiQuill".into(), Action::OpenWith(Opener::Own("spatiquill"))));
                }
            } else {
                for (name, program) in [("SpatiLoupe", "spatiloupe"), ("SpatiReel", "spatireel")] {
                    if own_program(program).is_some() && opener_for(item.kind) != Some(program) {
                        entries.push((format!("Open with {name}"), Action::OpenWith(Opener::Own(program))));
                    }
                }
                if !cfg!(target_os = "android") {
                    entries.push((format!("Open with {}", if cfg!(target_os = "macos") { "the Mac's app" } else { "another app" }), Action::OpenWith(Opener::Native)));
                }
            }
            entries.push(("Copy".into(), Action::Copy));
            entries.push(("Cut".into(), Action::Cut));
        }
        if self.clipboard.is_some() {
            entries.push(("Paste here".into(), Action::Paste));
        }
        if item.is_some() {
            entries.push(("Rename\u{2026}".into(), Action::Rename));
            let remote = matches!(self.tab().place, Place::Remote(..));
            entries.push((if remote || cfg!(target_os = "android") { "Delete\u{2026}".into() } else { "Move to the trash".into() }, Action::Trash));
            entries.push(("Copy the path".into(), Action::CopyPath));
        }
        entries.push(("New folder\u{2026}".into(), Action::NewFolder));
        entries.push((if self.hidden { "Hide hidden files" } else { "Show hidden files" }.into(), Action::ToggleHidden));
        entries.push(("Refresh".into(), Action::Refresh));
        let mut list = List::default();
        list.focused = true;
        self.overlay = Overlay::Menu { at: self.pointer, list, entries };
    }

    fn side_menu(&mut self, row: usize) {
        if let Some(SideRow::Go(Place::Remote(name, _))) = self.side_rows.get(row).cloned() {
            let mut list = List::default();
            list.focused = true;
            self.overlay = Overlay::Menu { at: self.pointer, list, entries: vec![("Open in a new tab".into(), Action::Refresh), (format!("Forget {name}\u{2026}"), Action::Forget(name))] };
        }
    }

    fn side_chosen(&mut self, row: usize) {
        match self.side_rows.get(row).cloned() {
            Some(SideRow::Go(place)) => {
                if matches!(place, Place::Remote(..)) && self.storage.is_none() {
                    self.fail("The storage service is not running.");
                    return;
                }
                self.tab().go(place, true);
            }
            Some(SideRow::AddRemote) => {
                let Some(storage) = self.storage.clone() else {
                    self.fail("The storage service is not running.");
                    return;
                };
                self.overlay = Overlay::Setup(Box::new(Setup::new()));
                self.jobs.send(Box::new(move || Done::Providers(storage.rc("config/providers", json!({})).map(|v| setup::providers(&v))))).ok();
            }
            Some(SideRow::Import) => {
                if let Some(storage) = self.storage.clone() {
                    self.jobs.send(Box::new(move || Done::Imported(storage.import()))).ok();
                }
            }
            Some(SideRow::Network) => self.overlay = Overlay::Network { connect: Button::default(), close: Button::default() },
            _ => {}
        }
    }

    fn configure(&mut self, want: setup::Want) {
        let Some(storage) = self.storage.clone() else { return };
        let result = |answer: Result<serde_json::Value, String>| -> Result<Option<(String, setup::Question)>, String> {
            let answer = answer?;
            let error = answer.get("Error").and_then(|e| e.as_str()).unwrap_or("");
            if !error.is_empty() {
                return Err(error.to_string());
            }
            let state = answer.get("State").and_then(|s| s.as_str()).unwrap_or("").to_string();
            if state.is_empty() {
                return Ok(None);
            }
            let question = answer.get("Option").map(setup::Question::from_json).unwrap_or_default();
            Ok(Some((state, question)))
        };
        match want {
            setup::Want::Nothing => {}
            setup::Want::Cancel => self.overlay = Overlay::None,
            setup::Want::Create { name, kind, answers } => {
                let parameters: serde_json::Map<String, serde_json::Value> = answers.into_iter().map(|(k, v)| (k, json!(v))).collect();
                self.jobs.send(Box::new(move || Done::Configured(result(storage.rc_waiting("config/create", json!({"name": name, "type": kind, "parameters": parameters, "opt": {"nonInteractive": true, "obscure": true}}), Duration::from_secs(600)))))).ok();
            }
            setup::Want::Answer { name, kind, state, answer } => {
                self.jobs.send(Box::new(move || Done::Configured(result(storage.rc_waiting("config/create", json!({"name": name, "type": kind, "parameters": {}, "opt": {"nonInteractive": true, "obscure": true, "continue": true, "state": state, "result": answer}}), Duration::from_secs(600)))))).ok();
            }
        }
    }

    fn handle(&mut self, event: Event) -> bool {
        self.dirty = true;
        match &event {
            Event::CloseRequested { .. } => return false,
            Event::Ready { .. } => self.ready = true,
            Event::Resized { width, height, .. } => {
                self.size = ((*width).max(800), (*height).max(500));
                let (window, size) = (self.window, self.size);
                self.app().resize(window, size);
            }
            Event::Key { modifiers, .. } => self.modifiers = *modifiers,
            Event::PointerMoved { x, y, .. } => {
                self.pointer = (*x, *y);
                setup::POINTER.with(|p| p.set((*x, *y)));
            }
            _ => {}
        }
        // The right button opens a menu for what it is over.
        if let Event::Button { button: PointerButton::Right, pressed: true, .. } = event {
            let (x, y) = self.pointer;
            if matches!(self.overlay, Overlay::None) {
                if self.content.contains(x, y) {
                    if let Some(at) = self.hit(x, y) {
                        let tab = self.tab();
                        tab.cursor = at;
                        if !tab.selected.contains(&tab.shown[at]) {
                            tab.selected.clear();
                        }
                    }
                    self.menu();
                } else if self.side.contains(x, y) {
                    if let Some(row) = side_row_at(&self.side, x, y) {
                        self.side_menu(row);
                    }
                }
            }
            return true;
        }
        let Some(input) = Input::of(&event, 1.0) else { return true };
        self.input(input);
        true
    }

    fn input(&mut self, input: Input) {
        // What floats over everything has the input first.
        match &mut self.overlay {
            Overlay::None => {}
            Overlay::Menu { at, list, entries } => {
                let rect = menu_rect(*at, entries.len(), self.size);
                let outside = matches!(input, Input::Press { pressed: true }) && !rect.contains(self.pointer.0, self.pointer.1);
                if outside || matches!(input, Input::Key { key: Key::Escape, .. }) {
                    self.overlay = Overlay::None;
                    return;
                }
                if let Some(row) = list.input(&input) {
                    let action = entries[row].1.clone();
                    let label = entries[row].0.clone();
                    let at = *at;
                    self.overlay = Overlay::None;
                    // "Open in a new tab" from a remote's own menu.
                    if label == "Open in a new tab" {
                        if let Some(SideRow::Go(place)) = side_row_at(&self.side, at_x(&at), at_y(&at)).and_then(|r| self.side_rows.get(r).cloned()) {
                            self.new_tab(place);
                        }
                        return;
                    }
                    self.act(action);
                }
                return;
            }
            Overlay::Ask { what, field, ok, cancel } => {
                if cancel.input(&input) || matches!(input, Input::Key { key: Key::Escape, .. }) {
                    self.overlay = Overlay::None;
                    return;
                }
                let entered = ok.input(&input) || matches!(field.input(&input), Some(Typed::Entered));
                if entered {
                    let name = field.text.trim().to_string();
                    let what = what.clone();
                    self.overlay = Overlay::None;
                    if name.is_empty() {
                        return;
                    }
                    let id = self.next_op;
                    self.next_op += 1;
                    let op = match what {
                        Asking::Rename(item) => Op::Rename { item, name: name.clone() },
                        Asking::NewFolder => Op::Mkdir { inside: self.tab().place.clone(), name: name.clone() },
                    };
                    self.tab().land_on = Some(name);
                    self.tab().tell(&ToTab::Do { id, op });
                }
                return;
            }
            Overlay::Confirm { action, ok, cancel, .. } => {
                if cancel.input(&input) || matches!(input, Input::Key { key: Key::Escape, .. }) {
                    self.overlay = Overlay::None;
                    return;
                }
                if ok.input(&input) || matches!(input, Input::Key { key: Key::Enter, .. }) {
                    let action = action.clone();
                    self.overlay = Overlay::None;
                    self.confirmed(action);
                }
                return;
            }
            Overlay::Network { connect, close } => {
                if close.input(&input) || matches!(input, Input::Key { key: Key::Escape, .. }) {
                    self.overlay = Overlay::None;
                    return;
                }
                if connect.input(&input) {
                    if let Some(storage) = &self.storage {
                        let (url, user, token) = storage.dav_root();
                        match connect_network_folder(&url, &user, &token) {
                            Ok(()) => self.say("Connecting the network folder\u{2026}"),
                            Err(e) => self.fail(e),
                        }
                    }
                }
                return;
            }
            Overlay::Setup(setup) => {
                let want = setup.input(&input);
                self.configure(want);
                return;
            }
        }
        // The filter field, while it is open.
        if self.filtering {
            if let Input::Key { key: Key::Escape, .. } = input {
                self.filtering = false;
                self.filter_field.set("");
                self.tab().filter.clear();
                let h = self.hidden;
                self.tab().refilter(h);
                return;
            }
            if let Some(typed) = self.filter_field.input(&input) {
                let text = self.filter_field.text.clone();
                let h = self.hidden;
                let tab = self.tab();
                tab.filter = text;
                tab.refilter(h);
                if typed == Typed::Entered {
                    self.filtering = false;
                    self.filter_field.focused = false;
                }
                return;
            }
        }
        if let Input::Key { key, text, modifiers } = &input {
            self.key(*key, text.as_deref().unwrap_or(""), *modifiers);
            return;
        }
        let (x, y) = self.pointer;
        // The bar of tabs.
        if let Input::Press { pressed: true } = input {
            if let Some(at) = self.placed_tabs.iter().position(|(r, _)| r.contains(x, y)) {
                if self.placed_tabs[at].1.inset(-6.0).contains(x, y) && self.tabs.len() > 1 {
                    self.close_tab(at);
                } else {
                    self.active = at;
                }
                return;
            }
            if self.plus.contains(x, y) {
                let place = self.tab().place.clone();
                self.new_tab(place);
                return;
            }
            if let Some((_, place)) = self.trail.iter().find(|(r, _)| r.contains(x, y)).cloned() {
                self.tab().go(place, true);
                return;
            }
        }
        if self.up.input(&input) {
            self.go_up();
            return;
        }
        if self.backward.input(&input) {
            self.history(true);
            return;
        }
        if self.forward.input(&input) {
            self.history(false);
            return;
        }
        if self.view_button.input(&input) {
            self.view = if self.view == View::Grid { View::List } else { View::Grid };
            return;
        }
        if let Some(row) = self.side.input(&input) {
            self.side_chosen(row);
            return;
        }
        match input {
            Input::Press { pressed: true } if self.content.contains(x, y) => {
                let hit = self.hit(x, y);
                let m = self.modifiers;
                let tab = &mut self.tabs[self.active];
                match hit {
                    Some(at) => {
                        let index = tab.shown[at];
                        if m.control || m.logo {
                            if !tab.selected.remove(&index) {
                                tab.selected.insert(index);
                            }
                        } else if m.shift {
                            let (a, b) = (tab.anchor.min(at), tab.anchor.max(at));
                            tab.selected = tab.shown[a..=b].iter().copied().collect();
                        } else {
                            tab.selected.clear();
                            tab.selected.insert(index);
                            tab.anchor = at;
                        }
                        tab.cursor = at;
                        // Twice quickly on the same thing: open it.
                        let double = self.last_click.is_some_and(|(t, i)| t.elapsed() < Duration::from_millis(450) && i == at);
                        self.last_click = Some((Instant::now(), at));
                        if double {
                            self.last_click = None;
                            self.open(None);
                        }
                    }
                    None => tab.selected.clear(),
                }
            }
            Input::Scroll { dy, .. } if self.content.contains(x, y) => {
                let tab = self.tab();
                tab.scroll = (tab.scroll + dy).max(0.0);
            }
            _ => {}
        }
    }

    fn go_up(&mut self) {
        if let Some(parent) = self.tab().place.parent() {
            self.tab().go(parent, true);
        }
    }

    fn history(&mut self, back: bool) {
        let tab = self.tab();
        let next = if back { tab.back.pop() } else { tab.forward.pop() };
        if let Some(place) = next {
            let here = tab.place.clone();
            if back { tab.forward.push(here) } else { tab.back.push(here) }
            tab.go(place, false);
        }
    }

    fn columns(&self) -> usize {
        match self.view {
            View::Grid => ((self.content.w - 8.0) / (TILE + theme::TILE_GAP)).floor().max(1.0) as usize,
            View::List => 1,
        }
    }

    fn key(&mut self, key: Key, text: &str, m: Modifiers) {
        let command = m.logo || m.control;
        let letter = match key {
            Key::Char(c) => Some(c.to_ascii_lowercase()),
            _ => None,
        };
        if command {
            match letter {
                Some('t') => {
                    let place = self.tab().place.clone();
                    self.new_tab(place);
                }
                Some('w') => {
                    let at = self.active;
                    self.close_tab(at);
                }
                Some('c') => self.act(Action::Copy),
                Some('x') => self.act(Action::Cut),
                Some('v') => self.act(Action::Paste),
                Some('a') => {
                    let tab = self.tab();
                    tab.selected = tab.shown.iter().copied().collect();
                }
                Some('f') => {
                    self.filtering = true;
                    self.filter_field.focused = true;
                }
                Some('n') if m.shift => self.act(Action::NewFolder),
                Some('r') => self.act(Action::Refresh),
                Some('h') if m.shift || m.logo => self.act(Action::ToggleHidden),
                Some('1') => self.view = View::Grid,
                Some('2') => self.view = View::List,
                Some('l') => {
                    let text = self.tab().place.text();
                    self.say(text);
                }
                _ => {}
            }
            match key {
                Key::Up => self.go_up(),
                Key::Down => self.act(Action::Open),
                Key::Backspace => self.act(Action::Trash),
                Key::Left | Key::Char('[') => self.history(true),
                Key::Right | Key::Char(']') => self.history(false),
                Key::Tab => self.active = (self.active + if m.shift { self.tabs.len() - 1 } else { 1 }) % self.tabs.len(),
                _ => {}
            }
            return;
        }
        if m.alt {
            match key {
                Key::Up => self.go_up(),
                Key::Left => self.history(true),
                Key::Right => self.history(false),
                _ => {}
            }
            return;
        }
        let columns = self.columns() as isize;
        let step = |shelf: &mut Shelf, by: isize| {
            let tab = shelf.tab();
            if tab.shown.is_empty() {
                return;
            }
            let to = (tab.cursor as isize + by).clamp(0, tab.shown.len() as isize - 1) as usize;
            tab.cursor = to;
            if m.shift {
                let (a, b) = (tab.anchor.min(to), tab.anchor.max(to));
                tab.selected = tab.shown[a..=b].iter().copied().collect();
            } else {
                tab.selected.clear();
                tab.anchor = to;
            }
        };
        match key {
            Key::Left if self.view == View::Grid => step(self, -1),
            Key::Right if self.view == View::Grid => step(self, 1),
            Key::Up => step(self, -columns),
            Key::Down => step(self, columns),
            Key::PageUp => step(self, -columns * 4),
            Key::PageDown => step(self, columns * 4),
            Key::Home => step(self, -100000),
            Key::End => step(self, 100000),
            Key::Enter => self.act(Action::Open),
            Key::Backspace => self.go_up(),
            Key::Delete => self.act(Action::Trash),
            Key::Escape => {
                self.tab().selected.clear();
            }
            Key::Space => {
                // Space marks the thing at the cursor: choosing several without a modifier,
                // which a glasses wearer with a pad has not always got.
                let tab = self.tab();
                if let Some(&index) = tab.shown.get(tab.cursor) {
                    if !tab.selected.remove(&index) {
                        tab.selected.insert(index);
                    }
                }
            }
            Key::Other if self_code_is_f2(text) => self.act(Action::Rename),
            Key::Char(_) if !text.is_empty() && !text.chars().any(char::is_control) => {
                // Typing a name goes to it.
                let (typed, at) = &mut self.typed;
                if at.elapsed() > Duration::from_millis(900) {
                    typed.clear();
                }
                typed.push_str(text);
                *at = Instant::now();
                let words = typed.to_lowercase();
                let tab = &mut self.tabs[self.active];
                if let Some(found) = tab.shown.iter().position(|&i| tab.items[i].name.to_lowercase().starts_with(&words)) {
                    tab.cursor = found;
                    tab.selected.clear();
                    tab.anchor = found;
                }
            }
            _ => {}
        }
    }

    /// Which item, by its place in `shown`, is at a point of the content.
    fn hit(&self, x: f32, y: f32) -> Option<usize> {
        let tab = &self.tabs[self.active];
        let c = self.content;
        let at = match self.view {
            View::Grid => {
                let columns = self.columns();
                let cell_h = TILE + 64.0;
                let col = ((x - c.x - 4.0) / (TILE + theme::TILE_GAP)).floor();
                let row = ((y - c.y + tab.scroll) / cell_h).floor();
                if col < 0.0 || row < 0.0 || col as usize >= columns {
                    return None;
                }
                // Between tiles is not a tile.
                if (x - c.x - 4.0) - col * (TILE + theme::TILE_GAP) > TILE {
                    return None;
                }
                row as usize * columns + col as usize
            }
            View::List => {
                let row = ((y - c.y - 40.0 + tab.scroll) / ROW).floor();
                if row < 0.0 {
                    return None;
                }
                row as usize
            }
        };
        (at < tab.shown.len()).then_some(at)
    }

    fn draw(&mut self) -> Result<(), String> {
        if !self.ready {
            return Ok(());
        }
        let animating = self.tabs.iter().any(|t| matches!(t.state, State::Loading(_)) || !t.doing.is_empty()) || matches!(self.overlay, Overlay::Setup(_));
        if !std::mem::take(&mut self.dirty) && !animating {
            return Ok(());
        }
        let size = self.size;
        let title = format!("{} \u{2014} SpatiShelf", self.tabs[self.active].label());
        let window = self.window;
        self.app().set_title(window, &title);
        // Pictures for what is on show, asked for once each.
        let (first, last) = self.visible_range();
        let tab = &mut self.tabs[self.active];
        let wanted: Vec<usize> = tab.shown.get(first..last.min(tab.shown.len())).unwrap_or(&[]).iter().copied().filter(|i| tab.items[*i].kind.pictured() && !tab.asked.contains(i)).collect();
        if !wanted.is_empty() {
            tab.asked.extend(wanted.iter().copied());
            tab.tell(&ToTab::Pictures { indices: wanted, size: 320 });
        }
        let mut app = self.app.take().expect("the app");
        let mut painter = self.painter.take();
        let window = self.window;
        let result = app.draw(window, |gl, picture| {
            let p = painter.get_or_insert_with(|| Painter::new(gl).expect("the painter"));
            p.begin(gl, picture, picture.1 as f32 / size.1 as f32, theme::GROUND);
            room::backdrop(p, gl);
            self.paint(p, gl);
            p.end(gl);
        });
        self.app = Some(app);
        self.painter = painter;
        result.map(|_| ())
    }

    fn visible_range(&self) -> (usize, usize) {
        let tab = &self.tabs[self.active];
        let c = self.content;
        match self.view {
            View::Grid => {
                let cols = self.columns();
                let cell_h = TILE + 64.0;
                let first_row = (tab.scroll.max(0.0) / cell_h).floor() as usize;
                let rows = (c.h / cell_h).ceil() as usize + 1;
                (first_row * cols, (first_row + rows) * cols)
            }
            View::List => {
                let first = (tab.scroll.max(0.0) / ROW).floor() as usize;
                (first, first + (c.h / ROW).ceil() as usize + 1)
            }
        }
    }

    fn paint(&mut self, p: &mut Painter, gl: &spatiand_app::Gl) {
        let all = p.bounds();
        self.paint_bar(p, gl, all);
        // The side.
        let side = Rect::new(all.x + 14.0, BAR + 6.0, SIDE - 20.0, all.h - BAR - 20.0);
        room::glass(p, gl, side, 22.0, 0.75);
        let rows = SIDE_LABELS.with(|l| l.borrow().clone());
        // The place being looked at is the one marked, when it is one of these.
        let here = self.tabs[self.active].place.clone();
        if let Some(at) = self.side_rows.iter().position(|r| matches!(r, SideRow::Go(p) if *p == here)) {
            if !self.side.focused {
                self.side.cursor = at;
            }
        }
        self.side.draw(p, gl, side.inset(14.0), &rows);
        // The main pane.
        let mut main = Rect::new(SIDE + 8.0, BAR + 6.0, all.w - SIDE - 22.0, all.h - BAR - 20.0);
        let mut head = main.take_top(Button::HEIGHT);
        for (button, label) in [(&mut self.backward, "\u{2039}"), (&mut self.forward, "\u{203a}"), (&mut self.up, "\u{2191}")] {
            button.draw(p, gl, head.take_left(Button::HEIGHT), label);
            head.take_left(8.0);
        }
        head.take_left(10.0);
        let view_label = if self.view == View::Grid { "List" } else { "Tiles" };
        let vw = Button::width(p, gl, view_label);
        let view_rect = Rect { x: head.right() - vw, w: vw, ..head };
        head.w -= vw + 12.0;
        self.view_button.draw(p, gl, view_rect, view_label);
        if self.filtering || !self.filter_field.text.is_empty() {
            let fw = 340.0f32.min(head.w * 0.4);
            self.filter_field.hint = "Find in this folder".into();
            self.filter_field.draw(p, gl, Rect { x: head.right() - fw, w: fw, ..head });
            head.w -= fw + 12.0;
        }
        // Where this is: each step a place to go back to.
        self.trail.clear();
        let trail = self.tabs[self.active].place.trail();
        let mut x = head.x;
        let em = theme::ROW_EM * 0.8;
        let shown_from = trail.len().saturating_sub(5);
        for (i, (name, place)) in trail.iter().enumerate().skip(shown_from) {
            let last = i + 1 == trail.len();
            let label = if i == shown_from && shown_from > 0 { format!("\u{2026}  {name}") } else { name.clone() };
            let w = p.measure(gl, &label, em, 8192.0).0.min(head.right() - x - 10.0).max(0.0);
            if w <= 0.0 {
                break;
            }
            let rect = Rect::new(x, head.y, w + 16.0, head.h);
            if !last && rect.contains(self.pointer.0, self.pointer.1) {
                p.fill(gl, rect, theme::ROW_RADIUS, theme::HOVER);
            }
            p.label(gl, &label, rect.inset(8.0), em, if last { theme::INK } else { theme::INK_ROW }, 0.0);
            self.trail.push((rect, place.clone()));
            x += rect.w;
            if !last {
                p.label(gl, "\u{203a}", Rect::new(x, head.y, 20.0, head.h), em, theme::INK_HINT, 0.5);
                x += 20.0;
            }
        }
        main.take_top(10.0);
        room::rule(p, gl, main.take_top(2.0));
        main.take_top(12.0);
        let foot = main.take_bottom(theme::FOOTER_EM * 1.9);
        self.content = main;
        self.paint_content(p, gl, main);
        self.paint_foot(p, gl, foot);
        self.paint_overlay(p, gl, all);
    }

    fn paint_bar(&mut self, p: &mut Painter, gl: &spatiand_app::Gl, all: Rect) {
        let bar = Rect::new(all.x + 12.0, all.y + 10.0, all.w - 24.0, BAR - 16.0);
        let n = self.tabs.len().max(1);
        let width = ((bar.w - 60.0) / n as f32).clamp(150.0, 320.0);
        self.placed_tabs.clear();
        let mut x = bar.x;
        for (i, tab) in self.tabs.iter().enumerate() {
            let rect = Rect::new(x, bar.y, width - 8.0, bar.h);
            x += width;
            let active = i == self.active;
            let hover = rect.contains(self.pointer.0, self.pointer.1);
            if active {
                p.glow(gl, rect, 14.0, [0.30, 0.62, 1.0, 0.14], 16.0);
                p.gradient(gl, rect, 14.0, [1.0, 1.0, 1.0, 0.11], [1.0, 1.0, 1.0, 0.05]);
                room::rule(p, gl, Rect::new(rect.x + 16.0, rect.bottom() - 3.0, rect.w - 32.0, 2.5));
            } else if hover {
                p.fill(gl, rect, 14.0, theme::HOVER);
            }
            let dot = if tab.crashed.is_some() {
                room::TROUBLE
            } else if !tab.doing.is_empty() {
                [1.0, 0.85, 0.4, 1.0]
            } else if matches!(tab.place, Place::Remote(..)) {
                room::VIOLET
            } else if active {
                room::TEAL
            } else {
                theme::INK_HINT
            };
            p.fill(gl, Rect::new(rect.x + 16.0, rect.y + rect.h * 0.5 - 4.0, 8.0, 8.0), 4.0, dot);
            let close = Rect::new(rect.right() - 38.0, rect.y + rect.h * 0.5 - 13.0, 26.0, 26.0);
            let show_close = (active || hover) && self.tabs.len() > 1;
            let label = Rect { x: rect.x + 34.0, w: rect.w - 34.0 - if show_close { 44.0 } else { 14.0 }, ..rect };
            p.label(gl, &tab.label(), label, theme::ROW_EM * 0.72, if active { theme::INK } else { theme::INK_ROW }, 0.0);
            if show_close {
                if close.inset(-6.0).contains(self.pointer.0, self.pointer.1) {
                    p.fill(gl, close, 13.0, [1.0, 1.0, 1.0, 0.14]);
                }
                p.label(gl, "\u{2715}", close, theme::FOOTER_EM * 0.8, theme::INK_DETAIL, 0.5);
            }
            self.placed_tabs.push((rect, close));
        }
        self.plus = Rect::new(x, bar.y + (bar.h - 44.0) * 0.5, 44.0, 44.0);
        if self.plus.contains(self.pointer.0, self.pointer.1) {
            p.fill(gl, self.plus, 22.0, theme::HOVER);
        }
        p.label(gl, "+", self.plus, theme::ROW_EM, theme::INK_DETAIL, 0.5);
    }

    fn paint_content(&mut self, p: &mut Painter, gl: &spatiand_app::Gl, area: Rect) {
        let view = self.view;
        let columns = self.columns();
        let pointer = self.pointer;
        let tab = &mut self.tabs[self.active];
        for t in tab.stale.drain(..) {
            p.forget(gl, t);
        }
        for (index, w, h, rgba) in tab.arrived.drain(..) {
            if let Ok(texture) = p.texture(gl, &rgba, w, h) {
                if let Some(old) = tab.pictures.insert(index, texture) {
                    p.forget(gl, old);
                }
            }
        }
        if let Some(why) = &tab.crashed {
            let pane = Rect::new(area.x + area.w * 0.5 - 340.0, area.y + area.h * 0.5 - 100.0, 680.0, 200.0);
            room::glass(p, gl, pane, 24.0, 1.0);
            let mut inner = pane.inset(28.0);
            p.label(gl, "This tab stopped", inner.take_top(50.0), theme::TITLE_EM * 0.8, room::TROUBLE, 0.0);
            p.label(gl, why, inner.take_top(40.0), theme::DETAIL_EM, theme::INK_DETAIL, 0.0);
            p.label(gl, "Close it and open another: the other tabs are as they were.", inner.take_top(40.0), theme::FOOTER_EM, theme::INK_HINT, 0.0);
            return;
        }
        match &tab.state {
            State::Loading(since) => {
                room::waiting(p, gl, area, *since);
                return;
            }
            State::Failed(why) => {
                room::message(p, gl, area, why, room::TROUBLE);
                return;
            }
            State::Ready if tab.shown.is_empty() => {
                let text = if !tab.filter.is_empty() { "Nothing here by that name." } else if tab.items.is_empty() { "This folder is empty." } else { "Only hidden files here." };
                room::message(p, gl, area, text, theme::INK_DETAIL);
                return;
            }
            State::Ready => {}
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let (cell_h, header) = match view {
            View::Grid => (TILE + 64.0, 0.0),
            View::List => (ROW, 40.0),
        };
        let rows_total = tab.shown.len().div_ceil(columns);
        let content_h = rows_total as f32 * cell_h + header;
        // Keep the cursor on show; on first showing, start at the top.
        let cursor_row = (tab.cursor / columns) as f32;
        if tab.scroll.is_nan() {
            tab.scroll = (cursor_row * cell_h - area.h * 0.4).max(0.0);
        }
        let top = cursor_row * cell_h;
        if top < tab.scroll {
            tab.scroll = top;
        }
        if top + cell_h + header > tab.scroll + area.h {
            tab.scroll = top + cell_h + header - area.h;
        }
        tab.scroll = tab.scroll.clamp(0.0, (content_h - area.h).max(0.0));
        p.clip(gl, area);
        if view == View::List {
            let h = Rect::new(area.x, area.y, area.w, header);
            let cols = list_columns(h);
            p.label(gl, "NAME", cols[0], theme::FOOTER_EM * 0.82, theme::INK_HINT, 0.0);
            p.label(gl, "KIND", cols[1], theme::FOOTER_EM * 0.82, theme::INK_HINT, 0.0);
            p.label(gl, "SIZE", cols[2], theme::FOOTER_EM * 0.82, theme::INK_HINT, 1.0);
            p.label(gl, "MODIFIED", cols[3], theme::FOOTER_EM * 0.82, theme::INK_HINT, 1.0);
        }
        for (at, &index) in tab.shown.iter().enumerate() {
            let (row, col) = (at / columns, at % columns);
            let y = area.y + header + row as f32 * cell_h - tab.scroll;
            if y + cell_h < area.y || y > area.bottom() {
                continue;
            }
            let item = &tab.items[index];
            let selected = tab.selected.contains(&index);
            let is_cursor = at == tab.cursor;
            let picture = tab.pictures.get(&index).copied();
            match view {
                View::Grid => {
                    let cell = Rect::new(area.x + 4.0 + col as f32 * (TILE + theme::TILE_GAP), y + 8.0, TILE, cell_h - 8.0);
                    let face = Rect { h: TILE, ..cell };
                    let hover = cell.contains(pointer.0, pointer.1);
                    if selected {
                        p.glow(gl, face, 16.0, [0.30, 0.62, 1.0, 0.40], 18.0);
                        p.fill(gl, face.inset(-4.0), 18.0, theme::MARK);
                    } else if is_cursor {
                        p.fill(gl, face.inset(-3.0), 17.0, [1.0, 1.0, 1.0, 0.30]);
                    } else if hover {
                        p.fill(gl, face.inset(-3.0), 17.0, [1.0, 1.0, 1.0, 0.14]);
                    }
                    look::face(p, gl, face, item.kind, picture, 14.0);
                    let name = Rect::new(cell.x + 2.0, face.bottom() + 8.0, cell.w - 4.0, 28.0);
                    p.label(gl, &item.name, name, theme::ROW_EM * 0.62, if selected || is_cursor { theme::INK } else { theme::INK_ROW }, 0.5);
                    let detail = if item.kind == Kind::Folder { String::new() } else { look::size(item.size) };
                    p.label(gl, &detail, Rect { y: name.bottom(), h: 24.0, ..name }, theme::FOOTER_EM * 0.8, theme::INK_HINT, 0.5);
                }
                View::List => {
                    let rect = Rect::new(area.x, y, area.w, ROW - 4.0);
                    let hover = rect.contains(pointer.0, pointer.1);
                    if selected {
                        p.fill(gl, rect, theme::ROW_RADIUS, theme::SELECTED);
                    } else if is_cursor {
                        p.fill(gl, rect, theme::ROW_RADIUS, theme::SELECTED_AWAY);
                    } else if hover {
                        p.fill(gl, rect, theme::ROW_RADIUS, theme::HOVER);
                    }
                    let cols = list_columns(rect);
                    let icon = Rect::new(cols[0].x, rect.y + 7.0, rect.h - 14.0, rect.h - 14.0);
                    look::face(p, gl, icon, item.kind, picture, 8.0);
                    let name = Rect { x: icon.right() + 14.0, w: cols[0].w - icon.w - 14.0, ..cols[0] };
                    p.label(gl, &item.name, name, theme::ROW_EM * 0.7, if selected || is_cursor { theme::INK } else { theme::INK_ROW }, 0.0);
                    p.label(gl, item.kind.word(), cols[1], theme::DETAIL_EM * 0.9, theme::INK_HINT, 0.0);
                    if item.kind != Kind::Folder {
                        p.label(gl, &look::size(item.size), cols[2], theme::DETAIL_EM * 0.9, theme::INK_DETAIL, 1.0);
                    }
                    p.label(gl, &look::when(item.modified, now), cols[3], theme::DETAIL_EM * 0.9, theme::INK_DETAIL, 1.0);
                }
            }
        }
        p.unclip(gl);
        if content_h > area.h {
            let track = Rect::new(area.right() - 5.0, area.y + header, 5.0, area.h - header);
            p.fill(gl, track, 2.5, theme::SCROLL_TRACK);
            let h = (track.h * area.h / content_h).max(30.0);
            let y = track.y + (track.h - h) * (tab.scroll / (content_h - area.h).max(1.0));
            p.fill(gl, Rect { y, h, ..track }, 2.5, theme::SCROLL_THUMB);
        }
    }

    fn paint_foot(&mut self, p: &mut Painter, gl: &spatiand_app::Gl, foot: Rect) {
        let tab = &self.tabs[self.active];
        // Something being done, as a bar.
        if let Some((what, done, total)) = tab.doing.values().next() {
            let bar = Rect::new(foot.right() - 360.0, foot.y + foot.h * 0.5 - 4.0, 360.0, 8.0);
            p.fill(gl, bar, 4.0, theme::SCROLL_TRACK);
            if *total > 0 {
                p.fill(gl, Rect { w: bar.w * (*done as f32 / *total as f32).min(1.0), ..bar }, 4.0, room::TEAL);
            }
            let words = if *total > 0 { format!("{what}  {} of {}", look::size(*done), look::size(*total)) } else { what.clone() };
            look::status(p, gl, Rect { w: foot.w - 380.0, ..foot }, &words, false);
            return;
        }
        if let Some((what, at)) = &self.trouble {
            if at.elapsed() < Duration::from_secs(8) {
                look::status(p, gl, foot, what, true);
                return;
            }
        }
        if let Some((what, at)) = &self.note {
            if at.elapsed() < Duration::from_secs(5) {
                look::status(p, gl, foot, what, false);
                return;
            }
        }
        let count = tab.shown.len();
        let mut words = if count == 1 { "1 item".to_string() } else { format!("{count} items") };
        if !tab.selected.is_empty() {
            let size: u64 = tab.selected.iter().filter_map(|&i| tab.items.get(i)).filter(|i| i.kind != Kind::Folder).map(|i| i.size).sum();
            words += &format!(", {} chosen ({})", tab.selected.len(), look::size(size));
        }
        if let Some(free) = tab.free {
            words += &format!("    {} free", look::size(free));
        }
        if let Place::Remote(r, _) = &tab.place {
            let kind = self.remotes.iter().find(|x| x.name == *r).map(|x| kind_word(&x.kind)).unwrap_or_default();
            words += &format!("    {r}{}  \u{b7}  through rclone", if kind.is_empty() { String::new() } else { format!("  \u{b7}  {kind}") });
        }
        look::status(p, gl, foot, &words, false);
    }

    fn paint_overlay(&mut self, p: &mut Painter, gl: &spatiand_app::Gl, all: Rect) {
        let size = self.size;
        let storage = self.storage.clone();
        match &mut self.overlay {
            Overlay::None => {}
            Overlay::Menu { at, list, entries } => {
                let rect = menu_rect(*at, entries.len(), size);
                room::glass(p, gl, rect, 20.0, 1.0);
                let rows: Vec<Row> = entries.iter().map(|(l, _)| Row::new(l.clone())).collect();
                list.draw(p, gl, rect.inset(10.0), &rows);
            }
            Overlay::Ask { what, field, ok, cancel } => {
                let pane = Rect::new(all.w * 0.5 - 380.0, all.h * 0.32, 760.0, 250.0);
                p.fill(gl, all, 0.0, [0.0, 0.0, 0.0, 0.35]);
                room::glass(p, gl, pane, 24.0, 1.0);
                let mut inner = pane.inset(28.0);
                let title = match what {
                    Asking::Rename(place) => format!("Rename {}", place.name()),
                    Asking::NewFolder => "A new folder".to_string(),
                };
                p.label(gl, &title, inner.take_top(48.0), theme::TITLE_EM * 0.75, theme::INK, 0.0);
                inner.take_top(10.0);
                field.draw(p, gl, inner.take_top(Field::HEIGHT));
                let mut buttons = inner.take_bottom(Button::HEIGHT);
                let w = 200.0;
                ok.draw(p, gl, Rect { x: buttons.right() - w, w, ..buttons }, if matches!(what, Asking::NewFolder) { "Make it" } else { "Rename" });
                buttons.w -= w + 12.0;
                cancel.draw(p, gl, Rect { x: buttons.right() - w, w, ..buttons }, "Cancel");
            }
            Overlay::Confirm { text, ok, cancel, .. } => {
                let pane = Rect::new(all.w * 0.5 - 400.0, all.h * 0.32, 800.0, 240.0);
                p.fill(gl, all, 0.0, [0.0, 0.0, 0.0, 0.35]);
                room::glass(p, gl, pane, 24.0, 1.0);
                let mut inner = pane.inset(28.0);
                p.text(gl, text, inner.x, inner.y, theme::ROW_EM * 0.8, inner.w, theme::INK);
                let mut buttons = inner.take_bottom(Button::HEIGHT);
                let w = 200.0;
                ok.draw(p, gl, Rect { x: buttons.right() - w, w, ..buttons }, "Yes");
                buttons.w -= w + 12.0;
                cancel.draw(p, gl, Rect { x: buttons.right() - w, w, ..buttons }, "No");
            }
            Overlay::Network { connect, close } => {
                let pane = Rect::new(all.w * 0.5 - 460.0, all.h * 0.22, 920.0, 440.0);
                p.fill(gl, all, 0.0, [0.0, 0.0, 0.0, 0.35]);
                room::glass(p, gl, pane, 24.0, 1.0);
                let mut inner = pane.inset(30.0);
                p.label(gl, "Your remotes, for other apps", inner.take_top(50.0), theme::TITLE_EM * 0.75, theme::INK, 0.0);
                let (_, h) = p.text(gl, "Spatiand's apps reach your remotes by themselves. Other apps reach them as a network folder (WebDAV) on this machine, which only this machine can open:", inner.x, inner.y, theme::DETAIL_EM, inner.w, theme::INK_DETAIL);
                inner.take_top(h + 18.0);
                match &storage {
                    Some(storage) => {
                        let (url, user, token) = storage.dav_root();
                        for (what, value) in [("Address", url), ("Name", user), ("Password", token)] {
                            let line = inner.take_top(40.0);
                            p.label(gl, &what.to_uppercase(), Rect { w: 180.0, ..line }, theme::FOOTER_EM * 0.82, theme::INK_HINT, 0.0);
                            p.label(gl, &value, Rect { x: line.x + 180.0, w: line.w - 180.0, ..line }, theme::ROW_EM * 0.72, theme::INK, 0.0);
                        }
                    }
                    None => {
                        p.label(gl, "The storage service is not running.", inner.take_top(40.0), theme::DETAIL_EM, room::TROUBLE, 0.0);
                    }
                }
                let mut buttons = inner.take_bottom(Button::HEIGHT);
                let w = 300.0;
                if cfg!(target_os = "macos") || cfg!(target_os = "linux") {
                    connect.draw(p, gl, Rect { x: buttons.right() - w, w, ..buttons }, if cfg!(target_os = "macos") { "Connect in Finder" } else { "Open in the file manager" });
                    buttons.w -= w + 12.0;
                }
                close.draw(p, gl, Rect { x: buttons.right() - 180.0, w: 180.0, ..buttons }, "Done");
            }
            Overlay::Setup(setup) => {
                let pane = Rect::new(SIDE + 8.0, BAR + 6.0, all.w - SIDE - 22.0, all.h - BAR - 20.0);
                p.fill(gl, pane.inset(-6.0), 22.0, [0.043, 0.05, 0.072, 0.96]);
                room::glass(p, gl, pane, 22.0, 0.6);
                setup.draw(p, gl, pane.inset(24.0));
            }
        }
    }

    fn run(&mut self) -> Result<(), String> {
        loop {
            let animating = self.tabs.iter().any(|t| matches!(t.state, State::Loading(_)) || !t.doing.is_empty());
            let wait = if animating { 33 } else { 250 };
            for event in self.app().wait(Some(Duration::from_millis(wait)))? {
                if !self.handle(event) {
                    return Ok(());
                }
            }
            self.hear();
            self.answers();
            self.draw()?;
        }
    }
}

fn at_x(at: &(f32, f32)) -> f32 {
    at.0
}
fn at_y(at: &(f32, f32)) -> f32 {
    at.1
}

fn self_code_is_f2(_text: &str) -> bool {
    false
}

fn side_row_at(list: &List, x: f32, y: f32) -> Option<usize> {
    // The list says which row it chose only on a click: for a right-click, the row is found
    // the same way it draws them.
    if !list.contains(x, y) {
        return None;
    }
    let first = SIDE_FIRST.with(|f| f.get());
    let per = theme::ROW_HEIGHT + theme::ROW_GAP;
    let top = BAR + 6.0 + 14.0;
    Some(first + ((y - top) / per).floor().max(0.0) as usize)
}

fn list_columns(rect: Rect) -> [Rect; 4] {
    let (kind_w, size_w, date_w) = (170.0, 140.0, 200.0);
    let name_w = rect.w - kind_w - size_w - date_w - 40.0;
    let mut x = rect.x + 10.0;
    let mut out = [Rect::default(); 4];
    for (i, w) in [name_w, kind_w, size_w, date_w].into_iter().enumerate() {
        out[i] = Rect::new(x, rect.y, w, rect.h);
        x += w + 10.0;
    }
    out
}

fn menu_rect(at: (f32, f32), rows: usize, size: (u32, u32)) -> Rect {
    let h = rows as f32 * (theme::ROW_HEIGHT + theme::ROW_GAP) + 16.0;
    let w = 420.0;
    let x = at.0.min(size.0 as f32 - w - 12.0).max(12.0);
    let y = at.1.min(size.1 as f32 - h - 12.0).max(12.0);
    Rect::new(x, y, w, h)
}

fn kind_word(kind: &str) -> String {
    match kind {
        "drive" => "Google Drive",
        "dropbox" => "Dropbox",
        "onedrive" => "OneDrive",
        "webdav" => "WebDAV",
        "sftp" => "SFTP",
        "smb" => "SMB",
        "s3" => "S3",
        "ftp" => "FTP",
        "alias" => "Folder",
        "local" => "Disk",
        "iclouddrive" => "iCloud",
        "protondrive" => "Proton",
        "box" => "Box",
        "pcloud" => "pCloud",
        "mega" => "MEGA",
        "b2" => "B2",
        other => other,
    }
    .to_string()
}

fn copy_text(text: &str) {
    let program = if cfg!(target_os = "macos") { "pbcopy" } else { "wl-copy" };
    if let Ok(mut child) = Command::new(program).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = std::io::Write::write_all(&mut stdin, text.as_bytes());
        }
        let _ = child.wait();
    }
}

/// Open with whatever this machine opens it with.
fn native_open(what: &str) -> Result<(), String> {
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(target_os = "android") {
        return Err("opening in Android's own apps is not possible from here yet".into());
    } else {
        Command::new("xdg-open")
    };
    command.arg(what);
    detached(command)
}

/// Put the remotes' network folder in the native file manager.
fn connect_network_folder(url: &str, user: &str, token: &str) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        let script = format!("tell application \"Finder\" to mount volume \"{url}\" as user name \"{user}\" with password \"{token}\"");
        let mut command = Command::new("osascript");
        command.args(["-e", &script]);
        return detached(command);
    }
    let address = url.replacen("http://", &format!("webdav://{user}:{token}@"), 1);
    native_open(&address)
}

thread_local! {
    static SIDE_LABELS: std::cell::RefCell<Vec<Row>> = const { std::cell::RefCell::new(Vec::new()) };
    static SIDE_FIRST: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--tab") {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
        if let Err(e) = tab::run() {
            eprintln!("spatishelf tab: {e}");
            std::process::exit(1);
        }
        return;
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let asked = std::env::args().nth(1);
    let run = || -> Result<(), String> {
        let mut app = App::connect("id.prasetya.spatiand.spatishelf")?;
        let window = app.open("SpatiShelf", WINDOW);
        let (jobs, done) = worker();
        let start = match asked.as_deref() {
            Some(text) => match spatiand_storage::virtual_place(text) {
                Some((remote, path)) => Place::Remote(remote, path),
                None => Place::Local(std::fs::canonicalize(text).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| text.to_string())),
            },
            None => Place::Local(home().map(|h| h.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into())),
        };
        let mut side = List::default();
        side.focused = false;
        let mut shelf = Shelf {
            app: Some(app),
            window,
            painter: None,
            tabs: Vec::new(),
            active: 0,
            view: View::Grid,
            hidden: false,
            ready: false,
            dirty: true,
            size: WINDOW,
            pointer: (0.0, 0.0),
            modifiers: Modifiers::default(),
            storage: None,
            remotes: Vec::new(),
            jobs,
            done,
            side,
            side_rows: Vec::new(),
            overlay: Overlay::None,
            clipboard: None,
            next_op: 1,
            trouble: None,
            note: None,
            last_click: None,
            filter_field: Field::default(),
            filtering: false,
            up: Button::default(),
            backward: Button::default(),
            forward: Button::default(),
            view_button: Button::default(),
            placed_tabs: Vec::new(),
            plus: Rect::default(),
            trail: Vec::new(),
            content: Rect::default(),
            typed: (String::new(), Instant::now()),
        };
        shelf.rebuild_side();
        let tab = ShelfTab::spawn(start)?;
        shelf.app().watch(tab.stdout.as_raw_fd());
        shelf.tabs.push(tab);
        shelf.connect();
        let ended = shelf.run();
        for tab in &mut shelf.tabs {
            tab.tell(&ToTab::Quit);
            let _ = tab.child.kill();
            let _ = tab.child.wait();
        }
        ended
    };
    if let Err(e) = run() {
        log::error!("{e}");
        std::process::exit(1);
    }
}
