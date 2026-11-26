//! Spatiand's file chooser: a window of the room in which the wearer chooses files, a folder, or a
//! name to save under, for a program that asked -- SpatiMesh's Chromium
//! (third_party/chromium/android-libc/0019). The same chooser on every platform.
//!
//! ```text
//! spatiand-choose --mode open|open-multiple|folder|save [--title T] [--dir D] [--name N]
//!                 [--accept png,jpg,...]
//! ```
//!
//! What was chosen is written to standard output, a path a line, and it exits 0; nothing chosen
//! (Cancel, Escape, the window closed) and it exits 1 having written nothing. The places, the
//! folder's own lines and a field to find in it or type a path into, as the Rust SpatiMesh's own
//! chooser had (its `Picker`, until 2026-11-10).
//!
//! **Remotes.** Under the places are the storage service's remotes, SpatiShelf's (SMB, SFTP,
//! cloud drives: `storage/`), asked for on a thread of their own so that the window is there at
//! once (`remote.rs`); a service that cannot be reached is said in one quiet line. A remote's
//! folder is listed the same way, "Loading..." until it comes, and found in, filtered by kind,
//! marked in and gone up from as this machine's are; `remote:path` typed goes there. The program
//! is given paths of this machine all the same, which is all it understands:
//!
//! - **opened** from a remote, a file, the files marked or a folder with everything in it are
//!   fetched first, with rclone, into `$XDG_RUNTIME_DIR/spatiand-choose/fetched/` (the temporary
//!   folder's where there is no runtime folder), the window saying how far it has come, with a
//!   Cancel that stops it; then their paths there are written. A fetch is taken away a day later,
//!   by the next chooser to start (`own.rs`);
//! - **saved** to a remote -- Replace asked first where the name is taken there, as here -- the
//!   path written is one in `.../spatiand-choose/outbox/`, in a folder of its own for this save,
//!   and a sender is started on its own (`spatiand-choose --send`, `send.rs`), which waits for the
//!   program to have written the file there and moves it to the remote.
//!
//! A program asking to begin in a folder of the chooser's own -- where its last choice was --
//! begins in the place that folder stands for; `--dir remote:path` begins in a remote.

mod own;
mod place;
mod remote;
mod send;

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use spatiand_app::ui::{draw_icon, look, theme, Button, Field, Icon, Input, List, Painter, Rect, Row};
use spatiand_app::{App, Event, Key};
use spatiand_storage::{Entry, Progress, Remote};

use place::Place;
use remote::{Answer, Request, Side};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Open,
    OpenMultiple,
    Folder,
    Save,
}

#[derive(Debug, Clone, PartialEq)]
enum Target {
    Nothing,
    Go(Place),
    File(Place),
    ShowAll,
}

struct Ask {
    mode: Mode,
    title: String,
    dir: Place,
    name: String,
    /// The endings asked for, lower case without the dot; empty for any.
    accept: Vec<String>,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into()))
}

fn ask_from_args(args: &[String], base: &Path) -> Result<Ask, String> {
    let mut ask = Ask { mode: Mode::Open, title: String::new(), dir: Place::Local(home()), name: String::new(), accept: Vec::new() };
    let mut dir = None;
    let mut args = args.iter().cloned();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} wants a value"));
        match arg.as_str() {
            "--mode" => {
                ask.mode = match value()?.as_str() {
                    "open" => Mode::Open,
                    "open-multiple" => Mode::OpenMultiple,
                    "folder" => Mode::Folder,
                    "save" => Mode::Save,
                    other => return Err(format!("no mode {other}")),
                }
            }
            "--title" => ask.title = value()?,
            "--dir" => dir = Some(value()?),
            "--name" => ask.name = value()?,
            "--accept" => {
                ask.accept = value()?
                    .split(',')
                    .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
                    .filter(|e| !e.is_empty() && e != "*")
                    .collect()
            }
            other => return Err(format!("not understood: {other}")),
        }
    }
    ask.dir = starting_place(dir.as_deref(), base);
    Ok(ask)
}

/// Where the chooser opens: the folder asked for -- or, for one of the chooser's own (a fetch, a
/// save on its way, where the program's last choice was), the place it stands for; `remote:path`
/// in a remote; home where it is none of these.
fn starting_place(asked: Option<&str>, base: &Path) -> Place {
    let home = home();
    let Some(asked) = asked else { return Place::Local(home) };
    let path = PathBuf::from(asked);
    if let Some(place) = own::stands_for(&path, base, &home) {
        return place;
    }
    if path.is_dir() {
        return Place::Local(path);
    }
    match Place::parse(asked) {
        Some(place @ Place::Remote(..)) => place,
        _ => Place::Local(home),
    }
}

