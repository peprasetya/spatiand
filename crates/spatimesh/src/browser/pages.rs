//! SpatiMesh's own pages, drawn by the window over a blank page of the engine's: the history,
//! the bookmarks, the downloads, the settings and the extensions, at `spatimesh:` addresses
//! (which the engine is never given). A list to search, each line opened by a press, with a
//! menu of its own on a right click and Delete to take it out. And the files and folders to
//! choose from -- an extension to load, files for a page's `<input type=file>` -- since the
//! room has no file chooser of its own to open.

use std::path::{Path, PathBuf};

use spatiand_app::ui::{draw_icon, look, theme, Field, Icon, Input, List, Painter, Rect, Row};
use spatiand_app::{Key, Modifiers};
use spatimesh::bookmarks;
use spatimesh::engine::TabId;
use spatimesh::extensions::install;
use spatimesh::profiles::PRIVATE;
use spatimesh::store::now_ms;
use spatimesh::wire::ToHost;

use super::downloads::State;
use super::{ext, Action, Browser, Own};

pub(super) const HISTORY: &str = "spatimesh:history";
pub(super) const BOOKMARKS: &str = "spatimesh:bookmarks";
pub(super) const DOWNLOADS: &str = "spatimesh:downloads";
pub(super) const SETTINGS: &str = "spatimesh:settings";
pub(super) const EXTENSIONS: &str = "spatimesh:extensions";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Page {
    History,
    Bookmarks,
    Downloads,
    Settings,
    Extensions,
}

impl Page {
    pub fn title(self) -> &'static str {
        match self {
            Page::History => "History",
            Page::Bookmarks => "Bookmarks",
            Page::Downloads => "Downloads",
            Page::Settings => "Settings",
            Page::Extensions => "Extensions",
        }
    }

    fn searches(self) -> bool {
        matches!(self, Page::History | Page::Bookmarks | Page::Downloads)
    }
}

/// Which of SpatiMesh's own pages an address is.
pub(super) fn internal(url: &str) -> Option<Page> {
    match url.split(['?', '#']).next().unwrap_or("") {
        HISTORY => Some(Page::History),
        BOOKMARKS => Some(Page::Bookmarks),
        DOWNLOADS => Some(Page::Downloads),
        SETTINGS => Some(Page::Settings),
        EXTENSIONS => Some(Page::Extensions),
        _ => None,
    }
}

/// What the engine is given for an address: one of SpatiMesh's own pages is a blank page to it.
pub(super) fn engine_url(url: &str) -> String {
    if internal(url).is_some() { "about:blank".into() } else { url.to_string() }
}

/// One line of a page: what pressing it does, what its menu has, what Delete does.
struct Line {
    row: Row,
    open: Action,
    menu: Vec<(Row, Action)>,
    delete: Option<Action>,
}

impl Line {
    fn new(row: Row, open: Action) -> Line {
        Line { row, open, menu: Vec::new(), delete: None }
    }

    fn heading(text: impl Into<String>) -> Line {
        Line::new(Row::heading(text), Action::Nothing)
    }
}

pub(super) struct Internal {
    pub page: Page,
    pub field: Field,
    list: List,
    lines: Vec<Line>,
    rows: Vec<Row>,
    /// To be made again before it is next drawn.
    pub stale: bool,
    /// What it says when it has nothing to show.
    empty: String,
    field_rect: Rect,
}

impl Internal {
    fn new(page: Page) -> Internal {
        let mut field = Field::default();
        field.hint = format!("Search the {}", page.title().to_lowercase());
        let mut list = List::default();
        list.focused = true;
        Internal { page, field, list, lines: Vec::new(), rows: Vec::new(), stale: true, empty: String::new(), field_rect: Rect::default() }
    }
}

/// The local day a time is in, said as a person says it: "Today", "Yesterday", "26 October".
fn day_of(ms: u64) -> (i64, String) {
    let local = |secs: i64| -> (i64, i32, i32, i32, i32, i32) {
        let t = secs as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&t, &mut tm) };
        let day = (secs + tm.tm_gmtoff) / 86_400;
        (day, tm.tm_year + 1900, tm.tm_mon, tm.tm_mday, tm.tm_hour, tm.tm_min)
    };
    let (day, _, month, date, _, _) = local((ms / 1000) as i64);
    let (today, ..) = local((now_ms() / 1000) as i64);
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let said = match today - day {
        0 => "Today".to_string(),
        1 => "Yesterday".to_string(),
        _ => format!("{date} {}", MONTHS[month.clamp(0, 11) as usize]),
    };
    (day, said)
}

/// The local time of day a time was: "14:05".
fn time_of(ms: u64) -> String {
    let t = (ms / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

/// When the local day began, in milliseconds since 1970.
fn start_of_today() -> u64 {
    let now = (now_ms() / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut tm) };
    let into_day = tm.tm_hour as u64 * 3600 + tm.tm_min as u64 * 60 + tm.tm_sec as u64;
    (now as u64).saturating_sub(into_day) * 1000
}

fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

fn matches(filter: &str, texts: &[&str]) -> bool {
    filter.is_empty() || filter.split_whitespace().all(|w| texts.iter().any(|t| t.to_lowercase().contains(w)))
}

/// What an installed extension is called, its version, and its options page, from its manifest.
fn about_extension(installed: &install::Installed) -> (String, String, Option<String>) {
    let manifest = std::fs::read_to_string(installed.root.join("manifest.json")).ok().and_then(|t| spatimesh::extensions::manifest::Manifest::parse(&t).ok());
    (ext::extension_name(installed), manifest.as_ref().map(|m| m.version.clone()).unwrap_or_default(), manifest.and_then(|m| m.options_page))
}

// ---- choosing files -------------------------------------------------------------------------

/// What the files or folders are being chosen for.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum PickFor {
    /// A folder to load an extension from, in a profile.
    ExtensionFolder(String),
    /// A package to install one from.
    ExtensionFile(String),
    /// A page's `<input type=file>`.
    PageFiles { tab: TabId, multiple: bool, accept: Vec<String> },
}

