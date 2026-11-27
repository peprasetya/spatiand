//! The screens that are not a film: home, a source's folders, adding a server.
//!
//! A screen draws itself and answers what the wearer did with an [`Act`] -- what it would
//! like to happen -- and never does it. Listing a folder, signing in, playing a film all take
//! a network or a decoder, and are `main`'s to start and a thread's to do.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use spatiand_app::ui::{theme, Button, Confirm, Icon, Field, Grid, Input, Menu, MenuSaid, Painter, Rect, Row, Shape, Strip, Texture, Tile, Typed};
use spatiand_app::{Gl, Key};
use spatiand_media::still::Still;

use crate::source::jellyfin::Flavour;
use crate::source::Item;
use crate::{library, Account, Entry, Kind};
use spatiand_app::ui::look;

/// What a screen would like to happen.
pub enum Act {
    Nothing,
    Home,
    /// Open a source, at its top or at a folder of it.
    Open { source: usize, folder: Option<Item> },
    /// List the folder the browse screen is now at.
    List,
    Play { source: usize, item: Item },
    Add,
    /// Sign out of a server and forget it.
    Forget(usize),
    SignIn { flavour: Flavour, address: String, user: String, password: String },
    /// Ask a Vibrefy server for a code and wait for it to be approved, until `cancelled`.
    Pair { address: String, cancelled: Arc<AtomicBool> },
    /// Open the sign-in link a Vibrefy server gave in SpatiMesh, in the profile chosen (none:
    /// the one SpatiMesh would open itself).
    OpenLink { url: String, profile: Option<String> },
    /// Pair with this Vibrefy server again, its session having run out: straight to a code.
    Reconnect(String),
    /// Stop offering to pair again.
    Dismiss,
}

pub enum Screen {
    Empty,
    Home(Home),
    Browse(Browse),
    Add(Add),
}

/// A picture wanted: (which asking, which tile, which source, of what).
pub type Wanted = (u64, usize, usize, Item, Option<Shown>);

/// Which pictures are on show right now, by their place in the list as it came. A picture
/// asked for and not yet fetched is let go of if its tile has been scrolled away from.
pub type Shown = Arc<Mutex<std::collections::HashSet<usize>>>;

/// How long a tile must stay on show before its picture is asked for: scrolling past does
/// not ask for the pictures passed.
const DWELL: Duration = Duration::from_millis(250);

impl Screen {
    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, sources: &[Entry]) {
        match self {
            Screen::Empty => {}
            Screen::Home(home) => home.draw(p, gl),
            Screen::Browse(browse) => browse.draw(p, gl),
            Screen::Add(add) => add.draw(p, gl),
        }
        let _ = sources;
    }

    pub fn input(&mut self, input: &Input, sources: &[Entry]) -> Act {
        match self {
            Screen::Empty => Act::Nothing,
            Screen::Home(home) => home.input(input, sources),
            Screen::Browse(browse) => browse.input(input),
            Screen::Add(add) => add.input(input),
        }
    }

    /// Something went wrong that the wearer should be told, wherever they are.
    /// A line at the foot of the screen; nothing, to take it away.
    pub fn trouble(&mut self, said: String) {
        let said = (!said.is_empty()).then_some(said);
        match self {
            Screen::Home(home) => home.trouble = said,
            Screen::Browse(browse) => browse.trouble = said,
            _ => {}
        }
    }

    /// A picture was not fetched, its tile having gone from view: it may be asked for again.
    pub fn passed(&mut self, ticket: u64, index: usize) {
        if let Screen::Browse(browse) = self {
            if browse.ticket == ticket {
                browse.passed(index);
            }
        }
    }

    pub fn pictured(&mut self, ticket: u64, index: usize, still: Option<Still>) {
        let Some(still) = still else { return };
        match self {
            Screen::Home(home) if home.ticket == ticket => home.arrived.push((index, still)),
            Screen::Browse(browse) if browse.ticket == ticket => browse.pictured(index, still),
            _ => {}
        }
    }

    /// The pictures the tiles on show are waiting for, each asked for once.
    pub fn wanted(&mut self) -> Vec<Wanted> {
        match self {
            Screen::Home(home) => home.wanted(),
            Screen::Browse(browse) => browse.wanted(),
            _ => Vec::new(),
        }
    }

    /// Its tiles' pictures, for a screen being replaced by another: let go of on the GPU at the
    /// next drawing (`Painter::forget`). A texture has no drop of its own, and a screen dropped
    /// with its pictures left them on the GPU for as long as SpatiReel ran.
    pub fn textures(self) -> Vec<Texture> {
        match self {
            Screen::Home(home) => home.left.into_iter().chain(home.films.into_iter().filter_map(|film| film.tile.picture)).collect(),
            Screen::Browse(browse) => browse.left.into_iter().chain(browse.tiles.into_iter().filter_map(|tile| tile.picture)).collect(),
            Screen::Empty | Screen::Add(_) => Vec::new(),
        }
    }

    /// Whether the screen moves by itself, and so wants drawing though nothing happened.
    pub fn animating(&self) -> bool {
        match self {
            Screen::Browse(browse) => matches!(browse.state, State::Loading(_)),
            Screen::Add(add) => add.busy.is_some(),
            _ => false,
        }
    }
}

// --- home ---

/// One part-watched film on the home screen.
struct Unfinished {
    /// What its picture is asked for and comes back by: its place among the tiles moves as
    /// other sources answer.
    id: usize,
    source: usize,
    item: Item,
    tile: Tile,
    asked: bool,
}

pub struct Home {
    pub ticket: u64,
    films: Vec<Unfinished>,
    next_id: usize,
    shelf: Strip,
    /// The sources' tiles, and after them the one that pairs again, if there is one, and the
    /// one that adds a server.
    places: Vec<Tile>,
    /// The Vibrefy server whose session ran out.
    lapsed: Option<String>,
    kinds: Vec<Kind>,
    sources: Strip,
    /// Which shelf the keys are going to: the films', or the sources'.
    on_films: bool,
    arrived: Vec<(usize, Still)>,
    /// Pictures of films no longer on the shelf, to be let go of at the next drawing.
    left: Vec<Texture>,
    pub trouble: Option<String>,
    /// Delete pressed on this source: the question whether to forget it, and which it is about.
    asking: Option<(usize, Confirm)>,
}

