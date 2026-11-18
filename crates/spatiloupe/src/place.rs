//! Where a file is: on this machine, or on a remote (`remote:folder/file`), reached through
//! the storage service. Everything that reads a file or a folder asks here, so that a remote's
//! pictures are looked at exactly as this machine's are -- read into memory as they are
//! wanted, and kept nowhere.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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

#[cfg(test)]
mod tests {
    use super::*;

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