#[derive(Debug, Clone, PartialEq)]
enum Target {
    Nothing,
    Go(PathBuf),
    File(PathBuf),
    ShowAll,
}

pub(super) struct Picker {
    pub purpose: PickFor,
    dir: PathBuf,
    field: Field,
    list: List,
    rows: Vec<Row>,
    targets: Vec<Target>,
    chosen: Vec<PathBuf>,
    /// The page's own choice of kinds of file is not kept to.
    all_kinds: bool,
    said: Option<String>,
    pub rect: Rect,
    ok: Rect,
    cancel: Rect,
    stale: bool,
}

/// The endings of the files a page's `accept` asks for, where they are known; empty for any.
fn endings(accept: &[String]) -> Vec<&'static str> {
    let mut out = Vec::new();
    for a in accept {
        let found: &[&str] = match a.as_str() {
            "image/*" => &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "avif", "heic"],
            "video/*" => &["mp4", "webm", "mkv", "mov", "m4v", "avi"],
            "audio/*" => &["mp3", "ogg", "oga", "wav", "flac", "m4a", "opus", "aac"],
            "image/png" => &["png"],
            "image/jpeg" => &["jpg", "jpeg"],
            "image/gif" => &["gif"],
            "image/webp" => &["webp"],
            "image/svg+xml" => &["svg"],
            "application/pdf" => &["pdf"],
            "text/plain" => &["txt"],
            "text/csv" => &["csv"],
            "application/json" => &["json"],
            "application/zip" => &["zip"],
            _ => return Vec::new(),
        };
        out.extend_from_slice(found);
    }
    out
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into()))
}

impl Picker {
    fn new(purpose: PickFor, dir: PathBuf) -> Picker {
        let mut field = Field::default();
        field.focused = true;
        field.hint = "Type to find here, or a path and Enter".into();
        let mut list = List::default();
        list.focused = true;
        Picker {
            purpose,
            dir,
            field,
            list,
            rows: Vec::new(),
            targets: Vec::new(),
            chosen: Vec::new(),
            all_kinds: false,
            said: None,
            rect: Rect::default(),
            ok: Rect::default(),
            cancel: Rect::default(),
            stale: true,
        }
    }

    fn folders_only(&self) -> bool {
        matches!(self.purpose, PickFor::ExtensionFolder(_))
    }

    fn wanted(&self, name: &str) -> bool {
        let ending = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        match &self.purpose {
            PickFor::ExtensionFolder(_) => false,
            PickFor::ExtensionFile(_) => self.all_kinds || matches!(ending.as_str(), "crx" | "xpi" | "zip"),
            PickFor::PageFiles { accept, .. } => {
                let known = endings(accept);
                self.all_kinds || known.is_empty() || known.contains(&ending.as_str())
            }
        }
    }

    fn title(&self) -> &'static str {
        match &self.purpose {
            PickFor::ExtensionFolder(_) => "The extension's folder",
            PickFor::ExtensionFile(_) => "The extension's package: .crx, .xpi or .zip",
            PickFor::PageFiles { multiple: true, .. } => "Files for the page",
            PickFor::PageFiles { .. } => "A file for the page",
        }
    }

    fn ok_label(&self) -> String {
        match &self.purpose {
            PickFor::ExtensionFolder(_) => "Load this folder".into(),
            PickFor::PageFiles { multiple: true, .. } if !self.chosen.is_empty() => format!("Choose {}", self.chosen.len()),
            _ => "Choose".into(),
        }
    }

    /// The folder's lines, again.
    fn build(&mut self) {
        self.stale = false;
        let filter = if self.field.text.starts_with('/') || self.field.text.starts_with('~') { String::new() } else { self.field.text.to_lowercase() };
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
        places.push(("The whole machine".to_string(), PathBuf::from("/"), Icon::Drive));
        for (name, path, icon) in places {
            let here = path == self.dir;
            rows.push(Row::with(name, if here { "\u{2713} here" } else { "" }).icon(icon));
            targets.push(Target::Go(path));
        }
        rows.push(Row::heading(self.dir.display().to_string()));
        targets.push(Target::Nothing);
        if let Some(up) = self.dir.parent() {
            rows.push(Row::with("\u{2039} Up", up.display().to_string()).icon(Icon::Up));
            targets.push(Target::Go(up.to_path_buf()));
        }
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
            } else if self.folders_only() {
                continue;
            } else if self.wanted(&name) {
                let size = std::fs::metadata(&path).map(|m| super::downloads::size(m.len())).unwrap_or_default();
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
        self.rows = rows;
        self.targets = targets;
    }

    fn go(&mut self, dir: PathBuf) {
        self.dir = dir;
        self.field.set(String::new());
        self.list.cursor = 0;
        self.said = None;
        self.stale = true;
    }
}

