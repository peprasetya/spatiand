//! Spatiand's file chooser: a window of the room in which the wearer chooses files, a folder, or a
//! name to save under, for a program that asked (SpatiMesh's Chromium on the Beam Pro, where there
//! is no desktop portal to ask: third_party/chromium/android-libc/0019).
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

use std::path::{Path, PathBuf};

use spatiand_app::ui::{draw_icon, look, theme, Field, Icon, Input, List, Painter, Rect, Row};
use spatiand_app::{App, Event, Key};

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
    Go(PathBuf),
    File(PathBuf),
    ShowAll,
}

struct Ask {
    mode: Mode,
    title: String,
    dir: PathBuf,
    name: String,
    /// The endings asked for, lower case without the dot; empty for any.
    accept: Vec<String>,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into()))
}

fn ask_from_args() -> Result<Ask, String> {
    let mut ask = Ask { mode: Mode::Open, title: String::new(), dir: home(), name: String::new(), accept: Vec::new() };
    let mut args = std::env::args().skip(1);
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
            "--dir" => ask.dir = PathBuf::from(value()?),
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
    if !ask.dir.is_dir() {
        ask.dir = home();
    }
    Ok(ask)
}

struct Chooser {
    ask: Ask,
    dir: PathBuf,
    field: Field,
    list: List,
    rows: Vec<Row>,
    targets: Vec<Target>,
    chosen: Vec<PathBuf>,
    all_kinds: bool,
    said: Option<String>,
    /// A name to save under that is there already, once the wearer has been told.
    replacing: Option<PathBuf>,
    ok: Rect,
    cancel: Rect,
    stale: bool,
    /// The cursor to go to the folder's own first line when the lines are next made: on opening,
    /// on going to a folder, and on finding (so that Enter takes what was found).
    to_folder: bool,
}

