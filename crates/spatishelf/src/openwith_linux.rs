//! Which applications open a file, the freedesktop way, as Dolphin and every Linux file manager
//! finds them:
//!
//! * the file's **MIME type** from shared-mime-info's `globs2` (by its name; a file the globs do
//!   not know is `text/plain` if it reads as text, else `application/octet-stream`), with its
//!   parent types from `subclasses` and `aliases`;
//! * the **desktop entries** (`applications/` under `$XDG_DATA_HOME` and each of
//!   `$XDG_DATA_DIRS`, and Flatpak's exported ones, which is where the Deck's applications are)
//!   whose `MimeType` lists it or a parent of it, with `mimeapps.list`'s added and removed
//!   associations;
//! * the **default** from `mimeapps.list`'s `[Default Applications]`, in the spec's order;
//! * started by the entry's **`Exec` line as written**, its field codes (`%f %F %u %U %i %c %k`)
//!   given the files -- so that a Flatpak's `@@u %U @@` hands them through its portal.
//!
//! Plain file reading, tested below with a made-up `XDG_DATA_HOME`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::App;

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from).filter(|p| p.is_absolute())
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into()))
}

fn data_home() -> PathBuf {
    env_path("XDG_DATA_HOME").unwrap_or_else(|| home().join(".local/share"))
}

/// The data folders, the most particular first: the user's, Flatpak's exports (where the
/// session has not put them already), the system's.
pub fn data_dirs() -> Vec<PathBuf> {
    let mut out = vec![data_home()];
    let system: Vec<PathBuf> = std::env::var("XDG_DATA_DIRS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into()).split(':').filter(|d| !d.is_empty()).map(PathBuf::from).collect();
    for flatpak in [data_home().join("flatpak/exports/share"), PathBuf::from("/var/lib/flatpak/exports/share")] {
        if !system.contains(&flatpak) {
            out.push(flatpak);
        }
    }
    out.extend(system);
    out
}

// --- the MIME type ---------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct Mime {
    /// (weight, pattern, type, case-sensitive)
    globs: Vec<(u32, String, String, bool)>,
    parents: HashMap<String, Vec<String>>,
    aliases: HashMap<String, String>,
}

impl Mime {
    pub fn load(dirs: &[PathBuf]) -> Mime {
        let mut mime = Mime::default();
        // The least particular first, so that a user's own definitions are read last and win
        // a tie.
        for dir in dirs.iter().rev() {
            let base = dir.join("mime");
            if let Ok(text) = std::fs::read_to_string(base.join("globs2")) {
                mime.read_globs(&text);
            }
            if let Ok(text) = std::fs::read_to_string(base.join("subclasses")) {
                for line in text.lines().filter(|l| !l.starts_with('#')) {
                    if let Some((child, parent)) = line.split_once(' ') {
                        mime.parents.entry(child.to_string()).or_default().push(parent.trim().to_string());
                    }
                }
            }
            if let Ok(text) = std::fs::read_to_string(base.join("aliases")) {
                for line in text.lines().filter(|l| !l.starts_with('#')) {
                    if let Some((alias, real)) = line.split_once(' ') {
                        mime.aliases.insert(alias.to_string(), real.trim().to_string());
                    }
                }
            }
        }
        mime
    }