impl Browser {
    /// The window's own page in front, made again where something it shows changed.
    pub(super) fn refresh_internal(&mut self) {
        for win in &mut self.wins {
            if let Some(i) = win.internal.as_mut() {
                i.stale = true;
                win.dirty = true;
            }
            win.start = None;
        }
    }

    /// The page in front of a window, if it is one of SpatiMesh's own: kept, and its lines made.
    pub(super) fn prepare_internal(&mut self, w: usize) {
        let key = self.wins[w].key;
        let page = self.tabs.active(key).and_then(|t| self.tabs.tab(t)).and_then(|t| internal(&t.url));
        let Some(page) = page else {
            self.wins[w].internal = None;
            return;
        };
        if self.wins[w].internal.as_ref().map(|i| i.page) != Some(page) {
            self.wins[w].internal = Some(Internal::new(page));
        }
        if !self.wins[w].internal.as_ref().is_some_and(|i| i.stale) {
            return;
        }
        let profile = self.tabs.window(key).map(|x| x.profile.clone()).unwrap_or_default();
        let filter = self.wins[w].internal.as_ref().map(|i| i.field.text.trim().to_lowercase()).unwrap_or_default();
        let (lines, empty) = match page {
            Page::History => self.history_lines(&profile, &filter),
            Page::Bookmarks => self.bookmark_lines(&profile, &filter),
            Page::Downloads => self.download_lines(&profile, &filter),
            Page::Settings => (self.settings_lines(&profile), String::new()),
            Page::Extensions => self.extension_lines(&profile),
        };
        if let Some(i) = self.wins[w].internal.as_mut() {
            i.rows = lines.iter().map(|l| l.row.clone()).collect();
            i.lines = lines;
            i.empty = empty;
            i.stale = false;
        }
    }

    fn history_lines(&mut self, profile: &str, filter: &str) -> (Vec<Line>, String) {
        if profile == PRIVATE {
            return (Vec::new(), "A private window keeps no history.".into());
        }
        let entries = match self.kept.get_mut(profile) {
            Some(kept) => kept.history.entries(&self.store),
            None => spatimesh::history::History::load(&self.store, profile).entries(&self.store),
        };
        let mut lines = Vec::new();
        let mut day = None;
        let mut shown = 0;
        for e in entries {
            if !matches(filter, &[&e.title, &e.url]) {
                continue;
            }
            shown += 1;
            if shown > 500 {
                break;
            }
            let (d, said) = day_of(e.last);
            if day != Some(d) {
                day = Some(d);
                lines.push(Line::heading(said));
            }
            let title = if e.title.trim().is_empty() { spatimesh::omnibox::stripped(&e.url).to_string() } else { e.title.clone() };
            let mut line = Line::new(Row::with(title, format!("{}  {}", host_of(&e.url), time_of(e.last))).icon(Icon::Globe), Action::Open(e.url.clone()));
            line.menu = vec![
                (Row::new("Open in new tab").icon(Icon::Plus), Action::OpenInNewTab(e.url.clone())),
                (Row::new("Open in new window").icon(Icon::Desktop), Action::OpenInNewWindow(e.url.clone())),
                (Row::new("Copy address").icon(Icon::Copy), Action::Copy(e.url.clone())),
                (Row::with("Remove from the history", "Delete").icon(Icon::Trash), Action::ForgetPage(e.url.clone())),
            ];
            line.delete = Some(Action::ForgetPage(e.url.clone()));
            lines.push(line);
        }
        // Last, where Enter on the first line cannot reach them by chance.
        if shown > 0 && filter.is_empty() {
            let hour_ago = now_ms().saturating_sub(3_600_000);
            lines.push(Line::heading("Clear the history, on every machine"));
            lines.push(Line::new(Row::new("Of the last hour").icon(Icon::Trash), Action::ForgetSince(hour_ago)));
            lines.push(Line::new(Row::new("Of today").icon(Icon::Trash), Action::ForgetSince(start_of_today())));
            lines.push(Line::new(Row::new("All of it\u{2026}").icon(Icon::Trash), Action::ForgetSince(0)));
        }
        let empty = if shown == 0 { if filter.is_empty() { "Nothing visited yet." } else { "Nothing in the history says that." } } else { "" };
        (lines, empty.into())
    }

