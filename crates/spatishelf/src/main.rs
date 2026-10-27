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

mod hand;
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
use spatiand_app::ui::{draw_icon, look as room, theme, Button, Field, Icon, Input, List, Painter, Rect, Row, Texture, Typed};
use spatiand_app::{App, Button as PointerButton, Event, Key, Modifiers, WindowId};
use spatiand_storage::{Remote, Storage};

use proto::{FromTab, Item, Kind, Op, Place, ToTab};
use setup::Setup;

const WINDOW: (u32, u32) = (1440, 900);
const BAR: f32 = 66.0;
const SIDE: f32 = 350.0;
const TILE: f32 = 176.0;
const ROW: f32 = 54.0;

#[derive(Clone, Copy, PartialEq, Debug)]
enum View {
    Grid,
    List,
}

/// What a folder's things are put in order by; folders first, whatever it is.
#[derive(Clone, Copy, PartialEq, Debug)]
enum SortBy {
    Name,
    Kind,
    Size,
    Modified,
}

impl SortBy {
    fn word(self) -> &'static str {
        match self {
            SortBy::Name => "Name",
            SortBy::Kind => "Kind",
            SortBy::Size => "Size",
            SortBy::Modified => "Modified",
        }
    }

    fn icon(self) -> Icon {
        match self {
            SortBy::Name => Icon::SortName,
            SortBy::Kind => Icon::Grid,
            SortBy::Size => Icon::Drive,
            SortBy::Modified => Icon::SortRecent,
        }
    }

    /// `a` against `b`, the first of a kind (the default direction: names A to Z, the biggest
    /// and the newest first).
    fn order(self, a: &Item, b: &Item) -> std::cmp::Ordering {
        let folders = (b.kind == Kind::Folder).cmp(&(a.kind == Kind::Folder));
        let by = match self {
            SortBy::Name => std::cmp::Ordering::Equal,
            SortBy::Kind => a.kind.word().cmp(b.kind.word()),
            SortBy::Size => b.size.cmp(&a.size),
            SortBy::Modified => b.modified.cmp(&a.modified),
        };
        folders.then(by).then_with(|| natural(&a.name, &b.name))
    }
}

/// Names as a person orders them: case aside, and numbers by their value ("2" before "10").
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let number = |c: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(d) = c.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(d);
                        c.next();
                    }
                    digits
                };
                let (n, m) = (number(&mut a), number(&mut b));
                let (n, m) = (n.trim_start_matches('0'), m.trim_start_matches('0'));
                let order = n.len().cmp(&m.len()).then_with(|| n.cmp(m));
                if order != std::cmp::Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != std::cmp::Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
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

    fn refilter(&mut self, hidden: bool, sort: (SortBy, bool)) {
        let words = self.filter.to_lowercase();
        let at_cursor = self.shown.get(self.cursor).copied();
        self.shown = self.items.iter().enumerate().filter(|(_, i)| (hidden || !i.hidden) && (words.is_empty() || i.name.to_lowercase().contains(&words))).map(|(n, _)| n).collect();
        let items = &self.items;
        self.shown.sort_by(|&a, &b| {
            let order = sort.0.order(&items[a], &items[b]);
            if sort.1 { order.reverse() } else { order }
        });
        // The cursor stays on what it was on.
        self.cursor = at_cursor.and_then(|i| self.shown.iter().position(|&s| s == i)).unwrap_or(self.cursor);
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
    Menu { at: (f32, f32), list: List, entries: Vec<(Row, Action)> },
    Ask { what: Asking, field: Field, ok: Button, cancel: Button },
    Confirm { text: String, action: Action, ok: Button, cancel: Button },
    Network { connect: Button, close: Button },
    About { close: Button },
    Setup(Box<Setup>),
}

