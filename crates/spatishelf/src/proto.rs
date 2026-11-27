//! What the window and a tab say to each other over the tab's standard input and output: a
//! byte for what it is, four for how long, and that much -- JSON, or a picture's pixels.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

// --- this machine's names, whatever their bytes ---------------------------------------
//
// A name on Linux or Android is bytes, not always UTF-8 (a file from an old disk, a zip from
// elsewhere: b"caf\xe9.txt"); a place and an item's name here are text, as JSON carries them
// between the window and the tab, and to another SpatiShelf. So a local path is held as text
// that turns back into its own bytes: valid UTF-8 as it is, and each byte that is not (always
// 0x80 or above) as one character of the last private-use plane, U+10FF80 + (byte - 0x80). A
// real character of that range (U+10FF80..U+10FFFF, which nobody names files with) is held
// as its four bytes so held, so that the text is the bytes' and nothing else's. Every name
// that is UTF-8 -- every one on a Mac, every remote's -- is its own text, unchanged. What
// opens, renames or moves a thing turns the text back with `os_of`/`path_of`; what shows it,
// with `shown` (the bytes read lossily, U+FFFD for what is not UTF-8).

/// The first character a byte not UTF-8 is held as: U+10FF80 for 0x80.
const HELD: u32 = 0x10FF00;

fn held(byte: u8) -> char {
    char::from_u32(HELD + byte as u32).expect("in the last plane")
}

fn is_held(c: char) -> bool {
    c as u32 >= HELD + 0x80
}

/// A name or path of this machine as text, whatever its bytes (see above).
pub fn text_of(name: &std::ffi::OsStr) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::with_capacity(name.len());
    for chunk in name.as_bytes().utf8_chunks() {
        for c in chunk.valid().chars() {
            if is_held(c) {
                let mut four = [0u8; 4];
                out.extend(c.encode_utf8(&mut four).bytes().map(held));
            } else {
                out.push(c);
            }
        }
        out.extend(chunk.invalid().iter().map(|&b| held(b)));
    }
    out
}

/// A name or path held as text, as its own bytes again.
pub fn os_of(text: &str) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt;
    let mut out = Vec::with_capacity(text.len());
    for c in text.chars() {
        if is_held(c) {
            out.push((c as u32 - HELD) as u8);
        } else {
            out.extend_from_slice(c.encode_utf8(&mut [0u8; 4]).as_bytes());
        }
    }
    std::ffi::OsString::from_vec(out)
}

pub fn path_of(text: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(os_of(text))
}

/// A name or path held as text, as it is shown: what is not UTF-8 in it as U+FFFD.
pub fn shown(text: &str) -> std::borrow::Cow<'_, str> {
    use std::os::unix::ffi::OsStrExt;
    if !text.chars().any(is_held) {
        return text.into();
    }
    String::from_utf8_lossy(os_of(text).as_bytes()).into_owned().into()
}

/// A place: a folder on this machine, or one on a remote (`remote:path`). A local path is
/// held as `text_of` writes it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Place {
    Local(String),
    Remote(String, String),
    /// What is in the trash -- every trash of this machine's -- as one place (`trash.rs`). Its
    /// things are each where they are in the trash (`Item::trashed`).
    Trash,
}

impl Place {
    pub fn join(&self, name: &str) -> Place {
        match self {
            // Its things are not by name in it: see `Item::trashed`.
            Place::Trash => Place::Trash,
            Place::Local(p) => Place::Local(std::path::Path::new(p).join(name).to_string_lossy().into_owned()),
            Place::Remote(r, p) if p.is_empty() => Place::Remote(r.clone(), name.to_string()),
            Place::Remote(r, p) => Place::Remote(r.clone(), format!("{}/{name}", p.trim_end_matches('/'))),
        }
    }

    pub fn parent(&self) -> Option<Place> {
        match self {
            Place::Local(p) => std::path::Path::new(p).parent().map(|q| Place::Local(q.to_string_lossy().into_owned())),
            Place::Remote(_, p) if p.is_empty() => None,
            Place::Remote(r, p) => Some(Place::Remote(r.clone(), p.rsplit_once('/').map_or(String::new(), |(a, _)| a.to_string()))),
            Place::Trash => None,
        }
    }

    pub fn name(&self) -> String {
        match self {
            Place::Trash => "Trash".into(),
            Place::Local(p) => std::path::Path::new(p).file_name().map_or_else(|| p.clone(), |n| n.to_string_lossy().into_owned()),
            Place::Remote(r, p) if p.is_empty() => r.clone(),
            Place::Remote(_, p) => p.rsplit('/').next().unwrap_or(p).to_string(),
        }
    }

