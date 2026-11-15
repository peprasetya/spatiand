//! The trash, as each system has it, so that SpatiShelf's trash and the system's own file
//! manager's are one trash both ways.
//!
//! * **Linux (the Deck)**: the freedesktop.org Trash specification
//!   (<https://specifications.freedesktop.org/trash-spec/latest/>), which Dolphin, Nautilus and
//!   every other file manager there follow: the home trash `$XDG_DATA_HOME/Trash` (by default
//!   `~/.local/share/Trash`) with `files/` and `info/<name>.trashinfo`; for what is on another
//!   disk, that disk's own `$topdir/.Trash/$uid` (made by its administrator, sticky, not a link)
//!   or else `$topdir/.Trash-$uid`, made when first wanted -- the spec's own rule for a machine
//!   that has no trash yet. A disk where neither can be made has its things copied into the
//!   home trash (the spec allows it) and taken off the disk.
//! * **Android (the Beam Pro)**: Android has no trash for files (MediaStore's covers its own
//!   media only), so SpatiShelf keeps one in the same format, at the top of each storage
//!   volume: `/storage/emulated/0/.Trash-spatiand` (what `/sdcard` is), and
//!   `/storage/XXXX-XXXX/.Trash-spatiand` on a card. On the volume, so that putting something
//!   in it is a rename, not a copy; in shared storage, so that it outlives the app being
//!   reinstalled (the app's own folders do not); named for Spatiand rather than for the uid,
//!   which Android changes when the app is installed again; hidden and with a `.nomedia`, so
//!   that the gallery and other apps do not show what is in it.
//! * **The Mac**: Finder's own Trash (`mac.rs`): put there by `NSFileManager`, so that Finder's
//!   Put Back works for it too, and listed from `~/.Trash`.
//!
//! Everything but the Mac's few calls is plain file work, tested below.

use std::ffi::OsString;
use std::io::{self, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
#[path = "trash_mac.rs"]
mod mac;

/// Something in the trash.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Where it is now, in the trash.
    pub file: PathBuf,
    /// Its name as it was (the trash's own may have a number added).
    pub name: String,
    /// Where it was, when known.
    pub origin: Option<PathBuf>,
    /// When it was put in the trash, in seconds since 1970.
    pub deleted: Option<i64>,
    pub folder: bool,
    /// All of it, a folder's things included.
    pub size: u64,
}

/// A freedesktop trash: its `files` and `info` folders, and, for one at the top of a disk,
/// that top, which the paths in its info files are written from.
#[derive(Debug, Clone, PartialEq)]
pub struct Can {
    pub root: PathBuf,
    pub top: Option<PathBuf>,
}

impl Can {
    pub fn files(&self) -> PathBuf {
        self.root.join("files")
    }

    pub fn info(&self) -> PathBuf {
        self.root.join("info")
    }

    fn info_of(&self, name: &std::ffi::OsStr) -> PathBuf {
        let mut file = name.to_os_string();
        file.push(".trashinfo");
        self.info().join(file)
    }

    /// Make its folders, as the spec has them made: for the user alone.
    pub fn make(&self) -> io::Result<()> {
        use std::os::unix::fs::DirBuilderExt;
        for dir in [&self.root, &self.files(), &self.info()] {
            if !dir.is_dir() {
                std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
            }
        }
        Ok(())
    }

    /// What its info files say `path` was: relative to the disk's top for a disk's own trash
    /// (when it is on it), else whole.
    fn written(&self, path: &Path) -> PathBuf {
        match &self.top {
            Some(top) => path.strip_prefix(top).map(Path::to_path_buf).unwrap_or_else(|_| path.to_path_buf()),
            None => path.to_path_buf(),
        }
    }

    /// Where an info file's `Path` was.
    fn origin(&self, written: &Path) -> PathBuf {
        match &self.top {
            Some(top) if written.is_relative() => top.join(written),
            _ => written.to_path_buf(),
        }
    }