impl Home {
    pub fn new(sources: &[Entry], lapsed: Option<String>, ticket: u64) -> Home {
        let tinted = |color: [f32; 4]| Some(([color[0], color[1], color[2], 0.55], [color[0] * 0.35, color[1] * 0.35, color[2] * 0.5, 0.35]));
        let mut places: Vec<Tile> = sources
            .iter()
            .map(|entry| {
                let (detail, glyph, tint) = match &entry.kind {
                    Kind::Local => ("Folders and films here".to_string(), "", look::TEAL),
                    Kind::Server(Account::Jellyfin(account)) => (format!("Signed in as {}", account.user), if account.flavour == Flavour::Emby { "E" } else { "J" }, look::VIOLET),
                    Kind::Server(Account::Vibrefy(account)) => (account.account.clone().map_or("Paired".to_string(), |who| format!("Signed in as {who}")), "V", look::BLUE),
                    Kind::Dlna => ("On this network".to_string(), "", [0.55, 0.62, 0.78, 1.0]),
                };
                // This machine and the network have icons; a server is its kind's letter.
                let icon = match &entry.kind {
                    Kind::Local => Some(Icon::Film),
                    Kind::Dlna => Some(Icon::Server),
                    _ => None,
                };
                Tile { title: entry.source.label(), detail: Some(detail), glyph: (!glyph.is_empty()).then(|| glyph.into()), icon, tint: tinted(tint), ..Tile::default() }
            })
            .collect();
        if let Some(address) = &lapsed {
            places.push(Tile {
                title: vibrefy_name(address),
                // A new code is all it takes: the address is kept.
                detail: Some("Login expired \u{b7} reconnect".into()),
                glyph: Some("V".into()),
                tint: tinted([0.62, 0.66, 0.74, 1.0]),
                ..Tile::default()
            });
        }
        places.push(Tile { title: "Add a server".into(), detail: Some("Jellyfin, Emby or Vibrefy".into()), icon: Some(Icon::Plus), ..Tile::default() });
        let mut home = Home {
            ticket,
            films: Vec::new(),
            next_id: 0,
            shelf: Strip::default(),
            places,
            kinds: sources.iter().map(|e| e.kind.clone()).collect(),
            lapsed,
            sources: Strip::default(),
            on_films: false,
            arrived: Vec::new(),
            left: Vec::new(),
            trouble: None,
            asking: None,
        };
        home.shelf.shape = Shape::Wide;
        home.sources.shape = Shape::Wide;
        home.sources.focused = true;
        home
    }

    /// One source's part-watched films have come: they join the shelf, after those of the
    /// sources before it.
    pub fn unfinished(&mut self, source: usize, label: &str, items: Vec<Item>) {
        let first = self.films.is_empty();
        let (gone, kept): (Vec<Unfinished>, Vec<Unfinished>) = std::mem::take(&mut self.films).into_iter().partition(|film| film.source == source);
        self.left.extend(gone.into_iter().filter_map(|film| film.tile.picture));
        self.films = kept;
        let at = self.films.iter().position(|film| film.source > source).unwrap_or(self.films.len());
        let fresh: Vec<Unfinished> = items
            .into_iter()
            .filter(|item| !item.folder)
            .map(|item| {
                self.next_id += 1;
                Unfinished {
                    id: self.next_id,
                    source,
                    tile: Tile {
                        title: item.title.clone(),
                        // Whose it is: the same film may be on two servers.
                        detail: Some(label.to_string()),
                        picture: None,
                        progress: Some(item.progress.unwrap_or(0.5).clamp(0.04, 1.0)),
                        ..Tile::default()
                    },
                    item,
                    asked: false,
                }
            })
            .collect();
        self.films.splice(at..at, fresh);
        // With something to go on with, that is where the cursor starts.
        if first && !self.films.is_empty() {
            self.focus(true);
        }
        if self.films.is_empty() {
            self.focus(false);
        }
    }

    fn focus(&mut self, on_films: bool) {
        self.on_films = on_films && !self.films.is_empty();
        self.shelf.focused = self.on_films;
        self.sources.focused = !self.on_films;
    }

    fn wanted(&mut self) -> Vec<Wanted> {
        let ticket = self.ticket;
        let on_show = self.shelf.in_view();
        let mut wanted = Vec::new();
        for (index, film) in self.films.iter_mut().enumerate() {
            if on_show.contains(&index) && film.item.pictured && !std::mem::replace(&mut film.asked, true) {
                wanted.push((ticket, film.id, film.source, film.item.clone(), None));
            }
        }
        wanted
    }

    fn draw(&mut self, p: &mut Painter, gl: &Gl) {
        for old in self.left.drain(..) {
            p.forget(gl, old);
        }
        for (id, still) in self.arrived.drain(..) {
            if let Some(film) = self.films.iter_mut().find(|film| film.id == id) {
                if let Some(old) = film.tile.picture.take() {
                    p.forget(gl, old);
                }
                film.tile.picture = p.texture(gl, &still.rgba, still.width, still.height).ok();
            }
        }
        let mut all = p.bounds().inset(theme::PAD);
        look::header(p, gl, &mut all, "SpatiReel", "");
        let hint = if self.on_films { "[Enter] Play from where it was left" } else { "[Enter] Open  [Delete] Forget a server" };
        look::foot(p, gl, &mut all, hint, self.trouble.as_deref());
        if !self.films.is_empty() {
            look::shelf(p, gl, &mut all, "Continue watching", self.on_films);
            let width = 330.0;
            let tiles: Vec<Tile> = self.films.iter().map(|film| film.tile.clone()).collect();
            self.shelf.draw(p, gl, all.take_top(Strip::height(Shape::Wide, width)), &tiles, width);
            all.take_top(6.0);
        }
        look::shelf(p, gl, &mut all, "Sources", !self.on_films);
        let width = 258.0;
        let strip = all.take_top(Strip::height(Shape::Wide, width).min(all.h));
        self.sources.draw(p, gl, strip, &self.places, width);
        if let Some((_, confirm)) = &mut self.asking {
            confirm.draw(p, gl, p.bounds());
        }
    }

