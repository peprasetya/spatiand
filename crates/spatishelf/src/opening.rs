//! Opening things: with what a double click opens them with, or with one chosen from "Open
//! with" (`openwith.rs`: the applications of this machine for the kind, Spatiand's own among
//! them; "Other..."; "always open this kind with it").

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use spatiand_app::ui::{Icon, Row, Texture};

use crate::openwith::{self, Always, Choice};
use crate::proto::{Item, Kind, Op, Place, ToTab};
use crate::{detached, own_program, Action, Done, Shelf};

/// Spatiand's own applications for a kind of thing, the first the one a double click uses.
pub fn own_for(kind: Kind) -> Vec<&'static str> {
    let all: &[&'static str] = match kind {
        Kind::Picture | Kind::Pdf | Kind::Model => &["spatiloupe"],
        Kind::Film | Kind::Music => &["spatireel"],
        Kind::Folder => &["spatiquill"],
        _ => &[],
    };
    all.iter().copied().filter(|p| own_program(p).is_some()).collect()
}

fn own_name(program: &str) -> String {
    match program {
        "spatiloupe" => "SpatiLoupe".into(),
        "spatireel" => "SpatiReel".into(),
        "spatiquill" => "SpatiQuill".into(),
        other => other.into(),
    }
}

fn own_icon(program: &str) -> Icon {
    match program {
        "spatireel" => Icon::Film,
        "spatiquill" => Icon::Terminal,
        _ => Icon::Eye,
    }
}

/// What "Open with" knows: the system's applications last found, their pictures, what is
/// remembered, and what waits to be opened once it has been fetched from a remote.
pub struct Opening {
    pub always: Always,
    /// The next application chosen is to open this kind always.
    pub always_next: bool,
    /// The kind of thing the open menu is for, and how it is said.
    pub kind: Option<(String, String)>,
    pub known: HashMap<String, openwith::App>,
    pub pictures: HashMap<String, Option<Texture>>,
    /// Pictures made on the worker, to be put on the GPU when next drawn.
    pub arrived: Vec<(String, Option<(u32, u32, Vec<u8>)>)>,
    /// Fetches from a remote, by their job, and what to open them with once done.
    pub after: HashMap<u64, (Choice, Vec<String>)>,
}

impl Opening {
    pub fn new() -> Opening {
        Opening { always: Always::load(), always_next: false, kind: None, known: HashMap::new(), pictures: HashMap::new(), arrived: Vec::new(), after: HashMap::new() }
    }

    fn name_of(&self, choice: &Choice) -> String {
        match choice {
            Choice::Own(p) => own_name(p),
            Choice::System(id) => self.known.get(id).map(|a| a.name.clone()).unwrap_or_else(|| id.clone()),
        }
    }
}

/// Where a remote's file is fetched to for an application that takes only files.
fn fetched_into(place: &Place) -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into())).join(".cache"));
    let mut dir = base.join("spatishelf").join("opened");
    if let Place::Remote(r, p) = place {
        dir = dir.join(r);
        if let Some((folder, _)) = p.rsplit_once('/') {
            dir = dir.join(folder);
        }
    }
    dir
}

impl Shelf {
    /// The thing at the cursor and what is chosen, for opening: the cursor's item and place,
    /// and every chosen thing's place (the cursor's alone if nothing is chosen).
    fn to_open(&mut self) -> Option<(Item, Place, Vec<Place>)> {
        let (index, item) = self.tab().item_at_cursor()?;
        let item = item.clone();
        let place = self.tab().place_of(index)?;
        let mut all = self.tab().chosen();
        if !all.contains(&place) {
            all = vec![place.clone()];
        }
        Some((item, place, all))
    }

    /// A path for the system to tell the kind of a thing by: a remote's by its name alone.
    fn kind_path(place: &Place) -> PathBuf {
        match place {
            Place::Local(p) => PathBuf::from(p),
            _ => PathBuf::from(place.name()),
        }
    }