    /// Put `path` (whole, as it is) in this trash, `stamp` being when (see [`stamp`]). The
    /// name in the trash is the thing's own, or with a number if that is taken; it is claimed by
    /// making its info file first, as the spec says, so that two putting at once cannot take
    /// one name. Moved by renaming when `copy` is false, else copied and taken away (another
    /// disk's thing into the home trash). Where it is in the trash.
    pub fn put(&self, path: &Path, stamp: &str, copy: bool) -> io::Result<PathBuf> {
        self.make()?;
        let name = path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "that cannot be put in the trash"))?.to_os_string();
        let (stem, ending) = split_name(&name);
        let mut n = 1;
        let (target, info) = loop {
            let candidate = if n == 1 { name.clone() } else { numbered(&stem, &ending, n) };
            n += 1;
            let target = self.files().join(&candidate);
            if target.symlink_metadata().is_ok() {
                continue;
            }
            let info = self.info_of(&candidate);
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&info) {
                Ok(mut file) => {
                    file.write_all(info_text(&self.written(path), stamp).as_bytes())?;
                    break (target, info);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        };
        let moved = if copy { copy_then_remove(path, &target) } else { std::fs::rename(path, &target) };
        if let Err(e) = moved {
            let _ = std::fs::remove_file(&info);
            return Err(e);
        }
        Ok(target)
    }

    /// What is in it: each info file and its thing; a thing with no info file too (where it
    /// came from not known), and an info file with no thing is not shown.
    pub fn list(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for entry in std::fs::read_dir(self.info()).into_iter().flatten().flatten() {
            let file_name = entry.file_name();
            let Some(stem) = file_name.as_bytes().strip_suffix(b".trashinfo") else { continue };
            let stem = OsString::from_vec(stem.to_vec());
            let file = self.files().join(&stem);
            let Ok(meta) = file.symlink_metadata() else { continue };
            let (origin, deleted) = std::fs::read_to_string(entry.path()).map(|t| parse_info(&t)).unwrap_or((None, None));
            let origin = origin.map(|o| self.origin(&o));
            let name = origin.as_ref().and_then(|o| o.file_name()).unwrap_or(&stem).to_string_lossy().into_owned();
            out.push(Entry { size: size_of(&file), file, name, origin, deleted: deleted.as_deref().and_then(parse_stamp), folder: meta.is_dir() });
            seen.insert(stem);
        }
        for entry in std::fs::read_dir(self.files()).into_iter().flatten().flatten() {
            if seen.contains(&entry.file_name()) {
                continue;
            }
            let file = entry.path();
            let folder = file.symlink_metadata().is_ok_and(|m| m.is_dir());
            out.push(Entry { size: size_of(&file), name: entry.file_name().to_string_lossy().into_owned(), file, origin: None, deleted: None, folder });
        }
        out
    }

    /// Put a thing back where it was. A folder it was in that is not there any more is made
    /// again; something there by its name already, it comes back beside it with a number.
    /// Where it went.
    pub fn restore(&self, file: &Path) -> Result<PathBuf, String> {
        let name = file.file_name().ok_or("not in the trash")?;
        let info = self.info_of(name);
        let text = std::fs::read_to_string(&info).map_err(|_| "Where it came from is not known: drag it to where it is to go.".to_string())?;
        let origin = parse_info(&text).0.map(|o| self.origin(&o)).ok_or("Where it came from is not known: drag it to where it is to go.")?;
        let to = put_back(file, &origin)?;
        let _ = std::fs::remove_file(&info);
        Ok(to)
    }

    /// Gone for good, and its info file with it.
    pub fn purge(&self, file: &Path) -> io::Result<()> {
        remove(file)?;
        if let Some(name) = file.file_name() {
            let _ = std::fs::remove_file(self.info_of(name));
        }
        Ok(())
    }

    /// Everything in it gone for good, info files left over included.
    pub fn empty(&self) -> io::Result<()> {
        for entry in std::fs::read_dir(self.files()).into_iter().flatten().flatten() {
            remove(&entry.path())?;
        }
        for entry in std::fs::read_dir(self.info()).into_iter().flatten().flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
        let _ = std::fs::remove_file(self.root.join("directorysizes"));
        Ok(())
    }
}