    fn bookmark_lines(&mut self, profile: &str, filter: &str) -> (Vec<Line>, String) {
        if profile == PRIVATE {
            return (Vec::new(), "A private window has no bookmarks.".into());
        }
        let all = self.bookmarks_of(profile).clone();
        let mut lines = Vec::new();
        let mut shown = 0;
        let mut add_folder = |lines: &mut Vec<Line>, parent: &str, heading: Option<&str>| {
            let children = bookmarks::children(&all, parent);
            let marks: Vec<_> = children.iter().filter(|b| !b.is_folder() && matches(filter, &[&b.title, b.url.as_deref().unwrap_or("")])).collect();
            if marks.is_empty() {
                return;
            }
            if let Some(h) = heading {
                lines.push(Line::heading(h));
            }
            for b in marks {
                shown += 1;
                let url = b.url.clone().unwrap_or_default();
                let mut line = Line::new(Row::with(b.title.clone(), spatimesh::omnibox::stripped(&url).to_string()).icon(Icon::Star), Action::Open(url.clone()));
                line.menu = vec![
                    (Row::new("Open in new tab").icon(Icon::Plus), Action::OpenInNewTab(url.clone())),
                    (Row::new("Open in new window").icon(Icon::Desktop), Action::OpenInNewWindow(url.clone())),
                    (Row::new("Rename\u{2026}"), Action::RenameBookmark(b.id.clone())),
                    (Row::new("Move up").icon(Icon::Up), Action::MoveBookmark(b.id.clone(), -1)),
                    (Row::new("Move down").icon(Icon::Down), Action::MoveBookmark(b.id.clone(), 1)),
                    (Row::new("Copy address").icon(Icon::Copy), Action::Copy(url.clone())),
                    (Row::with("Remove", "Delete").icon(Icon::Trash), Action::RemoveBookmark(b.id.clone())),
                ];
                line.delete = Some(Action::RemoveBookmark(b.id.clone()));
                lines.push(line);
            }
        };
        add_folder(&mut lines, "", None);
        let folders: Vec<(String, String)> = all.iter().filter(|b| b.is_folder()).map(|b| (b.id.clone(), b.title.clone())).collect();
        for (id, title) in folders {
            add_folder(&mut lines, &id, Some(&title));
        }
        let empty = if shown == 0 {
            if filter.is_empty() { "No bookmarks yet: the star at the end of the address bar keeps the page in front (Ctrl-D)." } else { "No bookmark says that." }
        } else {
            ""
        };
        (lines, empty.into())
    }

    fn download_lines(&mut self, profile: &str, filter: &str) -> (Vec<Line>, String) {
        let items: Vec<super::downloads::Item> = self.downloads.of(profile).into_iter().cloned().collect();
        let mut lines = Vec::new();
        let finished = items.iter().any(|i| i.state != State::Going);
        let mut shown = 0;
        for item in items {
            let name = item.name();
            if !matches(filter, &[&name, &item.url]) {
                continue;
            }
            shown += 1;
            let icon = match item.state {
                State::Failed(_) => Icon::Warning,
                _ => Icon::Download,
            };
            let mut status = item.status();
            if let Some(p) = item.progress() {
                status = format!("{:.0} %  {status}", p * 100.0);
            }
            let open = match item.state {
                State::Done => Action::OpenDownload(item.key),
                State::Failed(_) => Action::RetryDownload(item.key),
                State::Going => Action::Nothing,
            };
            let mut line = Line::new(Row::with(name, status).icon(icon), open);
            match item.state {
                State::Going => line.menu.push((Row::new("Cancel").icon(Icon::Close), Action::CancelDownload(item.key))),
                State::Done => {
                    line.menu.push((Row::new("Open").icon(Icon::Document), Action::OpenDownload(item.key)));
                    line.menu.push((Row::new("Show in its folder").icon(Icon::Folder), Action::ShowDownload(item.key)));
                }
                State::Failed(_) => line.menu.push((Row::new("Try again").icon(Icon::Refresh), Action::RetryDownload(item.key))),
            }
            line.menu.push((Row::new("Copy the address it came from").icon(Icon::Copy), Action::Copy(item.url.clone())));
            if item.state != State::Going {
                line.menu.push((Row::with("Remove from the list", "Delete").icon(Icon::Trash), Action::ForgetDownload(item.key)));
                line.delete = Some(Action::ForgetDownload(item.key));
            }
            lines.push(line);
        }
        if finished && filter.is_empty() {
            lines.push(Line::heading(""));
            lines.push(Line::new(Row::new("Clear the list").icon(Icon::Trash), Action::ClearDownloads));
        }
        let empty = if shown == 0 { "Nothing downloaded here yet. A link's or an image's menu saves it (right click)." } else { "" };
        (lines, empty.into())
    }