    fn input(&mut self, input: &Input, sources: &[Entry]) -> Act {
        // A question is being asked: it takes everything until it is answered.
        if let Some((at, confirm)) = &mut self.asking {
            let at = *at;
            return match confirm.input(input) {
                Some(yes) => {
                    self.asking = None;
                    if yes {
                        Act::Forget(at)
                    } else {
                        Act::Nothing
                    }
                }
                None => Act::Nothing,
            };
        }
        if let Input::Key { key, .. } = input {
            match key {
                Key::Up => self.focus(true),
                Key::Down => self.focus(false),
                Key::Delete | Key::Backspace if !self.on_films => {
                    let at = self.sources.cursor;
                    // Nothing is kept by the offer to pair again: once is enough to put it away.
                    if at == self.kinds.len() && self.lapsed.is_some() {
                        return Act::Dismiss;
                    }
                    if matches!(self.kinds.get(at), Some(Kind::Server(_))) {
                        let name = self.places.get(at).map_or(String::new(), |t| t.title.clone());
                        self.asking = Some((at, Confirm::careful(format!("Forget {name}? Its films are not touched; SpatiReel signs out of it."), "Forget", "Cancel")));
                        return Act::Nothing;
                    }
                }
                _ => {}
            }
        }
        // A press on a shelf gives it the keys as well.
        if let Input::Press { pressed: true, .. } = input {
            let _ = sources;
        }
        if let Some(chosen) = self.shelf.input(input) {
            self.focus(true);
            if let Some(film) = self.films.get(chosen) {
                return Act::Play { source: film.source, item: film.item.clone() };
            }
        }
        if let Some(chosen) = self.sources.input(input) {
            self.focus(false);
            if chosen < self.kinds.len() {
                return Act::Open { source: chosen, folder: None };
            }
            if let Some(address) = self.lapsed.clone().filter(|_| chosen == self.kinds.len()) {
                return Act::Reconnect(address);
            }
            return Act::Add;
        }
        Act::Nothing
    }
}

// --- a source's folders ---

pub enum State {
    Loading(Instant),
    Ready,
    Failed(String),
}

/// The order a folder is shown in. Every folder starts by name.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Sort {
    Name,
    Updated,
    Random,
}

impl Sort {
    const ALL: [Sort; 3] = [Sort::Name, Sort::Updated, Sort::Random];

    fn icon(self) -> Icon {
        match self {
            Sort::Name => Icon::SortName,
            Sort::Updated => Icon::SortRecent,
            Sort::Random => Icon::Shuffle,
        }
    }
}

pub struct Browse {
    pub source: usize,
    label: String,
    /// This machine's folders, which go up past where they started.
    local: bool,
    /// The folders gone into, each with where the cursor was in the one before it.
    trail: Vec<(Item, usize)>,
    items: Vec<Item>,
    tiles: Vec<Tile>,
    asked: Vec<bool>,
    /// Where each of these was in the list as it came, which is what its picture is asked
    /// for and comes back by: the order below is the wearer's to change.
    origin: Vec<usize>,
    /// Since when each tile has been on show, by `origin`.
    since: std::collections::HashMap<usize, Instant>,
    shown: Shown,
    sort: Sort,
    sorts: [Button; 3],
    grid: Grid,
    pub state: State,
    pub ticket: u64,
    /// Where the cursor is to be once the list has come.
    restore: Option<usize>,
    /// The folder just come up out of, to put the cursor back on.
    came_from: Option<String>,
    arrived: Vec<(usize, Still)>,
    /// Pictures of a folder that has been left, to be let go of at the next drawing.
    left: Vec<Texture>,
    home: Button,
    up: Button,
    /// A server's own page has a way to forget it, which asks first.
    server: bool,
    remove: Button,
    asking: Option<Confirm>,
    pub trouble: Option<String>,
}

impl Browse {
    pub fn new(source: usize, label: String, local: bool, server: bool) -> Browse {
        let mut grid = Grid::default();
        grid.focused = true;
        Browse {
            source,
            label,
            local,
            trail: Vec::new(),
            items: Vec::new(),
            tiles: Vec::new(),
            asked: Vec::new(),
            origin: Vec::new(),
            since: std::collections::HashMap::new(),
            shown: Shown::default(),
            sort: Sort::Name,
            sorts: Default::default(),
            grid,
            state: State::Loading(Instant::now()),
            ticket: 0,
            restore: None,
            came_from: None,
            arrived: Vec::new(),
            left: Vec::new(),
            home: Button::default(),
            up: Button::default(),
            server,
            remove: Button::default(),
            asking: None,
            trouble: None,
        }
    }

    pub fn folder(&self) -> Option<&Item> {
        self.trail.last().map(|(item, _)| item)
    }

    /// The folder on show and what is in it, once listed: what a film chosen here is beside.
    pub fn listing(&self) -> Option<crate::next::Listing> {
        let folder = self.folder()?;
        (!self.items.is_empty()).then(|| (folder.key.clone(), self.items.clone()))
    }

    pub fn enter(&mut self, folder: Item, cursor: usize) {
        self.trail.push((folder, cursor));
    }

    /// The list is about to be asked for again as it is: the cursor stays where it is.
    pub fn keep_cursor(&mut self) {
        self.restore = Some(self.grid.cursor);
    }

    pub fn loading(&mut self) {
        self.state = State::Loading(Instant::now());
        self.left.extend(self.tiles.drain(..).filter_map(|tile| tile.picture));
        self.items.clear();
        self.asked.clear();
        self.origin.clear();
        self.since.clear();
        self.shown.lock().unwrap().clear();
        self.arrived.clear();
        self.sort = Sort::Name;
    }

    /// Whether the order can be told apart from the name's: not when the server says nothing
    /// of when things changed.
    fn dated(&self) -> bool {
        self.items.iter().any(|item| item.modified > 0)
    }

    /// Show this folder in `sort` order. The pictures already fetched go with their tiles.
    fn arrange(&mut self, sort: Sort) {
        // Folders stay ahead of films, as the server lists them; the order is within each.
        let mut order: Vec<usize> = (0..self.items.len()).collect();
        order.sort_by_key(|i| (!self.items[*i].folder, self.origin[*i]));
        let split = order.iter().take_while(|i| self.items[**i].folder).count();
        let (folders, films) = order.split_at_mut(split);
        match sort {
            Sort::Name => {}
            Sort::Updated => {
                folders.sort_by_key(|i| std::cmp::Reverse(self.items[*i].modified));
                films.sort_by_key(|i| std::cmp::Reverse(self.items[*i].modified));
            }
            Sort::Random => {
                let mut dice = crate::source::Dice::new();
                dice.shuffle(folders);
                dice.shuffle(films);
            }
        }
        fn take<T>(from: &mut Vec<T>, order: &[usize]) -> Vec<T>
        where
            T: Default,
        {
            order.iter().map(|i| std::mem::take(&mut from[*i])).collect()
        }
        self.items = take(&mut self.items, &order);
        self.tiles = take(&mut self.tiles, &order);
        self.asked = order.iter().map(|i| self.asked[*i]).collect();
        self.origin = order.iter().map(|i| self.origin[*i]).collect();
        self.sort = sort;
        self.grid.cursor = 0;
        self.grid.top();
    }

