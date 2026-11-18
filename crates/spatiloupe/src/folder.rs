//! A folder's pictures, documents and objects, as tiles to choose from.

use std::path::{Path, PathBuf};
use std::time::Instant;

use spatiand_app::ui::{look, theme, Button, Icon, Grid, Input, Painter, Rect, Shape, Texture, Tile};
use spatiand_app::{Gl, Key};

use crate::view::Seen;

/// What a file is, to SpatiLoupe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Folder,
    Picture,
    Pdf,
    Model,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub kind: Kind,
    /// Bytes, for a file.
    pub size: u64,
}

pub fn kind_of(path: &Path) -> Option<Kind> {
    let ending = path.extension()?.to_str()?.to_ascii_lowercase();
    if crate::picture::PICTURES.contains(&ending.as_str()) {
        Some(Kind::Picture)
    } else if ending == "pdf" {
        Some(Kind::Pdf)
    } else if ending == "glb" || ending == "gltf" || ending == "vrm" {
        Some(Kind::Model)
    } else {
        None
    }
}

/// A Mac's package -- an application, a Photos library -- which Finder shows as one thing and
/// SpatiLoupe does not go into. Only on a Mac, and only by these endings: elsewhere a folder
/// called "v1.app" or "St. Louis Library" is a folder.
fn is_package(name: &str) -> bool {
    const PACKAGES: &[&str] = &["app", "bundle", "framework", "plugin", "kext", "photoslibrary", "photolibrary", "migratedphotolibrary", "aplibrary", "musiclibrary", "tvlibrary", "imovielibrary", "fcpbundle"];
    let Some((_, ending)) = name.rsplit_once('.') else { return false };
    cfg!(target_os = "macos") && PACKAGES.contains(&ending.to_lowercase().as_str())
}

/// Where pictures are kept on this kind of machine.
pub fn home() -> PathBuf {
    if let Some(named) = std::env::var_os("SPATILOUPE_FOLDER") {
        return named.into();
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into());
    let usual: &[&str] = if cfg!(target_os = "android") { &["/sdcard/DCIM", "/sdcard/Pictures", "/sdcard"] } else { &["Pictures"] };
    usual.iter().map(|f| home.join(f)).find(|p| p.is_dir()).unwrap_or(home)
}

/// A folder's folders, then the files SpatiLoupe shows, each by name.
pub fn list(folder: &Path) -> Result<Vec<Entry>, String> {
    let mut entries: Vec<Entry> = crate::place::list(folder)?
        .into_iter()
        .filter_map(|(name, path, is_dir, size)| {
            if name.starts_with('.') {
                return None;
            }
            let kind = if is_dir {
                if is_package(&name) {
                    return None;
                }
                Kind::Folder
            } else {
                kind_of(&path)?
            };
            Some(Entry { name, path, kind, size })
        })
        .collect();
    entries.sort_by(|a, b| (a.kind != Kind::Folder).cmp(&(b.kind != Kind::Folder)).then_with(|| natural(&a.name, &b.name)));
    Ok(entries)
}

/// Names in the order a person would put them: "IMG_9" before "IMG_10".
pub fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut n = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        n.push(c);
                        it.next();
                    }
                    n
                };
                let (m, n) = (take(&mut a), take(&mut b));
                let (m, n) = (m.trim_start_matches('0'), n.trim_start_matches('0'));
                let order = m.len().cmp(&n.len()).then_with(|| m.cmp(n));
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

