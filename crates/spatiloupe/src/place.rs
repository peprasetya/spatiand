//! Where a file is: on this machine, or on a remote (`remote:folder/file`), reached through
//! the storage service. Everything that reads a file or a folder asks here, so that a remote's
//! pictures are looked at exactly as this machine's are -- read into memory as they are
//! wanted, and kept nowhere. A remote's PDF is the one thing copied onto this machine: SpatiMesh,
//! which shows it, is given files of this machine only ([`fetched`]).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use spatiand_storage::Storage;

/// A remote and the path in it, if `path` is one.
pub fn remote(path: &Path) -> Option<(String, String)> {
    spatiand_storage::virtual_place(&path.to_string_lossy())
}

fn storage() -> Result<&'static Storage, String> {
    static STORAGE: OnceLock<Result<Storage, String>> = OnceLock::new();
    STORAGE.get_or_init(Storage::connect).as_ref().map_err(|e| e.clone())
}

/// The folder a place is in.
pub fn parent(path: &Path) -> Option<PathBuf> {
    match remote(path) {
        Some((r, p)) if p.is_empty() => {
            let _ = r;
            None
        }
        Some((r, p)) => Some(PathBuf::from(match p.rsplit_once('/') {
            Some((dir, _)) => format!("{r}:{dir}"),
            None => format!("{r}:"),
        })),
        None => path.parent().map(Path::to_path_buf),
    }
}

/// A place's last name.
pub fn name(path: &Path) -> String {
    match remote(path) {
        Some((r, p)) if p.is_empty() => r,
        Some((_, p)) => p.rsplit('/').next().unwrap_or(&p).to_string(),
        None => path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned()),
    }
}

pub fn join(folder: &Path, name: &str) -> PathBuf {
    match remote(folder) {
        Some((r, p)) if p.is_empty() => PathBuf::from(format!("{r}:{name}")),
        Some((r, p)) => PathBuf::from(format!("{r}:{}/{name}", p.trim_end_matches('/'))),
        None => folder.join(name),
    }
}

/// A file, whole, into memory.
pub fn read(path: &Path) -> Result<Vec<u8>, String> {
    match remote(path) {
        Some((r, p)) => storage()?.read(&r, &p, 2 << 30),
        None => std::fs::read(path).map_err(|e| e.to_string()),
    }
}

/// Whether there is something at `path`.
pub fn exists(path: &Path) -> bool {
    match remote(path) {
        Some((r, p)) => storage().and_then(|s| s.exists(&r, &p)).unwrap_or(false),
        None => path.symlink_metadata().is_ok(),
    }
}

/// A name for a file being written beside `path`, before it takes its place: hidden, and this
/// program's.
fn beside(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let name = format!(".{}.spatiloupe-{}-{}.part", name(path), std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed));
    match parent(path) {
        Some(folder) => join(&folder, &name),
        None => PathBuf::from(name),
    }
}

/// Write `bytes` into a new file beside `path`, whole and on the disk, with `like`'s permissions
/// if it is given: the first step of writing a file that is not seen half-written.
fn write_beside(path: &Path, bytes: &[u8], like: Option<&Path>) -> Result<PathBuf, String> {
    use std::io::Write;
    let part = beside(path);
    let written = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&part)?;
        file.write_all(bytes)?;
        if let Some(meta) = like.and_then(|like| std::fs::metadata(like).ok()) {
            unless_unsupported(file.set_permissions(meta.permissions()))?;
        }
        to_disk(&file)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&part);
        return Err(e.to_string());
    }
    Ok(part)
}

/// A step a disk may not have -- a network share's, a camera card's -- is no failure of the
/// writing: only what it has and fails at is.
fn unless_unsupported(done: std::io::Result<()>) -> std::io::Result<()> {
    match done {
        Err(e) if e.kind() == std::io::ErrorKind::Unsupported || e.raw_os_error() == Some(libc::EINVAL) => Ok(()),
        other => other,
    }
}