    /// A tile's picture has come, to be put on the GPU at the next drawing. Where the source
    /// could not say what shape it would be, it is known now, and the folder takes the shape
    /// most of its pictures have (`shape`): this machine's films are posters where they carry
    /// covers, frames where they do not.
    fn pictured(&mut self, origin: usize, still: Still) {
        let at = self.origin.iter().position(|o| *o == origin);
        if let Some(item) = at.and_then(|at| self.items.get_mut(at)).filter(|item| item.aspect.is_none() && still.height > 0) {
            item.aspect = Some(still.width as f32 / still.height as f32);
            self.grid.shape = Browse::shape(&self.items);
        }
        self.arrived.push((origin, still));
    }

    /// A picture not fetched, because its tile was no longer on show: it is not asked for.
    fn passed(&mut self, origin: usize) {
        if let Some(at) = self.origin.iter().position(|o| *o == origin) {
            self.asked[at] = false;
        }
    }

    /// Posters where most of what is here has posters, frames where it has frames: by the
    /// pictures' own shapes, whatever kind of server sent them -- as the source says they are,
    /// or, where it cannot say (this machine's films: a cover or a frame), as they have come.
    fn shape(items: &[Item]) -> Shape {
        Shape::of(items.iter().filter(|i| i.pictured).filter_map(|i| i.aspect), Shape::Wide)
    }

    pub fn listed(&mut self, result: Result<Vec<Item>, String>) {
        match result {
            Err(e) => self.state = State::Failed(e),
            Ok(items) => {
                self.grid.shape = Browse::shape(&items);
                self.tiles = items
                    .iter()
                    .map(|item| Tile { title: item.title.clone(), detail: item.detail.clone(), progress: item.progress, icon: item.folder.then_some(Icon::Folder), ..Tile::default() })
                    .collect();
                self.asked = vec![false; items.len()];
                self.origin = (0..items.len()).collect();
                self.sort = Sort::Name;
                let came_from = self.came_from.take().and_then(|key| items.iter().position(|item| item.key == key));
                self.grid.cursor = self.restore.take().or(came_from).unwrap_or(0).min(items.len().saturating_sub(1));
                self.items = items;
                self.state = State::Ready;
            }
        }
    }

    /// The pictures of the tiles that have been on show for a moment, each asked for once.
    /// Whatever is further than the view, or only passing through it, waits: a server may
    /// have to take a frame out of a large file for each.
    fn wanted(&mut self) -> Vec<Wanted> {
        let mut wanted = Vec::new();
        // The grid's idea of what is on show is from when it was last drawn, which may have
        // been a list with more in it.
        let on_show: Vec<usize> = self.grid.in_view().filter(|index| *index < self.items.len()).collect();
        let origins: std::collections::HashSet<usize> = on_show.iter().map(|index| self.origin[*index]).collect();
        self.since.retain(|origin, _| origins.contains(origin));
        *self.shown.lock().unwrap() = origins;
        for index in on_show {
            let origin = self.origin[index];
            let since = *self.since.entry(origin).or_insert_with(Instant::now);
            let item = &self.items[index];
            if item.pictured && !self.asked[index] && since.elapsed() >= DWELL {
                self.asked[index] = true;
                wanted.push((self.ticket, origin, self.source, item.clone(), Some(self.shown.clone())));
            }
        }
        wanted
    }

    /// Where this machine's folders are now, when that is what is being browsed.
    fn path(&self) -> std::path::PathBuf {
        self.folder().map_or_else(library::home, |item| item.path.clone().unwrap_or_else(|| item.key.clone().into()))
    }

    fn draw(&mut self, p: &mut Painter, gl: &Gl) {
        for old in self.left.drain(..) {
            p.forget(gl, old);
        }
        for (origin, still) in self.arrived.drain(..) {
            if let Some(tile) = self.origin.iter().position(|o| *o == origin).and_then(|at| self.tiles.get_mut(at)) {
                if let Some(old) = tile.picture.take() {
                    p.forget(gl, old);
                }
                tile.picture = p.texture(gl, &still.rgba, still.width, still.height).ok();
            }
        }
        let mut all = p.bounds().inset(theme::PAD);
        // The ways out, then where this is.
        let title = if self.local {
            let path = self.path();
            path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
        } else {
            self.folder().map_or(self.label.clone(), |item| item.title.clone())
        };
        let beside = if self.local {
            self.path().display().to_string()
        } else if title != self.label {
            self.label.clone()
        } else {
            String::new()
        };
        let can_remove = self.can_remove();
        let remove_label = "Forget";
        let can_go_up = self.local || !self.trail.is_empty();
        let sorting = matches!(self.state, State::Ready) && !self.local && self.items.len() > 1;
        let updated = self.items.iter().any(|item| item.modified > 0);
        let (home, up, remove, sorts, chosen) = (&mut self.home, &mut self.up, &mut self.remove, &mut self.sorts, self.sort);
        look::top_with(p, gl, &mut all, &title, &beside, |p, top| {
            let width = Button::icon_width(p, gl, "Home");
            home.draw_icon(p, gl, top.take_left(width), Icon::Home, "Home");
            top.take_left(12.0);
            if can_go_up {
                up.draw_icon(p, gl, top.take_left(Button::HEIGHT), Icon::Up, "");
                top.take_left(24.0);
            }
            // Taking a server away is for its top, where it is not mistaken for anything else: in
            // a folder it is a button where a slip of the pointer is all it takes.
            if can_remove {
                let width = Button::icon_width(p, gl, remove_label);
                remove.draw_icon(p, gl, top.take_right(width), Icon::Trash, remove_label);
                top.take_right(16.0);
            }
            if sorting {
                // Right to left, so the first reads first.
                for (button, sort) in sorts.iter_mut().zip(Sort::ALL).rev() {
                    if sort == Sort::Updated && !updated {
                        button.focused = false;
                        continue;
                    }
                    button.focused = chosen == sort;
                    button.draw_icon(p, gl, top.take_right(Button::HEIGHT), sort.icon(), "");
                    top.take_right(8.0);
                }
                top.take_right(8.0);
            }
        });
        look::foot(p, gl, &mut all, "[Enter] Open  [Backspace] Up  [Esc] Home", self.trouble.as_deref());
        match &self.state {
            State::Loading(since) => look::waiting(p, gl, all, *since),
            State::Failed(why) => look::message(p, gl, all, why, look::trouble()),
            State::Ready if self.tiles.is_empty() => look::message(p, gl, all, "Nothing here to watch.", theme::ink_detail()),
            State::Ready => self.grid.draw(p, gl, all, &self.tiles),
        }
        if let Some(confirm) = &mut self.asking {
            confirm.draw(p, gl, p.bounds());
        }
    }