    pub fn read_globs(&mut self, text: &str) {
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let mut parts = line.splitn(4, ':');
            let (Some(weight), Some(kind), Some(glob)) = (parts.next(), parts.next(), parts.next()) else { continue };
            let case = parts.next().is_some_and(|flags| flags.split(',').any(|f| f == "cs"));
            if glob == "__NOGLOBS__" {
                continue;
            }
            self.globs.push((weight.parse().unwrap_or(50), glob.to_string(), kind.to_string(), case));
        }
    }

    /// The type a file's name says it is: the heaviest glob that matches, the longest of
    /// those.
    pub fn by_name(&self, name: &str) -> Option<String> {
        let lower = name.to_lowercase();
        self.globs
            .iter()
            .filter(|(_, glob, _, case)| if *case { glob_match(glob, name) } else { glob_match(&glob.to_lowercase(), &lower) })
            .max_by_key(|(weight, glob, _, _)| (*weight, glob.len()))
            .map(|(_, _, kind, _)| self.canonical(kind))
    }

    pub fn canonical(&self, kind: &str) -> String {
        self.aliases.get(kind).cloned().unwrap_or_else(|| kind.to_string())
    }

    /// A file's type: by its name, else by whether it reads as text.
    pub fn of(&self, path: &Path, folder: bool) -> String {
        if folder {
            return "inode/directory".into();
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(kind) = self.by_name(&name) {
            return kind;
        }
        // Only a file is read: opened not waiting (a pipe's opening waits for a writer, its
        // reading for something written, and this is the window's thread).
        let mut head = vec![0u8; 512];
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path).ok().filter(|f| f.metadata().is_ok_and(|m| m.is_file()));
        let Some(mut file) = file else { return "application/octet-stream".into() };
        let read = std::io::Read::read(&mut file, &mut head).unwrap_or(0);
        head.truncate(read);
        let text = !head.contains(&0) && (std::str::from_utf8(&head).is_ok() || std::str::from_utf8(&head[..read.saturating_sub(3)]).is_ok());
        if text { "text/plain".into() } else { "application/octet-stream".into() }
    }

    /// The type and every type it is a kind of, nearest first: `text/x-rust`, `text/plain`,
    /// `application/octet-stream`.
    pub fn lineage(&self, kind: &str) -> Vec<String> {
        let mut out = vec![self.canonical(kind)];
        let mut i = 0;
        while i < out.len() {
            let here = out[i].clone();
            let mut next: Vec<String> = self.parents.get(&here).cloned().unwrap_or_default().iter().map(|p| self.canonical(p)).collect();
            // What the spec makes every type a kind of.
            if here.starts_with("text/") && here != "text/plain" {
                next.push("text/plain".into());
            }
            for n in next {
                if !out.contains(&n) {
                    out.push(n);
                }
            }
            i += 1;
        }
        if !kind.starts_with("inode/") && !out.iter().any(|k| k == "application/octet-stream") {
            out.push("application/octet-stream".into());
        }
        out
    }
}

/// A shell glob: `*`, `?` and `[...]`.
pub fn glob_match(glob: &str, name: &str) -> bool {
    fn go(g: &[char], n: &[char]) -> bool {
        match g.first() {
            None => n.is_empty(),
            Some('*') => (0..=n.len()).any(|i| go(&g[1..], &n[i..])),
            Some('?') => !n.is_empty() && go(&g[1..], &n[1..]),
            Some('[') => {
                let Some(end) = g.iter().position(|&c| c == ']').filter(|&e| e > 1) else { return n.first() == Some(&'[') && go(&g[1..], &n[1..]) };
                let Some(&c) = n.first() else { return false };
                let set = &g[1..end];
                let (negate, set) = if set.first() == Some(&'!') { (true, &set[1..]) } else { (false, set) };
                let mut hit = false;
                let mut i = 0;
                while i < set.len() {
                    if i + 2 < set.len() && set[i + 1] == '-' {
                        hit |= set[i] <= c && c <= set[i + 2];
                        i += 3;
                    } else {
                        hit |= set[i] == c;
                        i += 1;
                    }
                }
                hit != negate && go(&g[end + 1..], &n[1..])
            }
            Some(&c) => n.first() == Some(&c) && go(&g[1..], &n[1..]),
        }
    }
    let g: Vec<char> = glob.chars().collect();
    let n: Vec<char> = name.chars().collect();
    go(&g, &n)
}