    fn settings_lines(&mut self, profile: &str) -> Vec<Line> {
        let mut lines = Vec::new();
        let mark = |on: bool| if on { "\u{2713}" } else { "" };
        let name = self.profile(profile).name;
        lines.push(Line::heading(format!("Search engine of {name}")));
        let current = self.search_for(profile).name;
        for engine in spatimesh::address::search_engines() {
            lines.push(Line::new(Row::with(engine.name.clone(), mark(engine.name == current)).icon(Icon::Search), Action::SearchWith(engine.name)));
        }
        if profile != PRIVATE {
            lines.push(Line::new(Row::with("An address of your own\u{2026}", mark(current == "Custom")), Action::CustomSearch));
        }
        lines.push(Line::heading("When a profile opens with no window of it open"));
        lines.push(Line::new(Row::with("Its tabs, as they were when its last window closed", mark(self.settings.restore)).icon(Icon::Rotate), Action::SetRestore(true)));
        lines.push(Line::new(Row::with("Its home page", mark(!self.settings.restore)).icon(Icon::Home), Action::SetRestore(false)));
        lines.push(Line::heading("Home page"));
        let start = super::is_start(&self.settings.home);
        lines.push(Line::new(Row::with("The start page", mark(start)).icon(Icon::Home), Action::SetHome(false)));
        let shown = if start { "The page in front of this window".to_string() } else { spatimesh::omnibox::stripped(&self.settings.home).to_string() };
        lines.push(Line::new(Row::with(shown, if start { "set" } else { "\u{2713}  set again" }).icon(Icon::Globe), Action::SetHome(true)));
        lines.push(Line::heading("Pages"));
        lines.push(Line::new(Row::with("Bigger", format!("{:.0} %", self.settings.zoom() * 80.0)), Action::ZoomIn));
        lines.push(Line::new(Row::new("Smaller"), Action::ZoomOut));
        lines.push(Line::new(Row::new("Usual size"), Action::ZoomReset));
        lines.push(Line::new(Row::with("Bookmarks bar", mark(self.settings.bookmarks_bar)).icon(Icon::Star), Action::ToggleBookmarksBar));
        lines.push(Line::heading("Profiles"));
        if profile != PRIVATE {
            lines.push(Line::new(Row::new(format!("Settings of {name}: its name, place, lock, colour\u{2026}")).icon(Icon::Options), Action::ProfileSettings));
        }
        lines.push(Line::new(Row::new("New profile").icon(Icon::Plus), Action::NewProfile));
        lines.push(Line::new(Row::new("Lock profiles now").icon(Icon::Lock), Action::LockAll));
        lines.push(Line::heading("History and extensions"));
        lines.push(Line::new(Row::with("History", "Ctrl-H").icon(Icon::SortRecent), Action::Open(HISTORY.into())));
        lines.push(Line::new(Row::with("Downloads", "Ctrl-J").icon(Icon::Download), Action::Open(DOWNLOADS.into())));
        if profile != PRIVATE {
            lines.push(Line::new(Row::new("Extensions").icon(Icon::Puzzle), Action::Open(EXTENSIONS.into())));
        }
        lines.push(Line::heading("About"));
        let engine = self.hosts.get(profile).and_then(|h| h.engine.clone()).unwrap_or_else(|| "starting".into());
        lines.push(Line::new(Row::with("SpatiMesh", format!("pages by {engine}")).icon(Icon::Info), Action::Nothing));
        lines
    }

    fn extension_lines(&mut self, profile: &str) -> (Vec<Line>, String) {
        if profile == PRIVATE {
            return (Vec::new(), "Extensions are a profile's: a private window has none.".into());
        }
        let mut lines = Vec::new();
        lines.push(Line::heading("Get extensions"));
        lines.push(Line::new(Row::with("Chrome Web Store", "Chrome's extensions").icon(Icon::Globe), Action::OpenInNewTab(spatimesh::extensions::store::CHROME_WEB_STORE.into())));
        lines.push(Line::new(Row::with("Firefox Add-ons", "Firefox's").icon(Icon::Globe), Action::OpenInNewTab(spatimesh::extensions::store::FIREFOX_ADD_ONS.into())));
        lines.push(Line::new(Row::with("Load an unpacked folder\u{2026}", "developer mode").icon(Icon::Folder), Action::PickExtensionFolder));
        lines.push(Line::new(Row::with("Install from a file\u{2026}", ".crx  .xpi  .zip").icon(Icon::Document), Action::PickExtensionFile));
        lines.push(Line::new(Row::with("Load them all again", "from their files").icon(Icon::Refresh), Action::ReloadExtensions));
        let installed = install::list(&self.store, profile);
        if !installed.is_empty() {
            lines.push(Line::heading("Installed: a press turns one on or off"));
        }
        for i in &installed {
            let (name, version, options) = about_extension(i);
            let state = if i.enabled { "\u{2713} on" } else { "off" };
            let mut line = Line::new(Row::with(name, format!("{version}  {state}")).icon(Icon::Puzzle), Action::ToggleExtension(i.id.clone()));
            line.menu.push((Row::new(if i.enabled { "Turn off" } else { "Turn on" }), Action::ToggleExtension(i.id.clone())));
            if options.is_some() {
                line.menu.push((Row::new("Its options").icon(Icon::Options), Action::ExtensionOptions(i.id.clone())));
            }
            line.menu.push((Row::new("Copy its id").icon(Icon::Copy), Action::Copy(i.id.clone())));
            line.menu.push((Row::with("Remove", "Delete").icon(Icon::Trash), Action::UninstallExtension(i.id.clone())));
            line.delete = Some(Action::UninstallExtension(i.id.clone()));
            lines.push(line);
        }
        let empty = if installed.is_empty() { "None installed yet. Find one in a store above: its page has \u{201C}Add to SpatiMesh\u{201D} in the address bar." } else { "" };
        (lines, empty.into())
    }

    /// An extension's options page, in a tab.
    pub(super) fn open_extension_options(&mut self, w: usize, profile: &str, id: &str) {
        let Some(installed) = install::list(&self.store, profile).into_iter().find(|i| i.id == id) else { return };
        let (_, _, options) = about_extension(&installed);
        let Some(page) = options else { return };
        let origin = crate::engines::extension_addresses(&self.program).origin(installed.flavor, &installed.id);
        self.new_tab(w, &format!("{origin}/{}", page.trim_start_matches('/')));
    }

    // ---- input to a page of SpatiMesh's own ----------------------------------------------

    pub(super) fn internal_pointer(&mut self, w: usize, x: f32, y: f32) {
        if let Some(i) = self.wins[w].internal.as_mut() {
            i.list.input(&Input::Pointer { x, y });
        }
    }

    pub(super) fn internal_scroll(&mut self, w: usize, dx: f32, dy: f32) {
        if let Some(i) = self.wins[w].internal.as_mut() {
            i.list.focused = true;
            i.list.input(&Input::Scroll { dx, dy });
        }
    }