    /// Up one level. For this machine's folders that is the folder this one is in, however
    /// far up that goes: what may be read is the file system's to say, not this screen's.
    fn go_up(&mut self) -> Act {
        if self.local {
            let here = self.path();
            let Some(parent) = here.parent() else { return Act::Nothing };
            self.came_from = Some(here.to_string_lossy().into_owned());
            self.trail = vec![(crate::source::local::Local::item(parent), 0)];
            return Act::List;
        }
        match self.trail.pop() {
            Some((_, cursor)) => {
                self.restore = Some(cursor);
                Act::List
            }
            None => Act::Home,
        }
    }

    /// Whether the button that takes this server away is to be had: at its top only.
    fn can_remove(&self) -> bool {
        self.server && self.trail.is_empty()
    }

    fn input(&mut self, input: &Input) -> Act {
        // A question is being asked: it takes everything until it is answered.
        if let Some(confirm) = &mut self.asking {
            return match confirm.input(input) {
                Some(yes) => {
                    self.asking = None;
                    if yes {
                        Act::Forget(self.source)
                    } else {
                        Act::Nothing
                    }
                }
                None => Act::Nothing,
            };
        }
        if matches!(self.state, State::Ready) && !self.local {
            for (at, sort) in Sort::ALL.into_iter().enumerate() {
                let usable = sort != Sort::Updated || self.dated();
                if usable && self.sorts[at].input(input) && (sort == Sort::Random || self.sort != sort) {
                    self.arrange(sort);
                    return Act::Nothing;
                }
            }
        }
        if self.can_remove() && self.remove.input(input) {
            self.asking = Some(Confirm::careful(format!("Forget {}? Its films are not touched; SpatiReel signs out of it.", self.label), "Forget", "Cancel"));
            return Act::Nothing;
        }
        if self.home.input(input) || matches!(input, Input::Key { key: Key::Escape, .. }) {
            return Act::Home;
        }
        if self.up.input(input) || matches!(input, Input::Key { key: Key::Backspace, .. }) {
            self.trouble = None;
            return self.go_up();
        }
        if !matches!(self.state, State::Ready) {
            return Act::Nothing;
        }
        let Some(chosen) = self.grid.input(input) else { return Act::Nothing };
        let Some(item) = self.items.get(chosen).cloned() else { return Act::Nothing };
        self.trouble = None;
        if item.folder {
            if self.local {
                self.trail = vec![(item, 0)];
            } else {
                self.trail.push((item, chosen));
            }
            Act::List
        } else {
            Act::Play { source: self.source, item }
        }
    }
}

// --- adding a server ---

/// A Vibrefy server as the wearer would call it: its host, as the source's label has it.
pub(crate) fn vibrefy_name(address: &str) -> String {
    let address = address.trim_end_matches('/');
    let host = address.split_once("://").map_or(address, |(_, rest)| rest);
    format!("Vibrefy \u{b7} {}", host.split([':', '/']).next().unwrap_or(host))
}

/// The code a Vibrefy server gave to be approved, for the screen that is showing it. Set by
/// the thread that asked.
static CODE: Mutex<Option<String>> = Mutex::new(None);

pub fn show_code(code: &str) {
    *CODE.lock().unwrap() = Some(code.to_string());
}

/// The sign-in link the same server gave beside the code, where it gives one: approved in a
/// browser on this machine, which is SpatiMesh. Set by the thread that asked.
static LINK: Mutex<Option<String>> = Mutex::new(None);

pub fn show_link(url: &str) {
    *LINK.lock().unwrap() = Some(url.to_string());
}

fn forget_pairing() {
    *CODE.lock().unwrap() = None;
    *LINK.lock().unwrap() = None;
}

enum Stage {
    /// Which kind of server.
    Pick,
    /// A name and a password: Jellyfin, Emby.
    Form(Flavour),
    /// An address, and then a code to be approved: Vibrefy.
    Pair,
}

pub struct Add {
    stage: Stage,
    kinds: Strip,
    fields: [Field; 3],
    /// Which field, or the button after them, has the keys.
    focus: usize,
    go: Button,
    back: Button,
    /// Waiting on the server, since when.
    pub busy: Option<Instant>,
    cancelled: Arc<AtomicBool>,
    trouble: Option<String>,
    pointer: (f32, f32),
    /// The Vibrefy server last paired with, which its address field starts with.
    last: Option<String>,
    /// **Whose SpatiMesh signs in**: its profiles, read anew for every pairing and asked every
    /// time -- never remembered, since the people of one machine each have their own profile
    /// and their own films (the owner, 2026-11-23: "keep offer profile selection. I can have
    /// more than 2 user with different content"). They are chosen from a drop-down, every one
    /// of them, and nothing there says which has a lock: SpatiMesh asks for a locked one's
    /// code itself, as it does for a new window of that profile (the owner, 2026-11-24: "it
    /// doesn't give any hint if the profile is locked or no").
    profiles: Vec<spatiand_app::browser::Profile>,
    /// The drop-down, and the button that opens the link in the profile it names.
    whose: Button,
    sign: Button,
    /// Which profile the drop-down names: none until one is chosen, at every pairing.
    picked: Option<usize>,
    /// The drop-down's list, while it is open.
    menu: Option<Menu>,
    /// Which of the two has the keys: the drop-down (0) or the button (1).
    link_focus: usize,
    /// The link has been handed to SpatiMesh: said under the buttons.
    handed: Option<String>,
}

