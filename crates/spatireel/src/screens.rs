//! The screens that are not a film: home, a source's folders, adding a server.
//!
//! A screen draws itself and answers what the wearer did with an [`Act`] -- what it would
//! like to happen -- and never does it. Listing a folder, signing in, playing a film all take
//! a network or a decoder, and are `main`'s to start and a thread's to do.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use spatiand_app::ui::{theme, Button, Icon, Field, Grid, Input, Painter, Rect, Shape, Strip, Texture, Tile, Typed};
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
            Screen::Browse(browse) if browse.ticket == ticket => browse.arrived.push((index, still)),
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
    pub trouble: Option<String>,
    /// Delete pressed once on this source, and when: pressed again soon, it is removed.
    removing: Option<(usize, Instant)>,
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
                detail: Some("Signed out \u{b7} reconnect".into()),
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
            trouble: None,
            removing: None,
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
        self.films.retain(|film| film.source != source);
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
        for (id, still) in self.arrived.drain(..) {
            if let Some(film) = self.films.iter_mut().find(|film| film.id == id) {
                film.tile.picture = p.texture(gl, &still.rgba, still.width, still.height).ok();
            }
        }
        let mut all = p.bounds().inset(theme::PAD);
        look::header(p, gl, &mut all, "SpatiReel", "");
        let hint = if self.on_films { "[Enter] Play from where it was left" } else { "[Enter] Open  [Delete] Remove a server" };
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
    }

    fn input(&mut self, input: &Input, sources: &[Entry]) -> Act {
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
                        // Twice, as the server's own Remove button is: once is easily a slip.
                        if self.removing.is_some_and(|(was, since)| was == at && since.elapsed() < Duration::from_secs(4)) {
                            self.removing = None;
                            return Act::Forget(at);
                        }
                        self.removing = Some((at, Instant::now()));
                        let name = self.places.get(at).map_or(String::new(), |t| t.title.clone());
                        self.trouble = Some(format!("Press Delete again to remove {name}"));
                        return Act::Nothing;
                    }
                }
                _ => {
                    if self.removing.take().is_some() {
                        self.trouble = None;
                    }
                }
            }
        }
        // A press on a shelf gives it the keys as well.
        if let Input::Press { pressed: true } = input {
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
    /// A server's own page has a way to remove it: pressed once it asks, twice it is done.
    server: bool,
    remove: Button,
    removing: Option<Instant>,
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
            removing: None,
            trouble: None,
        }
    }

    /// Whether the remove button is waiting for its second press: for a few seconds only.
    fn asking_to_remove(&self) -> bool {
        self.removing.is_some_and(|since| since.elapsed() < Duration::from_secs(4))
    }

    pub fn folder(&self) -> Option<&Item> {
        self.trail.last().map(|(item, _)| item)
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

    /// A picture not fetched, because its tile was no longer on show: it is not asked for.
    fn passed(&mut self, origin: usize) {
        if let Some(at) = self.origin.iter().position(|o| *o == origin) {
            self.asked[at] = false;
        }
    }

    pub fn listed(&mut self, result: Result<Vec<Item>, String>) {
        match result {
            Err(e) => self.state = State::Failed(e),
            Ok(items) => {
                // Posters where most of what is here has posters, frames where it has
                // frames: by the pictures' own shapes, whatever kind of server sent them.
                self.grid.shape = Shape::of(items.iter().filter(|i| i.pictured).filter_map(|i| i.aspect), Shape::Wide);
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
        self.folder().map_or_else(library::home, |item| item.key.clone().into())
    }

    fn draw(&mut self, p: &mut Painter, gl: &Gl) {
        for old in self.left.drain(..) {
            p.forget(gl, old);
        }
        for (origin, still) in self.arrived.drain(..) {
            if let Some(tile) = self.origin.iter().position(|o| *o == origin).and_then(|at| self.tiles.get_mut(at)) {
                tile.picture = p.texture(gl, &still.rgba, still.width, still.height).ok();
            }
        }
        let mut all = p.bounds().inset(theme::PAD);
        // The ways out, then where this is.
        let mut top = all.take_top(Button::HEIGHT);
        let width = Button::icon_width(p, gl, "Home");
        self.home.draw_icon(p, gl, top.take_left(width), Icon::Home, "Home");
        top.take_left(12.0);
        if self.local || !self.trail.is_empty() {
            self.up.draw_icon(p, gl, top.take_left(Button::HEIGHT), Icon::Up, "");
            top.take_left(22.0);
        }
        // Taking a server away is for its top, where it is not mistaken for anything else: in
        // a folder it is a button where a slip of the pointer is all it takes.
        if self.can_remove() {
            let label = if self.asking_to_remove() { "Press again to remove" } else { "Remove" };
            let width = Button::icon_width(p, gl, label);
            self.remove.draw_icon(p, gl, top.take_right(width), Icon::Trash, label);
            top.take_right(16.0);
        }
        if matches!(self.state, State::Ready) && !self.local && self.items.len() > 1 {
            // Right to left, so the first reads first.
            for (button, sort) in self.sorts.iter_mut().zip(Sort::ALL).rev() {
                if sort == Sort::Updated && !self.items.iter().any(|item| item.modified > 0) {
                    button.focused = false;
                    continue;
                }
                button.focused = self.sort == sort;
                button.draw_icon(p, gl, top.take_right(Button::HEIGHT), sort.icon(), "");
                top.take_right(8.0);
            }
            top.take_right(8.0);
        }
        let title = if self.local {
            let path = self.path();
            path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
        } else {
            self.folder().map_or(self.label.clone(), |item| item.title.clone())
        };
        let taken = p.label(gl, &title, top, theme::TITLE_EM * 0.9, theme::ink(), 0.0);
        if title != self.label {
            let beside = Rect { x: top.x + taken + 20.0, w: (top.w - taken - 20.0).max(0.0), y: top.y + 5.0, ..top };
            p.label(gl, &self.label, beside, theme::DETAIL_EM, theme::ink_hint(), 0.0);
        }
        all.take_top(12.0);
        look::rule(p, gl, all.take_top(2.0));
        all.take_top(16.0);
        let hint = if self.local { self.path().display().to_string() } else { "[Enter] Open  [Backspace] Up  [Esc] Home".to_string() };
        look::foot(p, gl, &mut all, &hint, self.trouble.as_deref());
        match &self.state {
            State::Loading(since) => look::waiting(p, gl, all, *since),
            State::Failed(why) => look::message(p, gl, all, why, look::trouble()),
            State::Ready if self.tiles.is_empty() => look::message(p, gl, all, "Nothing here to watch.", theme::ink_detail()),
            State::Ready => self.grid.draw(p, gl, all, &self.tiles),
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
            if self.asking_to_remove() {
                return Act::Forget(self.source);
            }
            self.removing = Some(Instant::now());
            return Act::Nothing;
        }
        if matches!(input, Input::Key { .. }) {
            // A key is not the second press; a press elsewhere leaves the screen or lapses.
            self.removing = None;
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
fn vibrefy_name(address: &str) -> String {
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
}

impl Add {
    pub fn new(last: Option<String>) -> Add {
        *CODE.lock().unwrap() = None;
        let mut kinds = Strip::default();
        kinds.shape = Shape::Wide;
        kinds.focused = true;
        let mut fields = [Field::default(), Field::default(), Field::default()];
        fields[0].hint = "Address, like films.example.org or 192.168.1.20".into();
        fields[1].hint = "Name".into();
        fields[2].hint = "Password".into();
        fields[2].secret = true;
        Add { stage: Stage::Pick, kinds, fields, focus: 0, go: Button::default(), back: Button::default(), busy: None, cancelled: Arc::default(), trouble: None, pointer: (0.0, 0.0), last }
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
        *CODE.lock().unwrap() = None;
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
        let mut top = all.take_top(Button::HEIGHT);
        let width = Button::icon_width(p, gl, "Back");
        self.back.draw_icon(p, gl, top.take_left(width), Icon::Back, "Back");
        top.take_left(22.0);
        let title = match self.stage {
            Stage::Pick => "Add a server".to_string(),
            Stage::Form(flavour) => format!("Sign in to {}", flavour.label()),
            Stage::Pair => "Pair with Vibrefy".to_string(),
        };
        p.label(gl, &title, top, theme::TITLE_EM * 0.9, theme::ink(), 0.0);
        all.take_top(12.0);
        look::rule(p, gl, all.take_top(2.0));
        all.take_top(22.0);
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
                        column.take_top(theme::DETAIL_EM * 2.0),
                        theme::DETAIL_EM,
                        theme::ink_detail(),
                        0.0,
                    );
                }
            }
        }
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
                let address = if address.contains("://") { address } else { format!("https://{address}") };
                Act::Pair { address, cancelled: self.cancelled.clone() }
            }
            Stage::Pick => Act::Nothing,
        }
    }

    fn input(&mut self, input: &Input) -> Act {
        let leaving = self.back.input(input) || matches!(input, Input::Key { key: Key::Escape, .. });
        if leaving {
            // Whoever is waiting for a code to be approved is told to stop.
            self.cancelled.store(true, Ordering::Relaxed);
            *CODE.lock().unwrap() = None;
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
            return Act::Nothing;
        }
        let count = self.count();
        // A press on a field gives it the keys.
        if let Input::Pointer { x, y } = input {
            self.pointer = (*x, *y);
        }
        if let Input::Press { pressed: true } = input {
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
}