/// What was written to `file`, on its disk before it takes its place. A Mac's own way of that
/// (`F_FULLFSYNC`, which is what `sync_all` is there) is refused by disks that are not its own
/// -- "os error 45" on a network share, 2026-11-24 -- and the plain way is then asked for; a
/// disk with neither has the file as written.
fn to_disk(file: &std::fs::File) -> std::io::Result<()> {
    match file.sync_all() {
        Ok(()) => Ok(()),
        Err(_) => {
            use std::os::fd::AsRawFd;
            let plain = if unsafe { libc::fsync(file.as_raw_fd()) } == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) };
            unless_unsupported(plain)
        }
    }
}

/// The folder `path` is in, told that what is in it changed for good.
fn sync_folder(path: &Path) {
    if let Some(folder) = path.parent().and_then(|f| std::fs::File::open(f).ok()) {
        let _ = folder.sync_all();
    }
}

/// Write `bytes` over the file at `path`, whole or not at all: into a new file in the same
/// folder, which is then renamed over it -- with its permissions. A remote's, through the storage
/// service: written beside it under another name, then moved over it.
pub fn replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    match remote(path) {
        Some((r, p)) => {
            let storage = storage()?;
            let part = remote(&beside(path)).map(|(_, p)| p).ok_or("not a remote's")?;
            storage.write(&r, &part, bytes)?;
            storage.rename(&r, &part, &p).inspect_err(|_| {
                let _ = storage.delete(&r, &part);
            })
        }
        None => {
            let part = write_beside(path, bytes, Some(path))?;
            std::fs::rename(&part, path).map_err(|e| {
                let _ = std::fs::remove_file(&part);
                e.to_string()
            })?;
            sync_folder(path);
            Ok(())
        }
    }
}

/// Write `bytes` as a new file at `path`, not over anything there: whole, or not at all.
pub fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let taken = || format!("{} is there already", name(path));
    match remote(path) {
        Some((r, p)) => {
            let storage = storage()?;
            if storage.exists(&r, &p)? {
                return Err(taken());
            }
            storage.write(&r, &p, bytes)
        }
        None => {
            let part = write_beside(path, bytes, None)?;
            if path.symlink_metadata().is_ok() {
                let _ = std::fs::remove_file(&part);
                return Err(taken());
            }
            std::fs::rename(&part, path).map_err(|e| {
                let _ = std::fs::remove_file(&part);
                e.to_string()
            })?;
            sync_folder(path);
            Ok(())
        }
    }
}

/// A remote file to be read about in -- its camera's account -- without fetching it whole: what
/// is read is fetched a piece at a time, as far as it is wanted, by a range each. None for a
/// place on this machine, which is read about in its file.
pub fn pieces(path: &Path) -> Option<Pieces<impl FnMut(u64, u64) -> std::io::Result<(Vec<u8>, Option<u64>)>>> {
    let (r, p) = remote(path)?;
    Some(Pieces::new(move |start, len| storage().and_then(|s| s.read_part(&r, &p, start, len)).map_err(std::io::Error::other)))
}

/// A file read in pieces, each fetched by `fetch(start, len)` (its bytes, and the file's whole
/// length if known) as the reading comes to it and kept: read in order, a file's head is one
/// fetch, and a jump further in -- to where a HEIF's meta box says its Exif is -- one more. No
/// more than [`MOST_PIECES`] is fetched in all, so a reader wanting the whole file is refused,
/// not given it.
pub struct Pieces<F> {
    fetch: F,
    /// The file's length, once an answer has said it.
    length: Option<u64>,
    at: u64,
    /// What has been fetched: where each piece starts, and its bytes.
    held: Vec<(u64, Vec<u8>)>,
    fetched: u64,
}

/// How much of a file is fetched at a time: a phone's HEIC's boxes before its pictures, and its
/// Exif.
pub const PIECE: u64 = 64 << 10;

/// The most of a file fetched in pieces, all told: well past any camera's account (Exif is at
/// most 64 KB).
pub const MOST_PIECES: u64 = 1 << 20;