impl Add {
    pub fn new(last: Option<String>) -> Add {
        forget_pairing();
        let mut kinds = Strip::default();
        kinds.shape = Shape::Wide;
        kinds.focused = true;
        let mut fields = [Field::default(), Field::default(), Field::default()];
        fields[0].hint = "Address, like films.example.org or 192.168.1.20".into();
        fields[1].hint = "Name".into();
        fields[2].hint = "Password".into();
        fields[2].secret = true;
        Add { stage: Stage::Pick, kinds, fields, focus: 0, go: Button::default(), back: Button::default(), busy: None, cancelled: Arc::default(), trouble: None, pointer: (0.0, 0.0), last, profiles: Vec::new(), whose: Button::default(), sign: Button::default(), picked: None, menu: None, link_focus: 0, handed: None }
    }

    /// Straight to pairing with the server last paired with: the code is asked for at once.
    pub fn pair_again(&mut self) -> Act {
        self.pair();
        self.submit()
    }

    /// The Vibrefy form, its address the last one paired with until something else is typed.
    fn pair(&mut self) {
        self.stage = Stage::Pair;
        if self.fields[0].text.trim().is_empty() {
            if let Some(last) = &self.last {
                self.fields[0].set(last.trim_end_matches('/'));
            }
        }
        // With the address there already, the keys start on the button that asks for a code.
        self.focus = if self.fields[0].text.is_empty() { 0 } else { 1 };
    }

    pub fn failed(&mut self, why: String) {
        self.busy = None;
        self.trouble = Some(why);
        forget_pairing();
    }

    /// The link could not be handed to SpatiMesh: the code is still good, and the wait goes on.
    pub fn link_failed(&mut self, why: String) {
        self.handed = None;
        self.trouble = Some(why);
    }

    /// Whether the wearer is asked whose: with one profile, or none written down yet, the
    /// link opens in what SpatiMesh opens.
    fn asks_whose(&self) -> bool {
        self.profiles.len() > 1
    }

    /// How many fields this stage has.
    fn count(&self) -> usize {
        match self.stage {
            Stage::Form(_) => 3,
            _ => 1,
        }
    }

    fn draw(&mut self, p: &mut Painter, gl: &Gl) {
        let mut all = p.bounds().inset(theme::PAD);
        let title = match self.stage {
            Stage::Pick => "Add a server".to_string(),
            Stage::Form(flavour) => format!("Sign in to {}", flavour.label()),
            Stage::Pair => "Pair with Vibrefy".to_string(),
        };
        look::top(p, gl, &mut all, &mut self.back, "Back", Icon::Back, &title, "");
        look::foot(p, gl, &mut all, "[Tab] Next field  [Enter] Go on  [Esc] Back", self.trouble.as_deref());
        match self.stage {
            Stage::Pick => {
                look::shelf(p, gl, &mut all, "What kind", true);
                let tiles: Vec<Tile> = [("Jellyfin", "A name and a password"), ("Emby", "A name and a password"), ("Vibrefy", "A code, approved on a device signed in")]
                    .into_iter()
                    .zip([look::VIOLET, [0.32, 0.78, 0.42, 1.0], look::BLUE])
                    .map(|((title, detail), c)| Tile {
                        title: title.into(),
                        detail: Some(detail.into()),
                        tint: Some(([c[0], c[1], c[2], 0.55], [c[0] * 0.35, c[1] * 0.35, c[2] * 0.5, 0.35])),
                        ..Tile::default()
                    })
                    .collect();
                self.kinds.draw(p, gl, all.take_top(Strip::height(Shape::Wide, 300.0)), &tiles, 300.0);
            }
            Stage::Form(_) | Stage::Pair => {
                let count = self.count();
                let column = Rect { w: all.w.min(820.0), ..all };
                let mut column = column;
                // What is said under the form has the window's whole width: at the form's it was cut short.
                let wide = all.w;
                let code = CODE.lock().unwrap().clone();
                for (index, field) in self.fields.iter_mut().take(count).enumerate() {
                    field.focused = self.focus == index && self.busy.is_none();
                    field.draw(p, gl, column.take_top(Field::HEIGHT));
                    column.take_top(14.0);
                }
                column.take_top(8.0);
                let label = match self.stage {
                    Stage::Pair => "Get a code",
                    _ => "Sign in",
                };
                let width = Button::width(p, gl, label);
                self.go.focused = self.focus == count && self.busy.is_none();
                let mut row = column.take_top(Button::HEIGHT);
                if code.is_none() {
                    self.go.draw(p, gl, row.take_left(width), label);
                }
                if let Some(since) = self.busy {
                    look::waiting(p, gl, row.take_left(160.0), since);
                }
                if let Some(code) = code {
                    column.take_top(18.0);
                    let shown = vibrefy_api::pairing::PairingCode::format(&code);
                    p.label(gl, &shown, column.take_top(theme::TITLE_EM * 3.2), theme::TITLE_EM * 2.4, look::TEAL, 0.0);
                    p.label(
                        gl,
                        "Type this into Vibrefy on a device that is signed in. This screen goes on by itself.",
                        Rect { w: wide, ..column.take_top(theme::DETAIL_EM * 2.0) },
                        theme::DETAIL_EM,
                        theme::ink_detail(),
                        0.0,
                    );
                    if LINK.lock().unwrap().is_some() {
                        self.draw_link(p, gl, &mut column, wide);
                    }
                }
            }
        }
    }