    /// A press on the page: the search field, or a line.
    pub(super) fn internal_press(&mut self, w: usize, x: f32, y: f32, pressed: bool) {
        let Some(i) = self.wins[w].internal.as_mut() else { return };
        if pressed && i.field_rect.contains(x, y) {
            i.field.focused = true;
            self.wins[w].editing = false;
            self.wins[w].address.focused = false;
            return;
        }
        if let Some(index) = i.list.input(&Input::Press { pressed }) {
            let open = i.lines.get(index).map(|l| l.open.clone());
            if let Some(action) = open.filter(|a| *a != Action::Nothing) {
                self.internal_act(w, action);
            }
        }
    }

    /// Lines that take something away are asked about first.
    fn internal_act(&mut self, w: usize, action: Action) {
        let profile = self.tabs.window(self.wins[w].key).map(|x| x.profile.clone()).unwrap_or_default();
        match action {
            Action::ForgetSince(0) => {
                self.ask_own_with(w, Own::ClearHistory(profile), "Clear all of this profile's history, on every machine it is on? Its bookmarks, logins and open tabs stay. Type yes to say so:", false, "")
            }
            Action::UninstallExtension(ref id) => {
                let name = install::list(&self.store, &profile).into_iter().find(|i| &i.id == id).map(|i| ext::extension_name(&i)).unwrap_or_default();
                self.ask_own_with(w, Own::Uninstall(profile, id.clone()), &format!("Remove {name}, and what it kept? Type yes to say so:"), false, "")
            }
            action => self.act(w, action),
        }
    }

    /// A right click on the page: the line's own menu.
    pub(super) fn internal_menu(&mut self, w: usize) {
        let Some(i) = self.wins[w].internal.as_ref() else { return };
        let Some(index) = i.list.hovered() else { return };
        let Some(line) = i.lines.get(index) else { return };
        if line.menu.is_empty() {
            return;
        }
        let (rows, actions): (Vec<Row>, Vec<Action>) = line.menu.iter().cloned().unzip();
        let at = self.wins[w].pointer;
        let height = rows.len() as f32 * (theme::ROW_HEIGHT + theme::ROW_GAP) + 24.0;
        self.open_popup(w, rows, actions, Rect::new(at.0, at.1 + 4.0, 600.0, height));
    }

    /// A key on the page (the address bar not being typed into). Whether it took it.
    pub(super) fn internal_key(&mut self, w: usize, key: Key, text: Option<String>, m: Modifiers) -> bool {
        let Some(i) = self.wins[w].internal.as_mut() else { return false };
        match key {
            Key::Up | Key::Down | Key::PageUp | Key::PageDown => {
                i.list.input(&Input::Key { key, text, modifiers: m });
            }
            Key::Enter => {
                let open = i.lines.get(i.list.cursor).map(|l| l.open.clone());
                if let Some(action) = open.filter(|a| *a != Action::Nothing) {
                    self.internal_act(w, action);
                }
            }
            Key::Delete if !i.field.focused || i.field.text.is_empty() => {
                let delete = i.lines.get(i.list.cursor).and_then(|l| l.delete.clone());
                if let Some(action) = delete {
                    self.internal_act(w, action);
                }
            }
            Key::Escape if i.field.focused && !i.field.text.is_empty() => {
                i.field.set(String::new());
                i.stale = true;
            }
            _ if i.page.searches() && (i.field.focused || text.as_deref().is_some_and(|t| !t.trim().is_empty() && !m.control && !m.logo)) => {
                // Typing on the page searches it.
                i.field.focused = true;
                let before = i.field.text.clone();
                i.field.input(&Input::Key { key, text, modifiers: m });
                if i.field.text != before {
                    i.stale = true;
                    i.list.cursor = 0;
                }
            }
            _ => return false,
        }
        self.wins[w].dirty = true;
        true
    }

    // ---- choosing files ------------------------------------------------------------------

    pub(super) fn pick(&mut self, w: usize, purpose: PickFor) {
        if matches!(&purpose, PickFor::ExtensionFolder(p) | PickFor::ExtensionFile(p) if p == PRIVATE) {
            self.say("Extensions are installed in a profile, not in a private window".into());
            return;
        }
        let start = self.pick_dir.clone().filter(|d| d.is_dir()).unwrap_or_else(|| {
            let downloads = home().join("Downloads");
            if matches!(purpose, PickFor::ExtensionFile(_)) && downloads.is_dir() { downloads } else { home() }
        });
        self.wins[w].popup = None;
        self.wins[w].editing = false;
        self.wins[w].address.focused = false;
        self.wins[w].picker = Some(Picker::new(purpose, start));
        self.wins[w].dirty = true;
        self.focus_pages();
    }

    /// A page's `<input type=file>`: the files to choose from, over its window.
    pub(super) fn pick_files_for_page(&mut self, tab: TabId, multiple: bool, accept: Vec<String>) {
        let Some(w) = self.tabs.window_of(tab).and_then(|k| self.win_of_key(k)) else {
            self.tell(tab, ToHost::Files { tab, files: None });
            return;
        };
        self.pick(w, PickFor::PageFiles { tab, multiple, accept });
    }