    /// Its name as it is shown (see `shown`).
    pub fn shown_name(&self) -> String {
        shown(&self.name()).into_owned()
    }

    /// The path of a place on this machine, as its own bytes.
    pub fn path(&self) -> Option<std::path::PathBuf> {
        match self {
            Place::Local(p) => Some(path_of(p)),
            _ => None,
        }
    }

    /// Whether `other` is somewhere inside this folder (not this folder itself).
    pub fn holds(&self, other: &Place) -> bool {
        let mut at = other.parent();
        while let Some(place) = at {
            if place == *self {
                return true;
            }
            at = place.parent();
        }
        false
    }

    /// As a person or a Spatiand application writes it: a path, or `remote:path`. (Shown: a
    /// name that is not UTF-8 cannot be written, and is written lossily.)
    pub fn text(&self) -> String {
        match self {
            Place::Local(p) => shown(p).into_owned(),
            Place::Remote(r, p) => format!("{r}:{p}"),
            Place::Trash => "Trash".into(),
        }
    }

    /// The steps from the top down, each with its own place: for the path along the top.
    pub fn trail(&self) -> Vec<(String, Place)> {
        let mut steps = Vec::new();
        let mut at = Some(self.clone());
        while let Some(place) = at {
            let name = match &place {
                Place::Local(p) if p == "/" => "/".to_string(),
                _ => place.shown_name(),
            };
            at = place.parent();
            steps.push((name, place));
        }
        steps.reverse();
        steps
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Folder,
    Picture,
    Film,
    Music,
    Pdf,
    Model,
    Document,
    Archive,
    Code,
    Other,
}

impl Kind {
    pub fn of(name: &str, folder: bool) -> Kind {
        if folder {
            return Kind::Folder;
        }
        let ending = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        match ending.as_str() {
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "heic" | "heif" | "avif" | "jp2" | "svg" | "mpo" | "jps" | "ico" | "jxl" | "raw" | "dng" | "cr2" | "nef" | "arw" => Kind::Picture,
            "mp4" | "m4v" | "mkv" | "webm" | "mov" | "avi" | "ts" | "m2ts" | "mts" | "mpg" | "mpeg" | "wmv" | "flv" | "ogv" | "3gp" | "vob" | "h264" | "264" | "h265" | "265" | "hevc" | "m2v" | "ivf" | "obu" | "strm" => Kind::Film,
            "mp3" | "flac" | "m4a" | "aac" | "ogg" | "opus" | "wav" | "aiff" | "aif" | "wma" | "alac" => Kind::Music,
            "pdf" => Kind::Pdf,
            "glb" | "gltf" | "vrm" | "obj" | "stl" | "fbx" | "usdz" => Kind::Model,
            "txt" | "md" | "doc" | "docx" | "odt" | "rtf" | "pages" | "xls" | "xlsx" | "ods" | "csv" | "ppt" | "pptx" | "odp" | "key" | "numbers" | "epub" => Kind::Document,
            "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "zst" | "dmg" | "iso" | "apk" | "deb" | "rpm" | "pkg" => Kind::Archive,
            "rs" | "c" | "h" | "cpp" | "hpp" | "py" | "js" | "tsx" | "jsx" | "go" | "java" | "kt" | "swift" | "sh" | "zsh" | "toml" | "json" | "yaml" | "yml" | "xml" | "html" | "css" | "lua" | "rb" | "php" | "sql" => Kind::Code,
            _ => Kind::Other,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Kind::Folder => "Folder",
            Kind::Picture => "Picture",
            Kind::Film => "Film",
            Kind::Music => "Sound",
            Kind::Pdf => "PDF",
            Kind::Model => "3D object",
            Kind::Document => "Document",
            Kind::Archive => "Archive",
            Kind::Code => "Code",
            Kind::Other => "File",
        }
    }