    /// Under the code: the same pairing approved here, in SpatiMesh, by whoever's profile is chosen.
    fn draw_link(&mut self, p: &mut Painter, gl: &Gl, column: &mut Rect, wide: f32) {
        let asks = self.asks_whose();
        self.picked = self.picked.filter(|&at| at < self.profiles.len());
        if !asks {
            self.link_focus = 1;
        }
        column.take_top(22.0);
        let ask = if asks { "Or sign in here, in SpatiMesh. Whose profile?" } else { "Or sign in here, in SpatiMesh:" };
        p.label(gl, ask, Rect { w: wide, ..column.take_top(theme::DETAIL_EM * 2.0) }, theme::DETAIL_EM, theme::ink_detail(), 0.0);
        column.take_top(6.0);
        let mut row = column.take_top(Button::HEIGHT);
        let mut below = (row.x, row.bottom() + 6.0);
        if asks {
            // The drop-down: the chosen profile's colour and name, and the mark that it opens.
            let side = Button::HEIGHT * 0.36;
            let mark_room = theme::ROW_EM * 0.9 + 14.0;
            let mut widest = Button::width(p, gl, "Choose a profile");
            for profile in &self.profiles {
                widest = widest.max(Button::width(p, gl, &profile.name));
            }
            let width = (widest + side + 10.0 + mark_room).min(row.w * 0.6);
            let area = row.take_left(width);
            row.take_left(12.0);
            below = (area.x, area.bottom() + 6.0);
            self.whose.focused = self.link_focus == 0 && self.menu.is_none();
            self.whose.draw(p, gl, area, "");
            let ink = if self.whose.focused { theme::ink() } else { theme::ink_row() };
            let mut inside = Rect { x: area.x + theme::ROW_INSET * 0.7, w: area.w - theme::ROW_INSET * 0.7 - mark_room, ..area };
            let label = match self.picked.and_then(|at| self.profiles.get(at)) {
                Some(profile) => {
                    let [r, g, b] = profile.colour;
                    let dot = Rect::new(inside.x, area.y + (area.h - side) * 0.5, side, side);
                    p.fill(gl, dot, side * 0.5, [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]);
                    inside = Rect { x: inside.x + side + 10.0, w: inside.w - side - 10.0, ..inside };
                    profile.name.as_str()
                }
                None => "Choose a profile",
            };
            p.label(gl, label, inside, theme::ROW_EM * 0.86, ink, 0.0);
            let mark = theme::ROW_EM * 0.9;
            spatiand_app::ui::draw_icon(p, gl, Icon::Disclosed, Rect::new(area.right() - mark - 14.0, area.y + (area.h - mark) * 0.5, mark, mark), ink);
        }
        let label = "Sign in with SpatiMesh";
        let width = Button::width(p, gl, label);
        self.sign.focused = self.link_focus == 1 && self.menu.is_none();
        self.sign.draw(p, gl, row.take_left(width), label);
        if let Some(handed) = &self.handed {
            column.take_top(8.0);
            p.label(gl, handed, Rect { w: wide, ..column.take_top(theme::DETAIL_EM * 2.0) }, theme::DETAIL_EM, look::TEAL, 0.0);
        }
        // The drop-down's list, over everything under it.
        if let Some(menu) = self.menu.as_mut() {
            let rows: Vec<Row> = self.profiles.iter().map(|pr| Row::new(pr.name.clone())).collect();
            menu.at = below;
            let within = p.bounds();
            menu.draw(p, gl, &rows, within);
        }
    }

    /// While the code is up: the drop-down of profiles, and the button that hands the link to
    /// SpatiMesh in the one chosen.
    fn link_input(&mut self, input: &Input) -> Act {
        let Some(url) = LINK.lock().unwrap().clone() else { return Act::Nothing };
        if let Input::Pointer { x, y } = input {
            self.pointer = (*x, *y);
        }
        let asks = self.asks_whose();
        // Open, the list has everything.
        if let Some(menu) = self.menu.as_mut() {
            match menu.input(input, self.pointer) {
                Some(MenuSaid::Chose(at)) if at < self.profiles.len() => {
                    self.picked = Some(at);
                    self.menu = None;
                    self.link_focus = 1;
                    self.trouble = None;
                }
                Some(_) => self.menu = None,
                None => {}
            }
            return Act::Nothing;
        }
        if let Input::Key { key, .. } = input {
            match key {
                Key::Left | Key::Right | Key::Tab if asks => self.link_focus = 1 - self.link_focus.min(1),
                Key::Down | Key::Up if asks && self.link_focus == 0 => {
                    self.open_whose();
                    return Act::Nothing;
                }
                _ => {}
            }
        }
        self.whose.focused = asks && self.link_focus == 0;
        self.sign.focused = !asks || self.link_focus == 1;
        if asks && self.whose.input(input) {
            self.link_focus = 0;
            self.open_whose();
            return Act::Nothing;
        }
        if !self.sign.input(input) {
            return Act::Nothing;
        }
        self.link_focus = 1;
        let profile = if asks {
            // Whose is asked first: none is taken for granted.
            let Some(profile) = self.picked.and_then(|at| self.profiles.get(at)) else {
                self.link_focus = 0;
                self.open_whose();
                return Act::Nothing;
            };
            Some((profile.id.clone(), profile.name.clone()))
        } else {
            None
        };
        self.trouble = None;
        self.handed = Some(match &profile {
            Some((_, name)) => format!("Opened in SpatiMesh for {name}. Approve it there; this screen goes on by itself."),
            None => "Opened in SpatiMesh. Approve it there; this screen goes on by itself.".to_string(),
        });
        Act::OpenLink { url, profile: profile.map(|(id, _)| id) }
    }

    /// The drop-down's list, its keys on the profile named, or the first.
    fn open_whose(&mut self) {
        let mut menu = Menu::new((0.0, 0.0));
        menu.list.cursor = self.picked.unwrap_or(0);
        self.menu = Some(menu);
    }

    fn submit(&mut self) -> Act {
        let address = self.fields[0].text.trim().to_string();
        if address.is_empty() {
            self.trouble = Some("The server's address is wanted first.".into());
            self.focus = 0;
            return Act::Nothing;
        }
        self.trouble = None;
        self.busy = Some(Instant::now());
        match self.stage {
            Stage::Form(flavour) => Act::SignIn { flavour, address, user: self.fields[1].text.trim().to_string(), password: self.fields[2].text.clone() },
            Stage::Pair => {
                self.cancelled = Arc::default();
                // Whose profiles there are now: read for this pairing, and asked again at the next.
                self.profiles = spatiand_app::browser::profiles();
                self.picked = None;
                self.menu = None;
                self.link_focus = 0;
                self.handed = None;
                let address = if address.contains("://") { address } else { format!("https://{address}") };
                Act::Pair { address, cancelled: self.cancelled.clone() }
            }
            Stage::Pick => Act::Nothing,
        }
    }