/// Move a thing out of the trash to `origin`, making the folders on the way, numbered beside
/// what is there already; copied when it is on another disk.
pub fn put_back(file: &Path, origin: &Path) -> Result<PathBuf, String> {
    let folder = origin.parent().ok_or("Where it came from is not known.")?;
    std::fs::create_dir_all(folder).map_err(|e| format!("{} could not be made again: {e}", folder.display()))?;
    let name = origin.file_name().ok_or("Where it came from is not known.")?;
    let to = free_place(folder, name);
    match std::fs::rename(file, &to) {
        Ok(()) => Ok(to),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => copy_then_remove(file, &to).map(|_| to).map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// `name` in `folder`, or with a number ("a 2.jpg") if something is there by it.
fn free_place(folder: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let first = folder.join(name);
    if first.symlink_metadata().is_err() {
        return first;
    }
    let (stem, ending) = split_name(name);
    (2..).map(|n| folder.join(numbered(&stem, &ending, n))).find(|p| p.symlink_metadata().is_err()).unwrap()
}

/// A name's part before its ending and the ending with its dot (none for ".bashrc").
fn split_name(name: &std::ffi::OsStr) -> (Vec<u8>, Vec<u8>) {
    let bytes = name.as_bytes();
    match bytes.iter().rposition(|&b| b == b'.') {
        Some(at) if at > 0 => (bytes[..at].to_vec(), bytes[at..].to_vec()),
        _ => (bytes.to_vec(), Vec::new()),
    }
}

fn numbered(stem: &[u8], ending: &[u8], n: usize) -> OsString {
    let mut out = stem.to_vec();
    out.extend_from_slice(format!(" {n}").as_bytes());
    out.extend_from_slice(ending);
    OsString::from_vec(out)
}

fn remove(path: &Path) -> io::Result<()> {
    match path.symlink_metadata() {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) => Err(e),
    }
}

/// Copied to `to` (links as links), then the original taken away: between disks.
fn copy_then_remove(from: &Path, to: &Path) -> io::Result<()> {
    copy_all(from, to)?;
    remove(from)
}

fn copy_all(from: &Path, to: &Path) -> io::Result<()> {
    let meta = from.symlink_metadata()?;
    if meta.file_type().is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(from)?, to)
    } else if meta.is_dir() {
        std::fs::create_dir(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_all(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// All of a thing, a folder's things included, links not followed.
pub fn size_of(path: &Path) -> u64 {
    match path.symlink_metadata() {
        Ok(m) if m.is_dir() => std::fs::read_dir(path).into_iter().flatten().flatten().map(|e| size_of(&e.path())).sum(),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

// --- the info file ---------------------------------------------------------------------

/// A `.trashinfo`: where it was, URL-escaped as the spec says, and when, in local time.
pub fn info_text(path: &Path, stamp: &str) -> String {
    format!("[Trash Info]\nPath={}\nDeletionDate={stamp}\n", escape(path.as_os_str().as_bytes()))
}

/// Where a `.trashinfo` says it was, and when (as written).
pub fn parse_info(text: &str) -> (Option<PathBuf>, Option<String>) {
    let mut in_group = false;
    let (mut path, mut date) = (None, None);
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with('[') {
            in_group = line == "[Trash Info]";
            continue;
        }
        if !in_group {
            continue;
        }
        if let Some(value) = line.strip_prefix("Path=") {
            path = Some(PathBuf::from(OsString::from_vec(unescape(value))));
        } else if let Some(value) = line.strip_prefix("DeletionDate=") {
            date = Some(value.trim().to_string());
        }
    }
    (path, date)
}

/// RFC 2396's escaping, as the spec has it: every byte but the unreserved ones and `/` as
/// `%XX`.
pub fn escape(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &b in bytes {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn unescape(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// A moment as an info file writes it: `YYYY-MM-DDThh:mm:ss`, this machine's local time.
pub fn stamp(seconds: i64) -> String {
    let t = seconds as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// The fields of a written moment: year, month, day, hour, minute, second.
pub fn stamp_fields(text: &str) -> Option<[i32; 6]> {
    let (date, time) = text.trim().split_once('T')?;
    let mut d = date.split('-').map(|n| n.parse::<i32>().ok());
    let mut t = time.get(..8).unwrap_or(time).split(':').map(|n| n.parse::<i32>().ok());
    Some([d.next()??, d.next()??, d.next()??, t.next()??, t.next()??, t.next().flatten().unwrap_or(0)])
}

/// A written moment, in local time, as seconds since 1970.
pub fn parse_stamp(text: &str) -> Option<i64> {
    let [y, mo, d, h, mi, s] = stamp_fields(text)?;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = y - 1900;
    tm.tm_mon = mo - 1;
    tm.tm_mday = d;
    tm.tm_hour = h;
    tm.tm_min = mi;
    tm.tm_sec = s;
    tm.tm_isdst = -1;
    let t = unsafe { libc::mktime(&mut tm) };
    (t != -1).then_some(t as i64)
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

// --- which trash ------------------------------------------------------------------------

fn uid() -> u32 {
    unsafe { libc::getuid() }
}

fn device(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.dev())
}

/// The home trash: `$XDG_DATA_HOME/Trash`, by default `~/.local/share/Trash`. None on
/// Android, whose files are on the storage volumes, not in the app's home.
pub fn home_can() -> Option<Can> {
    if cfg!(target_os = "android") {
        return None;
    }
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(Can { root: data.join("Trash"), top: None })
}

/// The disk's own trash at `top`, for this user: `.Trash/$uid` where the administrator made a
/// `.Trash` for everyone (a folder, sticky, not a link), else `.Trash-$uid`. On Android, the
/// volume's `.Trash-spatiand`.
pub fn top_can(top: &Path, android: bool) -> Can {
    if android {
        return Can { root: top.join(".Trash-spatiand"), top: Some(top.to_path_buf()) };
    }
    let shared = top.join(".Trash");
    if let Ok(meta) = shared.symlink_metadata() {
        use std::os::unix::fs::PermissionsExt;
        if meta.is_dir() && meta.permissions().mode() & 0o1000 != 0 {
            return Can { root: shared.join(uid().to_string()), top: Some(top.to_path_buf()) };
        }
    }
    Can { root: top.join(format!(".Trash-{}", uid())), top: Some(top.to_path_buf()) }
}

/// The top of the disk `path` is on: the last folder up from it on the same disk.
pub fn top_of(path: &Path) -> Option<PathBuf> {
    let dev = device(path)?;
    let mut at = path.parent()?.to_path_buf();
    if device(&at) != Some(dev) {
        return Some(path.to_path_buf());
    }
    while let Some(up) = at.parent() {
        if device(up) != Some(dev) {
            break;
        }
        at = up.to_path_buf();
    }
    Some(at)
}

/// The Android storage volume `path` is on: `/storage/emulated/<user>` (where `/sdcard`
/// leads) or a card's `/storage/XXXX-XXXX`.
pub fn android_volume(path: &Path) -> Option<PathBuf> {
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let parts: Vec<_> = real.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    match parts.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["/", "storage", "emulated", user, ..] => Some(PathBuf::from(format!("/storage/emulated/{user}"))),
        ["/", "storage", card, ..] if *card != "self" && *card != "emulated" => Some(PathBuf::from(format!("/storage/{card}"))),
        _ => None,
    }
}

/// Every trash there is on this machine: the home one and each disk's (Linux), each volume's
/// (Android).
pub fn cans() -> Vec<Can> {
    let mut out = Vec::new();
    if cfg!(target_os = "android") {
        let mut volumes = vec![PathBuf::from("/storage/emulated/0")];
        for entry in std::fs::read_dir("/storage").into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name != "emulated" && name != "self" {
                volumes.push(entry.path());
            }
        }
        out.extend(volumes.into_iter().map(|v| top_can(&v, true)).filter(|c| c.root.is_dir()));
        return out;
    }
    out.extend(home_can());
    // Each disk's, where one has been made. Network disks are left alone: a look at one that
    // has gone away can hang.
    let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    for (top, kind) in mount_points(&mounts) {
        if top == Path::new("/") || NOT_DISKS.contains(&kind.as_str()) || kind.starts_with("fuse.") && kind != "fuse.ntfs-3g" && kind != "fuseblk" {
            continue;
        }
        if top.starts_with("/proc") || top.starts_with("/sys") || top.starts_with("/dev") {
            continue;
        }
        for can in [Can { root: top.join(".Trash").join(uid().to_string()), top: Some(top.clone()) }, Can { root: top.join(format!(".Trash-{}", uid())), top: Some(top.clone()) }] {
            if can.root.is_dir() && !out.contains(&can) {
                out.push(can);
            }
        }
    }
    out
}

/// What is not a disk of files, among the mounts: the kernel's own, and the network's.
const NOT_DISKS: &[&str] = &[
    "proc", "sysfs", "devtmpfs", "devpts", "tmpfs", "cgroup", "cgroup2", "securityfs", "debugfs", "tracefs", "pstore", "bpf", "mqueue", "hugetlbfs", "configfs", "fusectl", "autofs", "binfmt_misc", "nsfs", "efivarfs", "ramfs", "rpc_pipefs", "nfs", "nfs4", "cifs", "smb3", "9p", "overlay", "squashfs", "selinuxfs",
];

/// The mount points of a `/proc/self/mounts`, with their kinds (spaces in a path written
/// `\040`).
pub fn mount_points(text: &str) -> Vec<(PathBuf, String)> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split(' ');
            let (_, at, kind) = (fields.next()?, fields.next()?, fields.next()?);
            Some((PathBuf::from(at.replace("\\040", " ").replace("\\011", "\t").replace("\\134", "\\")), kind.to_string()))
        })
        .collect()
}

/// Which trash `path` goes into, and whether it is copied there (another disk with no trash
/// of its own to be had).
fn can_for(path: &Path) -> Result<(Can, bool), String> {
    if cfg!(target_os = "android") {
        let volume = android_volume(path).ok_or("There is no trash for this place.")?;
        let can = top_can(&volume, true);
        can.make().map_err(|e| e.to_string())?;
        // What is in it is not the gallery's.
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(can.root.join(".nomedia"));
        return Ok((can, false));
    }
    let home = home_can().ok_or("There is no home folder for a trash.")?;
    // The home trash's disk, from the nearest folder of it that is there.
    let mut probe = home.root.clone();
    while !probe.exists() {
        if !probe.pop() {
            break;
        }
    }
    let here = device(path.parent().unwrap_or(path));
    if here.is_some() && here == device(&probe) {
        return Ok((home, false));
    }
    if let Some(top) = top_of(path) {
        let can = top_can(&top, false);
        if can.make().is_ok() {
            return Ok((can, false));
        }
    }
    Ok((home, true))
}

/// The trash a thing in one is in, if it is at its top.
pub fn can_holding(file: &Path) -> Option<Can> {
    #[cfg(target_os = "macos")]
    {
        let _ = file;
        None
    }
    #[cfg(not(target_os = "macos"))]
    {
        let files = file.parent()?;
        if files.file_name()? != "files" {
            return None;
        }
        let root = files.parent()?;
        let can = cans().into_iter().find(|c| c.root == root);
        can.or_else(|| {
            // One not on the list (a disk's trash looked at directly): known by its shape.
            let name = root.file_name()?.to_string_lossy().into_owned();
            let shaped = name == "Trash" || name.starts_with(".Trash-") || root.parent().and_then(|p| p.file_name()).is_some_and(|n| n == ".Trash");
            (shaped && root.join("info").is_dir()).then(|| Can { root: root.to_path_buf(), top: if name == "Trash" { None } else { root.parent().map(|p| if name.starts_with(".Trash-") { p.to_path_buf() } else { p.parent().unwrap_or(p).to_path_buf() }) } })
        })
    }
}

/// Whether `path` is in a trash, anywhere down: what is there is deleted for good, not put in
/// the trash again.
pub fn is_within(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        return mac::is_within(path);
    }
    #[allow(unreachable_code)]
    {
        let mut at = Some(path);
        while let Some(p) = at {
            if can_holding(p).is_some() {
                return true;
            }
            at = p.parent();
        }
        false
    }
}

// --- what SpatiShelf does ---------------------------------------------------------------

/// Into the trash.
pub fn put(path: &Path) -> Result<(), String> {
    if path.symlink_metadata().is_err() {
        return Err(format!("{} is not there any more.", path.display()));
    }
    if is_within(path) {
        return Err("That is in the Trash already.".into());
    }
    #[cfg(target_os = "macos")]
    {
        return mac::put(path);
    }
    #[allow(unreachable_code)]
    {
        let (can, copy) = can_for(path)?;
        // Kept as the path was given, as the file manager shows it: a link to a folder stays
        // the link it was gone through.
        let whole = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().map(|d| d.join(path)).unwrap_or_else(|_| path.to_path_buf()) };
        can.put(&whole, &stamp(now()), copy).map(|_| ()).map_err(|e| e.to_string())
    }
}

/// What is in every trash.
pub fn list() -> Result<Vec<Entry>, String> {
    #[cfg(target_os = "macos")]
    {
        return mac::list();
    }
    #[allow(unreachable_code)]
    Ok(cans().iter().flat_map(Can::list).collect())
}

/// The folders whose changes change what the trash holds: to watch.
pub fn folders() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        return mac::folders();
    }
    #[allow(unreachable_code)]
    {
        let mut out: Vec<PathBuf> = cans().iter().flat_map(|c| [c.files(), c.info()]).collect();
        // The home trash before anything has been put in it: its folders are made now, so
        // that the first thing put in it from anywhere is seen.
        if let Some(home) = home_can() {
            if !home.root.is_dir() && home.make().is_ok() {
                out.extend([home.files(), home.info()]);
            }
        }
        out
    }
}