    /// Whether a tile of it can have a picture of what is in it: a picture, a film (its cover or
    /// a frame of it), a song (its cover).
    pub fn pictured(self) -> bool {
        matches!(self, Kind::Picture | Kind::Film | Kind::Music)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub name: String,
    pub kind: Kind,
    pub size: u64,
    /// Seconds since 1970, if known.
    pub modified: Option<i64>,
    pub hidden: bool,
    /// For a thing in the trash: where it is in it, where it came from, and when.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trashed: Option<Trashed>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trashed {
    /// Where it is now, in the trash.
    pub file: String,
    pub origin: Option<String>,
    /// Seconds since 1970.
    pub deleted: Option<i64>,
}

/// Something done to files, which takes as long as it takes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    Copy { from: Vec<Place>, to: Place },
    Move { from: Vec<Place>, to: Place },
    /// To the trash, where there is one.
    Trash(Vec<Place>),
    /// Gone for good (after asking): from the trash, from a remote, or with Shift-Delete.
    /// A thing in the trash goes with what the trash knew of it.
    Delete(Vec<Place>),
    /// Things in the trash put back where they came from.
    Restore(Vec<Place>),
    /// Everything in every trash gone for good.
    EmptyTrash,
    /// A remote's file copied into a folder of this machine's, for an application that opens
    /// files and not addresses.
    Fetch { item: Place, into: String },
    Rename { item: Place, name: String },
    Mkdir { inside: Place, name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToTab {
    Go(Place),
    Refresh,
    /// What is in a folder opened in the list, beside the one gone to: answered with
    /// [`FromTab::Contents`], while the tab goes on with everything else.
    List(Place),
    /// Pictures for these items of the listing, at most `size` pixels a side.
    Pictures { indices: Vec<usize>, size: u32 },
    Do { id: u64, op: Op },
    /// The folders on show -- the one gone to and those opened in the list -- to be watched,
    /// and all others not: answered with [`FromTab::Changed`] whenever one changes.
    Watch(Vec<Place>),
    Quit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FromTab {
    Listed { place: Place, items: Vec<Item>, free: Option<u64> },
    Failed { place: Place, error: String },
    /// What is in a folder asked about with [`ToTab::List`], or why it could not be listed.
    Contents { place: Place, items: Result<Vec<Item>, String> },
    Progress { id: u64, done: u64, total: u64, what: String },
    Done { id: u64, error: Option<String> },
    /// A folder being watched changed, from anywhere: to be listed again.
    Changed(Place),
    /// Sent as a picture frame, not JSON: see [`picture_frame`].
    #[serde(skip)]
    Picture { place: Place, index: usize, width: u32, height: u32, rgba: Vec<u8> },
}

fn frame(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 5);
    out.push(tag);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    out
}

pub fn encode_to_tab(m: &ToTab) -> Vec<u8> {
    frame(b'J', &serde_json::to_vec(m).unwrap_or_default())
}

pub fn encode_from_tab(m: &FromTab) -> Vec<u8> {
    if let FromTab::Picture { place, index, width, height, rgba } = m {
        let place = serde_json::to_vec(place).unwrap_or_default();
        let mut body = Vec::with_capacity(rgba.len() + place.len() + 16);
        body.extend_from_slice(&(place.len() as u32).to_le_bytes());
        body.extend_from_slice(&place);
        for v in [*index as u32, *width, *height] {
            body.extend_from_slice(&v.to_le_bytes());
        }
        body.extend_from_slice(rgba);
        return frame(b'P', &body);
    }
    frame(b'J', &serde_json::to_vec(m).unwrap_or_default())
}

pub fn send(to: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    to.write_all(bytes)?;
    to.flush()
}

pub fn read_to_tab(from: &mut impl Read) -> io::Result<Option<ToTab>> {
    loop {
        let mut head = [0u8; 5];
        match from.read_exact(&mut head) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        }
        let n = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
        let mut body = vec![0u8; n];
        from.read_exact(&mut body)?;
        if let Ok(m) = serde_json::from_slice(&body) {
            return Ok(Some(m));
        }
    }
}

/// A tab's messages, from whatever has arrived of them.
#[derive(Default)]
pub struct Gather {
    pending: Vec<u8>,
}

impl Gather {
    pub fn push(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
    }