impl<F: FnMut(u64, u64) -> std::io::Result<(Vec<u8>, Option<u64>)>> Pieces<F> {
    pub fn new(fetch: F) -> Pieces<F> {
        Pieces { fetch, length: None, at: 0, held: Vec::new(), fetched: 0 }
    }

    /// How many bytes have been fetched.
    #[cfg(test)]
    pub fn fetched(&self) -> u64 {
        self.fetched
    }

    /// The piece holding `at`, and where in it `at` is.
    fn piece(&self, at: u64) -> Option<(usize, usize)> {
        self.held.iter().position(|(start, bytes)| at >= *start && at < start + bytes.len() as u64).map(|i| (i, (at - self.held[i].0) as usize))
    }
}

impl<F: FnMut(u64, u64) -> std::io::Result<(Vec<u8>, Option<u64>)>> std::io::BufRead for Pieces<F> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.piece(self.at).is_none() {
            if self.length.is_some_and(|length| self.at >= length) {
                return Ok(&[]);
            }
            // Up to the next piece held, so that none is fetched twice.
            let next = self.held.iter().map(|(start, _)| *start).filter(|&start| start > self.at).min();
            let len = PIECE.min(next.map_or(u64::MAX, |next| next - self.at));
            if self.fetched + len > MOST_PIECES {
                return Err(std::io::Error::other("read about in further than a file's head"));
            }
            let (bytes, length) = (self.fetch)(self.at, len)?;
            self.fetched += len;
            self.length = self.length.or(length);
            if bytes.is_empty() {
                // Past its end.
                self.length = Some(self.length.unwrap_or(self.at).min(self.at));
                return Ok(&[]);
            }
            self.held.push((self.at, bytes));
        }
        let (i, offset) = self.piece(self.at).expect("held");
        Ok(&self.held[i].1[offset..])
    }

    fn consume(&mut self, amount: usize) {
        self.at += amount as u64;
    }
}

impl<F: FnMut(u64, u64) -> std::io::Result<(Vec<u8>, Option<u64>)>> std::io::Read for Pieces<F> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let held = std::io::BufRead::fill_buf(self)?;
        let n = held.len().min(out.len());
        out[..n].copy_from_slice(&held[..n]);
        std::io::BufRead::consume(self, n);
        Ok(n)
    }
}

impl<F: FnMut(u64, u64) -> std::io::Result<(Vec<u8>, Option<u64>)>> std::io::Seek for Pieces<F> {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        let at = match to {
            std::io::SeekFrom::Start(n) => Some(n),
            std::io::SeekFrom::Current(d) => self.at.checked_add_signed(d),
            std::io::SeekFrom::End(d) => {
                // The length, from the first piece's answer if nothing has been read yet.
                if self.length.is_none() && self.held.is_empty() {
                    let at = std::mem::replace(&mut self.at, 0);
                    std::io::BufRead::fill_buf(self)?;
                    self.at = at;
                }
                let length = self.length.ok_or_else(|| std::io::Error::other("a file of a length not known"))?;
                length.checked_add_signed(d)
            }
        };
        self.at = at.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "before the file's start"))?;
        Ok(self.at)
    }
}

/// What FFmpeg is to open for a file: its path, or the stream of it.
pub fn source(path: &Path) -> String {
    match remote(path).and_then(|(r, p)| storage().ok().map(|s| s.url(&r, &p))) {
        Some(url) => url,
        None => path.to_string_lossy().into_owned(),
    }
}

/// A folder's things: (name to show, where it is, is a folder, bytes, when it was last changed).
/// Where it is is the system's own name for it: a name that is not UTF-8 is shown as near as it
/// can be, and opened by what it is. When it was changed is only to be compared: a remote's own
/// words for it (rclone's ModTime), or this machine's time as seconds and nanoseconds.
pub fn list(folder: &Path) -> Result<Vec<(String, PathBuf, bool, u64, String)>, String> {
    match remote(folder) {
        Some((r, p)) => Ok(storage()?.list(&r, &p)?.into_iter().map(|e| (e.name.clone(), join(folder, &e.name), e.is_dir, e.size.max(0) as u64, e.modified)).collect()),
        None => {
            let read = std::fs::read_dir(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
            Ok(read
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    let meta = std::fs::metadata(&path).ok()?;
                    Some((entry.file_name().to_string_lossy().into_owned(), path, meta.is_dir(), meta.len(), modified(&meta)))
                })
                .collect())
        }
    }
}

