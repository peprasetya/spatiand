//! A folder's pictures, documents and objects, as tiles to choose from.

use std::path::{Path, PathBuf};
use std::time::Instant;

use spatiand_app::ui::{look, theme, Button, Grid, Input, Painter, Rect, Shape, Texture, Tile};
use spatiand_app::{Gl, Key};

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

fn is_package(name: &str) -> bool {
    let Some((_, ending)) = name.rsplit_once('.') else { return false };
    let ending = ending.to_lowercase();
    ending.ends_with("library") || ["app", "bundle", "framework", "plugin", "kext", "photoslibrary"].contains(&ending.as_str())
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
    let read = std::fs::read_dir(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let mut entries: Vec<Entry> = read
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let path = entry.path();
            let meta = std::fs::metadata(&path).ok()?;
            let kind = if meta.is_dir() {
                if is_package(&name) {
                    return None;
                }
                Kind::Folder
            } else {
                kind_of(&path)?
            };
            Some(Entry { name, path, kind, size: meta.len() })
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
    asked: Vec<bool>,
    grid: Grid,
    pub state: State,
    pub ticket: u64,
    /// The name to put the cursor on when the list comes: the folder come up out of, or the
    /// file just looked at.
    pub cursor_on: Option<PathBuf>,
    arrived: Vec<(usize, u32, u32, Vec<u8>, Option<String>)>,
    left: Vec<Texture>,
    up: Button,
}

impl Folder {
    pub fn new(path: PathBuf, ticket: u64) -> Folder {
        let mut grid = Grid::default();
        grid.focused = true;
        grid.shape = Shape::Square;
        Folder { path, entries: Vec::new(), tiles: Vec::new(), asked: Vec::new(), grid, state: State::Loading(Instant::now()), ticket, cursor_on: None, arrived: Vec::new(), left: Vec::new(), up: Button::default() }
    }

    pub fn listed(&mut self, result: Result<Vec<Entry>, String>) {
        match result {
            Err(e) => self.state = State::Failed(e),
            Ok(entries) => {
                self.left.extend(self.tiles.drain(..).filter_map(|t| t.picture));
                self.tiles = entries
                    .iter()
                    .map(|e| {
                        let (glyph, tint, detail) = match e.kind {
                            Kind::Folder => (Some("\u{25a4}".to_string()), Some(([0.20, 0.55, 0.62, 0.40], [0.10, 0.22, 0.32, 0.25])), None),
                            Kind::Model => (Some("\u{25c6}".to_string()), Some(([0.45, 0.36, 0.85, 0.45], [0.18, 0.14, 0.40, 0.25])), Some(format!("3D object  \u{b7}  {}", size_words(e.size)))),
                            Kind::Pdf => (None, None, Some(format!("PDF  \u{b7}  {}", size_words(e.size)))),
                            Kind::Picture => (None, None, Some(size_words(e.size))),
                        };
                        Tile { title: e.name.clone(), detail, glyph, tint, ..Tile::default() }
                    })
                    .collect();
                self.asked = vec![false; entries.len()];
                let at = self.cursor_on.take().and_then(|p| entries.iter().position(|e| e.path == p));
                self.grid.cursor = at.unwrap_or(0).min(entries.len().saturating_sub(1));
                self.entries = entries;
                self.state = State::Ready;
            }
        }
    }

    /// A tile's picture has come, and perhaps a word to put under it ("3D").
    pub fn pictured(&mut self, ticket: u64, index: usize, width: u32, height: u32, rgba: Vec<u8>, word: Option<String>) {
        if ticket == self.ticket {
            self.arrived.push((index, width, height, rgba, word));
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
        for (index, w, h, rgba, word) in self.arrived.drain(..) {
            if let Some(tile) = self.tiles.get_mut(index) {
                tile.picture = p.texture(gl, &rgba, w, h).ok();
                if let Some(word) = word {
                    tile.detail = Some(match tile.detail.take() {
                        Some(d) => format!("{word}  \u{b7}  {d}"),
                        None => word,
                    });
                }
            }
        }
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl) {
        self.take_arrived(p, gl);
        let mut all = p.bounds().inset(theme::PAD);
        let mut top = all.take_top(Button::HEIGHT);
        if self.path.parent().is_some() {
            let width = Button::width(p, gl, "\u{2191}  Up");
            self.up.draw(p, gl, top.take_left(width), "\u{2191}  Up");
            top.take_left(22.0);
        }
        let name = self.path.file_name().map_or_else(|| self.path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let taken = p.label(gl, &name, top, theme::TITLE_EM * 0.9, theme::INK, 0.0);
        let count = match self.state {
            State::Ready => {
                let files = self.entries.iter().filter(|e| e.kind != Kind::Folder).count();
                if files == 1 { "1 item".to_string() } else { format!("{files} items") }
            }
            _ => String::new(),
        };
        let beside = Rect { x: top.x + taken + 20.0, w: (top.w - taken - 20.0).max(0.0), y: top.y + 5.0, ..top };
        p.label(gl, &format!("SpatiLoupe   {count}"), beside, theme::DETAIL_EM, theme::INK_HINT, 0.0);
        all.take_top(12.0);
        look::rule(p, gl, all.take_top(2.0));
        all.take_top(16.0);
        look::foot(p, gl, &mut all, &self.path.display().to_string(), None);
        match &self.state {
            State::Loading(since) => look::waiting(p, gl, all, *since),
            State::Failed(why) => look::message(p, gl, all, why, look::TROUBLE),
            State::Ready if self.tiles.is_empty() => look::message(p, gl, all, "No pictures, documents or objects here.", theme::INK_DETAIL),
            State::Ready => self.grid.draw(p, gl, all, &self.tiles),
        }
    }

    pub fn input(&mut self, input: &Input) -> Chose {
        if self.up.input(input) || matches!(input, Input::Key { key: Key::Backspace, .. }) {
            if let Some(parent) = self.path.parent() {
                return Chose::Folder(parent.to_path_buf());
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

    #[test]
    fn sizes_read_as_a_person_says_them() {
        assert_eq!(size_words(512), "512 bytes");
        assert_eq!(size_words(1_400_000), "1.4 MB");
        assert_eq!(size_words(23_000_000_000), "23 GB");
    }
}