// --- desktop entries -----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Entry {
    /// Its desktop file id: the path under `applications/`, `/` written `-`.
    pub id: String,
    pub name: String,
    pub exec: String,
    pub icon: Option<String>,
    pub types: Vec<String>,
    /// Not to be shown in a list of all applications, though it may open files.
    pub no_display: bool,
    pub path: PathBuf,
}

/// A value of a desktop file, its escapes undone (`\s`, `\n`, `\t`, `\r`, `\\`).
fn unescape(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// The languages a localised key is looked for in: `de_DE`, then `de`.
fn languages() -> Vec<String> {
    let lang = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().filter_map(|v| std::env::var(v).ok()).find(|v| !v.is_empty()).unwrap_or_default();
    let lang = lang.split('.').next().unwrap_or("").split('@').next().unwrap_or("").to_string();
    if lang.is_empty() || lang == "C" || lang == "POSIX" {
        return Vec::new();
    }
    let mut out = vec![lang.clone()];
    if let Some((short, _)) = lang.split_once('_') {
        out.push(short.to_string());
    }
    out
}

/// One desktop file, if it is an application that can be started.
pub fn parse(text: &str, id: &str, path: &Path, langs: &[String]) -> Option<Entry> {
    let mut entry = Entry { id: id.to_string(), path: path.to_path_buf(), ..Entry::default() };
    let mut in_group = false;
    let (mut kind, mut hidden, mut terminal, mut try_exec, mut ours) = (None, false, false, None, false);
    let mut localised: Option<(usize, String)> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_group || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        if let Some(lang) = key.strip_prefix("Name[").and_then(|k| k.strip_suffix(']')) {
            if let Some(rank) = langs.iter().position(|l| l == lang) {
                if localised.as_ref().is_none_or(|(r, _)| rank < *r) {
                    localised = Some((rank, unescape(value)));
                }
            }
            continue;
        }
        match key {
            "Type" => kind = Some(value.to_string()),
            "Name" => entry.name = unescape(value),
            "Exec" => entry.exec = unescape(value),
            "TryExec" => try_exec = Some(unescape(value)),
            "Icon" => entry.icon = Some(unescape(value)).filter(|i| !i.is_empty()),
            "MimeType" => entry.types = value.split(';').filter(|t| !t.is_empty()).map(str::to_string).collect(),
            "NoDisplay" => entry.no_display = value.eq_ignore_ascii_case("true"),
            "Hidden" => hidden = value.eq_ignore_ascii_case("true"),
            // Started where there is no terminal to show it, it would seem to do nothing.
            "Terminal" => terminal = value.eq_ignore_ascii_case("true"),
            // Spatiand's own applications' entries on the desktop: they are listed as our own.
            "Categories" => ours = value.split(';').any(|c| c == "X-Spatiand"),
            _ => {}
        }
    }
    if let Some((_, name)) = localised {
        entry.name = name;
    }
    if hidden || terminal || ours || kind.as_deref() != Some("Application") || entry.exec.is_empty() || entry.name.is_empty() {
        return None;
    }
    // TryExec: a program not installed is not offered.
    if let Some(program) = try_exec {
        let found = if program.starts_with('/') { Path::new(&program).exists() } else { std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(&program).exists())) };
        if !found {
            return None;
        }
    }
    Some(entry)
}

/// Every desktop entry, each id once: the most particular folder's.
pub fn entries(dirs: &[PathBuf]) -> Vec<Entry> {
    let langs = languages();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for dir in dirs {
        let root = dir.join("applications");
        let mut stack = vec![root.clone()];
        while let Some(at) = stack.pop() {
            for item in std::fs::read_dir(&at).into_iter().flatten().flatten() {
                let path = item.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "desktop") {
                    continue;
                }
                let id = path.strip_prefix(&root).unwrap_or(&path).to_string_lossy().replace('/', "-");
                // Seen in a more particular folder: that one is it, hidden or not.
                if !seen.insert(id.clone()) {
                    continue;
                }
                if let Some(entry) = std::fs::read_to_string(&path).ok().and_then(|t| parse(&t, &id, &path, &langs)) {
                    out.push(entry);
                }
            }
        }
    }
    out
}