/// When a file on this machine was last changed, as words only to be compared: none where the
/// system does not say.
pub fn modified(meta: &std::fs::Metadata) -> String {
    match meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()) {
        Some(d) => format!("{}.{:09}", d.as_secs(), d.subsec_nanos()),
        None => String::new(),
    }
}

/// A file of this machine as an address for SpatiMesh: `file://` and its whole path, every byte of
/// it but the letters, the digits, `-._~` and `/` written as `%XX` -- a space, a quote, a `#` or a
/// `?` in a name is the name's, not the address's (and the room takes no address with a space or
/// a quote in it), and a name that is not UTF-8 is its bytes, as a file's address is.
pub fn file_url(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let whole = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut url = String::from("file://");
    for &byte in whole.as_os_str().as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => url.push(byte as char),
            _ => url.push_str(&format!("%{byte:02X}")),
        }
    }
    url
}

/// The wearer's cache folder, on the disk, as the room's chooser keeps what it fetches from a remote
/// (`crates/spatiand-choose/src/own.rs`): `~/.cache` (or `XDG_CACHE_HOME`), a Mac's
/// `~/Library/Caches`, and on Android the app's runtime folder, which is its own cache on the disk
/// there. Not the runtime folder elsewhere: on the Deck that is in memory, and a tenth of it.
fn cache_base() -> PathBuf {
    let var = |name: &str| std::env::var_os(name).filter(|d| !d.is_empty()).map(PathBuf::from);
    let cache = if cfg!(target_os = "android") {
        var("XDG_RUNTIME_DIR")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|home| home.join("Library/Caches"))
    } else {
        var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|home| home.join(".cache")))
    };
    cache.unwrap_or_else(std::env::temp_dir)
}

/// Where SpatiLoupe keeps its copies of remotes' PDFs: a folder of its own in the cache.
pub fn copies() -> PathBuf {
    cache_base().join("spatiloupe").join("remote")
}

/// How long a copy of a remote's PDF is kept after it was last opened: a day, as the chooser keeps
/// what it fetched.
const KEPT_FOR: Duration = Duration::from_secs(24 * 3600);