/// Put back where it came from. Where it went.
pub fn restore(file: &Path) -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    {
        return mac::restore(file);
    }
    #[allow(unreachable_code)]
    can_holding(file).ok_or("That is not in the Trash.")?.restore(file)
}

/// Gone for good.
pub fn purge(file: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        return mac::purge(file);
    }
    #[allow(unreachable_code)]
    match can_holding(file) {
        Some(can) => can.purge(file).map_err(|e| e.to_string()),
        None => remove(file).map_err(|e| e.to_string()),
    }
}

/// A thing at the top of a trash moved out of it some other way than Put back (dragged
/// elsewhere): its info file goes too.
pub fn moved_out(file: &Path) {
    #[cfg(target_os = "macos")]
    {
        mac::moved_out(file);
    }
    #[cfg(not(target_os = "macos"))]
    if let (Some(can), Some(name)) = (can_holding(file), file.file_name()) {
        let _ = std::fs::remove_file(can.info_of(name));
    }
}

/// Every trash emptied.
pub fn empty() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        return mac::empty();
    }
    #[allow(unreachable_code)]
    cans().iter().try_for_each(Can::empty).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spatishelf-trash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn paths_are_escaped_as_the_spec_says() {
        assert_eq!(escape(b"/home/me/a b/caf\xc3\xa9 #1%.txt"), "/home/me/a%20b/caf%C3%A9%20%231%25.txt");
        assert_eq!(unescape("/home/me/a%20b/caf%C3%A9%20%231%25.txt"), b"/home/me/a b/caf\xc3\xa9 #1%.txt");
        assert_eq!(unescape("50%"), b"50%", "a lone % stays");
        assert_eq!(unescape("%zz"), b"%zz");
    }

    #[test]
    fn info_files_read_back() {
        let text = info_text(Path::new("/home/me/Photos/a b.jpg"), "2026-11-15T09:08:07");
        assert_eq!(text, "[Trash Info]\nPath=/home/me/Photos/a%20b.jpg\nDeletionDate=2026-11-15T09:08:07\n");
        assert_eq!(parse_info(&text), (Some(PathBuf::from("/home/me/Photos/a b.jpg")), Some("2026-11-15T09:08:07".into())));
        // Dolphin's, with a group before and a line ending of its own.
        let kde = "[Other]\nPath=/nope\n[Trash Info]\r\nPath=Music/x.flac\r\nDeletionDate=2020-01-02T03:04:05\r\n";
        assert_eq!(parse_info(kde), (Some(PathBuf::from("Music/x.flac")), Some("2020-01-02T03:04:05".into())));
        assert_eq!(stamp_fields("2026-11-15T09:08:07"), Some([2026, 11, 15, 9, 8, 7]));
        assert_eq!(stamp_fields("nonsense"), None);
    }

    #[test]
    fn a_moment_written_and_read_is_itself() {
        let t = 1_794_000_000;
        assert_eq!(parse_stamp(&stamp(t)), Some(t));
    }

    #[test]
    fn put_listed_put_back_and_names_that_clash() {
        let dir = scratch("put");
        let can = Can { root: dir.join("Trash"), top: None };
        std::fs::create_dir_all(dir.join("Photos/2024")).unwrap();
        std::fs::write(dir.join("Photos/a b.jpg"), b"one").unwrap();
        std::fs::write(dir.join("Photos/2024/a b.jpg"), b"two!").unwrap();
        let first = can.put(&dir.join("Photos/a b.jpg"), "2026-11-15T09:08:07", false).unwrap();
        let second = can.put(&dir.join("Photos/2024/a b.jpg"), "2026-11-15T09:08:08", false).unwrap();
        assert_eq!(first, can.files().join("a b.jpg"));
        assert_eq!(second, can.files().join("a b 2.jpg"), "the second of a name is numbered");
        assert!(can.info().join("a b 2.jpg.trashinfo").is_file());
        assert!(!dir.join("Photos/a b.jpg").exists());
        let listed = can.list();
        assert_eq!(listed.len(), 2);
        let numbered = listed.iter().find(|e| e.file == second).unwrap();
        assert_eq!(numbered.name, "a b.jpg", "shown by its own name");
        assert_eq!(numbered.origin.as_deref(), Some(dir.join("Photos/2024/a b.jpg").as_path()));
        assert_eq!(numbered.size, 4);
        assert!(numbered.deleted.is_some());
        // Something has come by its name meanwhile: back beside it, numbered.
        std::fs::write(dir.join("Photos/a b.jpg"), b"new").unwrap();
        assert_eq!(can.restore(&first).unwrap(), dir.join("Photos/a b 2.jpg"));
        assert_eq!(std::fs::read(dir.join("Photos/a b 2.jpg")).unwrap(), b"one");
        assert!(!can.info().join("a b.jpg.trashinfo").exists(), "its info file goes with it");
        // The folder it was in has gone: made again.
        std::fs::remove_dir_all(dir.join("Photos/2024")).unwrap();
        assert_eq!(can.restore(&second).unwrap(), dir.join("Photos/2024/a b.jpg"));
        assert_eq!(std::fs::read(dir.join("Photos/2024/a b.jpg")).unwrap(), b"two!");
        assert!(can.list().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folders_their_size_purging_and_emptying() {
        let dir = scratch("empty");
        let can = Can { root: dir.join("Trash"), top: None };
        std::fs::create_dir_all(dir.join("Old/inner")).unwrap();
        std::fs::write(dir.join("Old/inner/f"), vec![0u8; 1000]).unwrap();
        std::fs::write(dir.join("Old/g"), vec![0u8; 24]).unwrap();
        std::os::unix::fs::symlink("/etc", dir.join("Old/link")).unwrap();
        std::fs::write(dir.join("note.txt"), b"x").unwrap();
        let old = can.put(&dir.join("Old"), "2026-11-15T09:08:07", false).unwrap();
        let note = can.put(&dir.join("note.txt"), "2026-11-15T09:08:07", false).unwrap();
        let listed = can.list();
        let folder = listed.iter().find(|e| e.file == old).unwrap();
        assert!(folder.folder);
        assert!(folder.size >= 1024 && folder.size < 1024 + 100, "the link counted as itself, not what it leads to: {}", folder.size);
        // A thing with no info file (put there by hand) is shown, from nowhere known.
        std::fs::write(can.files().join("stray"), b"s").unwrap();
        assert!(can.list().iter().any(|e| e.name == "stray" && e.origin.is_none()));
        assert!(can.restore(&can.files().join("stray")).is_err(), "where it goes back to is not known");
        can.purge(&note).unwrap();
        assert!(!note.exists() && !can.info().join("note.txt.trashinfo").exists());
        assert!(Path::new("/etc").is_dir(), "a link is taken away, not what it leads to");
        can.empty().unwrap();
        assert!(can.list().is_empty());
        assert_eq!(std::fs::read_dir(can.info()).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_disks_trash_writes_paths_from_its_top_and_copies_when_it_must() {
        let dir = scratch("top");
        std::fs::create_dir_all(dir.join("disk/Music")).unwrap();
        std::fs::write(dir.join("disk/Music/x.flac"), b"flac").unwrap();
        let can = top_can(&dir.join("disk"), false);
        assert_eq!(can.root, dir.join(format!("disk/.Trash-{}", uid())));
        let put = can.put(&dir.join("disk/Music/x.flac"), "2026-11-15T09:08:07", true).unwrap();
        let text = std::fs::read_to_string(can.info().join("x.flac.trashinfo")).unwrap();
        assert!(text.contains("\nPath=Music/x.flac\n"), "relative to the disk's top: {text}");
        assert_eq!(can.list()[0].origin.as_deref(), Some(dir.join("disk/Music/x.flac").as_path()));
        assert_eq!(std::fs::read(&put).unwrap(), b"flac");
        assert!(!dir.join("disk/Music/x.flac").exists(), "copied, then taken away");
        // The administrator's shared .Trash, sticky: this user's folder in it.
        std::fs::create_dir_all(dir.join("disk2/.Trash")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.join("disk2/.Trash"), std::fs::Permissions::from_mode(0o1777)).unwrap();
        assert_eq!(top_can(&dir.join("disk2"), false).root, dir.join(format!("disk2/.Trash/{}", uid())));
        // Not sticky: not to be trusted, so the user's own.
        std::fs::set_permissions(dir.join("disk2/.Trash"), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(top_can(&dir.join("disk2"), false).root, dir.join(format!("disk2/.Trash-{}", uid())));
        assert_eq!(top_can(&dir.join("disk2"), true).root, dir.join("disk2/.Trash-spatiand"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_top_of_a_disk_is_where_its_device_ends() {
        // /proc is a disk of its own on every Linux; its files' top is it.
        if Path::new("/proc/cpuinfo").exists() {
            assert_eq!(top_of(Path::new("/proc/cpuinfo")), Some(PathBuf::from("/proc")));
        }
        assert_eq!(top_of(Path::new("/no/such/thing")), None);
    }

    #[test]
    fn mounts_and_android_volumes() {
        let mounts = "/dev/nvme0n1p2 / ext4 rw 0 0\nproc /proc proc rw 0 0\n/dev/sda1 /run/media/me/My\\040Disk exfat rw 0 0\n";
        assert_eq!(mount_points(mounts)[2], (PathBuf::from("/run/media/me/My Disk"), "exfat".to_string()));
        assert_eq!(android_volume(Path::new("/storage/emulated/0/DCIM/a.jpg")), Some(PathBuf::from("/storage/emulated/0")));
        assert_eq!(android_volume(Path::new("/storage/1A2B-3C4D/Music")), Some(PathBuf::from("/storage/1A2B-3C4D")));
        assert_eq!(android_volume(Path::new("/data/local/tmp/x")), None);
    }

    #[test]
    fn a_trash_is_known_by_its_shape() {
        let dir = scratch("shape");
        let can = top_can(&dir, false);
        std::fs::write(dir.join("f"), b"").unwrap();
        let put = can.put(&dir.join("f"), "2026-11-15T09:08:07", false).unwrap();
        let found = can_holding(&put).unwrap();
        assert_eq!(found.root, can.root);
        assert_eq!(found.top.as_deref(), Some(dir.as_path()));
        assert!(is_within(&put) && !is_within(&dir));
        moved_out(&put);
        assert!(!can.info().join("f.trashinfo").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