    fn input(&mut self, input: &Input) -> Act {
        // The drop-down of profiles, open, has everything: Escape puts it away and leaves nothing.
        if self.menu.is_some() && self.busy.is_some() && matches!(self.stage, Stage::Pair) {
            return self.link_input(input);
        }
        self.menu = None;
        let leaving = self.back.input(input) || matches!(input, Input::Key { key: Key::Escape, .. });
        if leaving {
            // Whoever is waiting for a code to be approved is told to stop.
            self.cancelled.store(true, Ordering::Relaxed);
            forget_pairing();
            self.handed = None;
            if matches!(self.stage, Stage::Pick) {
                return Act::Home;
            }
            self.stage = Stage::Pick;
            self.busy = None;
            self.trouble = None;
            return Act::Nothing;
        }
        if matches!(self.stage, Stage::Pick) {
            if let Some(chosen) = self.kinds.input(input) {
                self.focus = 0;
                match chosen {
                    0 => self.stage = Stage::Form(Flavour::Jellyfin),
                    1 => self.stage = Stage::Form(Flavour::Emby),
                    _ => self.pair(),
                }
                self.trouble = None;
            }
            return Act::Nothing;
        }
        if self.busy.is_some() {
            return if matches!(self.stage, Stage::Pair) { self.link_input(input) } else { Act::Nothing };
        }
        let count = self.count();
        // A press on a field gives it the keys.
        if let Input::Pointer { x, y } = input {
            self.pointer = (*x, *y);
        }
        if let Input::Press { pressed: true, .. } = input {
            if let Some(index) = self.fields.iter().take(count).position(|field| field.contains(self.pointer.0, self.pointer.1)) {
                self.focus = index;
            }
        }
        if self.go.input(input) {
            return self.submit();
        }
        if let Input::Key { key, modifiers, .. } = input {
            match key {
                Key::Tab if modifiers.shift => self.focus = (self.focus + count) % (count + 1),
                Key::Tab | Key::Down => self.focus = (self.focus + 1) % (count + 1),
                Key::Up => self.focus = (self.focus + count) % (count + 1),
                _ => {}
            }
        }
        if self.focus < count {
            self.fields[self.focus].focused = true;
            if self.fields[self.focus].input(input) == Some(Typed::Entered) {
                // Enter goes on to the next thing to fill in, and from the last, signs in.
                if self.focus + 1 < count {
                    self.focus += 1;
                } else {
                    return self.submit();
                }
            }
        }
        Act::Nothing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(key: &str, folder: bool, modified: i64) -> Item {
        Item { key: key.into(), title: key.into(), folder, modified, pictured: !folder, ..Item::default() }
    }

    fn browse() -> Browse {
        let mut browse = Browse::new(0, "Vibrefy".into(), false, true);
        browse.listed(Ok(vec![item("Dir", true, 5), item("a", false, 10), item("b", false, 30), item("c", false, 20)]));
        browse
    }

    fn keys(browse: &Browse) -> Vec<&str> {
        browse.items.iter().map(|i| i.key.as_str()).collect()
    }

    #[test]
    fn a_folder_starts_by_name_and_is_sorted_by_what_is_asked() {
        let mut b = browse();
        assert_eq!(keys(&b), ["Dir", "a", "b", "c"]);
        b.arrange(Sort::Updated);
        assert_eq!(keys(&b), ["Dir", "b", "c", "a"]);
        b.arrange(Sort::Name);
        assert_eq!(keys(&b), ["Dir", "a", "b", "c"]);
        b.arrange(Sort::Random);
        assert_eq!(b.items[0].key, "Dir");
        let mut films = keys(&b)[1..].to_vec();
        films.sort();
        assert_eq!(films, ["a", "b", "c"]);
        // The tiles, the asked and the origins went with the items.
        assert_eq!(b.tiles.len(), 4);
        for (at, item) in b.items.iter().enumerate() {
            assert_eq!(["Dir", "a", "b", "c"][b.origin[at]], item.key);
            assert_eq!(b.tiles[at].title, item.title);
        }
        // Listing another folder starts by name again.
        b.loading();
        b.listed(Ok(vec![item("x", false, 1)]));
        assert_eq!(b.sort, Sort::Name);
    }

    #[test]
    fn remove_is_only_at_the_top_of_a_server() {
        let mut b = browse();
        assert!(b.can_remove());
        b.enter(item("Dir", true, 0), 0);
        assert!(!b.can_remove());
    }

    #[test]
    fn only_tiles_that_stay_on_show_are_asked_for() {
        let mut b = browse();
        // Nothing has been drawn, so nothing is on show.
        assert!(b.wanted().is_empty());
    }

    fn picture(width: u32, height: u32) -> Still {
        Still { width, height, rgba: vec![0; (width * height * 4) as usize] }
    }

    /// This machine's films: a cover or a frame, which shape is not known until it comes. The
    /// folder is as wide as films are until then, and takes the shape most of what has come has;
    /// a server's pictures, of the shape it said, change nothing.
    #[test]
    fn a_folder_of_this_machines_films_takes_the_shape_of_the_pictures_that_come() {
        let mut b = Browse::new(0, "This Deck".into(), true, false);
        b.listed(Ok(vec![item("Dir", true, 0), item("a.mkv", false, 0), item("b.mkv", false, 0), item("c.mp4", false, 0)]));
        assert_eq!(b.grid.shape, Shape::Wide);
        // The first cover: posters.
        b.pictured(1, picture(320, 480));
        assert_eq!(b.grid.shape, Shape::Tall);
        assert_eq!(b.arrived.len(), 1);
        // A frame of a film without one: one each, the posters kept (as Shape::of weighs a tie).
        b.pictured(3, picture(480, 270));
        assert_eq!(b.grid.shape, Shape::Tall);
        // Another frame: most are frames now.
        b.pictured(2, picture(480, 200));
        assert_eq!(b.grid.shape, Shape::Wide);
        // The same tile's picture again changes nothing: its shape is known.
        b.pictured(2, picture(320, 480));
        assert_eq!(b.grid.shape, Shape::Wide);
        // In whatever order the wearer has put them: by where each was in the list as it came.
        b.arrange(Sort::Random);
        assert!(b.items.iter().filter(|i| !i.folder).all(|i| i.aspect.is_some()));

        // A server that says its pictures' shapes: the folder keeps the shape it said.
        let mut b = Browse::new(0, "Jellyfin".into(), false, true);
        let poster = |key: &str| Item { aspect: Some(2.0 / 3.0), ..item(key, false, 0) };
        b.listed(Ok(vec![poster("a"), poster("b")]));
        assert_eq!(b.grid.shape, Shape::Tall);
        b.pictured(0, picture(480, 270));
        assert_eq!(b.grid.shape, Shape::Tall);
    }
}