/// The name a copy is kept under: the place it is a copy of, and its size and time as the remote's
/// listing gave them -- another file there since is another copy. FNV-1a, which is the same from
/// one build to the next.
fn copy_key(path: &Path, size: u64, modified: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let said = format!("{}\0{size}\0{modified}", path.to_string_lossy());
    for byte in said.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The copy `name` in `root`'s folder `key`, if it is there whole: as long as the remote said
/// (`size`, where it said one). Marked used now, so that it is kept a day from now: its folder's
/// time is changed by a file arriving in it, which `.used` does, made anew.
fn kept_copy(root: &Path, key: &str, name: &str, size: u64) -> Option<PathBuf> {
    let file = root.join(key).join(name);
    let meta = std::fs::metadata(&file).ok()?;
    if !meta.is_file() || (size > 0 && meta.len() != size) {
        return None;
    }
    let used = root.join(key).join(".used");
    let _ = std::fs::remove_file(&used);
    let _ = std::fs::write(&used, b"");
    Some(file)
}

/// The copies in `root` last used longer than `kept` ago, taken away: a folder's own time is when
/// something last arrived in it (a copy, or its `.used`). Half-made ones too (`.<key>-...`), left
/// by a SpatiLoupe that ended while it copied.
fn sweep(root: &Path, kept: Duration) {
    let Ok(read) = std::fs::read_dir(root) else { return };
    let now = SystemTime::now();
    for entry in read.flatten() {
        let old = entry.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| now.duration_since(t).ok()).is_some_and(|age| age > kept);
        if old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// A copy made in `root` as `key`/`name` by `fetch`, which is given the folder to write `name` into
/// and says whether it finished (false: it was stopped). Made in a folder of its own and moved
/// into its place whole, so that a copy there is never half one; another SpatiLoupe's that got
/// there first is taken.
fn make_copy(root: &Path, key: &str, name: &str, fetch: impl FnOnce(&Path) -> Result<bool, String>) -> Result<Option<PathBuf>, String> {
    use std::os::unix::fs::DirBuilderExt;
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNT: AtomicU32 = AtomicU32::new(0);
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let making = root.join(format!(".{key}-{}-{}", std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed)));
    std::fs::DirBuilder::new().mode(0o700).create(&making).map_err(|e| e.to_string())?;
    let done = fetch(&making);
    if !matches!(done, Ok(true)) || !making.join(name).is_file() {
        let _ = std::fs::remove_dir_all(&making);
        return match done {
            Ok(true) => Err("the copy did not arrive".into()),
            Ok(false) => Ok(None),
            Err(e) => Err(e),
        };
    }
    let place = root.join(key);
    // One there already, whole, is as good; one that is not is made way for.
    if place.join(name).is_file() {
        let _ = std::fs::remove_dir_all(&making);
        return Ok(Some(place.join(name)));
    }
    let _ = std::fs::remove_dir_all(&place);
    if std::fs::rename(&making, &place).is_err() && !place.join(name).is_file() {
        let _ = std::fs::remove_dir_all(&making);
        return Err("the copy could not be put in its place".into());
    }
    let _ = std::fs::remove_dir_all(&making);
    Ok(Some(place.join(name)))
}

/// A remote's file copied onto this machine, for SpatiMesh, which is given only this machine's
/// files: into SpatiLoupe's folder in the wearer's cache ([`copies`]) by the storage service's
/// rclone, as the room's chooser fetches (`operations/copyfile`), told how far it has come
/// (`going(done, total)`) and stopped when that says no. `size` and `modified` are what the
/// remote's listing said of it: a copy of the same, made in the last day, is opened again rather
/// than fetched again. `None` if it was stopped.
pub fn fetched(path: &Path, size: u64, modified: &str, mut going: impl FnMut(u64, u64) -> bool) -> Result<Option<PathBuf>, String> {
    let (remote, inside) = remote(path).ok_or("it is not a remote's")?;
    let name = self::name(path);
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err("it has no name to be copied by".into());
    }
    let root = copies();
    let key = copy_key(path, size, modified);
    if let Some(kept) = kept_copy(&root, &key, &name, size) {
        return Ok(Some(kept));
    }
    sweep(&root, KEPT_FOR);
    let storage = storage()?;
    let inside = inside.trim_matches('/');
    let (dir, leaf) = inside.rsplit_once('/').unwrap_or(("", inside));
    make_copy(&root, &key, &name, |into| {
        let asked = serde_json::json!({ "srcFs": format!("{remote}:{dir}"), "srcRemote": leaf, "dstFs": into.to_string_lossy(), "dstRemote": name });
        storage.job("operations/copyfile", asked, Duration::from_millis(250), |p| going(p.done, p.total.max(size)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_s_address_has_its_whole_path_and_nothing_a_name_holds_is_taken_for_the_address_s() {
        assert_eq!(file_url(Path::new("/home/me/Documents/report.pdf")), "file:///home/me/Documents/report.pdf");
        // A space, quotes, a hash, a question mark, a percent and a backslash: the name's own.
        assert_eq!(file_url(Path::new("/tmp/a b/it's \"x\" #2?.pdf")), "file:///tmp/a%20b/it%27s%20%22x%22%20%232%3F.pdf");
        assert_eq!(file_url(Path::new("/tmp/100%\\x.pdf")), "file:///tmp/100%25%5Cx.pdf");
        // Other scripts as their UTF-8, and a name that is not UTF-8 as its bytes.
        assert_eq!(file_url(Path::new("/tmp/Grüße 你好.pdf")), "file:///tmp/Gr%C3%BC%C3%9Fe%20%E4%BD%A0%E5%A5%BD.pdf");
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(file_url(Path::new(std::ffi::OsStr::from_bytes(b"/tmp/caf\xe9.pdf"))), "file:///tmp/caf%E9.pdf");
        // Nothing the room refuses in an address is left in one.
        let url = file_url(Path::new("/x/ a'b`c\"d\\e\n.pdf"));
        assert!(!url.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\\' | '\'' | '`')), "{url}");
    }

    #[test]
    fn a_remote_s_copy_is_made_whole_kept_and_found_again_and_an_old_one_swept_away() {
        let root = std::env::temp_dir().join(format!("spatiloupe-copies-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let key = copy_key(Path::new("Drive:Docs/report.pdf"), 9, "2026-11-27T10:00:00Z");
        // Another time or size there is another copy.
        assert_ne!(key, copy_key(Path::new("Drive:Docs/report.pdf"), 9, "2026-11-27T10:00:01Z"));
        assert_ne!(key, copy_key(Path::new("Drive:Docs/report.pdf"), 10, "2026-11-27T10:00:00Z"));
        assert_eq!(kept_copy(&root, &key, "report.pdf", 9), None);
        // Stopped part way: nothing of it left.
        let stopped = make_copy(&root, &key, "report.pdf", |into| {
            std::fs::write(into.join("report.pdf"), b"%PDF").unwrap();
            Ok(false)
        });
        assert_eq!(stopped, Ok(None));
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0, "no half copy left");
        // Failed: said, and nothing left either.
        assert_eq!(make_copy(&root, &key, "report.pdf", |_| Err("the remote is gone".into())), Err("the remote is gone".to_string()));
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        // Made: in its place, under its own name, whole.
        let made = make_copy(&root, &key, "report.pdf", |into| {
            std::fs::write(into.join("report.pdf"), b"%PDF-1.7").unwrap();
            Ok(true)
        });
        let file = root.join(&key).join("report.pdf");
        assert_eq!(made, Ok(Some(file.clone())));
        assert_eq!(std::fs::read(&file).unwrap(), b"%PDF-1.7");
        let names: Vec<String> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, [key.clone()]);
        // Found again as it is; not if the remote's is another size now.
        assert_eq!(kept_copy(&root, &key, "report.pdf", 8), Some(file.clone()));
        assert_eq!(kept_copy(&root, &key, "report.pdf", 0), Some(file.clone()), "a size not known");
        assert_eq!(kept_copy(&root, &key, "report.pdf", 9), None);
        // A day old: swept away. A fresh one stays.
        sweep(&root, Duration::from_secs(3600));
        assert!(file.is_file());
        std::thread::sleep(Duration::from_millis(20));
        sweep(&root, Duration::from_millis(1));
        assert!(!root.join(&key).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn copies_are_kept_in_the_wearer_s_cache_on_the_disk() {
        let copies = copies();
        assert!(copies.ends_with("spatiloupe/remote"), "{}", copies.display());
        if cfg!(target_os = "macos") {
            assert!(copies.to_string_lossy().contains("/Library/Caches/"));
        } else if cfg!(target_os = "linux") {
            let cache = std::env::var_os("XDG_CACHE_HOME").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".cache"));
            assert_eq!(copies, cache.join("spatiloupe/remote"));
        }
    }

    #[test]
    fn remote_places_go_up_to_the_remote_and_no_further() {
        assert_eq!(parent(Path::new("Drive:Photos/a.jpg")), Some(PathBuf::from("Drive:Photos")));
        assert_eq!(parent(Path::new("Drive:Photos")), Some(PathBuf::from("Drive:")));
        assert_eq!(parent(Path::new("Drive:")), None);
        assert_eq!(join(Path::new("Drive:"), "a.jpg"), PathBuf::from("Drive:a.jpg"));
        assert_eq!(name(Path::new("Drive:Photos/a.jpg")), "a.jpg");
        assert_eq!(parent(Path::new("/Users/me/a.jpg")), Some(PathBuf::from("/Users/me")));
    }
}