pub fn size_words(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// An entry's tile before its picture has come: its name, what it is, and how big.
fn tile_of(e: &Entry) -> Tile {
    let (icon, tint, detail) = match e.kind {
        Kind::Folder => (Some(Icon::Folder), Some(([0.20, 0.55, 0.62, 0.40], [0.10, 0.22, 0.32, 0.25])), None),
        Kind::Model => (Some(Icon::Cube), Some(([0.45, 0.36, 0.85, 0.45], [0.18, 0.14, 0.40, 0.25])), Some(format!("3D object  \u{b7}  {}", size_words(e.size)))),
        Kind::Pdf => (Some(Icon::Document), None, Some(format!("PDF  \u{b7}  {}", size_words(e.size)))),
        Kind::Picture => (Some(Icon::Picture), None, Some(size_words(e.size))),
    };
    Tile { title: e.name.clone(), detail, icon, tint, ..Tile::default() }
}

pub enum State {
    Loading(Instant),
    Failed(String),
    Ready,
}

/// What choosing in a folder comes to.
pub enum Chose {
    Nothing,
    /// Go into this folder, or up to it.
    Folder(PathBuf),
    /// Look at the file at this place in the list.
    Open(usize),
}

/// The folder screen.
pub struct Folder {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
    tiles: Vec<Tile>,
    /// What each tile's picture said of the one it was made from, once it has come: what a tile
    /// standing in for its picture is drawn as.
    seen: Vec<Option<Seen>>,
    asked: Vec<bool>,
    grid: Grid,
    pub state: State,
    pub ticket: u64,
    /// The name to put the cursor on when the list comes: the folder come up out of, or the
    /// file just looked at.
    pub cursor_on: Option<PathBuf>,
    arrived: Vec<Arrived>,
    left: Vec<Texture>,
    up: Button,
    /// Moved on whenever a tile changes -- the listing, a picture come or let go of -- so that
    /// the viewer's strip is made again from them then, and only then.
    pub generation: u64,
}

/// How many entries beyond those in view keep their tiles' pictures on the GPU: further off, they
/// are let go of, and made again if they are scrolled back to.
const TILES_KEPT: usize = 120;

/// A tile's picture, come from the thread that makes them, to go on the GPU.
pub struct Arrived {
    pub index: usize,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// A word to put under it ("3D").
    pub word: Option<String>,
    pub seen: Option<Seen>,
}

impl Folder {
    pub fn new(path: PathBuf, ticket: u64) -> Folder {
        let mut grid = Grid::default();
        grid.focused = true;
        grid.shape = Shape::Square;
        Folder {
            path,
            entries: Vec::new(),
            tiles: Vec::new(),
            seen: Vec::new(),
            asked: Vec::new(),
            grid,
            state: State::Loading(Instant::now()),
            ticket,
            cursor_on: None,
            arrived: Vec::new(),
            left: Vec::new(),
            up: Button::default(),
            generation: 0,
        }
    }

    /// This folder is giving way to `next`: its tiles' pictures go with it, for `next` to let go
    /// of on the GPU when it is next drawn, rather than being left there for ever.
    pub fn give_way(&mut self, next: &mut Folder) {
        next.left.extend(self.left.drain(..).chain(self.tiles.drain(..).filter_map(|t| t.picture)));
    }

    /// Let go of the tiles' pictures far from `near`, the entries on show: they are asked for
    /// again if they come back into view.
    pub fn let_go_far(&mut self, near: std::ops::Range<usize>) {
        if near.is_empty() {
            return;
        }
        let keep = near.start.saturating_sub(TILES_KEPT)..near.end.saturating_add(TILES_KEPT);
        for (index, tile) in self.tiles.iter_mut().enumerate() {
            if !keep.contains(&index) {
                if let Some(texture) = tile.picture.take() {
                    self.left.push(texture);
                    self.asked[index] = false;
                    self.generation += 1;
                }
            }
        }
    }

    pub fn listed(&mut self, result: Result<Vec<Entry>, String>) {
        match result {
            Err(e) => self.state = State::Failed(e),
            Ok(entries) => {
                // Listed again: what was there and is the same keeps its tile, picture and all,
                // and the cursor stays on what it was on.
                let on = self.cursor_on.take().or_else(|| self.entries.get(self.grid.cursor).map(|e| e.path.clone()));
                let mut had: std::collections::HashMap<PathBuf, (Entry, Tile, Option<Seen>)> = std::collections::HashMap::new();
                for ((entry, tile), seen) in self.entries.drain(..).zip(self.tiles.drain(..)).zip(self.seen.drain(..)) {
                    had.insert(entry.path.clone(), (entry, tile, seen));
                }
                // Tiles on their way were asked for by the places of the listing before.
                self.arrived.clear();
                self.tiles.clear();
                self.seen.clear();
                self.asked.clear();
                for entry in &entries {
                    match had.remove(&entry.path).filter(|(was, _, _)| was == entry) {
                        Some((_, tile, seen)) => {
                            self.asked.push(tile.picture.is_some());
                            self.tiles.push(tile);
                            self.seen.push(seen);
                        }
                        None => {
                            self.tiles.push(tile_of(entry));
                            self.seen.push(None);
                            self.asked.push(false);
                        }
                    }
                }
                self.left.extend(had.into_values().filter_map(|(_, tile, _)| tile.picture));
                let at = on.and_then(|p| entries.iter().position(|e| e.path == p));
                self.grid.cursor = at.unwrap_or(0).min(entries.len().saturating_sub(1));
                self.entries = entries;
                self.state = State::Ready;
                self.generation += 1;
            }
        }
    }

    /// One more entry, not in the listing -- a file asked for by name -- with its tile; returns
    /// where it is. Its tile and its having been asked for are kept beside it, as the listing's
    /// are, or every list indexed by entry runs one short.
    pub fn add(&mut self, entry: Entry) -> usize {
        self.tiles.push(tile_of(&entry));
        self.generation += 1;
        self.seen.push(None);
        self.asked.push(false);
        self.entries.push(entry);
        self.entries.len() - 1
    }

    /// A tile's picture has come, for the listing of `ticket`.
    pub fn pictured(&mut self, ticket: u64, arrived: Arrived) {
        if ticket == self.ticket {
            self.arrived.push(arrived);
        }
    }

    /// The pictures wanted for what is on show, each asked for once: (index, path, kind).
    pub fn wanted(&mut self) -> Vec<(usize, PathBuf, Kind)> {
        let mut wanted = Vec::new();
        if !matches!(self.state, State::Ready) {
            return wanted;
        }
        for index in self.grid.in_view() {
            let Some(entry) = self.entries.get(index) else { break };
            if matches!(entry.kind, Kind::Picture | Kind::Pdf) && !std::mem::replace(&mut self.asked[index], true) {
                wanted.push((index, entry.path.clone(), entry.kind));
            }
        }
        wanted
    }

    /// The entries on show, as last drawn.
    pub fn in_view(&self) -> std::ops::Range<usize> {
        self.grid.in_view()
    }

    pub fn tiles(&self) -> &[Tile] {
        &self.tiles
    }

    /// Put the cursor on the entry at `path`: the file just looked at, coming back.
    pub fn point_at(&mut self, path: &Path) {
        if let Some(at) = self.entries.iter().position(|e| e.path == path) {
            self.grid.cursor = at;
        }
    }

    /// The pictures wanted for the files near `index`, for the viewer's strip.
    pub fn wanted_near(&mut self, index: usize, reach: usize) -> Vec<(usize, PathBuf, Kind)> {
        let mut wanted = Vec::new();
        if !matches!(self.state, State::Ready) {
            return wanted;
        }
        for at in index.saturating_sub(reach)..(index + reach + 1).min(self.entries.len()) {
            let entry = &self.entries[at];
            if matches!(entry.kind, Kind::Picture | Kind::Pdf) && !std::mem::replace(&mut self.asked[at], true) {
                wanted.push((at, entry.path.clone(), entry.kind));
            }
        }
        wanted
    }

    pub fn animating(&self) -> bool {
        matches!(self.state, State::Loading(_))
    }

    /// Put the tiles' pictures that have come on the GPU: when the folder is drawn, and when
    /// the viewer's strip is.
    pub fn take_arrived(&mut self, p: &mut Painter, gl: &Gl) {
        for old in self.left.drain(..) {
            p.forget(gl, old);
        }
        for Arrived { index, width, height, rgba, word, seen } in self.arrived.drain(..) {
            self.generation += 1;
            if let Some(tile) = self.tiles.get_mut(index) {
                if let Some(old) = tile.picture.take() {
                    p.forget(gl, old);
                }
                tile.picture = p.texture(gl, &rgba, width, height).ok();
                if let Some(word) = word {
                    tile.detail = Some(match tile.detail.take() {
                        Some(d) => format!("{word}  \u{b7}  {d}"),
                        None => word,
                    });
                }
            }
            if let Some(kept) = self.seen.get_mut(index) {
                *kept = seen;
            }
        }
    }

    /// What the files' tiles have said of their pictures, in the order of
    /// [`Folder::file_tiles`]: the files only.
    pub fn file_seen(&self) -> Vec<Option<Seen>> {
        self.entries.iter().zip(&self.seen).filter(|(e, _)| e.kind != Kind::Folder).map(|(_, s)| *s).collect()
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl) {
        self.take_arrived(p, gl);
        let mut all = p.bounds().inset(theme::PAD);
        let mut top = all.take_top(Button::HEIGHT);
        if crate::place::parent(&self.path).is_some() {
            self.up.draw_icon(p, gl, top.take_left(Button::HEIGHT), Icon::Up, "");
            top.take_left(22.0);
        }
        let name = crate::place::name(&self.path);
        let taken = p.label(gl, &name, top, theme::TITLE_EM * 0.9, theme::ink(), 0.0);
        let count = match self.state {
            State::Ready => {
                let files = self.entries.iter().filter(|e| e.kind != Kind::Folder).count();
                if files == 1 { "1 item".to_string() } else { format!("{files} items") }
            }
            _ => String::new(),
        };
        let beside = Rect { x: top.x + taken + 20.0, w: (top.w - taken - 20.0).max(0.0), y: top.y + 5.0, ..top };
        p.label(gl, &format!("SpatiLoupe   {count}"), beside, theme::DETAIL_EM, theme::ink_hint(), 0.0);
        all.take_top(12.0);
        look::rule(p, gl, all.take_top(2.0));
        all.take_top(16.0);
        look::foot(p, gl, &mut all, &self.path.display().to_string(), None);
        match &self.state {
            State::Loading(since) => look::waiting(p, gl, all, *since),
            State::Failed(why) => look::message(p, gl, all, why, look::trouble()),
            State::Ready if self.tiles.is_empty() => look::message(p, gl, all, "No pictures, documents or objects here.", theme::ink_detail()),
            State::Ready => self.grid.draw(p, gl, all, &self.tiles),
        }
    }

    pub fn input(&mut self, input: &Input) -> Chose {
        if self.up.input(input) || matches!(input, Input::Key { key: Key::Backspace, .. }) {
            if let Some(parent) = crate::place::parent(&self.path) {
                return Chose::Folder(parent);
            }
            return Chose::Nothing;
        }
        if !matches!(self.state, State::Ready) {
            return Chose::Nothing;
        }
        let Some(chosen) = self.grid.input(input) else { return Chose::Nothing };
        match self.entries.get(chosen) {
            Some(entry) if entry.kind == Kind::Folder => Chose::Folder(entry.path.clone()),
            Some(_) => Chose::Open(chosen),
            None => Chose::Nothing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_in_names_are_in_number_order() {
        let mut names = vec!["IMG_10.jpg", "img_9.jpg", "IMG_1.jpg", "b.png", "A.png"];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(names, ["A.png", "b.png", "IMG_1.jpg", "img_9.jpg", "IMG_10.jpg"]);
    }

    #[test]
    fn only_what_can_be_shown_is_listed_and_folders_first() {
        let dir = std::env::temp_dir().join(format!("spatiloupe-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("trip")).unwrap();
        for name in ["b.HEIC", "a.jpg", "notes.txt", "helmet.glb", "paper.pdf", ".hidden.png"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let names: Vec<String> = list(&dir).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["trip", "a.jpg", "b.HEIC", "helmet.glb", "paper.pdf"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn entry(name: &str, size: u64) -> Entry {
        Entry { name: name.into(), path: PathBuf::from("/f").join(name), kind: kind_of(Path::new(name)).unwrap_or(Kind::Folder), size }
    }

    #[test]
    fn listed_again_the_same_entries_keep_their_tiles_and_the_cursor_its_entry() {
        let mut folder = Folder::new(PathBuf::from("/f"), 1);
        folder.listed(Ok(vec![entry("a.jpg", 1), entry("b.jpg", 1), entry("c.jpg", 1)]));
        folder.grid.cursor = 1;
        // As if b's and c's pictures had come.
        folder.tiles[1].detail = Some("b's".into());
        folder.seen[1] = Some(Seen { aspect: 2.0, height: 10.0, stereo: false });
        folder.asked[1] = true;
        folder.asked[2] = true;
        let generation = folder.generation;
        // A new file before them, and c changed.
        folder.listed(Ok(vec![entry("0.jpg", 1), entry("a.jpg", 1), entry("b.jpg", 1), entry("c.jpg", 2)]));
        assert!(folder.generation > generation);
        assert_eq!(folder.grid.cursor, 2, "still on b");
        assert_eq!(folder.tiles[2].detail.as_deref(), Some("b's"));
        assert_eq!(folder.seen[2].map(|s| s.aspect), Some(2.0));
        // c is new as it is now, and asked for again; b, with no picture yet, too.
        assert_eq!(folder.asked, [false, false, false, false]);
        assert_eq!(folder.tiles.len(), 4);
    }

    #[test]
    fn only_a_mac_s_packages_are_passed_over() {
        assert!(!is_package("St. Louis Library"));
        assert!(!is_package("Lib.Library"));
        assert!(!is_package("trip"));
        assert_eq!(is_package("Photos Library.photoslibrary"), cfg!(target_os = "macos"));
        assert_eq!(is_package("v1.app"), cfg!(target_os = "macos"));
    }

    #[cfg(unix)]
    #[test]
    fn a_name_that_is_not_utf8_is_listed_where_it_is() {
        use std::os::unix::ffi::OsStrExt;
        let dir = std::env::temp_dir().join(format!("spatiloupe-bytes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // "café.png" in Latin-1, as an old camera card or a zip may name it.
        let name = std::ffi::OsStr::from_bytes(b"caf\xe9.png");
        std::fs::write(dir.join(name), b"x").unwrap();
        let entries = list(&dir).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, dir.join(name));
        assert!(entries[0].path.is_file());
        assert_eq!(entries[0].name, "caf\u{FFFD}.png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sizes_read_as_a_person_says_them() {
        assert_eq!(size_words(512), "512 bytes");
        assert_eq!(size_words(1_400_000), "1.4 MB");
        assert_eq!(size_words(23_000_000_000), "23 GB");
    }
}