impl Chooser {
    fn new(ask: Ask) -> Chooser {
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
        Chooser {
            dir: ask.dir.clone(),
            ask,
            field,
            list,
            rows: Vec::new(),
            targets: Vec::new(),
            chosen: Vec::new(),
            all_kinds: false,
            said: None,
            replacing: None,
            ok: Rect::default(),
            cancel: Rect::default(),
            stale: true,
            to_folder: true,
        }
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

    /// In the field, for opening: words to find among the folder's lines (not a path, not a name
    /// to save under).
    fn filter(&self) -> String {
        let typed = &self.field.text;
        if self.ask.mode == Mode::Save || typed.starts_with('/') || typed.starts_with('~') {
            String::new()
        } else {
            typed.to_lowercase()
        }
    }

    fn build(&mut self) {
        self.stale = false;
        let filter = self.filter();
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
            let here = path == self.dir;
            rows.push(Row::with(name, if here { "\u{2713} here" } else { "" }).icon(icon));
            targets.push(Target::Go(path));
        }
        rows.push(Row::heading(self.dir.display().to_string()));
        targets.push(Target::Nothing);
        if let Some(up) = self.dir.parent() {
            // The folder's name alone: a whole path runs over the line's own label.
            let name = up.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into());
            rows.push(Row::with("\u{2039} Up", name).icon(Icon::Up));
            targets.push(Target::Go(up.to_path_buf()));
        }
        // The folder's own first line (or Up, in an empty folder): where the cursor starts.
        let first = rows.len();
        let mut entries: Vec<(String, PathBuf, bool)> = std::fs::read_dir(&self.dir)
            .map(|d| {
                d.flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        let path = e.path();
                        (!name.starts_with('.')).then(|| (name, path.clone(), path.is_dir()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        entries.sort_by(|a, b| (!a.2, a.0.to_lowercase()).cmp(&(!b.2, b.0.to_lowercase())));
        let mut hidden = 0;
        for (name, path, is_dir) in entries {
            if !filter.is_empty() && !name.to_lowercase().contains(&filter) {
                continue;
            }
            if is_dir {
                rows.push(Row::new(name).icon(Icon::Folder));
                targets.push(Target::Go(path));
            } else if self.ask.mode == Mode::Folder {
                continue;
            } else if self.wanted(&name) {
                let size = std::fs::metadata(&path).map(|m| size(m.len())).unwrap_or_default();
                let mark = if self.chosen.contains(&path) { format!("\u{2713}  {size}") } else { size };
                rows.push(Row::with(name, mark).icon(Icon::Document));
                targets.push(Target::File(path));
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
            self.to_folder = false;
        }
        self.rows = rows;
        self.targets = targets;
    }

    fn go(&mut self, dir: PathBuf) {
        self.dir = dir;
        if self.ask.mode != Mode::Save {
            self.field.set(String::new());
        }
        self.said = None;
        self.replacing = None;
        self.stale = true;
        self.to_folder = true;
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
            Target::File(path) => match self.ask.mode {
                Mode::OpenMultiple => {
                    match self.chosen.iter().position(|p| *p == path) {
                        Some(i) => {
                            self.chosen.remove(i);
                        }
                        None => self.chosen.push(path),
                    }
                    self.stale = true;
                    None
                }
                // Saving: a file pressed lends its name.
                Mode::Save => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.field.set(name);
                    self.replacing = None;
                    None
                }
                _ => Some(vec![path]),
            },
        }
    }

    /// The chooser's own button.
    fn ok(&mut self) -> Option<Vec<PathBuf>> {
        match self.ask.mode {
            Mode::Folder => Some(vec![self.dir.clone()]),
            Mode::Save => {
                let name = self.field.text.trim().to_string();
                if name.is_empty() {
                    self.said = Some("Type the file's name first".into());
                    return None;
                }
                let path = self.dir.join(&name);
                if path.is_dir() {
                    self.go(path);
                    return None;
                }
                if path.exists() && self.replacing.as_ref() != Some(&path) {
                    self.said = Some(format!("{name} is here already: press Replace to write over it"));
                    self.replacing = Some(path);
                    return None;
                }
                Some(vec![path])
            }
            _ if !self.chosen.is_empty() => Some(self.chosen.clone()),
            _ => match self.targets.get(self.list.cursor).cloned() {
                Some(Target::File(path)) => Some(vec![path]),
                _ => {
                    self.said = Some("Choose a file first".into());
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
                self.go(path);
                if self.ask.mode == Mode::Save {
                    self.field.set(String::new());
                }
                return None;
            }
            if self.ask.mode == Mode::Save {
                if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
                    if parent.is_dir() {
                        self.dir = parent.to_path_buf();
                        self.field.set(name.to_string_lossy().into_owned());
                        self.stale = true;
                        return self.ok();
                    }
                }
            } else if path.is_file() {
                return Some(vec![path]);
            }
            self.said = Some(format!("Nothing is at {}", path.display()));
            return None;
        }
        if control || self.ask.mode == Mode::Save || self.ask.mode == Mode::Folder {
            return self.ok();
        }
        let target = self.targets.get(self.list.cursor).cloned().unwrap_or(Target::Nothing);
        self.target(target)
    }

    fn draw(&mut self, p: &mut Painter, gl: &spatiand_app::Gl, hover: (f32, f32)) {
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
        p.label(gl, &self.title(), head, theme::TITLE_EM * 0.7, theme::ink(), 0.0);
        if let Some(said) = self.said.as_deref() {
            p.label(gl, said, inner.take_top(36.0), theme::DETAIL_EM, look::trouble(), 0.0);
        }
        inner.take_top(8.0);
        self.field.draw(p, gl, inner.take_top(theme::ROW_HEIGHT));
        inner.take_top(14.0);
        let mut buttons = inner.take_bottom(theme::ROW_HEIGHT);
        inner.take_bottom(14.0);
        let label = self.ok_label();
        let ok_w = (p.measure(gl, &label, theme::ROW_EM * 0.86, 800.0).0 + 60.0).max(180.0);
        self.ok = buttons.take_right(ok_w);
        buttons.take_right(16.0);
        self.cancel = buttons.take_right(180.0);
        let lit = |r: Rect| r.contains(hover.0, hover.1);
        p.fill(gl, self.ok, theme::ROW_RADIUS, if lit(self.ok) { theme::mark() } else { theme::selected() });
        p.label(gl, &label, self.ok, theme::ROW_EM * 0.86, theme::ink(), 0.5);
        p.fill(gl, self.cancel, theme::ROW_RADIUS, if lit(self.cancel) { theme::wash(0.14) } else { theme::raised() });
        p.label(gl, "Cancel", self.cancel, theme::ROW_EM * 0.86, theme::ink_row(), 0.5);
        let keys = match self.ask.mode {
            Mode::Save => "[Enter] Save  [Esc] Cancel",
            Mode::Folder => "[Enter] Choose this folder  [Esc] Cancel",
            _ => "[Enter] Open  [Backspace] Up  [Esc] Cancel",
        };
        look::keys(p, gl, buttons, keys, 0.0, 0.8);
        self.list.draw(p, gl, inner, &self.rows);
    }
}

/// A file's size as a person reads it.
fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// What is under a path and Backspace: the folder up from it.
fn up(dir: &Path) -> Option<PathBuf> {
    dir.parent().map(Path::to_path_buf)
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let ask = match ask_from_args() {
        Ok(ask) => ask,
        Err(e) => {
            eprintln!("spatiand-choose: {e}");
            std::process::exit(2);
        }
    };
    match run(ask) {
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

fn run(ask: Ask) -> Result<Option<Vec<PathBuf>>, String> {
    let mut app = App::connect("id.prasetya.spatiand.choose")?;
    let mut chooser = Chooser::new(ask);
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
                Event::CloseRequested { .. } => return Ok(None),
                Event::Resized { width, height, .. } => app.resize(window, (*width, *height)),
                _ => {}
            }
            let Some(input) = Input::of(&event, 1.0) else { continue };
            let done = match input {
                Input::Pointer { x, y } => {
                    hover = (x, y);
                    chooser.list.input(&input);
                    None
                }
                Input::Press { pressed } => {
                    if !pressed && chooser.ok.contains(hover.0, hover.1) {
                        chooser.ok()
                    } else if !pressed && chooser.cancel.contains(hover.0, hover.1) {
                        return Ok(None);
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
                Input::Key { key, text, modifiers } => match key {
                    Key::Escape => return Ok(None),
                    Key::Up | Key::Down | Key::PageUp | Key::PageDown => {
                        chooser.list.input(&Input::Key { key, text, modifiers });
                        None
                    }
                    Key::Backspace if chooser.field.text.is_empty() => {
                        if let Some(dir) = up(&chooser.dir) {
                            chooser.go(dir);
                        }
                        None
                    }
                    Key::Enter => chooser.enter(modifiers.control || modifiers.logo),
                    _ => {
                        let before = chooser.field.text.clone();
                        chooser.field.input(&Input::Key { key, text, modifiers });
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
        if !dirty {
            continue;
        }
        app.draw(window, |gl, size| {
            let p = painter.get_or_insert_with(|| Painter::new(gl).expect("the painter"));
            p.begin(gl, size, 1.0, theme::ground());
            chooser.draw(p, gl, hover);
            p.end(gl);
        })?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(mode: Mode, dir: &Path) -> Ask {
        Ask { mode, title: String::new(), dir: dir.to_path_buf(), name: "page.html".into(), accept: vec!["png".into()] }
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
        let mut c = Chooser::new(ask(Mode::Open, &dir));
        c.build();
        assert!(c.targets.contains(&Target::File(dir.join("a.png"))));
        assert!(!c.targets.contains(&Target::File(dir.join("b.txt"))));
        assert!(c.targets.contains(&Target::Go(dir.join("folder"))));
        assert!(c.targets.contains(&Target::ShowAll));
        assert_eq!(c.target(Target::File(dir.join("a.png"))), Some(vec![dir.join("a.png")]));
    }

    #[test]
    fn several_are_marked_then_chosen() {
        let dir = scratch("multi");
        let mut c = Chooser::new(Ask { accept: Vec::new(), ..ask(Mode::OpenMultiple, &dir) });
        c.build();
        assert_eq!(c.target(Target::File(dir.join("a.png"))), None);
        assert_eq!(c.target(Target::File(dir.join("b.txt"))), None);
        assert_eq!(c.ok(), Some(vec![dir.join("a.png"), dir.join("b.txt")]));
    }

    #[test]
    fn saving_asks_before_writing_over_a_file() {
        let dir = scratch("save");
        let mut c = Chooser::new(ask(Mode::Save, &dir));
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
        let mut c = Chooser::new(Ask { accept: Vec::new(), ..ask(Mode::Open, &dir) });
        c.build();
        assert_eq!(c.targets[c.list.cursor], Target::Go(dir.join("folder")), "the folder's first line, not Places");
        c.field.set("b.t".to_string());
        c.stale = true;
        c.to_folder = true;
        c.build();
        assert_eq!(c.enter(false), Some(vec![dir.join("b.txt")]));
    }

    #[test]
    fn a_folder_is_the_one_it_is_at() {
        let dir = scratch("folder");
        let mut c = Chooser::new(ask(Mode::Folder, &dir));
        c.build();
        assert!(!c.targets.iter().any(|t| matches!(t, Target::File(_))), "folders only");
        c.target(Target::Go(dir.join("folder")));
        assert_eq!(c.ok(), Some(vec![dir.join("folder")]));
    }
}