    fn picked_files(&mut self, w: usize, files: Vec<PathBuf>) {
        let Some(picker) = self.wins[w].picker.take() else { return };
        self.pick_dir = Some(picker.dir.clone());
        self.wins[w].dirty = true;
        match picker.purpose {
            PickFor::ExtensionFolder(profile) | PickFor::ExtensionFile(profile) => {
                let Some(path) = files.first() else { return };
                match self.install_extension(&profile, &path.to_string_lossy()) {
                    Ok(name) => self.say(format!("{name} is installed")),
                    Err(e) => self.say(format!("Not installed: {e}")),
                }
                self.refresh_internal();
            }
            PickFor::PageFiles { tab, .. } => {
                let files = files.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                self.tell(tab, ToHost::Files { tab, files: Some(files) });
            }
        }
        self.focus_pages();
    }

    pub(super) fn cancel_pick(&mut self, w: usize) {
        if let Some(picker) = self.wins[w].picker.take() {
            if let PickFor::PageFiles { tab, .. } = picker.purpose {
                self.tell(tab, ToHost::Files { tab, files: None });
            }
        }
        self.wins[w].dirty = true;
        self.focus_pages();
    }

    fn pick_target(&mut self, w: usize, target: Target) {
        let Some(picker) = self.wins[w].picker.as_mut() else { return };
        match target {
            Target::Nothing => {}
            Target::Go(dir) => picker.go(dir),
            Target::ShowAll => {
                picker.all_kinds = true;
                picker.stale = true;
            }
            Target::File(path) => {
                if matches!(picker.purpose, PickFor::PageFiles { multiple: true, .. }) {
                    // Several: each pressed one marked, then Choose.
                    match picker.chosen.iter().position(|p| *p == path) {
                        Some(i) => {
                            picker.chosen.remove(i);
                        }
                        None => picker.chosen.push(path),
                    }
                    picker.stale = true;
                } else {
                    self.picked_files(w, vec![path]);
                }
            }
        }
        self.wins[w].dirty = true;
    }

    /// The picker's own button: the folder it is at, or the files marked.
    fn pick_ok(&mut self, w: usize) {
        let Some(picker) = self.wins[w].picker.as_mut() else { return };
        match &picker.purpose {
            PickFor::ExtensionFolder(_) => {
                let dir = picker.dir.clone();
                if !dir.join("manifest.json").is_file() {
                    picker.said = Some("There is no manifest.json in this folder: an unpacked extension has one".into());
                    self.wins[w].dirty = true;
                    return;
                }
                self.picked_files(w, vec![dir]);
            }
            _ if !picker.chosen.is_empty() => {
                let chosen = picker.chosen.clone();
                self.picked_files(w, chosen);
            }
            _ => {
                let target = picker.targets.get(picker.list.cursor).cloned();
                match target {
                    Some(Target::File(path)) => self.picked_files(w, vec![path]),
                    _ => {
                        picker.said = Some("Choose a file first".into());
                        self.wins[w].dirty = true;
                    }
                }
            }
        }
    }

    pub(super) fn picker_pointer(&mut self, w: usize, x: f32, y: f32) {
        if let Some(picker) = self.wins[w].picker.as_mut() {
            picker.list.input(&Input::Pointer { x, y });
        }
    }

    pub(super) fn picker_scroll(&mut self, w: usize, dx: f32, dy: f32) {
        if let Some(picker) = self.wins[w].picker.as_mut() {
            picker.list.input(&Input::Scroll { dx, dy });
        }
    }

    pub(super) fn picker_press(&mut self, w: usize, x: f32, y: f32, pressed: bool) {
        let Some(picker) = self.wins[w].picker.as_mut() else { return };
        if !pressed {
            if picker.ok.contains(x, y) {
                self.pick_ok(w);
                return;
            }
            if picker.cancel.contains(x, y) {
                self.cancel_pick(w);
                return;
            }
        } else if !picker.rect.contains(x, y) {
            self.cancel_pick(w);
            return;
        }
        if let Some(index) = picker.list.input(&Input::Press { pressed }) {
            let target = picker.targets.get(index).cloned().unwrap_or(Target::Nothing);
            self.pick_target(w, target);
        }
    }

    pub(super) fn picker_key(&mut self, w: usize, key: Key, text: Option<String>, m: Modifiers) {
        self.wins[w].dirty = true;
        let Some(picker) = self.wins[w].picker.as_mut() else { return };
        match key {
            Key::Escape => self.cancel_pick(w),
            Key::Up | Key::Down | Key::PageUp | Key::PageDown => {
                picker.list.input(&Input::Key { key, text, modifiers: m });
            }
            Key::Backspace if picker.field.text.is_empty() => {
                if let Some(up) = picker.dir.parent().map(Path::to_path_buf) {
                    picker.go(up);
                }
            }
            Key::Enter => {
                let typed = picker.field.text.trim().to_string();
                if typed.starts_with('/') || typed.starts_with('~') {
                    let path = if let Some(rest) = typed.strip_prefix('~') { home().join(rest.trim_start_matches('/')) } else { PathBuf::from(&typed) };
                    if path.is_dir() {
                        picker.go(path);
                    } else if path.is_file() {
                        self.picked_files(w, vec![path]);
                    } else {
                        picker.said = Some(format!("Nothing is at {}", path.display()));
                    }
                    return;
                }
                if m.control || m.logo {
                    self.pick_ok(w);
                    return;
                }
                let target = picker.targets.get(picker.list.cursor).cloned().unwrap_or(Target::Nothing);
                self.pick_target(w, target);
            }
            _ => {
                let before = picker.field.text.clone();
                picker.field.input(&Input::Key { key, text, modifiers: m });
                if picker.field.text != before {
                    picker.stale = true;
                    picker.list.cursor = 0;
                }
            }
        }
    }
}