/// An error as a line of its own: begun with a capital, ended with a stop.
fn sentence(text: &str) -> String {
    let text = text.trim();
    let mut chars = text.chars();
    let mut out = chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default();
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

/// A remote's mark, as SpatiShelf's side shows it.
fn remote_icon(kind: &str) -> Icon {
    match kind {
        "sftp" | "smb" | "ftp" | "webdav" | "http" => Icon::Server,
        "alias" | "local" => Icon::Folder,
        _ => Icon::Cloud,
    }
}

/// The storage service's remotes, as far as it has said.
#[derive(Debug, Clone, PartialEq)]
enum Known {
    Asking,
    Got(Vec<Remote>),
    /// It could not be reached: why, as a line.
    Failed(String),
}

/// What is known of the remote folder on show: asked for on going there, and numbered, so that
/// an answer for a folder left is let go of.
#[derive(Debug, Default)]
struct Listing {
    generation: u64,
    state: Listed,
}

#[derive(Debug, Default, PartialEq)]
enum Listed {
    #[default]
    Loading,
    Here(Vec<Entry>),
    Failed(String),
}

/// A question asked of a remote, and what its answer is for.
#[derive(Debug, Clone, PartialEq)]
enum Looking {
    /// Saving under this name: whether it is taken there.
    Save(Place),
    /// A path typed: whether it is a folder, a file, or nothing.
    Typed(Place),
    /// A path typed to save under, where nothing is: whether its folder is there.
    Into(Place),
}

impl Looking {
    fn place(&self) -> &Place {
        match self {
            Looking::Save(p) | Looking::Typed(p) | Looking::Into(p) => p,
        }
    }
}

/// What is being fetched, as the window shows it.
#[derive(Debug, Clone)]
struct Fetching {
    /// The chooser's own folder it is fetched into, taken away if it is stopped.
    into: PathBuf,
    /// The folder it comes from.
    from: String,
    name: String,
    index: usize,
    of: usize,
    progress: Progress,
    folder: bool,
    stopping: bool,
}

/// What the answers since last asked came to.
#[derive(Debug, PartialEq)]
enum Heard {
    Nothing,
    Changed,
    /// The choice, made: a fetch's end, or a save's name found free.
    Done(Vec<PathBuf>),
}

/// How a file saved to a remote is sent on: [`send::start`], or a stand-in in the tests.
type Sender = Box<dyn FnMut(&Path, &Place) -> Result<(), String>>;

struct Chooser {
    ask: Ask,
    dir: Place,
    field: Field,
    list: List,
    rows: Vec<Row>,
    targets: Vec<Target>,
    chosen: Vec<Place>,
    all_kinds: bool,
    said: Option<String>,
    /// Something being waited on, said quietly where `said` would be.
    note: Option<String>,
    /// A name to save under that is there already, once the wearer has been told.
    replacing: Option<Place>,
    ok: Button,
    cancel: Button,
    stale: bool,
    /// The cursor to go to the folder's own first line when the lines are next made: on opening,
    /// on going to a folder, and on finding (so that Enter takes what was found). A remote's
    /// folder still being listed keeps it until its lines come.
    to_folder: bool,
    /// The storage service, asked on threads of its own; none until [`Chooser::connect`].
    side: Option<Side>,
    known: Known,
    listing: Listing,
    /// The question asked of a remote that is being waited on, by its number.
    looking: Option<(u64, Looking)>,
    asked: u64,
    fetching: Option<Fetching>,
    /// Where the chooser's own folders are (`own::base`).
    base: PathBuf,
    sender: Sender,
}

impl Chooser {
    fn new(ask: Ask, base: PathBuf, sender: Sender) -> Chooser {
        let mut field = Field::default();
        field.focused = true;
        field.hint = match ask.mode {
            Mode::Save => "The file's name, or a path and Enter".into(),
            _ => "Type to find here, or a path and Enter".into(),
        };
        if ask.mode == Mode::Save {
            field.set(ask.name.clone());
        }
        let mut list = List::default();
        list.focused = true;
        // The yes is the lit one, as a question's is: Enter answers with it.
        let mut ok = Button::default();
        ok.focused = true;
        let mut chooser = Chooser {
            dir: ask.dir.clone(),
            ask,
            field,
            list,
            rows: Vec::new(),
            targets: Vec::new(),
            chosen: Vec::new(),
            all_kinds: false,
            said: None,
            note: None,
            replacing: None,
            ok,
            cancel: Button::default(),
            stale: true,
            to_folder: true,
            side: None,
            known: Known::Asking,
            listing: Listing::default(),
            looking: None,
            asked: 0,
            fetching: None,
            base,
            sender,
        };
        chooser.list_remote();
        chooser
    }

    /// The storage service to ask: its remotes asked for at once, and the folder on show if it
    /// is a remote's.
    fn connect(&mut self, side: Side) {
        side.ask(Request::Remotes);
        self.side = Some(side);
        self.list_remote();
    }

    fn title(&self) -> String {
        if !self.ask.title.is_empty() {
            return self.ask.title.clone();
        }
        match self.ask.mode {
            Mode::Open => "Choose a file".into(),
            Mode::OpenMultiple => "Choose files".into(),
            Mode::Folder => "Choose a folder".into(),
            Mode::Save => "Save as".into(),
        }
    }

    fn ok_label(&self) -> String {
        match self.ask.mode {
            Mode::Folder => "Choose this folder".into(),
            Mode::Save if self.replacing.is_some() => "Replace".into(),
            Mode::Save => "Save".into(),
            Mode::OpenMultiple if !self.chosen.is_empty() => format!("Choose {}", self.chosen.len()),
            _ => "Choose".into(),
        }
    }

    fn wanted(&self, name: &str) -> bool {
        let ending = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        self.all_kinds || self.ask.accept.is_empty() || self.ask.accept.contains(&ending)
    }

    /// A remote's place typed, `remote:path`, for one the service has said is there.
    fn typed_remote(&self, text: &str) -> Option<Place> {
        match &self.known {
            Known::Got(remotes) => Place::typed(text, remotes),
            _ => None,
        }
    }

    /// In the field, for opening: words to find among the folder's lines (not a path, not a name
    /// to save under).
    fn filter(&self) -> String {
        let typed = &self.field.text;
        if self.ask.mode == Mode::Save || typed.starts_with('/') || typed.starts_with('~') || self.typed_remote(typed.trim()).is_some() {
            String::new()
        } else {
            typed.to_lowercase()
        }
    }

    fn build(&mut self) {
        self.stale = false;
        let filter = self.filter();
        // The line the cursor is on, kept on where lines come in above it (the remotes, once the
        // service has said what they are).
        let kept = if self.to_folder { None } else { self.targets.get(self.list.cursor).cloned().filter(|t| *t != Target::Nothing) };
        let mut rows = Vec::new();
        let mut targets = Vec::new();
        rows.push(Row::heading("Places"));
        targets.push(Target::Nothing);
        let home = home();
        let mut places = vec![("Home".to_string(), home.clone(), Icon::Home)];
        for (name, icon) in [("Downloads", Icon::Download), ("Documents", Icon::Document), ("Desktop", Icon::Desktop), ("Pictures", Icon::Picture)] {
            let p = home.join(name);
            if p.is_dir() {
                places.push((name.to_string(), p, icon));
            }
        }
        // Android's shared storage, where a phone's photos and downloads are: only where it may
        // be read (Spatiand given "All files access").
        for shared in ["/storage/emulated/0", "/sdcard"] {
            let p = PathBuf::from(shared);
            if cfg!(target_os = "android") && std::fs::read_dir(&p).is_ok() {
                places.push(("The phone's storage".to_string(), p, Icon::Drive));
                break;
            }
        }
        places.push(("The whole machine".to_string(), PathBuf::from("/"), Icon::Drive));
        for (name, path, icon) in places {
            let place = Place::Local(path);
            let here = place == self.dir;
            rows.push(Row::with(name, if here { "\u{2713} here" } else { "" }).icon(icon));
            targets.push(Target::Go(place));
        }
        // The remotes, as soon as the service has said; nothing while there are none.
        match &self.known {
            Known::Asking => {}
            Known::Got(remotes) if remotes.is_empty() => {}
            Known::Got(remotes) => {
                rows.push(Row::heading("Remotes"));
                targets.push(Target::Nothing);
                for remote in remotes {
                    let place = Place::remote(&remote.name, "");
                    let here = place == self.dir;
                    rows.push(Row::with(remote.name.clone(), if here { "\u{2713} here" } else { "" }).icon(remote_icon(&remote.kind)));
                    targets.push(Target::Go(place));
                }
            }
            Known::Failed(why) => {
                rows.push(Row::heading("Remotes"));
                targets.push(Target::Nothing);
                rows.push(Row::heading(why.clone()));
                targets.push(Target::Nothing);
            }
        }
        rows.push(Row::heading(self.dir.shown()));
        targets.push(Target::Nothing);
        if let Some(up) = self.dir.parent() {
            // The folder's name alone: a whole path runs over the line's own label.
            rows.push(Row::with("\u{2039} Up", up.name()).icon(Icon::Up));
            targets.push(Target::Go(up));
        }
        // The folder's own first line (or Up, in an empty folder): where the cursor starts.
        let first = rows.len();
        let mut waiting = false;
        // Each of the folder's things: its name, where it is, whether a folder, and its size
        // where the listing said it (a remote's; this machine's is read as it is shown).
        let mut entries: Vec<(String, Place, bool, Option<u64>)> = match &self.dir {
            Place::Local(dir) => std::fs::read_dir(dir)
                .map(|d| {
                    d.flatten()
                        .filter_map(|e| {
                            let name = e.file_name().to_string_lossy().into_owned();
                            let path = e.path();
                            (!name.starts_with('.')).then(|| (name, Place::Local(path.clone()), path.is_dir(), None))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Place::Remote(..) => match &self.listing.state {
                Listed::Loading => {
                    waiting = true;
                    rows.push(Row::new("Loading\u{2026}"));
                    targets.push(Target::Nothing);
                    Vec::new()
                }
                Listed::Failed(why) => {
                    rows.push(Row::new(format!("It could not be opened: {why}")));
                    targets.push(Target::Nothing);
                    Vec::new()
                }
                Listed::Here(listed) => listed
                    .iter()
                    .filter(|e| !e.name.starts_with('.'))
                    .map(|e| (e.name.clone(), self.dir.join(&e.name), e.is_dir, u64::try_from(e.size).ok()))
                    .collect(),
            },
        };
        entries.sort_by(|a, b| (!a.2, a.0.to_lowercase()).cmp(&(!b.2, b.0.to_lowercase())));
        let mut hidden = 0;
        for (name, place, is_dir, size) in entries {
            if !filter.is_empty() && !name.to_lowercase().contains(&filter) {
                continue;
            }
            if is_dir {
                rows.push(Row::new(name).icon(Icon::Folder));
                targets.push(Target::Go(place));
            } else if self.ask.mode == Mode::Folder {
                continue;
            } else if self.wanted(&name) {
                let size = match (&place, size) {
                    (Place::Local(path), _) => std::fs::metadata(path).map(|m| look::size(m.len())).unwrap_or_default(),
                    (_, Some(size)) => look::size(size),
                    _ => String::new(),
                };
                let mark = if self.chosen.contains(&place) { format!("\u{2713}  {size}") } else { size };
                rows.push(Row::with(name, mark).icon(Icon::Document));
                targets.push(Target::File(place));
            } else {
                hidden += 1;
            }
        }
        if hidden > 0 {
            rows.push(Row::with("Show every kind of file", format!("{hidden} not shown")));
            targets.push(Target::ShowAll);
        }
        if self.to_folder {
            self.list.cursor = if first < rows.len() { first } else { first.saturating_sub(1) };
            self.to_folder = waiting;
        } else if let Some(at) = kept.and_then(|kept| targets.iter().position(|t| *t == kept)) {
            self.list.cursor = at;
        }
        self.rows = rows;
        self.targets = targets;
    }

    fn go(&mut self, dir: Place) {
        self.dir = dir;
        if self.ask.mode != Mode::Save {
            self.field.set(String::new());
        }
        self.said = None;
        self.note = None;
        self.replacing = None;
        self.looking = None;
        self.stale = true;
        self.to_folder = true;
        self.list_remote();
    }

    /// The folder on show changed, the rest kept: as a path typed to save under.
    fn set_dir(&mut self, dir: Place) {
        self.dir = dir;
        self.stale = true;
        self.list_remote();
    }

    /// A remote's folder on show: its listing asked for, "Loading..." until it comes.
    fn list_remote(&mut self) {
        if !self.dir.is_remote() {
            return;
        }
        self.listing.generation += 1;
        self.listing.state = match &self.side {
            Some(side) => {
                side.ask(Request::List { generation: self.listing.generation, place: self.dir.clone() });
                Listed::Loading
            }
            None => Listed::Failed("the storage service cannot be asked".into()),
        };
    }

    /// Ask a remote what is at a place; the answer, when it comes, does what `looking` is for.
    fn look(&mut self, place: Place, looking: Looking) {
        let Some(side) = &self.side else {
            self.said = Some("The storage service cannot be asked.".into());
            return;
        };
        self.asked += 1;
        side.ask(Request::Look { token: self.asked, place: place.clone() });
        let folder = if matches!(looking, Looking::Into(_)) { place } else { place.parent().unwrap_or(place) };
        self.note = Some(format!("Looking in {}\u{2026}", folder.shown()));
        self.said = None;
        self.looking = Some((self.asked, looking));
    }

    /// What the name typed to save under stands for, here.
    fn save_place(&self) -> Option<Place> {
        let name = self.field.text.trim();
        (!name.is_empty()).then(|| self.dir.join(name))
    }

    /// A line pressed or Entered: a folder gone into, a file chosen or marked.
    fn target(&mut self, target: Target) -> Option<Vec<PathBuf>> {
        match target {
            Target::Nothing => None,
            Target::Go(dir) => {
                self.go(dir);
                None
            }
            Target::ShowAll => {
                self.all_kinds = true;
                self.stale = true;
                None
            }
            Target::File(place) => match self.ask.mode {
                Mode::OpenMultiple => {
                    match self.chosen.iter().position(|p| *p == place) {
                        Some(i) => {
                            self.chosen.remove(i);
                        }
                        None => self.chosen.push(place),
                    }
                    self.stale = true;
                    None
                }
                // Saving: a file pressed lends its name.
                Mode::Save => {
                    self.field.set(place.name());
                    self.replacing = None;
                    None
                }
                _ => self.finish(vec![place], false),
            },
        }
    }

    /// The chooser's own button.
    fn ok(&mut self) -> Option<Vec<PathBuf>> {
        match self.ask.mode {
            Mode::Folder => self.finish(vec![self.dir.clone()], true),
            Mode::Save => {
                let name = self.field.text.trim().to_string();
                if name.is_empty() {
                    self.said = Some("Type the file's name first.".into());
                    return None;
                }
                let place = self.dir.join(&name);
                let Place::Local(path) = &place else {
                    // A remote is asked whether the name is taken there, as this machine is
                    // below: unless the wearer has been told, and said Replace.
                    if self.replacing.as_ref() == Some(&place) {
                        return self.send_to(place);
                    }
                    self.look(place.clone(), Looking::Save(place));
                    return None;
                };
                if path.is_dir() {
                    self.go(place);
                    return None;
                }
                if path.exists() && self.replacing.as_ref() != Some(&place) {
                    self.said = Some(format!("{name} is here already: press Replace to write over it."));
                    self.replacing = Some(place);
                    return None;
                }
                Some(vec![path.clone()])
            }
            _ if !self.chosen.is_empty() => self.finish(self.chosen.clone(), false),
            _ => match self.targets.get(self.list.cursor).cloned() {
                Some(Target::File(place)) => self.finish(vec![place], false),
                _ => {
                    self.said = Some("Choose a file first.".into());
                    None
                }
            },
        }
    }

    /// Enter: a path typed is gone to (or, a file, chosen); otherwise the line at the cursor, or
    /// for saving the name typed.
    fn enter(&mut self, control: bool) -> Option<Vec<PathBuf>> {
        let typed = self.field.text.trim().to_string();
        if typed.starts_with('/') || typed.starts_with('~') {
            let path = match typed.strip_prefix('~') {
                Some(rest) => home().join(rest.trim_start_matches('/')),
                None => PathBuf::from(&typed),
            };
            if path.is_dir() {
                self.go(Place::Local(path));
                if self.ask.mode == Mode::Save {
                    self.field.set(String::new());
                }
                return None;
            }
            if self.ask.mode == Mode::Save {
                if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
                    if parent.is_dir() {
                        self.set_dir(Place::Local(parent.to_path_buf()));
                        self.field.set(name.to_string_lossy().into_owned());
                        return self.ok();
                    }
                }
            } else if path.is_file() {
                return self.finish(vec![Place::Local(path)], false);
            }
            self.said = Some(format!("Nothing is at {}.", path.display()));
            return None;
        }
        // A remote's place: what is there is asked, and the answer goes on from there.
        if let Some(place) = self.typed_remote(&typed) {
            self.look(place.clone(), Looking::Typed(place));
            return None;
        }
        if control || self.ask.mode == Mode::Save || self.ask.mode == Mode::Folder {
            return self.ok();
        }
        let target = self.targets.get(self.list.cursor).cloned().unwrap_or(Target::Nothing);
        self.target(target)
    }

    /// A remote's answer to [`Chooser::look`], acted on as this machine's would have been.
    fn looked(&mut self, looking: Looking, found: Result<Option<bool>, String>) -> Option<Vec<PathBuf>> {
        self.note = None;
        let found = match found {
            Ok(found) => found,
            Err(why) => {
                let remote = match looking.place() {
                    Place::Remote(remote, _) => remote.clone(),
                    Place::Local(path) => path.display().to_string(),
                };
                self.said = Some(format!("{remote} could not be reached: {why}"));
                return None;
            }
        };
        let saving = self.ask.mode == Mode::Save;
        match (looking, found) {
            // Typed on since: the answer is for another name.
            (Looking::Save(place), _) if self.save_place().as_ref() != Some(&place) => None,
            (Looking::Save(place), Some(true)) => {
                self.go(place);
                None
            }
            (Looking::Save(place), Some(false)) => {
                self.said = Some(format!("{} is here already: press Replace to write over it.", self.field.text.trim()));
                self.replacing = Some(place);
                None
            }
            (Looking::Save(place), None) => self.send_to(place),
            (Looking::Typed(place), Some(true)) => {
                self.go(place);
                if saving {
                    self.field.set(String::new());
                }
                None
            }
            // A file there, to save over: its folder gone to and its name to save under, and
            // asked about as any name is.
            (Looking::Typed(place), Some(false)) if saving => {
                if let Some(folder) = place.parent() {
                    self.set_dir(folder);
                }
                self.field.set(place.name());
                self.ok()
            }
            (Looking::Typed(place), Some(false)) => self.finish(vec![place], false),
            (Looking::Typed(place), None) if saving => {
                match place.parent() {
                    Some(folder) => self.look(folder, Looking::Into(place)),
                    None => self.said = Some(format!("Nothing is at {}.", place.shown())),
                }
                None
            }
            (Looking::Typed(place), None) => {
                self.said = Some(format!("Nothing is at {}.", place.shown()));
                None
            }
            // Its folder is there, and nothing of its name: saved under it.
            (Looking::Into(place), Some(true)) => {
                if let Some(folder) = place.parent() {
                    self.set_dir(folder);
                }
                self.field.set(place.name());
                self.send_to(place)
            }
            (Looking::Into(place), _) => {
                self.said = Some(format!("Nothing is at {}.", place.parent().unwrap_or(place).shown()));
                None
            }
        }
    }

    /// The choice made: paths of this machine at once; a remote's fetched first (`None`
    /// meanwhile: the fetch's end, heard, says).
    fn finish(&mut self, chosen: Vec<Place>, folder: bool) -> Option<Vec<PathBuf>> {
        let Some(first) = chosen.iter().find(|p| p.is_remote()).cloned() else {
            return Some(chosen.into_iter().filter_map(|p| if let Place::Local(path) = p { Some(path) } else { None }).collect());
        };
        let Some(side) = &self.side else {
            self.said = Some("The storage service cannot be asked.".into());
            return None;
        };
        let into = match own::folder(&self.base.join("fetched"), &self.dir) {
            Ok(into) => into,
            Err(e) => {
                self.said = Some(format!("There is no room to fetch it into: {e}"));
                return None;
            }
        };
        side.stop.store(false, Ordering::SeqCst);
        let of = chosen.iter().filter(|p| p.is_remote()).count();
        side.ask(Request::Fetch { chosen, folder, into: into.clone() });
        let from = first.parent().unwrap_or_else(|| first.clone()).shown();
        self.fetching = Some(Fetching { into, from, name: first.name(), index: 1, of, progress: Progress::default(), folder, stopping: false });
        self.said = None;
        None
    }

    /// Saving to a remote: the program is given a path in a folder of the outbox of its own, and
    /// a sender started to move what it writes there on.
    fn send_to(&mut self, place: Place) -> Option<Vec<PathBuf>> {
        let folder = match own::folder(&self.base.join("outbox"), &place.parent().unwrap_or_else(|| place.clone())) {
            Ok(folder) => folder,
            Err(e) => {
                self.said = Some(format!("There is no room to keep it in on its way: {e}"));
                return None;
            }
        };
        let local = folder.join(place.name());
        match (self.sender)(&local, &place) {
            Ok(()) => Some(vec![local]),
            Err(e) => {
                let _ = std::fs::remove_dir_all(&folder);
                let remote = if let Place::Remote(remote, _) = &place { remote.as_str() } else { "the remote" };
                self.said = Some(format!("It could not be sent on to {remote}: {e}"));
                None
            }
        }
    }

    /// Cancel, or Escape, while fetching: the fetch stopped, and the wearer back to choosing.
    fn stop_fetch(&mut self) {
        if let (Some(fetching), Some(side)) = (self.fetching.as_mut(), self.side.as_ref()) {
            side.stop.store(true, Ordering::SeqCst);
            fetching.stopping = true;
        }
    }

    /// The window closing: a fetch under way stopped first, and what it fetched taken away.
    fn abandon(&mut self) {
        let Some(fetching) = self.fetching.take() else { return };
        if let Some(side) = &self.side {
            side.stop.store(true, Ordering::SeqCst);
            let until = Instant::now() + Duration::from_secs(6);
            while let Some(answer) = side.next(until.saturating_duration_since(Instant::now())) {
                if matches!(answer, Answer::Fetched(_)) {
                    break;
                }
            }
        }
        let _ = std::fs::remove_dir_all(&fetching.into);
    }

    /// What the storage service has answered since last asked.
    fn hear(&mut self) -> Heard {
        let Some(answers) = self.side.as_ref().map(Side::heard) else { return Heard::Nothing };
        if answers.is_empty() {
            return Heard::Nothing;
        }
        // How far a fetch has come is drawn from `fetching`: no lines to make again for it.
        if !answers.iter().all(|a| matches!(a, Answer::Fetching { .. })) {
            self.stale = true;
        }
        for answer in answers {
            match answer {
                Answer::Remotes(Ok(remotes)) => self.known = Known::Got(remotes),
                Answer::Remotes(Err(why)) => {
                    log::warn!("the remotes are not to be had: {why}");
                    self.known = Known::Failed(sentence(&why));
                }
                Answer::Listed { generation, result } if generation == self.listing.generation => {
                    self.listing.state = match result {
                        Ok(entries) => Listed::Here(entries),
                        Err(why) => Listed::Failed(why),
                    };
                }
                Answer::Listed { .. } => {}
                Answer::Looked { token, result } => match self.looking.take() {
                    Some((asked, looking)) if asked == token => {
                        if let Some(done) = self.looked(looking, result) {
                            return Heard::Done(done);
                        }
                    }
                    other => self.looking = other,
                },
                Answer::Fetching { index, of, name, progress } => {
                    if let Some(fetching) = self.fetching.as_mut() {
                        (fetching.index, fetching.of, fetching.name, fetching.progress) = (index, of, name, progress);
                    }
                }
                Answer::Fetched(result) => {
                    let Some(fetching) = self.fetching.take() else { continue };
                    match result {
                        Ok(Some(paths)) => return Heard::Done(paths),
                        Ok(None) => self.said = Some("Fetching was stopped: nothing was chosen.".into()),
                        Err(why) => self.said = Some(format!("{} could not be fetched: {why}", fetching.name)),
                    }
                    let _ = std::fs::remove_dir_all(&fetching.into);
                }
            }
        }
        Heard::Changed
    }

    /// What it shows, for dictation and the assistant: the folder's names and what is typed.
    fn description(&self) -> spatiand_app::Description {
        let names: Vec<String> = self
            .targets
            .iter()
            .filter_map(|t| match t {
                Target::Go(p) | Target::File(p) if p.parent().as_ref() == Some(&self.dir) => Some(p.name()),
                _ => None,
            })
            .collect();
        spatiand_app::Description {
            kind: "files".into(),
            app: "Spatiand".into(),
            title: self.title(),
            text: self.field.text.clone(),
            vocabulary: names.into_iter().take(150).collect(),
            directory: self.dir.shown(),
            ..Default::default()
        }
    }

    fn draw(&mut self, p: &mut Painter, gl: &spatiand_app::Gl) {
        if self.stale {
            self.build();
        }
        let page = p.bounds();
        p.fill(gl, page, 0.0, theme::ground());
        let mut inner = page.inset(28.0);
        let mut head = inner.take_top(48.0);
        let mark = head.take_left(52.0);
        let icon = match self.ask.mode {
            Mode::Save => Icon::Download,
            Mode::Folder => Icon::Folder,
            _ => Icon::Document,
        };
        draw_icon(p, gl, icon, Rect::new(mark.x, mark.y + (mark.h - 34.0) * 0.5, 34.0, 34.0), theme::ink_hint());
        p.label(gl, &self.title(), head, theme::TITLE_EM * 0.8, theme::ink(), 0.0);
        if let Some(said) = self.said.as_deref() {
            p.label(gl, said, inner.take_top(36.0), theme::DETAIL_EM, look::trouble(), 0.0);
        } else if let Some(note) = self.note.as_deref() {
            p.label(gl, note, inner.take_top(36.0), theme::DETAIL_EM, theme::ink_hint(), 0.0);
        }
        inner.take_top(8.0);
        if self.fetching.is_some() {
            self.draw_fetching(p, gl, inner);
            return;
        }
        self.field.draw(p, gl, inner.take_top(theme::ROW_HEIGHT));
        inner.take_top(14.0);
        let mut buttons = inner.take_bottom(theme::ROW_HEIGHT);
        inner.take_bottom(14.0);
        let label = self.ok_label();
        // The no on the left, the yes on the right and lit, 12 apart, as a question's two.
        let ok = buttons.take_right(Button::width(p, gl, &label).max(180.0));
        buttons.take_right(12.0);
        let cancel = buttons.take_right(Button::width(p, gl, "Cancel").max(180.0));
        self.ok.draw(p, gl, ok, &label);
        self.cancel.draw(p, gl, cancel, "Cancel");
        let keys = match self.ask.mode {
            Mode::Save => "[Enter] Save  [Esc] Cancel",
            Mode::Folder => "[Enter] Choose this folder  [Esc] Cancel",
            _ => "[Enter] Open  [Backspace] Up  [Esc] Cancel",
        };
        look::keys(p, gl, buttons, keys, 0.0, 0.8);
        self.list.draw(p, gl, inner, &self.rows);
    }

    /// While a remote's are fetched: what, from where, how far, and a Cancel that stops it.
    fn draw_fetching(&mut self, p: &mut Painter, gl: &spatiand_app::Gl, mut inner: Rect) {
        let Some(fetching) = self.fetching.clone() else { return };
        let mut buttons = inner.take_bottom(theme::ROW_HEIGHT);
        let cancel = buttons.take_right(Button::width(p, gl, "Cancel").max(180.0));
        self.cancel.draw(p, gl, cancel, "Cancel");
        if !fetching.stopping {
            look::keys(p, gl, buttons, "[Esc] Stop fetching", 0.0, 0.8);
        }
        inner.take_top(40.0);
        let what = if fetching.of > 1 { format!("Fetching {} ({} of {})", fetching.name, fetching.index, fetching.of) } else { format!("Fetching {}", fetching.name) };
        p.label(gl, &what, inner.take_top(theme::ROW_HEIGHT), theme::ROW_EM, theme::ink(), 0.0);
        p.label(gl, &format!("from {}", fetching.from), inner.take_top(theme::DETAIL_EM * 1.8), theme::DETAIL_EM, theme::ink_detail(), 0.0);
        inner.take_top(30.0);
        let bar = inner.take_top(10.0);
        p.fill(gl, bar, 5.0, theme::scroll_track());
        let Progress { done, total, file } = fetching.progress;
        if total > 0 {
            p.fill(gl, Rect { w: bar.w * (done as f32 / total as f32).min(1.0), ..bar }, 5.0, look::TEAL);
        }
        inner.take_top(18.0);
        let amount = if fetching.stopping {
            "Stopping\u{2026}".to_string()
        } else if total > 0 {
            let mut amount = format!("{} of {}", look::size(done), look::size(total));
            // A folder's: the file it is at, too.
            if let Some(file) = file.filter(|_| fetching.folder) {
                amount += &format!("    {file}");
            }
            amount
        } else {
            "Starting\u{2026}".to_string()
        };
        p.label(gl, &amount, inner.take_top(theme::DETAIL_EM * 1.8), theme::DETAIL_EM, theme::ink_detail(), 0.0);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The sender, started by a chooser that saved to a remote: no window, a log of its own.
    if args.first().map(String::as_str) == Some("--send") {
        let code = match args.as_slice() {
            [_, file, to] => send::run(Path::new(file), to),
            _ => {
                eprintln!("spatiand-choose: --send <file> <remote:path>");
                2
            }
        };
        std::process::exit(code);
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let base = own::base();
    let ask = match ask_from_args(&args, &base) {
        Ok(ask) => ask,
        Err(e) => {
            eprintln!("spatiand-choose: {e}");
            std::process::exit(2);
        }
    };
    match run(ask, base) {
        Ok(Some(paths)) => {
            for path in paths {
                println!("{}", path.display());
            }
        }
        Ok(None) => std::process::exit(1),
        Err(e) => {
            eprintln!("spatiand-choose: {e}");
            std::process::exit(1);
        }
    }
}

fn run(ask: Ask, base: PathBuf) -> Result<Option<Vec<PathBuf>>, String> {
    let mut app = App::connect("id.prasetya.spatiand.choose")?;
    let mut chooser = Chooser::new(ask, base.clone(), Box::new(send::start));
    // What earlier choosers left, taken away meanwhile.
    std::thread::spawn(move || own::tidy(&base, SystemTime::now()));
    match Side::start(Arc::new(remote::Service::default())) {
        Ok(side) => {
            app.watch(side.fd());
            chooser.connect(side);
        }
        Err(e) => {
            log::warn!("the remotes cannot be asked for: {e}");
            chooser.known = Known::Failed(sentence(&format!("the remotes cannot be asked for: {e}")));
        }
    }
    let window = app.open(&chooser.title(), (1100, 760));
    let mut painter: Option<Painter> = None;
    let mut hover = (-1.0f32, -1.0f32);
    loop {
        let mut dirty = false;
        for event in app.wait(None)? {
            // Every event is drawn after, as the SDK's gallery does: the window's first picture
            // (which shows it) comes from the first, which is not input.
            dirty = true;
            match &event {
                // Closed, or Command-Q or Control-Q (the SDK's): quitting a chooser is its Cancel.
                Event::CloseRequested { .. } | Event::Quit { .. } => {
                    app.close(window);
                    chooser.abandon();
                    return Ok(None);
                }
                Event::Resized { width, height, .. } => app.resize(window, (*width, *height)),
                Event::Describe { window } => {
                    let description = chooser.description();
                    app.describe(*window, &description);
                    continue;
                }
                _ => {}
            }
            let Some(input) = Input::of(&event, 1.0) else { continue };
            // Fetching: only its Cancel, or Escape, which stop it.
            if chooser.fetching.is_some() {
                match input {
                    Input::Pointer { x, y } => {
                        hover = (x, y);
                        chooser.cancel.input(&input);
                    }
                    Input::PointerLeft => {
                        hover = (-1.0, -1.0);
                        chooser.cancel.input(&input);
                    }
                    Input::Press { .. } => {
                        if chooser.cancel.input(&input) {
                            chooser.stop_fetch();
                        }
                    }
                    Input::Key { key: Key::Escape, .. } => chooser.stop_fetch(),
                    _ => {}
                }
                continue;
            }
            let done = match input {
                Input::Pointer { x, y } => {
                    hover = (x, y);
                    chooser.ok.input(&input);
                    chooser.cancel.input(&input);
                    chooser.list.input(&input);
                    None
                }
                Input::PointerLeft => {
                    hover = (-1.0, -1.0);
                    chooser.ok.input(&input);
                    chooser.cancel.input(&input);
                    chooser.list.input(&input);
                    None
                }
                Input::Press { .. } => {
                    let ok = chooser.ok.input(&input);
                    let cancel = chooser.cancel.input(&input);
                    if ok {
                        chooser.ok()
                    } else if cancel {
                        return Ok(None);
                    } else if chooser.ok.contains(hover.0, hover.1) || chooser.cancel.contains(hover.0, hover.1) {
                        None
                    } else if let Some(index) = chooser.list.input(&input) {
                        let target = chooser.targets.get(index).cloned().unwrap_or(Target::Nothing);
                        chooser.target(target)
                    } else {
                        None
                    }
                }
                Input::Scroll { .. } => {
                    chooser.list.input(&input);
                    None
                }
                Input::Key { key, text, modifiers, letter } => match key {
                    Key::Escape => return Ok(None),
                    Key::Up | Key::Down | Key::PageUp | Key::PageDown => {
                        chooser.list.input(&Input::Key { key, text, modifiers, letter });
                        // Moved by the wearer: not moved again when a folder's lines come.
                        chooser.to_folder = false;
                        None
                    }
                    Key::Backspace if chooser.field.text.is_empty() => {
                        if let Some(dir) = chooser.dir.parent() {
                            chooser.go(dir);
                        }
                        None
                    }
                    Key::Enter => chooser.enter(modifiers.control || modifiers.logo),
                    _ => {
                        let before = chooser.field.text.clone();
                        chooser.field.input(&Input::Key { key, text, modifiers, letter });
                        if chooser.field.text != before {
                            chooser.replacing = None;
                            if chooser.ask.mode != Mode::Save {
                                chooser.stale = true;
                                chooser.to_folder = true;
                            }
                        }
                        None
                    }
                },
                _ => None,
            };
            if done.is_some() {
                return Ok(done);
            }
        }
        match chooser.hear() {
            Heard::Nothing => {}
            Heard::Changed => dirty = true,
            Heard::Done(paths) => return Ok(Some(paths)),
        }
        if !dirty {
            continue;
        }
        app.draw(window, |gl, size| {
            let p = painter.get_or_insert_with(|| Painter::new(gl).expect("the painter"));
            p.begin(gl, size, 1.0, theme::ground());
            chooser.draw(p, gl);
            p.end(gl);
        })?;
        // Lines that moved under a pointer resting on them (the remotes come in over the folder's
        // lines): the line under it is the one drawn there now.
        if hover.0 >= 0.0 {
            chooser.list.input(&Input::Pointer { x: hover.0, y: hover.1 });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::stand_in::Folder;
    use std::sync::Mutex;

    fn ask(mode: Mode, dir: &Path) -> Ask {
        Ask { mode, title: String::new(), dir: Place::Local(dir.to_path_buf()), name: "page.html".into(), accept: vec!["png".into()] }
    }

    fn local(path: PathBuf) -> Place {
        Place::Local(path)
    }

    /// A chooser with its own folders in `base`, and no sending.
    fn chooser_at(ask: Ask, base: PathBuf) -> Chooser {
        Chooser::new(ask, base, Box::new(|_, _| Err("nothing is sent in the tests".into())))
    }

    fn chooser(ask: Ask) -> Chooser {
        let base = std::env::temp_dir().join(format!("spatiand-choose-base-{}", std::process::id()));
        chooser_at(ask, base)
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spatiand-choose-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        std::fs::write(dir.join("a.png"), b"x").unwrap();
        std::fs::write(dir.join("b.txt"), b"x").unwrap();
        dir
    }

    #[test]
    fn opening_lists_the_kinds_asked_for_and_says_what_it_hides() {
        let dir = scratch("open");
        let mut c = chooser(ask(Mode::Open, &dir));
        c.build();
        assert!(c.targets.contains(&Target::File(local(dir.join("a.png")))));
        assert!(!c.targets.contains(&Target::File(local(dir.join("b.txt")))));
        assert!(c.targets.contains(&Target::Go(local(dir.join("folder")))));
        assert!(c.targets.contains(&Target::ShowAll));
        assert_eq!(c.target(Target::File(local(dir.join("a.png")))), Some(vec![dir.join("a.png")]));
    }

    #[test]
    fn several_are_marked_then_chosen() {
        let dir = scratch("multi");
        let mut c = chooser(Ask { accept: Vec::new(), ..ask(Mode::OpenMultiple, &dir) });
        c.build();
        assert_eq!(c.target(Target::File(local(dir.join("a.png")))), None);
        assert_eq!(c.target(Target::File(local(dir.join("b.txt")))), None);
        assert_eq!(c.ok(), Some(vec![dir.join("a.png"), dir.join("b.txt")]));
    }

    #[test]
    fn saving_asks_before_writing_over_a_file() {
        let dir = scratch("save");
        let mut c = chooser(ask(Mode::Save, &dir));
        assert_eq!(c.ok(), Some(vec![dir.join("page.html")]), "a new name is saved under at once");
        c.field.set("b.txt".to_string());
        assert_eq!(c.ok(), None, "told first");
        assert_eq!(c.ok(), Some(vec![dir.join("b.txt")]), "then replaced");
        c.field.set(format!("{}/folder/new.txt", dir.display()));
        assert_eq!(c.enter(false), Some(vec![dir.join("folder").join("new.txt")]), "a path typed");
    }

    #[test]
    fn it_opens_on_the_folder_and_enter_takes_what_was_found() {
        let dir = scratch("find");
        let mut c = chooser(Ask { accept: Vec::new(), ..ask(Mode::Open, &dir) });
        c.build();
        assert_eq!(c.targets[c.list.cursor], Target::Go(local(dir.join("folder"))), "the folder's first line, not Places");
        c.field.set("b.t".to_string());
        c.stale = true;
        c.to_folder = true;
        c.build();
        assert_eq!(c.enter(false), Some(vec![dir.join("b.txt")]));
    }

    #[test]
    fn a_folder_is_the_one_it_is_at() {
        let dir = scratch("folder");
        let mut c = chooser(ask(Mode::Folder, &dir));
        c.build();
        assert!(!c.targets.iter().any(|t| matches!(t, Target::File(_))), "folders only");
        c.target(Target::Go(local(dir.join("folder"))));
        assert_eq!(c.ok(), Some(vec![dir.join("folder")]));
    }

    // --- remotes, with a stand-in for the storage service: a folder of this machine ---

    /// A scratch folder, taken away when the test is over.
    struct Gone(PathBuf);

    impl Drop for Gone {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `local/` as in [`scratch`], and `remote/`, the stand-in remote "Fake"'s files.
    fn with_remote_files(name: &str) -> (PathBuf, Gone) {
        let dir = scratch(name);
        let remote = dir.join("remote");
        std::fs::create_dir_all(remote.join("Docs/sub")).unwrap();
        std::fs::create_dir_all(remote.join("album/inner")).unwrap();
        std::fs::write(remote.join("Docs/report.pdf"), b"%PDF report").unwrap();
        std::fs::write(remote.join("Docs/notes.txt"), b"some notes").unwrap();
        std::fs::write(remote.join("Docs/.hidden"), b"").unwrap();
        std::fs::write(remote.join("album/one.png"), b"one").unwrap();
        std::fs::write(remote.join("album/inner/two.png"), b"two").unwrap();
        std::fs::write(remote.join("photo.png"), b"photo").unwrap();
        (dir.clone(), Gone(dir))
    }

    /// A chooser asking the stand-in, its own folders in `dir/base`, its sends kept in `sent`.
    fn with_remote(ask: Ask, dir: &Path, stand_in: Folder, sent: Arc<Mutex<Vec<(PathBuf, Place)>>>) -> Chooser {
        let sender: Sender = Box::new(move |file, to| {
            sent.lock().unwrap().push((file.to_path_buf(), to.clone()));
            Ok(())
        });
        let mut c = Chooser::new(ask, dir.join("base"), sender);
        c.connect(Side::start(Arc::new(stand_in)).unwrap());
        c
    }

    fn fake(dir: &Path) -> Folder {
        Folder::new("Fake", &dir.join("remote"))
    }

    /// The service's answers heard until `until` holds, or the choice is made (its paths).
    fn settle(c: &mut Chooser, what: &str, until: impl Fn(&Chooser) -> bool) -> Option<Vec<PathBuf>> {
        let started = Instant::now();
        loop {
            if let Heard::Done(paths) = c.hear() {
                return Some(paths);
            }
            if until(c) {
                c.build();
                return None;
            }
            assert!(started.elapsed() < Duration::from_secs(10), "{what}: not in ten seconds");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn listed(c: &Chooser) -> bool {
        matches!(c.listing.state, Listed::Here(_) | Listed::Failed(_))
    }

    #[test]
    fn the_remotes_come_under_the_places_and_the_cursor_stays_where_it_was() {
        let (dir, _gone) = with_remote_files("remotes");
        let mut c = with_remote(Ask { accept: Vec::new(), ..ask(Mode::Open, &dir) }, &dir, fake(&dir), Default::default());
        c.build();
        let before = c.targets[c.list.cursor].clone();
        assert_eq!(before, Target::Go(local(dir.join("folder"))));
        assert!(!c.rows.iter().any(|r| r.label == "Remotes"), "nothing until the service has said");
        settle(&mut c, "the remotes", |c| matches!(c.known, Known::Got(_)));
        let at = c.targets.iter().position(|t| *t == Target::Go(Place::remote("Fake", ""))).expect("the remote's line");
        assert!(c.rows[at - 1].heading && c.rows[at - 1].label == "Remotes");
        assert_eq!(c.rows[at].icon, Some(Icon::Folder), "an alias is a folder, as in SpatiShelf");
        let heading = c.rows.iter().position(|r| r.heading && r.label == dir.display().to_string()).unwrap();
        assert!(heading > at, "the folder's lines after the remotes");
        assert_eq!(c.targets[c.list.cursor], before, "still on the line it was on");
        assert_eq!(remote_icon("smb"), Icon::Server);
        assert_eq!(remote_icon("drive"), Icon::Cloud);
    }

    #[test]
    fn a_service_not_reached_is_one_quiet_line() {
        let (dir, _gone) = with_remote_files("unreached");
        let mut stand_in = fake(&dir);
        stand_in.unreachable = Some("the storage service did not answer".into());
        let mut c = with_remote(ask(Mode::Open, &dir), &dir, stand_in, Default::default());
        settle(&mut c, "the refusal", |c| matches!(c.known, Known::Failed(_)));
        let at = c.rows.iter().position(|r| r.label == "Remotes").unwrap();
        assert!(c.rows[at + 1].heading, "quiet, and never chosen");
        assert_eq!(c.rows[at + 1].label, "The storage service did not answer.");
        assert!(!c.targets.iter().any(|t| matches!(t, Target::Go(Place::Remote(..)))));
        // This machine's places work as ever.
        assert!(c.targets.contains(&Target::File(local(dir.join("a.png")))));
    }

    #[test]
    fn a_remote_folder_is_listed_away_from_the_window_and_shown_as_a_folder_here_is() {
        let (dir, _gone) = with_remote_files("listed");
        let mut c = with_remote(Ask { accept: vec!["pdf".into()], ..ask(Mode::Open, &dir) }, &dir, fake(&dir), Default::default());
        settle(&mut c, "the remotes", |c| matches!(c.known, Known::Got(_)));
        c.target(Target::Go(Place::remote("Fake", "Docs")));
        c.build();
        let loading = c.rows.iter().position(|r| r.label == "Loading\u{2026}").expect("said while it is listed");
        assert_eq!(c.list.cursor, loading, "Enter on it does nothing");
        assert_eq!(c.enter(false), None);
        settle(&mut c, "the listing", listed);
        let row = |c: &Chooser, target: &Target| c.targets.iter().position(|t| t == target).map(|i| c.rows[i].clone());
        assert_eq!(c.targets[c.list.cursor], Target::Go(Place::remote("Fake", "Docs/sub")), "the folder's first line once it came");
        let report = row(&c, &Target::File(Place::remote("Fake", "Docs/report.pdf"))).expect("a kind asked for");
        assert_eq!(report.trailing.as_deref(), Some("11 bytes"), "its size, from the listing");
        assert!(row(&c, &Target::File(Place::remote("Fake", "Docs/notes.txt"))).is_none(), "not a kind asked for");
        assert!(!c.rows.iter().any(|r| r.label == ".hidden"));
        assert!(c.targets.contains(&Target::ShowAll));
        let up = c.rows.iter().position(|r| r.label == "\u{2039} Up").unwrap();
        assert_eq!((c.rows[up].trailing.as_deref(), &c.targets[up]), (Some("Fake"), &Target::Go(Place::remote("Fake", ""))));
        assert!(c.rows.iter().any(|r| r.heading && r.label == "Fake:Docs"));
        // Shown all, found in.
        c.target(Target::ShowAll);
        c.field.set("note".to_string());
        c.stale = true;
        c.to_folder = true;
        c.build();
        assert_eq!(c.targets[c.list.cursor], Target::File(Place::remote("Fake", "Docs/notes.txt")));
        // Up, to the remote's top, where there is no Up.
        c.go(Place::remote("Fake", "Docs").parent().unwrap());
        settle(&mut c, "the top", listed);
        assert!(!c.rows.iter().any(|r| r.label == "\u{2039} Up"));
        assert!(c.targets.contains(&Target::Go(Place::remote("Fake", "album"))));
    }

    #[test]
    fn what_is_found_in_a_remote_is_fetched_and_its_path_here_given() {
        let (dir, _gone) = with_remote_files("fetch");
        let mut c = with_remote(Ask { accept: Vec::new(), dir: Place::remote("Fake", "Docs"), ..ask(Mode::Open, &dir) }, &dir, fake(&dir), Default::default());
        settle(&mut c, "the listing", listed);
        c.field.set("note".to_string());
        c.stale = true;
        c.to_folder = true;
        c.build();
        assert_eq!(c.enter(false), None, "fetched first");
        assert!(c.fetching.is_some());
        let paths = settle(&mut c, "the fetch", |_| false).unwrap();
        assert_eq!(paths.len(), 1);
        assert!(paths[0].starts_with(dir.join("base/fetched")), "{paths:?}");
        assert_eq!(paths[0].file_name().unwrap(), "notes.txt");
        assert_eq!(std::fs::read(&paths[0]).unwrap(), b"some notes");
        // The program asking to begin where that was begins in the remote.
        let home = home();
        assert_eq!(own::stands_for(paths[0].parent().unwrap(), &dir.join("base"), &home), Some(Place::remote("Fake", "Docs")));
        assert_eq!(starting_place(Some(&paths[0].parent().unwrap().display().to_string()), &dir.join("base")), Place::remote("Fake", "Docs"));
        assert_eq!(starting_place(Some("Fake:album"), &dir.join("base")), Place::remote("Fake", "album"));
        assert_eq!(starting_place(Some(&dir.display().to_string()), &dir.join("base")), local(dir.clone()));
        assert_eq!(starting_place(Some("/no/such/folder"), &dir.join("base")), local(home));
    }

    #[test]
    fn several_marked_here_and_in_a_remote_are_given_in_their_order() {
        let (dir, _gone) = with_remote_files("several");
        let mut c = with_remote(Ask { accept: Vec::new(), ..ask(Mode::OpenMultiple, &dir) }, &dir, fake(&dir), Default::default());
        c.build();
        c.target(Target::File(local(dir.join("a.png"))));
        c.go(Place::remote("Fake", ""));
        settle(&mut c, "the top", listed);
        c.target(Target::File(Place::remote("Fake", "photo.png")));
        c.go(Place::remote("Fake", "album"));
        settle(&mut c, "the album", listed);
        c.target(Target::File(Place::remote("Fake", "album/one.png")));
        c.build();
        let marked = c.targets.iter().position(|t| *t == Target::File(Place::remote("Fake", "album/one.png"))).unwrap();
        assert!(c.rows[marked].trailing.as_deref().unwrap().starts_with('\u{2713}'), "marked");
        assert_eq!(c.ok_label(), "Choose 3");
        assert_eq!(c.ok(), None);
        let paths = settle(&mut c, "the fetch", |_| false).unwrap();
        assert_eq!(paths[0], dir.join("a.png"), "this machine's as it is");
        assert_eq!(std::fs::read(&paths[1]).unwrap(), b"photo");
        assert_eq!(std::fs::read(&paths[2]).unwrap(), b"one");
        assert_eq!(paths[1].parent(), paths[2].parent(), "beside each other");
    }

    #[test]
    fn a_remote_folder_is_chosen_with_all_that_is_in_it() {
        let (dir, _gone) = with_remote_files("whole");
        let mut c = with_remote(Ask { dir: Place::remote("Fake", "album"), ..ask(Mode::Folder, &dir) }, &dir, fake(&dir), Default::default());
        settle(&mut c, "the listing", listed);
        assert!(!c.targets.iter().any(|t| matches!(t, Target::File(_))), "folders only");
        assert!(c.targets.contains(&Target::Go(Place::remote("Fake", "album/inner"))));
        assert_eq!(c.enter(false), None);
        let paths = settle(&mut c, "the fetch", |_| false).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].file_name().unwrap(), "album");
        assert_eq!(std::fs::read(paths[0].join("inner/two.png")).unwrap(), b"two");
        assert_eq!(std::fs::read(paths[0].join("one.png")).unwrap(), b"one");
    }

    #[test]
    fn a_fetch_stopped_leaves_nothing_behind_and_the_wearer_choosing() {
        let (dir, _gone) = with_remote_files("stopped");
        let mut slow = fake(&dir);
        slow.slow = true;
        let mut c = with_remote(Ask { accept: Vec::new(), dir: Place::remote("Fake", "Docs"), ..ask(Mode::Open, &dir) }, &dir, slow, Default::default());
        settle(&mut c, "the listing", listed);
        assert_eq!(c.target(Target::File(Place::remote("Fake", "Docs/notes.txt"))), None);
        settle(&mut c, "some of it", |c| c.fetching.as_ref().is_some_and(|f| f.progress.done > 2));
        let into = c.fetching.as_ref().unwrap().into.clone();
        assert!(into.is_dir());
        c.stop_fetch();
        assert_eq!(settle(&mut c, "the stop", |c| c.fetching.is_none()), None);
        assert_eq!(c.said.as_deref(), Some("Fetching was stopped: nothing was chosen."));
        assert!(!into.exists(), "what was fetched taken away");
        assert!(c.targets.contains(&Target::File(Place::remote("Fake", "Docs/notes.txt"))), "back to choosing");
    }

    #[test]
    fn the_window_closed_while_fetching_stops_it_and_takes_away_what_was_fetched() {
        let (dir, _gone) = with_remote_files("closed");
        let mut slow = fake(&dir);
        slow.slow = true;
        let mut c = with_remote(Ask { accept: Vec::new(), dir: Place::remote("Fake", "Docs"), ..ask(Mode::Open, &dir) }, &dir, slow, Default::default());
        settle(&mut c, "the listing", listed);
        assert_eq!(c.target(Target::File(Place::remote("Fake", "Docs/notes.txt"))), None);
        settle(&mut c, "some of it", |c| c.fetching.as_ref().is_some_and(|f| f.progress.done > 2));
        let into = c.fetching.as_ref().unwrap().into.clone();
        let started = Instant::now();
        c.abandon();
        assert!(started.elapsed() < Duration::from_secs(3), "the stop waited for, not the whole fetch");
        assert!(c.fetching.is_none() && !into.exists());
    }

    #[test]
    fn saving_to_a_remote_asks_before_writing_over_and_goes_by_the_outbox() {
        let (dir, _gone) = with_remote_files("saving");
        let sent: Arc<Mutex<Vec<(PathBuf, Place)>>> = Default::default();
        let mut c = with_remote(Ask { dir: Place::remote("Fake", "Docs"), ..ask(Mode::Save, &dir) }, &dir, fake(&dir), sent.clone());
        settle(&mut c, "the listing", listed);
        // A new name: the outbox's path, and the sender started for it.
        assert_eq!(c.ok(), None, "asked first");
        assert!(c.note.as_deref().unwrap().starts_with("Looking in Fake:Docs"));
        let paths = settle(&mut c, "the look", |_| false).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].file_name().unwrap(), "page.html");
        assert_eq!(paths[0].parent().unwrap().parent().unwrap(), dir.join("base/outbox"));
        assert_eq!(*sent.lock().unwrap(), vec![(paths[0].clone(), Place::remote("Fake", "Docs/page.html"))]);
        // A name taken: told, then replaced.
        let mut c = with_remote(Ask { dir: Place::remote("Fake", "Docs"), name: "report.pdf".into(), ..ask(Mode::Save, &dir) }, &dir, fake(&dir), sent.clone());
        assert_eq!(c.ok(), None);
        assert_eq!(settle(&mut c, "the look", |c| c.looking.is_none()), None);
        assert_eq!(c.said.as_deref(), Some("report.pdf is here already: press Replace to write over it."));
        assert_eq!(c.ok_label(), "Replace");
        let paths = c.ok().expect("replaced, asked no more");
        assert_eq!(sent.lock().unwrap().last().unwrap(), &(paths[0].clone(), Place::remote("Fake", "Docs/report.pdf")));
        // A folder's name: gone into, as here.
        c.field.set("sub".to_string());
        assert_eq!(c.ok(), None);
        assert_eq!(settle(&mut c, "the look", |c| c.looking.is_none()), None);
        assert_eq!(c.dir, Place::remote("Fake", "Docs/sub"));
        // Typed on while it was asked: the answer for the old name is let go of.
        c.field.set("x.txt".to_string());
        assert_eq!(c.ok(), None);
        c.field.set("y.txt".to_string());
        assert_eq!(settle(&mut c, "the look", |c| c.looking.is_none()), None);
        assert_eq!(c.said, None);
        assert_eq!(sent.lock().unwrap().len(), 2, "nothing sent for a name no longer typed");
    }

    #[test]
    fn a_remote_s_place_typed_is_gone_to_or_chosen_or_saved_under() {
        let (dir, _gone) = with_remote_files("typed");
        let sent: Arc<Mutex<Vec<(PathBuf, Place)>>> = Default::default();
        let mut c = with_remote(Ask { accept: Vec::new(), ..ask(Mode::Open, &dir) }, &dir, fake(&dir), sent.clone());
        settle(&mut c, "the remotes", |c| matches!(c.known, Known::Got(_)));
        // A remote that is not there: words to find, as before.
        c.field.set("Nowhere:x".to_string());
        assert_eq!(c.filter(), "nowhere:x");
        c.field.set("Fake:album".to_string());
        assert_eq!(c.filter(), "", "a place, not words to find");
        assert_eq!(c.enter(false), None);
        settle(&mut c, "the look", |c| c.looking.is_none());
        assert_eq!(c.dir, Place::remote("Fake", "album"));
        assert_eq!(c.field.text, "");
        c.field.set("Fake:album/one.png".to_string());
        assert_eq!(c.enter(false), None);
        let paths = settle(&mut c, "the file", |_| false).unwrap();
        assert_eq!(std::fs::read(&paths[0]).unwrap(), b"one");
        c.field.set("Fake:nothing/here.png".to_string());
        assert_eq!(c.enter(false), None);
        settle(&mut c, "the look", |c| c.looking.is_none());
        assert_eq!(c.said.as_deref(), Some("Nothing is at Fake:nothing/here.png."));
        // Saving: a new name in a folder that is there; a folder that is not.
        let mut c = with_remote(ask(Mode::Save, &dir), &dir, fake(&dir), sent.clone());
        settle(&mut c, "the remotes", |c| matches!(c.known, Known::Got(_)));
        c.field.set("Fake:Docs/new.txt".to_string());
        assert_eq!(c.enter(false), None);
        let paths = settle(&mut c, "the looks", |_| false).unwrap();
        assert_eq!(paths[0].file_name().unwrap(), "new.txt");
        assert_eq!(sent.lock().unwrap().last().unwrap().1, Place::remote("Fake", "Docs/new.txt"));
        assert_eq!(c.dir, Place::remote("Fake", "Docs"));
        c.field.set("Fake:Gone/new.txt".to_string());
        assert_eq!(c.enter(false), None);
        settle(&mut c, "the looks", |c| c.looking.is_none() && c.said.is_some());
        assert_eq!(c.said.as_deref(), Some("Nothing is at Fake:Gone."));
    }

    /// The real storage service, on a remote of rclone's own kind `alias` over a scratch folder:
    /// listed, fetched from (a file, slowed so that how far it has come is seen; a folder),
    /// asked about a name, and saved to through the outbox with a browser's download written
    /// beside it and renamed. Its own runtime folder and remotes file: nobody else's service or
    /// remotes are touched. Run alone, as it sets the environment the service is found by:
    ///
    /// ```text
    /// SPATIAND_STORAGE_PROGRAM=/path/to/spatiand-storage \
    ///     cargo test -p spatiand-choose -- --ignored with_the_real_storage_service
    /// ```
    #[test]
    #[ignore = "needs the storage service built: SPATIAND_STORAGE_PROGRAM"]
    fn with_the_real_storage_service() {
        let Some(program) = std::env::var_os("SPATIAND_STORAGE_PROGRAM").map(PathBuf::from) else {
            eprintln!("SPATIAND_STORAGE_PROGRAM is not set: nothing tried");
            return;
        };
        let (dir, _gone) = with_remote_files("real");
        let big: Vec<u8> = (0..4_000_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        std::fs::write(dir.join("remote/Docs/film.bin"), &big).unwrap();
        // Short: a socket's path has to fit in it.
        let runtime = PathBuf::from(format!("/tmp/spc-{}", std::process::id()));
        std::fs::create_dir_all(&runtime).unwrap();
        let conf = dir.join("remotes.conf");
        std::fs::write(&conf, format!("[scratch]\ntype = alias\nremote = {}\n", dir.join("remote").display())).unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", &runtime);
        std::env::set_var("SPATIAND_STORAGE_CONFIG", &conf);
        std::env::set_var("SPATIAND_APPS_DIR", program.parent().unwrap());
        let service = Arc::new(remote::Service::default());
        let stop_service = || {
            let place = std::fs::read_to_string(runtime.join("spatiand-storage.json")).unwrap_or_default();
            let pid = place.split("\"pid\":").nth(1).and_then(|p| p.split(|c: char| !c.is_ascii_digit()).next()).and_then(|p| p.parse::<i32>().ok());
            if let Some(pid) = pid {
                unsafe { libc::kill(pid, libc::SIGTERM) };
            }
        };
        let check = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            use remote::Remotes;
            assert_eq!(service.remotes().unwrap(), vec![Remote { name: "scratch".into(), kind: "alias".into() }]);
            // A file, slowed to two megabytes a second: how far it has come, told as it goes.
            service.with(|s| s.rc("core/bwlimit", serde_json::json!({"rate": "2M"}))).unwrap();
            let open = Ask { accept: Vec::new(), dir: Place::remote("scratch", "Docs"), ..ask(Mode::Open, &dir) };
            let mut c = Chooser::new(open, dir.join("base"), Box::new(|_, _| Err("not here".into())));
            c.connect(Side::start(service.clone()).unwrap());
            settle(&mut c, "the listing", listed);
            let film = Target::File(Place::remote("scratch", "Docs/film.bin"));
            let at = c.targets.iter().position(|t| *t == film).expect("listed");
            assert_eq!(c.rows[at].trailing.as_deref(), Some("4.0 MB"));
            assert!(!c.targets.contains(&Target::File(Place::remote("scratch", "Docs/.hidden"))));
            assert_eq!(c.target(film), None);
            settle(&mut c, "part of it", |c| c.fetching.as_ref().is_some_and(|f| f.progress.done > 0 && f.progress.done < f.progress.total));
            let told = c.fetching.as_ref().unwrap().progress.clone();
            assert_eq!(told.total, 4_000_000, "{told:?}");
            let paths = settle(&mut c, "the fetch", |_| false).unwrap();
            assert_eq!(std::fs::read(&paths[0]).unwrap(), big, "byte for byte");
            service.with(|s| s.rc("core/bwlimit", serde_json::json!({"rate": "off"}))).unwrap();
            // A folder, whole.
            let folder = Ask { dir: Place::remote("scratch", "album"), ..ask(Mode::Folder, &dir) };
            let mut c = Chooser::new(folder, dir.join("base"), Box::new(|_, _| Err("not here".into())));
            c.connect(Side::start(service.clone()).unwrap());
            settle(&mut c, "the listing", listed);
            assert_eq!(c.ok(), None);
            let paths = settle(&mut c, "the folder", |_| false).unwrap();
            assert_eq!(std::fs::read(paths[0].join("inner/two.png")).unwrap(), b"two");
            assert_eq!(std::fs::read(paths[0].join("one.png")).unwrap(), b"one");
            // Saving: a name taken is asked about; a new one goes by the outbox.
            let sent: Arc<Mutex<Vec<(PathBuf, Place)>>> = Default::default();
            let kept = sent.clone();
            let save = Ask { dir: Place::remote("scratch", "Docs"), name: "report.pdf".into(), ..ask(Mode::Save, &dir) };
            let mut c = Chooser::new(save, dir.join("base"), Box::new(move |file, to| {
                kept.lock().unwrap().push((file.to_path_buf(), to.clone()));
                Ok(())
            }));
            c.connect(Side::start(service.clone()).unwrap());
            assert_eq!(c.ok(), None);
            settle(&mut c, "the look", |c| c.looking.is_none());
            assert_eq!(c.ok_label(), "Replace");
            c.field.set("download.bin".to_string());
            assert_eq!(c.ok(), None);
            let paths = settle(&mut c, "the look", |_| false).unwrap();
            let (file, to) = sent.lock().unwrap()[0].clone();
            assert_eq!((&file, &to), (&paths[0], &Place::remote("scratch", "Docs/download.bin")));
            // The browser: its download written in parts beside the name, then renamed to it.
            let part = file.with_file_name("download.bin.crdownload");
            let writing = {
                let (part, file, big) = (part.clone(), file.clone(), big.clone());
                std::thread::spawn(move || {
                    let mut out = std::fs::File::create(&part).unwrap();
                    for chunk in big.chunks(1_000_000) {
                        std::io::Write::write_all(&mut out, chunk).unwrap();
                        std::thread::sleep(Duration::from_millis(700));
                    }
                    drop(out);
                    std::fs::rename(&part, &file).unwrap();
                })
            };
            let folder = file.parent().unwrap();
            assert_eq!(send::wait(folder, "download.bin", &send::Rules::default()), send::Waited::Ready);
            writing.join().unwrap();
            send::send(&*service, folder, "download.bin", &to, &send::PAUSES).unwrap();
            assert_eq!(std::fs::read(dir.join("remote/Docs/download.bin")).unwrap(), big, "the remote has it, byte for byte");
            assert!(!file.exists(), "moved, not copied");
        }));
        stop_service();
        let _ = std::fs::remove_dir_all(&runtime);
        let _ = std::fs::remove_dir_all(&dir);
        if let Err(e) = check {
            std::panic::resume_unwind(e);
        }
    }

    #[test]
    fn errors_are_said_as_lines() {
        assert_eq!(sentence("the storage service did not answer"), "The storage service did not answer.");
        assert_eq!(sentence("Refused!"), "Refused!");
    }
}