// --- mimeapps.list --------------------------------------------------------------------------

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Associations {
    /// Per type, the defaults in the order the files say them, most particular file first.
    pub defaults: HashMap<String, Vec<String>>,
    pub added: HashMap<String, Vec<String>>,
    pub removed: HashMap<String, HashSet<String>>,
}

/// The `mimeapps.list` files, most particular first, as the spec orders them.
pub fn mimeapps_files() -> Vec<PathBuf> {
    let desktops: Vec<String> = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().split(':').filter(|d| !d.is_empty()).map(|d| d.to_lowercase()).collect();
    let config_home = env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home().join(".config"));
    let config_dirs: Vec<PathBuf> = std::env::var("XDG_CONFIG_DIRS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/etc/xdg".into()).split(':').map(PathBuf::from).collect();
    let mut folders = vec![config_home];
    folders.extend(config_dirs);
    folders.push(data_home().join("applications"));
    for d in data_dirs().into_iter().skip(1) {
        folders.push(d.join("applications"));
    }
    let mut out = Vec::new();
    for folder in folders {
        for desktop in &desktops {
            out.push(folder.join(format!("{desktop}-mimeapps.list")));
        }
        out.push(folder.join("mimeapps.list"));
    }
    out
}

impl Associations {
    pub fn load(files: &[PathBuf]) -> Associations {
        let mut a = Associations::default();
        for file in files {
            if let Ok(text) = std::fs::read_to_string(file) {
                a.read(&text);
            }
        }
        a
    }

    /// One file's, after the more particular ones already read.
    pub fn read(&mut self, text: &str) {
        let mut group = "";
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                group = match line {
                    "[Default Applications]" => "default",
                    "[Added Associations]" => "added",
                    "[Removed Associations]" => "removed",
                    _ => "",
                };
                continue;
            }
            let Some((kind, apps)) = line.split_once('=') else { continue };
            let apps = apps.split(';').map(str::trim).filter(|a| !a.is_empty()).map(str::to_string);
            let kind = kind.trim().to_string();
            match group {
                "default" => self.defaults.entry(kind).or_default().extend(apps),
                "added" => self.added.entry(kind).or_default().extend(apps),
                "removed" => self.removed.entry(kind).or_default().extend(apps),
                _ => {}
            }
        }
    }
}

/// Everything that is known of what opens what.
pub struct Known {
    pub mime: Mime,
    pub entries: Vec<Entry>,
    pub associations: Associations,
}

impl Known {
    pub fn load() -> Known {
        let dirs = data_dirs();
        Known { mime: Mime::load(&dirs), entries: entries(&dirs), associations: Associations::load(&mimeapps_files()) }
    }

    fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// The default for a type: the first installed one `[Default Applications]` names for it
    /// or, failing that, for the nearest type it is a kind of.
    pub fn default_for(&self, kind: &str) -> Option<&Entry> {
        for k in self.mime.lineage(kind) {
            if let Some(found) = self.associations.defaults.get(&k).into_iter().flatten().find_map(|id| self.entry(id)) {
                return Some(found);
            }
        }
        None
    }

    /// The applications for a type, the default first: added for it, then listing it, then the
    /// same for each type it is a kind of; those removed for it left out.
    pub fn apps_for(&self, kind: &str) -> Vec<&Entry> {
        let mut out: Vec<&Entry> = Vec::new();
        let default = self.default_for(kind);
        if let Some(d) = default {
            out.push(d);
        }
        for k in self.mime.lineage(kind) {
            // A file is not offered to everything that takes "any file".
            if k == "application/octet-stream" && kind != k {
                break;
            }
            let removed = self.associations.removed.get(&k);
            let added = self.associations.added.get(&k).into_iter().flatten().filter_map(|id| self.entry(id));
            let listing = self.entries.iter().filter(|e| e.types.iter().any(|t| self.mime.canonical(t) == k)).filter(|e| removed.is_none_or(|r| !r.contains(&e.id)));
            for e in added.chain(listing) {
                if !out.iter().any(|o| o.id == e.id) {
                    out.push(e);
                }
            }
        }
        out
    }
}