// ---- drawing ----------------------------------------------------------------------------------

/// A page of SpatiMesh's own, filling `page`.
pub(super) fn draw_internal(p: &mut Painter, gl: &spatiand_app::Gl, page: Rect, i: &mut Internal, profile_name: &str) {
    p.fill(gl, page, 12.0, theme::ground());
    let mut all = page.inset(theme::PAD);
    // A column a person can read across, in a wide window.
    if all.w > 1500.0 {
        all = Rect { x: all.x + (all.w - 1500.0) * 0.5, w: 1500.0, ..all };
    }
    look::header(p, gl, &mut all, i.page.title(), profile_name);
    if i.page.searches() {
        let mut line = all.take_top(theme::ROW_HEIGHT);
        let mark = line.take_left(52.0);
        draw_icon(p, gl, Icon::Search, Rect::new(mark.x + 8.0, mark.y + (mark.h - 30.0) * 0.5, 30.0, 30.0), theme::ink_hint());
        let field = Rect { w: line.w.min(760.0), ..line };
        i.field_rect = field;
        i.field.draw(p, gl, field);
        all.take_top(18.0);
    } else {
        i.field_rect = Rect::default();
    }
    if !i.empty.is_empty() {
        // Nothing of its own to list: the lines it always has (clearing, getting extensions),
        // and then what it would show, said.
        if i.lines.iter().any(|l| !l.row.heading) {
            let height = (i.rows.len() as f32 * (theme::ROW_HEIGHT + theme::ROW_GAP)).min(all.h * 0.6);
            let top = all.take_top(height);
            i.list.draw(p, gl, top, &i.rows);
        }
        look::message(p, gl, all, &i.empty, theme::ink_hint());
        return;
    }
    let foot = all.take_bottom(theme::FOOTER_EM * 1.9);
    i.list.draw(p, gl, all, &i.rows);
    let hint = if i.lines.iter().any(|l| l.delete.is_some()) { "[Enter] Open  [Delete] Remove  Right click: more" } else { "[Enter] Choose" };
    look::keys(p, gl, foot, hint, 0.0, 0.8);
}

/// The files to choose from, over the page.
pub(super) fn draw_picker(p: &mut Painter, gl: &spatiand_app::Gl, page: Rect, picker: &mut Picker, hover: (f32, f32)) {
    if picker.stale {
        picker.build();
    }
    p.fill(gl, page, 12.0, [0.0, 0.0, 0.0, 0.7]);
    let width = page.w.min(1300.0) - 40.0;
    let height = page.h - 40.0;
    let rect = Rect::new(page.x + (page.w - width) * 0.5, page.y + 20.0, width, height);
    picker.rect = rect;
    p.fill(gl, rect, 24.0, theme::ground());
    look::glass(p, gl, rect, 24.0, 1.0);
    let mut inner = rect.inset(28.0);
    p.label(gl, picker.title(), inner.take_top(48.0), theme::TITLE_EM * 0.7, theme::ink(), 0.0);
    let said = picker.said.clone();
    if let Some(said) = said.as_deref() {
        p.label(gl, said, inner.take_top(36.0), theme::DETAIL_EM, look::trouble(), 0.0);
    }
    inner.take_top(8.0);
    picker.field.draw(p, gl, inner.take_top(theme::ROW_HEIGHT));
    inner.take_top(14.0);
    let mut buttons = inner.take_bottom(theme::ROW_HEIGHT);
    inner.take_bottom(14.0);
    let label = picker.ok_label();
    let ok_w = (p.measure(gl, &label, theme::ROW_EM * 0.86, 800.0).0 + 60.0).max(180.0);
    picker.ok = buttons.take_right(ok_w);
    buttons.take_right(16.0);
    picker.cancel = buttons.take_right(180.0);
    let lit = |r: Rect| r.contains(hover.0, hover.1);
    p.fill(gl, picker.ok, theme::ROW_RADIUS, if lit(picker.ok) { theme::mark() } else { theme::selected() });
    p.label(gl, &label, picker.ok, theme::ROW_EM * 0.86, theme::ink(), 0.5);
    p.fill(gl, picker.cancel, theme::ROW_RADIUS, if lit(picker.cancel) { theme::wash(0.14) } else { theme::raised() });
    p.label(gl, "Cancel", picker.cancel, theme::ROW_EM * 0.86, theme::ink_row(), 0.5);
    look::keys(p, gl, buttons, "[Enter] Open  [Backspace] Up  [Esc] Cancel", 0.0, 0.8);
    picker.list.draw(p, gl, inner, &picker.rows);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_pages_are_blank_to_the_engine() {
        assert_eq!(internal("spatimesh:history"), Some(Page::History));
        assert_eq!(internal("spatimesh:settings#search"), Some(Page::Settings));
        assert_eq!(internal("https://spatimesh:history"), None);
        assert_eq!(engine_url(BOOKMARKS), "about:blank");
        assert_eq!(engine_url("https://a.example/"), "https://a.example/");
    }

    #[test]
    fn a_pages_kinds_of_file() {
        assert!(endings(&["image/*".into()]).contains(&"png"));
        assert_eq!(endings(&["application/pdf".into(), "image/png".into()]), vec!["pdf", "png"]);
        assert!(endings(&["application/x-something".into()]).is_empty(), "unknown: every file");
    }
}