    /// Open what is chosen: with `with`, or with what a double click opens its kind with.
    pub(crate) fn open(&mut self, with: Option<Choice>) {
        let Some((item, place, all)) = self.to_open() else { return };
        if item.kind == Kind::Folder && with.is_none() {
            self.tab().go(place, true);
            return;
        }
        let folder = item.kind == Kind::Folder;
        let choice = match with {
            Some(choice) => Some(choice),
            None => {
                let path = Self::kind_path(&place);
                let kind = openwith::type_of(&path, folder);
                let always = self.opening.always.get(&kind).cloned();
                let own = own_for(item.kind);
                // The system is asked only when nothing nearer says.
                let system = if always.is_none() && own.is_empty() { self.system_apps(&path, &kind) } else { Vec::new() };
                openwith::first_choice(always.as_ref(), own.first().copied(), &system)
            }
        };
        let Some(choice) = choice else {
            self.fail(format!("Nothing on this machine opens {}.", item.name));
            return;
        };
        self.open_with(choice, all);
    }

    fn system_apps(&mut self, path: &Path, kind: &str) -> Vec<openwith::App> {
        let mut app = self.app.take().expect("the app");
        let found = openwith::apps_for(path, kind, &mut app);
        self.app = Some(app);
        for a in &found {
            self.opening.known.insert(a.id.clone(), a.clone());
        }
        found
    }

    /// Open these with that.
    pub(crate) fn open_with(&mut self, choice: Choice, places: Vec<Place>) {
        let name = self.opening.name_of(&choice);
        let what = if places.len() == 1 { places[0].name() } else { format!("{} items", places.len()) };
        let result = match &choice {
            Choice::Own(program) => places.iter().try_for_each(|p| self.open_own(program, p)),
            Choice::System(id) => {
                let Some(app) = self.opening.known.get(id).cloned() else {
                    self.fail(format!("{name} is not known here any more."));
                    return;
                };
                // Remotes' files: by their WebDAV address where the application takes one,
                // else fetched first.
                let mut files = Vec::new();
                for p in &places {
                    match p {
                        Place::Local(path) => files.push(path.clone()),
                        Place::Remote(r, path) if app.takes_urls => match &self.storage {
                            Some(storage) => files.push(storage.url(r, path)),
                            None => return self.fail("The storage service is not running."),
                        },
                        Place::Remote(..) => {
                            let into = fetched_into(p);
                            let id = self.next_op;
                            self.next_op += 1;
                            self.opening.after.insert(id, (choice.clone(), vec![into.join(p.name()).to_string_lossy().into_owned()]));
                            self.tab().tell(&ToTab::Do { id, op: Op::Fetch { item: p.clone(), into: into.to_string_lossy().into_owned() } });
                        }
                        Place::Trash => {}
                    }
                }
                if files.is_empty() {
                    self.say(format!("Fetching {what} for {name}\u{2026}"));
                    return;
                }
                let mut app_handle = self.app.take().expect("the app");
                let result = openwith::launch(&app, &files, &mut app_handle);
                self.app = Some(app_handle);
                result
            }
        };
        match result {
            Ok(()) => self.say(format!("Opening {what} with {name}\u{2026}")),
            Err(e) => self.fail(format!("{what} would not open with {name}: {e}")),
        }
    }

    /// A fetch for opening is done: open what it fetched.
    pub(crate) fn fetched(&mut self, id: u64, error: Option<&str>) -> bool {
        let Some((choice, files)) = self.opening.after.remove(&id) else { return false };
        if error.is_some() {
            return true;
        }
        let Choice::System(app) = &choice else { return true };
        let Some(app) = self.opening.known.get(app).cloned() else { return true };
        let mut handle = self.app.take().expect("the app");
        let result = openwith::launch(&app, &files, &mut handle);
        self.app = Some(handle);
        if let Err(e) = result {
            self.fail(format!("{} would not open: {e}", app.name));
        }
        true
    }

    fn open_own(&self, program: &str, place: &Place) -> Result<(), String> {
        let path = own_program(program).ok_or_else(|| format!("{} is not installed here.", own_name(program)))?;
        let mut command = Command::new(path);
        match place {
            Place::Local(p) => {
                command.arg(p);
            }
            // SpatiLoupe knows remotes itself; SpatiReel is given the stream.
            Place::Remote(r, p) if program == "spatireel" => {
                if let Some(storage) = &self.storage {
                    command.arg(storage.url(r, p));
                }
            }
            Place::Remote(..) => {
                command.arg(place.text());
            }
            Place::Trash => return Ok(()),
        }
        if program == "spatiquill" {
            command.env("SPATIQUILL_FOLDER", match place {
                Place::Local(p) => p.clone(),
                _ => String::new(),
            });
        }
        detached(command)
    }