fn app_of(entry: &Entry, default: bool) -> App {
    App { id: entry.id.clone(), name: entry.name.clone(), system_default: default, takes_urls: entry.exec.contains("%u") || entry.exec.contains("%U") }
}

pub fn type_of(path: &Path, folder: bool) -> String {
    Mime::load(&data_dirs()).of(path, folder)
}

pub fn apps_for(_path: &Path, kind: &str, _room: &mut spatiand_app::App) -> Vec<App> {
    let known = Known::load();
    let default = known.default_for(kind).map(|e| e.id.clone());
    known.apps_for(kind).into_iter().map(|e| app_of(e, Some(&e.id) == default.as_ref())).collect()
}

pub fn all_apps(_room: &mut spatiand_app::App) -> Vec<App> {
    entries(&data_dirs()).iter().filter(|e| !e.no_display).map(|e| app_of(e, false)).collect()
}

// --- starting one ---------------------------------------------------------------------------

/// A path as a `file:` address; an address as itself.
fn as_uri(file: &str) -> String {
    if file.contains("://") {
        return file.to_string();
    }
    let mut out = String::from("file://");
    for b in file.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The commands an entry's `Exec` line comes to for these files: one, or one a file where it
/// takes one at a time (`%f`, `%u`). A line with no field code for files has them put at its
/// end, as GLib does.
pub fn commands(entry: &Entry, files: &[String]) -> Vec<Vec<String>> {
    let words = spatiand_platform::launch::split_command(&entry.exec);
    let many = words.iter().any(|w| w == "%F" || w == "%U");
    let one_at_a_time = !many && words.iter().any(|w| w.contains("%f") || w.contains("%u"));
    let takes = many || one_at_a_time;
    let expand = |chosen: &[String]| -> Vec<String> {
        let mut out = Vec::new();
        for word in &words {
            match word.as_str() {
                "%F" => out.extend(chosen.iter().cloned()),
                "%U" => out.extend(chosen.iter().map(|f| as_uri(f))),
                "%i" => {
                    if let Some(icon) = &entry.icon {
                        out.push("--icon".into());
                        out.push(icon.clone());
                    }
                }
                _ => {
                    // Codes inside a word: put in place, or taken out.
                    let mut text = String::new();
                    let mut chars = word.chars().peekable();
                    while let Some(c) = chars.next() {
                        if c != '%' {
                            text.push(c);
                            continue;
                        }
                        match chars.next() {
                            Some('%') => text.push('%'),
                            Some('f') => text.push_str(chosen.first().map(String::as_str).unwrap_or("")),
                            Some('u') => text.push_str(&chosen.first().map(|f| as_uri(f)).unwrap_or_default()),
                            Some('c') => text.push_str(&entry.name),
                            Some('k') => text.push_str(&entry.path.to_string_lossy()),
                            _ => {}
                        }
                    }
                    // A word that was only a code for nothing is no word.
                    if !text.is_empty() || !word.starts_with('%') {
                        out.push(text);
                    }
                }
            }
        }
        if !takes {
            out.extend(chosen.iter().cloned());
        }
        out
    };
    if one_at_a_time && files.len() > 1 {
        files.iter().map(|f| expand(std::slice::from_ref(f))).collect()
    } else {
        vec![expand(files)]
    }
}

pub fn launch(app: &App, files: &[String], _room: &mut spatiand_app::App) -> Result<(), String> {
    let known = entries(&data_dirs());
    let entry = known.iter().find(|e| e.id == app.id).ok_or_else(|| format!("{} is not installed any more.", app.name))?;
    for mut words in commands(entry, files) {
        if words.is_empty() {
            continue;
        }
        let program = words.remove(0);
        // Chromium and Electron programs pick X11 unless told: the room's own rule.
        let extra = spatiand_platform::launch::wayland_arguments(&program, &words);
        let mut command = std::process::Command::new(&program);
        command.args(&words).args(extra);
        crate::detached(command).map_err(|e| format!("{program}: {e}"))?;
    }
    Ok(())
}

/// An application's picture: its icon from the icon theme the room reads
/// (`spatiand_platform::icons`), or from Flatpak's exported icons.
pub fn icon(app: &App, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    let known = entries(&data_dirs());
    let name = known.iter().find(|e| e.id == app.id)?.icon.clone()?;
    let file = spatiand_platform::icons::resolve(&name).or_else(|| {
        let roots = [data_home().join("flatpak/exports/share/icons"), PathBuf::from("/var/lib/flatpak/exports/share/icons")];
        roots.iter().flat_map(|r| ["scalable", "512x512", "256x256", "128x128", "64x64", "48x48"].map(|s| r.join("hicolor").join(s).join("apps"))).flat_map(|d| ["svg", "png"].map(|e| d.join(format!("{name}.{e}")))).find(|p| p.is_file())
    })?;
    super::picture_file(&file, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GLOBS: &str = "# comment\n50:image/jpeg:*.jpg\n50:image/jpeg:*.jpeg\n50:text/x-rust:*.rs\n50:application/gzip:*.gz\n50:application/x-compressed-tar:*.tar.gz\n10:text/x-readme:README*\n50:text/x-csrc:*.c\n50:text/x-c++src:*.C:cs\n";

    fn mime() -> Mime {
        let mut m = Mime::default();
        m.read_globs(GLOBS);
        m.parents.insert("text/x-rust".into(), vec!["text/plain".into()]);
        m.parents.insert("application/x-compressed-tar".into(), vec!["application/gzip".into()]);
        m.aliases.insert("image/jpg".into(), "image/jpeg".into());
        m
    }

    #[test]
    fn globs_choose_the_heaviest_and_the_longest() {
        let m = mime();
        assert_eq!(m.by_name("Holiday.JPG").as_deref(), Some("image/jpeg"), "case aside");
        assert_eq!(m.by_name("a.tar.gz").as_deref(), Some("application/x-compressed-tar"), "the longer glob");
        assert_eq!(m.by_name("x.gz").as_deref(), Some("application/gzip"));
        assert_eq!(m.by_name("README.md").as_deref(), Some("text/x-readme"));
        assert_eq!(m.by_name("main.C").as_deref(), Some("text/x-c++src"), "case kept where it must be");
        assert_eq!(m.by_name("main.c").as_deref(), Some("text/x-csrc"));
        assert_eq!(m.by_name("nothing"), None);
        assert!(glob_match("*.[ch]", "a.h") && !glob_match("*.[!ch]", "a.h") && glob_match("file?.txt", "file1.txt"));
    }

    #[test]
    fn lineage_goes_up_to_any_file() {
        let m = mime();
        assert_eq!(m.lineage("text/x-rust"), ["text/x-rust", "text/plain", "application/octet-stream"]);
        assert_eq!(m.lineage("application/x-compressed-tar"), ["application/x-compressed-tar", "application/gzip", "application/octet-stream"]);
        assert_eq!(m.lineage("text/x-readme"), ["text/x-readme", "text/plain", "application/octet-stream"], "text/* is text/plain");
        assert_eq!(m.lineage("image/jpg")[0], "image/jpeg", "an alias is its type");
        assert_eq!(m.lineage("inode/directory"), ["inode/directory"]);
    }

    #[test]
    fn a_file_the_globs_do_not_know_is_text_or_bytes() {
        let dir = std::env::temp_dir().join(format!("spatishelf-mime-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes"), "plain words, café\n").unwrap();
        std::fs::write(dir.join("blob"), [0u8, 159, 146, 150]).unwrap();
        let m = mime();
        assert_eq!(m.of(&dir.join("notes"), false), "text/plain");
        assert_eq!(m.of(&dir.join("blob"), false), "application/octet-stream");
        assert_eq!(m.of(&dir, true), "inode/directory");
        // A pipe is not waited on: no writer will come.
        let fifo = std::ffi::CString::new(dir.join("pipe").to_string_lossy().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert_eq!(m.of(&dir.join("pipe"), false), "application/octet-stream");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn entry(text: &str, id: &str) -> Option<Entry> {
        parse(text, id, Path::new("/x.desktop"), &["de_DE".into(), "de".into()])
    }

    #[test]
    fn desktop_entries_as_the_spec_has_them() {
        let vlc = entry("[Desktop Entry]\nType=Application\nName=VLC media player\nName[de]=VLC-Medienwiedergabe\nExec=/usr/bin/flatpak run --branch=stable --arch=x86_64 --command=vlc --file-forwarding org.videolan.VLC @@u %U @@\nIcon=org.videolan.VLC\nMimeType=video/mp4;audio/mpeg;\n[Desktop Action Play]\nName=Play\nExec=vlc --play\n", "org.videolan.VLC.desktop").unwrap();
        assert_eq!(vlc.name, "VLC-Medienwiedergabe", "in the wearer's language");
        assert_eq!(vlc.types, ["video/mp4", "audio/mpeg"]);
        assert!(vlc.exec.ends_with("@@u %U @@"), "the action's Exec is not the app's");
        assert!(entry("[Desktop Entry]\nType=Application\nName=X\nExec=x\nHidden=true\n", "x").is_none());
        assert!(entry("[Desktop Entry]\nType=Application\nName=Top\nExec=top\nTerminal=true\n", "t").is_none());
        assert!(entry("[Desktop Entry]\nType=Link\nName=L\nURL=http://x\n", "l").is_none());
        assert!(entry("[Desktop Entry]\nType=Application\nName=SpatiLoupe\nExec=spatiloupe %f\nCategories=Graphics;X-Spatiand;\n", "s").is_none(), "our own are listed as our own");
        assert!(entry("[Desktop Entry]\nType=Application\nName=Gone\nExec=gone\nTryExec=/no/such/program\n", "g").is_none());
        let hidden_handler = entry("[Desktop Entry]\nType=Application\nName=Handler\nExec=h %u\nNoDisplay=true\nMimeType=x-scheme-handler/h;\n", "h").unwrap();
        assert!(hidden_handler.no_display, "not in Other..., but it opens its types");
        assert_eq!(entry("[Desktop Entry]\nType=Application\nName=A\\sB\nExec=a\n", "a").unwrap().name, "A B");
    }

    #[test]
    fn exec_lines_given_files() {
        let e = |exec: &str| Entry { id: "x.desktop".into(), name: "X App".into(), exec: exec.into(), icon: Some("x-icon".into()), path: PathBuf::from("/a/x.desktop"), ..Entry::default() };
        let files = ["/home/me/a b.mp4".to_string(), "/home/me/c.mp4".to_string()];
        let flatpak = e("/usr/bin/flatpak run --file-forwarding org.videolan.VLC @@u %U @@");
        assert_eq!(commands(&flatpak, &files), [vec!["/usr/bin/flatpak", "run", "--file-forwarding", "org.videolan.VLC", "@@u", "file:///home/me/a%20b.mp4", "file:///home/me/c.mp4", "@@"]]);
        // One at a time: once a file.
        assert_eq!(commands(&e("gimp %f"), &files), [vec!["gimp", "/home/me/a b.mp4"], vec!["gimp", "/home/me/c.mp4"]]);
        assert_eq!(commands(&e("mpv --title=%c %F"), &files), [vec!["mpv", "--title=X App", "/home/me/a b.mp4", "/home/me/c.mp4"]]);
        assert_eq!(commands(&e("app %i %k %u"), &files[..1]), [vec!["app", "--icon", "x-icon", "/a/x.desktop", "file:///home/me/a%20b.mp4"]]);
        // An address stays an address.
        assert_eq!(commands(&e("vlc %U"), &["http://127.0.0.1:5/dav/r/a.mkv".into()]), [vec!["vlc", "http://127.0.0.1:5/dav/r/a.mkv"]]);
        // No code for files: put at the end. Quotes and a literal percent.
        assert_eq!(commands(&e("\"/opt/My App/run\" --x=100%%"), &files[..1]), [vec!["/opt/My App/run", "--x=100%", "/home/me/a b.mp4"]]);
        // A code for nothing (no files): gone, not an empty word.
        assert_eq!(commands(&e("dolphin %u"), &[]), [vec!["dolphin"]]);
    }

    /// A made-up machine: two folders of desktop entries, a mime database and mimeapps lists.
    fn machine() -> (PathBuf, Known) {
        let dir = std::env::temp_dir().join(format!("spatishelf-apps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (user, system) = (dir.join("user"), dir.join("system"));
        for d in [&user, &system] {
            std::fs::create_dir_all(d.join("applications/kde")).unwrap();
            std::fs::create_dir_all(d.join("mime")).unwrap();
        }
        std::fs::write(system.join("mime/globs2"), GLOBS).unwrap();
        std::fs::write(system.join("mime/subclasses"), "text/x-rust text/plain\n").unwrap();
        let app = |d: &Path, file: &str, name: &str, types: &str| std::fs::write(d.join("applications").join(file), format!("[Desktop Entry]\nType=Application\nName={name}\nExec={name} %F\nMimeType={types}\n")).unwrap();
        app(&system, "eog.desktop", "Eye", "image/jpeg;");
        app(&system, "gimp.desktop", "GIMP", "image/jpeg;image/png;");
        app(&system, "kde/gwenview.desktop", "Gwenview", "image/jpeg;");
        app(&system, "kate.desktop", "Kate", "text/plain;");
        app(&system, "ghex.desktop", "GHex", "application/octet-stream;");
        // The user's own entry shadows the system's of the same id.
        app(&user, "gimp.desktop", "GIMP (mine)", "image/jpeg;");
        let dirs = vec![user.clone(), system.clone()];
        let mut associations = Associations::default();
        associations.read("[Default Applications]\nimage/jpeg=not-installed.desktop;kde-gwenview.desktop;\n[Removed Associations]\nimage/jpeg=eog.desktop;\n");
        associations.read("[Default Applications]\nimage/jpeg=gimp.desktop\n[Added Associations]\ntext/x-rust=gimp.desktop;\n");
        (dir, Known { mime: Mime::load(&dirs), entries: entries(&dirs), associations })
    }

    #[test]
    fn the_applications_for_a_type_the_default_first() {
        let (dir, known) = machine();
        assert_eq!(known.entries.iter().find(|e| e.id == "gimp.desktop").unwrap().name, "GIMP (mine)");
        assert!(known.entries.iter().any(|e| e.id == "kde-gwenview.desktop"), "a subfolder's id has a dash");
        let ids = |kind: &str| known.apps_for(kind).iter().map(|e| e.id.clone()).collect::<Vec<_>>();
        // The first installed default of the most particular list; eog removed.
        assert_eq!(known.default_for("image/jpeg").unwrap().id, "kde-gwenview.desktop");
        assert_eq!(ids("image/jpeg"), ["kde-gwenview.desktop", "gimp.desktop"]);
        // Added for it, then what takes the type it is a kind of; not what takes any file.
        assert_eq!(ids("text/x-rust"), ["gimp.desktop", "kate.desktop"]);
        assert_eq!(ids("application/octet-stream"), ["ghex.desktop"]);
        assert_eq!(known.mime.of(Path::new("/x/photo.jpeg"), false), "image/jpeg");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