#[derive(Clone, PartialEq)]
enum Asking {
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
    About,
    OpenInTab(Place),
    PasteInto(Place),
    Duplicate,
    /// Where the chosen things can be moved (or copied) to, as a menu.
    MoveToMenu,
    CopyToMenu,
    Send { to: Place, copy: bool },
    SortMenu,
    Sort(SortBy),
    SetView(View),
    TerminalHere,
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
    press: Option<hand::Press>,
    drag: Option<hand::Drag>,
    band: Option<hand::Band>,
    renaming: Option<hand::Renaming>,
    /// A rename to start, of this thing (in `items`), at this time: after a click on its name.
    rename_due: Option<(Instant, usize)>,
    /// What the folders' things are in order by, and whether the other way round.
    sort: (SortBy, bool),
    /// Where the last menu opened: where the one it leads to opens.
    menu_at: (f32, f32),
    /// Something dragged here from elsewhere in the room.
    incoming: Option<hand::Incoming>,
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
            let icon = match name.as_str() {
                "Home" | "Phone" => Icon::Home,
                "Desktop" => Icon::Desktop,
                "Documents" => Icon::Document,
                "Downloads" => Icon::Download,
                "Pictures" | "Camera" => Icon::Picture,
                "Films" => Icon::Film,
                "Music" => Icon::Music,
                _ => Icon::Drive,
            };
            rows.push((Row::new(name).icon(icon), SideRow::Go(place)));
        }
        rows.push((Row::heading("REMOTES"), SideRow::Heading));
        for remote in &self.remotes {
            // The kind is said in the foot when it is open; here there is room for the name.
            let icon = match remote.kind.as_str() {
                "sftp" | "smb" | "ftp" | "webdav" | "http" => Icon::Server,
                "alias" | "local" => Icon::Folder,
                _ => Icon::Cloud,
            };
            rows.push((Row::new(remote.name.clone()).icon(icon), SideRow::Go(Place::Remote(remote.name.clone(), String::new()))));
        }
        rows.push((Row::new("Add remote").icon(Icon::Plus), SideRow::AddRemote));
        if spatiand_storage::own_rclone_config().is_some() {
            rows.push((Row::new("Import remotes").icon(Icon::Import), SideRow::Import));
        }
        rows.push((Row::new("Other apps").icon(Icon::Link), SideRow::Network));
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
        let (hidden, sort) = (self.hidden, self.sort);
        let mut finished = Vec::new();
        let mut listed = Vec::new();
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
                        tab.shown.clear();
                        tab.refilter(hidden, sort);
                        listed.push(t);
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
        for t in listed {
            self.relisted(t);
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
                        self.say("Those remotes are here already.");
                    } else {
                        self.say(format!("Brought in {}.", names.join(", ")));
                    }
                }
                Done::Imported(Err(e)) => self.fail(e),
            }
        }
    }

    fn act(&mut self, action: Action) {
        let (hidden, sort) = (self.hidden, self.sort);
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
                let Some((index, _)) = self.tab().item_at_cursor() else { return };
                self.start_rename(index);
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
                    tab.refilter(!hidden, sort);
                }
            }
            Action::OpenInTab(place) => self.new_tab(place),
            Action::PasteInto(place) => {
                let Some((from, moving)) = self.clipboard.clone() else { return };
                self.send(from, place, !moving);
                if moving {
                    self.clipboard = None;
                }
            }
            Action::Duplicate => {
                let chosen = self.tab().chosen();
                let here = self.tab().place.clone();
                if matches!(here, Place::Local(_)) {
                    self.send(chosen, here, true);
                }
            }
            Action::MoveToMenu | Action::CopyToMenu => {
                let at = self.menu_at;
                self.destinations_menu(at, action == Action::CopyToMenu);
            }
            Action::Send { to, copy } => {
                let chosen = self.tab().chosen();
                self.send(chosen, to, copy);
            }
            Action::SortMenu => {
                let at = self.menu_at;
                self.sort_menu(at);
            }
            Action::Sort(by) => {
                // The same again turns it round.
                self.sort = if self.sort.0 == by { (by, !self.sort.1) } else { (by, false) };
                let sort = self.sort;
                for tab in &mut self.tabs {
                    tab.refilter(hidden, sort);
                }
            }
            Action::SetView(view) => self.view = view,
            Action::TerminalHere => {
                if let (Place::Local(folder), Some(program)) = (self.tab().place.clone(), own_program("spatiquill")) {
                    let mut command = Command::new(program);
                    command.env("SPATIQUILL_FOLDER", &folder).arg(&folder);
                    if let Err(e) = detached(command) {
                        self.fail(format!("SpatiQuill would not start: {e}"));
                    }
                }
            }
            Action::About => self.overlay = Overlay::About { close: Button::default() },
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
            // What is held at the moment of a click, from the keyboard's own state: a key event
            // is not sent for Shift or Control pressed on their own, so the last one seen
            // could say Control long after it was let go.
            Event::Button { .. } => self.modifiers = self.app().modifiers(),
            Event::PointerMoved { x, y, .. } => {
                self.pointer = (*x, *y);
                setup::POINTER.with(|p| p.set((*x, *y)));
                if matches!(self.overlay, Overlay::None) {
                    self.pointer_moved(*x, *y);
                }
            }
            _ => {}
        }
        // Something dragged, as the room says: this window's own, or from elsewhere.
        if matches!(event, Event::DragEntered { .. } | Event::DragMoved { .. } | Event::DragLeft { .. } | Event::Dropped { .. } | Event::DragEnded { .. }) {
            self.room_drag(event);
            return true;
        }
        // The right button opens a menu for what it is over.
        if let Event::Button { button: PointerButton::Right, pressed: true, .. } = event {
            let (x, y) = self.pointer;
            if matches!(self.overlay, Overlay::None) && self.drag.is_none() {
                self.context_menu(x, y);
            }
            return true;
        }
        // Keys the toolkit has no name for, by their number: F2 renames, the menu key (and
        // Shift-F10) opens the menu.
        if let Event::Key { code, pressed: true, modifiers, .. } = &event {
            if matches!(self.overlay, Overlay::None) && self.renaming.is_none() && !self.filtering {
                match (*code, modifiers.shift) {
                    (60, _) => {
                        self.act(Action::Rename);
                        return true;
                    }
                    (127, _) | (68, true) => {
                        self.menu_at_cursor();
                        return true;
                    }
                    _ => {}
                }
            }
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
                    self.overlay = Overlay::None;
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
            Overlay::About { close } => {
                if close.input(&input) || matches!(input, Input::Key { key: Key::Escape | Key::Enter, .. }) {
                    self.overlay = Overlay::None;
                }
                return;
            }
            Overlay::Setup(setup) => {
                let want = setup.input(&input);
                self.configure(want);
                return;
            }
        }
        // A name being typed over has the keys; a press anywhere else is the end of it.
        if let Some(r) = &mut self.renaming {
            match &input {
                Input::Key { key: Key::Escape, .. } => {
                    self.renaming = None;
                    return;
                }
                Input::Key { key: Key::Tab, .. } => {
                    self.finish_rename();
                    return;
                }
                Input::Key { .. } => {
                    if r.field.input(&input) == Some(Typed::Entered) {
                        self.finish_rename();
                    }
                    return;
                }
                Input::Press { pressed: true } if r.field.contains(self.pointer.0, self.pointer.1) => return,
                Input::Press { pressed: true } => self.finish_rename(),
                _ => {}
            }
        }
        // Letting go: the end of a press, a drag or a band, wherever the pointer is.
        if let Input::Press { pressed: false } = input {
            if self.press.is_some() || self.drag.is_some() || self.band.is_some() {
                self.release();
                return;
            }
        }
        if let Input::Key { key: Key::Escape, .. } = input {
            if self.drag.take().is_some() || self.band.take().is_some() {
                self.press = None;
                return;
            }
        }
        // The filter field, while it is open.
        if self.filtering {
            if let Input::Key { key: Key::Escape, .. } = input {
                self.filtering = false;
                self.filter_field.set("");
                self.tab().filter.clear();
                let (h, sort) = (self.hidden, self.sort);
                self.tab().refilter(h, sort);
                return;
            }
            if let Some(typed) = self.filter_field.input(&input) {
                let text = self.filter_field.text.clone();
                let (h, sort) = (self.hidden, self.sort);
                let tab = self.tab();
                tab.filter = text;
                tab.refilter(h, sort);
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
            Input::Press { pressed: true } if self.content.contains(x, y) => self.content_press(x, y),
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
                Some('d') => self.act(Action::Duplicate),
                Some('r') => self.act(Action::Refresh),
                // Control-H on Linux, as its file managers have it; Shift-Command-. on a Mac.
                Some('h') if m.shift || m.logo || !cfg!(target_os = "macos") => self.act(Action::ToggleHidden),
                Some('.') if m.shift && m.logo => self.act(Action::ToggleHidden),
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
            p.begin(gl, picture, picture.1 as f32 / size.1 as f32, theme::ground());
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
        for (button, icon) in [(&mut self.backward, Icon::Back), (&mut self.forward, Icon::Forward), (&mut self.up, Icon::Up)] {
            button.draw_icon(p, gl, head.take_left(Button::HEIGHT), icon, "");
            head.take_left(8.0);
        }
        head.take_left(10.0);
        let (view_icon, view_label) = if self.view == View::Grid { (Icon::List, "List") } else { (Icon::Grid, "Tiles") };
        let vw = Button::icon_width(p, gl, view_label);
        let view_rect = Rect { x: head.right() - vw, w: vw, ..head };
        head.w -= vw + 12.0;
        self.view_button.draw_icon(p, gl, view_rect, view_icon, view_label);
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
                p.fill(gl, rect, theme::ROW_RADIUS, theme::hover());
            }
            p.label(gl, &label, rect.inset(8.0), em, if last { theme::ink() } else { theme::ink_row() }, 0.0);
            self.trail.push((rect, place.clone()));
            x += rect.w;
            if !last {
                p.label(gl, "\u{203a}", Rect::new(x, head.y, 20.0, head.h), em, theme::ink_hint(), 0.5);
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
        self.paint_hand(p, gl);
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
                p.gradient(gl, rect, 14.0, theme::wash(0.11), theme::wash(0.05));
                room::rule(p, gl, Rect::new(rect.x + 16.0, rect.bottom() - 3.0, rect.w - 32.0, 2.5));
            } else if hover {
                p.fill(gl, rect, 14.0, theme::hover());
            }
            let dot = if tab.crashed.is_some() {
                room::trouble()
            } else if !tab.doing.is_empty() {
                [1.0, 0.85, 0.4, 1.0]
            } else if matches!(tab.place, Place::Remote(..)) {
                room::VIOLET
            } else if active {
                room::TEAL
            } else {
                theme::ink_hint()
            };
            p.fill(gl, Rect::new(rect.x + 16.0, rect.y + rect.h * 0.5 - 4.0, 8.0, 8.0), 4.0, dot);
            let close = Rect::new(rect.right() - 38.0, rect.y + rect.h * 0.5 - 13.0, 26.0, 26.0);
            let show_close = (active || hover) && self.tabs.len() > 1;
            let label = Rect { x: rect.x + 34.0, w: rect.w - 34.0 - if show_close { 44.0 } else { 14.0 }, ..rect };
            p.label(gl, &tab.label(), label, theme::ROW_EM * 0.72, if active { theme::ink() } else { theme::ink_row() }, 0.0);
            if show_close {
                if close.inset(-6.0).contains(self.pointer.0, self.pointer.1) {
                    p.fill(gl, close, 13.0, theme::wash(0.14));
                }
                draw_icon(p, gl, Icon::Close, close.inset(4.0), theme::ink_detail());
            }
            self.placed_tabs.push((rect, close));
        }
        self.plus = Rect::new(x, bar.y + (bar.h - 44.0) * 0.5, 44.0, 44.0);
        if self.plus.contains(self.pointer.0, self.pointer.1) {
            p.fill(gl, self.plus, 22.0, theme::hover());
        }
        draw_icon(p, gl, Icon::Plus, self.plus.inset(10.0), theme::ink_detail());
    }

    fn paint_content(&mut self, p: &mut Painter, gl: &spatiand_app::Gl, area: Rect) {
        let view = self.view;
        let columns = self.columns();
        let pointer = self.pointer;
        let sort = self.sort;
        // The name being renamed is typed over where it is, not written.
        let renaming = self.renaming.as_ref().filter(|r| r.tab == self.active).map(|r| r.name.clone());
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
            p.label(gl, "This tab stopped", inner.take_top(50.0), theme::TITLE_EM * 0.8, room::trouble(), 0.0);
            p.label(gl, why, inner.take_top(40.0), theme::DETAIL_EM, theme::ink_detail(), 0.0);
            p.label(gl, "Close it and open another: the other tabs are as they were.", inner.take_top(40.0), theme::FOOTER_EM, theme::ink_hint(), 0.0);
            return;
        }
        match &tab.state {
            State::Loading(since) => {
                room::waiting(p, gl, area, *since);
                return;
            }
            State::Failed(why) => {
                room::message(p, gl, area, why, room::trouble());
                return;
            }
            State::Ready if tab.shown.is_empty() => {
                let text = if !tab.filter.is_empty() { "Nothing here by that name." } else if tab.items.is_empty() { "This folder is empty." } else { "Only hidden files here." };
                room::message(p, gl, area, text, theme::ink_detail());
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
            // Each head sorts by itself, the one sorted by marked with which way.
            for (i, (word, by, align)) in [("NAME", SortBy::Name, 0.0), ("KIND", SortBy::Kind, 0.0), ("SIZE", SortBy::Size, 1.0), ("MODIFIED", SortBy::Modified, 1.0)].into_iter().enumerate() {
                let on = sort.0 == by;
                let word = if on { format!("{word} {}", if sort.1 { "\u{25bc}" } else { "\u{25b2}" }) } else { word.to_string() };
                if cols[i].contains(pointer.0, pointer.1) {
                    p.fill(gl, cols[i].inset(-4.0), theme::ROW_RADIUS, theme::hover());
                }
                p.label(gl, &word, cols[i], theme::FOOTER_EM * 0.82, if on { theme::ink_detail() } else { theme::ink_hint() }, align);
            }
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
                        p.fill(gl, face.inset(-4.0), 18.0, theme::mark());
                    } else if is_cursor {
                        p.fill(gl, face.inset(-3.0), 17.0, theme::wash(0.30));
                    } else if hover {
                        p.fill(gl, face.inset(-3.0), 17.0, theme::wash(0.14));
                    }
                    look::face(p, gl, face, item.kind, picture, 14.0);
                    let name = Rect::new(cell.x + 2.0, face.bottom() + 8.0, cell.w - 4.0, 28.0);
                    if renaming.as_deref() != Some(item.name.as_str()) {
                        p.label(gl, &item.name, name, theme::ROW_EM * 0.62, if selected || is_cursor { theme::ink() } else { theme::ink_row() }, 0.5);
                    }
                    let detail = if item.kind == Kind::Folder { String::new() } else { look::size(item.size) };
                    p.label(gl, &detail, Rect { y: name.bottom(), h: 24.0, ..name }, theme::FOOTER_EM * 0.8, theme::ink_hint(), 0.5);
                }
                View::List => {
                    let rect = Rect::new(area.x, y, area.w, ROW - 4.0);
                    let hover = rect.contains(pointer.0, pointer.1);
                    if selected {
                        p.fill(gl, rect, theme::ROW_RADIUS, theme::selected());
                    } else if is_cursor {
                        p.fill(gl, rect, theme::ROW_RADIUS, theme::selected_away());
                    } else if hover {
                        p.fill(gl, rect, theme::ROW_RADIUS, theme::hover());
                    }
                    let cols = list_columns(rect);
                    let icon = Rect::new(cols[0].x, rect.y + 7.0, rect.h - 14.0, rect.h - 14.0);
                    look::face(p, gl, icon, item.kind, picture, 8.0);
                    let name = Rect { x: icon.right() + 14.0, w: cols[0].w - icon.w - 14.0, ..cols[0] };
                    if renaming.as_deref() != Some(item.name.as_str()) {
                        p.label(gl, &item.name, name, theme::ROW_EM * 0.7, if selected || is_cursor { theme::ink() } else { theme::ink_row() }, 0.0);
                    }
                    p.label(gl, item.kind.word(), cols[1], theme::DETAIL_EM * 0.9, theme::ink_hint(), 0.0);
                    if item.kind != Kind::Folder {
                        p.label(gl, &look::size(item.size), cols[2], theme::DETAIL_EM * 0.9, theme::ink_detail(), 1.0);
                    }
                    p.label(gl, &look::when(item.modified, now), cols[3], theme::DETAIL_EM * 0.9, theme::ink_detail(), 1.0);
                }
            }
        }
        p.unclip(gl);
        if content_h > area.h {
            let track = Rect::new(area.right() - 5.0, area.y + header, 5.0, area.h - header);
            p.fill(gl, track, 2.5, theme::scroll_track());
            let h = (track.h * area.h / content_h).max(30.0);
            let y = track.y + (track.h - h) * (tab.scroll / (content_h - area.h).max(1.0));
            p.fill(gl, Rect { y, h, ..track }, 2.5, theme::scroll_thumb());
        }
        if let Some(at) = self.renaming_at() {
            let rect = self.rename_rect(at);
            if let Some(r) = self.renaming.as_mut() {
                p.fill(gl, rect.inset(-2.0), theme::ROW_RADIUS, theme::ground());
                r.field.draw(p, gl, rect);
            }
        }
    }

    fn paint_foot(&mut self, p: &mut Painter, gl: &spatiand_app::Gl, foot: Rect) {
        let tab = &self.tabs[self.active];
        // Something being done, as a bar.
        if let Some((what, done, total)) = tab.doing.values().next() {
            let bar = Rect::new(foot.right() - 360.0, foot.y + foot.h * 0.5 - 4.0, 360.0, 8.0);
            p.fill(gl, bar, 4.0, theme::scroll_track());
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
            words += &format!("    {r}{}", if kind.is_empty() { String::new() } else { format!("  \u{b7}  {kind}") });
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
                // On the ground's own colour under the glass: a menu over pictures is read, not
                // seen through.
                p.glow(gl, rect, 20.0, [0.0, 0.0, 0.0, 0.45], 24.0);
                p.fill(gl, rect, 20.0, theme::ground());
                room::glass(p, gl, rect, 20.0, 1.0);
                let rows: Vec<Row> = entries.iter().map(|(r, _)| r.clone()).collect();
                list.draw(p, gl, rect.inset(10.0), &rows);
            }
            Overlay::Ask { what, field, ok, cancel } => {
                let pane = Rect::new(all.w * 0.5 - 380.0, all.h * 0.32, 760.0, 250.0);
                p.fill(gl, all, 0.0, [0.0, 0.0, 0.0, 0.35]);
                room::glass(p, gl, pane, 24.0, 1.0);
                let mut inner = pane.inset(28.0);
                let title = match what {
                    Asking::NewFolder => "A new folder".to_string(),
                };
                p.label(gl, &title, inner.take_top(48.0), theme::TITLE_EM * 0.75, theme::ink(), 0.0);
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
                p.text(gl, text, inner.x, inner.y, theme::ROW_EM * 0.8, inner.w, theme::ink());
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
                p.label(gl, "Your remotes, for other apps", inner.take_top(50.0), theme::TITLE_EM * 0.75, theme::ink(), 0.0);
                let (_, h) = p.text(gl, "Spatiand's apps reach your remotes by themselves. Other apps reach them as a network folder (WebDAV) on this machine, which only this machine can open:", inner.x, inner.y, theme::DETAIL_EM, inner.w, theme::ink_detail());
                inner.take_top(h + 18.0);
                match &storage {
                    Some(storage) => {
                        let (url, user, token) = storage.dav_root();
                        for (what, value) in [("Address", url), ("Name", user), ("Password", token)] {
                            let line = inner.take_top(40.0);
                            p.label(gl, &what.to_uppercase(), Rect { w: 180.0, ..line }, theme::FOOTER_EM * 0.82, theme::ink_hint(), 0.0);
                            p.label(gl, &value, Rect { x: line.x + 180.0, w: line.w - 180.0, ..line }, theme::ROW_EM * 0.72, theme::ink(), 0.0);
                        }
                    }
                    None => {
                        p.label(gl, "The storage service is not running.", inner.take_top(40.0), theme::DETAIL_EM, room::trouble(), 0.0);
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
            Overlay::About { close } => {
                let pane = Rect::new(all.w * 0.5 - 460.0, all.h * 0.2, 920.0, 480.0);
                p.fill(gl, all, 0.0, [0.0, 0.0, 0.0, 0.35]);
                room::glass(p, gl, pane, 24.0, 1.0);
                let mut inner = pane.inset(30.0);
                p.label(gl, "SpatiShelf", inner.take_top(50.0), theme::TITLE_EM * 0.8, theme::ink(), 0.0);
                let text = "Spatiand's file manager. Remotes are reached through Spatiand's storage service, which is built on rclone (MIT License, copyright Nick Craig-Wood and the rclone contributors). PDF pages are drawn by PDFium (BSD and Apache 2.0, the Chromium authors); films and pictures are read by FFmpeg (LGPL 2.1).";
                let (_, h) = p.text(gl, text, inner.x, inner.y, theme::DETAIL_EM, inner.w, theme::ink_detail());
                inner.take_top(h);
                let buttons = inner.take_bottom(Button::HEIGHT);
                close.draw(p, gl, Rect { x: buttons.right() - 180.0, w: 180.0, ..buttons }, "Done");
            }
            Overlay::Setup(setup) => {
                let pane = Rect::new(SIDE + 8.0, BAR + 6.0, all.w - SIDE - 22.0, all.h - BAR - 20.0);
                p.fill(gl, pane.inset(-6.0), 22.0, { let mut g = theme::ground(); g[3] = 0.96; g });
                room::glass(p, gl, pane, 22.0, 0.6);
                setup.draw(p, gl, pane.inset(24.0));
            }
        }
    }

    fn run(&mut self) -> Result<(), String> {
        loop {
            let animating = self.tabs.iter().any(|t| matches!(t.state, State::Loading(_)) || !t.doing.is_empty()) || self.waiting();
            let wait = if animating { 33 } else { 250 };
            for event in self.app().wait(Some(Duration::from_millis(wait)))? {
                if !self.handle(event) {
                    return Ok(());
                }
            }
            self.hear();
            self.answers();
            self.tick();
            self.draw()?;
        }
    }
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
    let w = 620.0;
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
    // `pbcopy` takes what text is from the locale, which a Mac application started from the
    // Finder has not got: MacRoman, then, and a name past ASCII turned to mojibake.
    let utf8 = [("LANG", "en_US.UTF-8"), ("LC_CTYPE", "en_US.UTF-8")];
    if let Ok(mut child) = Command::new(program).envs(utf8).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
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
            press: None,
            drag: None,
            band: None,
            renaming: None,
            rename_due: None,
            sort: (SortBy::Name, false),
            menu_at: (0.0, 0.0),
            incoming: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, kind: Kind, size: u64, modified: i64) -> Item {
        Item { name: name.into(), kind, size, modified: Some(modified), hidden: false }
    }

    #[test]
    fn names_in_a_persons_order() {
        let mut names = vec!["chapter 10", "Chapter 2", "chapter 1", "b", "A", "x09", "x9", "x10"];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(names, ["A", "b", "chapter 1", "Chapter 2", "chapter 10", "x09", "x9", "x10"]);
    }

    #[test]
    fn folders_first_however_sorted() {
        let items = [item("z", Kind::Folder, 0, 1), item("big", Kind::Film, 900, 5), item("a", Kind::Document, 10, 9)];
        let order = |by: SortBy| {
            let mut shown: Vec<usize> = (0..items.len()).collect();
            shown.sort_by(|&a, &b| by.order(&items[a], &items[b]));
            shown.into_iter().map(|i| items[i].name.as_str()).collect::<Vec<_>>()
        };
        assert_eq!(order(SortBy::Name), ["z", "a", "big"]);
        assert_eq!(order(SortBy::Size), ["z", "big", "a"]);
        assert_eq!(order(SortBy::Modified), ["z", "a", "big"]);
    }
}