    /// "Open with": the applications for the thing at the cursor, the one a double click uses
    /// first and marked; "Other..."; and whether the one chosen is to open this kind always.
    pub(crate) fn open_with_menu(&mut self, at: (f32, f32)) {
        let Some((item, place, _)) = self.to_open() else { return };
        let folder = item.kind == Kind::Folder;
        let path = Self::kind_path(&place);
        let kind = openwith::type_of(&path, folder);
        let words = openwith::kind_words(&item.name, &kind, folder);
        let system = self.system_apps(&path, &kind);
        let own = own_for(item.kind);
        let always = self.opening.always.get(&kind).cloned();
        let first = openwith::first_choice(always.as_ref(), own.first().copied(), &system);
        let order = openwith::ordered(first.as_ref(), &own, &system);
        let mut entries: Vec<(Row, Action)> = Vec::new();
        for choice in order {
            let name = self.opening.name_of(&choice);
            let mut row = if Some(&choice) == first.as_ref() { Row::with(name, "default") } else { Row::new(name) };
            row = match &choice {
                Choice::Own(p) => row.icon(own_icon(p)),
                Choice::System(_) => row.icon(Icon::Link),
            };
            entries.push((row, Action::OpenWith(choice)));
        }
        if entries.is_empty() {
            entries.push((Row::new("No application here opens it").icon(Icon::Info), Action::OtherAppsMenu));
        }
        entries.push((Row::new("Other\u{2026}").icon(Icon::Search), Action::OtherAppsMenu));
        let on = if self.opening.always_next { "on" } else { "off" };
        entries.push((Row::with(format!("Always open {words} with the one chosen"), on).icon(Icon::Pin), Action::ToggleAlways));
        if let Some(choice) = &always {
            entries.push((Row::new(format!("Forget {} for {words}", self.opening.name_of(choice))).icon(Icon::Close), Action::ForgetAlways));
        }
        self.opening.kind = Some((kind, words));
        self.ask_pictures(&system);
        self.open_menu(at, entries);
    }

    /// Every application, for a choice of any.
    pub(crate) fn other_apps_menu(&mut self, at: (f32, f32)) {
        let mut app = self.app.take().expect("the app");
        let all = openwith::all_apps(&mut app);
        self.app = Some(app);
        if all.is_empty() {
            self.fail("No other application was found.");
            return;
        }
        for a in &all {
            self.opening.known.insert(a.id.clone(), a.clone());
        }
        let entries = all.iter().map(|a| (Row::new(a.name.clone()).icon(Icon::Link), Action::OpenWith(Choice::System(a.id.clone())))).collect();
        self.ask_pictures(&all);
        self.open_menu(at, entries);
    }

    /// Pictures of applications not yet had, made on the worker.
    fn ask_pictures(&mut self, apps: &[openwith::App]) {
        for a in apps {
            if self.opening.pictures.contains_key(&a.id) {
                continue;
            }
            self.opening.pictures.insert(a.id.clone(), None);
            let app = a.clone();
            self.jobs.send(Box::new(move || Done::AppPicture(app.id.clone(), openwith::icon(&app, 64)))).ok();
        }
    }

    /// An application chosen from "Open with": remembered for the kind if asked, then used.
    pub(crate) fn chose_app(&mut self, choice: Choice) {
        if std::mem::take(&mut self.opening.always_next) {
            if let Some((kind, words)) = self.opening.kind.clone() {
                self.opening.always.set(&kind, choice.clone());
                self.opening.always.save();
                let name = self.opening.name_of(&choice);
                self.say(format!("{} now open with {name}.", crate::capitalised(&words)));
            }
        }
        let Some((_, _, all)) = self.to_open() else { return };
        self.open_with(choice, all);
    }

    pub(crate) fn forget_always(&mut self) {
        if let Some((kind, words)) = self.opening.kind.clone() {
            if self.opening.always.forget(&kind) {
                self.opening.always.save();
                self.say(format!("{} open as they did before.", crate::capitalised(&words)));
            }
        }
    }
}