    pub fn next(&mut self) -> Option<FromTab> {
        loop {
            if self.pending.len() < 5 {
                return None;
            }
            let n = u32::from_le_bytes([self.pending[1], self.pending[2], self.pending[3], self.pending[4]]) as usize;
            if self.pending.len() < 5 + n {
                return None;
            }
            let tag = self.pending[0];
            let body: Vec<u8> = self.pending[5..5 + n].to_vec();
            self.pending.drain(..5 + n);
            let message = match tag {
                b'J' => serde_json::from_slice(&body).ok(),
                b'P' if body.len() >= 4 => {
                    let pl = u32::from_le_bytes([body[0], body[1], body[2], body[3]]) as usize;
                    let rest = &body[4..];
                    if rest.len() < pl + 12 {
                        None
                    } else {
                        let place: Option<Place> = serde_json::from_slice(&rest[..pl]).ok();
                        let r = &rest[pl..];
                        let num = |i: usize| u32::from_le_bytes([r[i], r[i + 1], r[i + 2], r[i + 3]]);
                        place.map(|place| FromTab::Picture { place, index: num(0) as usize, width: num(4), height: num(8), rgba: r[12..].to_vec() })
                    }
                }
                _ => None,
            };
            if message.is_some() {
                return message;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_not_utf8_are_held_as_text_and_come_back_as_their_bytes() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let os = |b: &[u8]| std::ffi::OsStr::from_bytes(b).to_os_string();
        for bytes in [&b"caf\xe9.txt"[..], b"\xff\xfe", b"/home/me/caf\xc3\xa9/\x80", "a\u{10FFFF}b\u{10FF80}".as_bytes(), b"plain", b""] {
            let text = text_of(&os(bytes));
            assert_eq!(os_of(&text).into_vec(), bytes, "{bytes:?} back from {text:?}");
        }
        // UTF-8 is its own text, and shown as it is.
        assert_eq!(text_of(&os("café/ü.txt".as_bytes())), "café/ü.txt");
        assert_eq!(shown("café"), "café");
        // What is not is shown lossily, and two names that differ only there stay two.
        assert_eq!(shown(&text_of(&os(b"caf\xe9.txt"))), "caf\u{FFFD}.txt");
        assert_ne!(text_of(&os(b"\xff\xfe")), text_of(&os(b"\xfe\xff")));
        assert_ne!(text_of(&os("\u{10FF80}".as_bytes())), text_of(&os(b"\x80")), "a real character of the held range is not a byte");
        // A place of such a name: joined, named, its parent, its path.
        let dir = Place::Local("/tmp".into());
        let file = dir.join(&text_of(&os(b"\xff\xfe")));
        assert_eq!(file.path().unwrap().as_os_str().as_bytes(), b"/tmp/\xff\xfe");
        assert_eq!(file.parent(), Some(dir));
        assert_eq!(file.shown_name(), "\u{FFFD}\u{FFFD}");
        assert_eq!(file.text(), "/tmp/\u{FFFD}\u{FFFD}");
    }

    #[test]
    fn places_go_up_and_down() {
        let p = Place::Remote("Drive".into(), "Photos/2024".into());
        assert_eq!(p.parent(), Some(Place::Remote("Drive".into(), "Photos".into())));
        assert_eq!(p.parent().unwrap().parent(), Some(Place::Remote("Drive".into(), String::new())));
        assert_eq!(Place::Remote("Drive".into(), String::new()).parent(), None);
        assert_eq!(Place::Remote("Drive".into(), String::new()).join("a"), Place::Remote("Drive".into(), "a".into()));
        assert_eq!(p.text(), "Drive:Photos/2024");
        assert!(Place::Remote("Drive".into(), String::new()).holds(&p) && Place::Remote("Drive".into(), "Photos".into()).holds(&p));
        assert!(!p.holds(&p) && !Place::Remote("Other".into(), String::new()).holds(&p));
        assert!(Place::Local("/a".into()).holds(&Place::Local("/a/b/c".into())) && !Place::Local("/a".into()).holds(&Place::Local("/ab".into())));
        let trail: Vec<String> = Place::Local("/Users/me/Pictures".into()).trail().into_iter().map(|(n, _)| n).collect();
        assert_eq!(trail, ["/", "Users", "me", "Pictures"]);
    }

    #[test]
    fn messages_and_pictures_come_through_in_pieces() {
        let a = FromTab::Listed { place: Place::Local("/".into()), items: vec![Item { name: "a.jpg".into(), kind: Kind::Picture, size: 3, modified: Some(1), hidden: false, trashed: None }], free: Some(9) };
        let b = FromTab::Picture { place: Place::Local("/".into()), index: 2, width: 1, height: 1, rgba: vec![1, 2, 3, 4] };
        let bytes = [encode_from_tab(&a), encode_from_tab(&b)].concat();
        let mut g = Gather::default();
        for piece in bytes.chunks(5) {
            g.push(piece);
        }
        assert_eq!(g.next(), Some(a));
        assert_eq!(g.next(), Some(b));
        assert_eq!(g.next(), None);
    }

    #[test]
    fn kinds_are_known_by_their_endings() {
        assert_eq!(Kind::of("IMG_1.HEIC", false), Kind::Picture);
        assert_eq!(Kind::of("film.mkv", false), Kind::Film);
        assert_eq!(Kind::of("x", true), Kind::Folder);
        assert_eq!(Kind::of("README", false), Kind::Other);
    }
}
