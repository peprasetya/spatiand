//! What the window and a tab say to each other over the tab's standard input and output: a
//! byte for what it is, four for how long, and that much -- JSON, or a picture's pixels.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// A place: a folder on this machine, or one on a remote (`remote:path`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Place {
    Local(String),
    Remote(String, String),
}

impl Place {
    pub fn join(&self, name: &str) -> Place {
        match self {
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
        }
    }

    pub fn name(&self) -> String {
        match self {
            Place::Local(p) => std::path::Path::new(p).file_name().map_or_else(|| p.clone(), |n| n.to_string_lossy().into_owned()),
            Place::Remote(r, p) if p.is_empty() => r.clone(),
            Place::Remote(_, p) => p.rsplit('/').next().unwrap_or(p).to_string(),
        }
    }

    /// As a person or a Spatiand application writes it: a path, or `remote:path`.
    pub fn text(&self) -> String {
        match self {
            Place::Local(p) => p.clone(),
            Place::Remote(r, p) => format!("{r}:{p}"),
        }
    }

    /// The steps from the top down, each with its own place: for the path along the top.
    pub fn trail(&self) -> Vec<(String, Place)> {
        let mut steps = Vec::new();
        let mut at = Some(self.clone());
        while let Some(place) = at {
            let name = match &place {
                Place::Local(p) if p == "/" => "/".to_string(),
                _ => place.name(),
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
            "mp4" | "m4v" | "mkv" | "webm" | "mov" | "avi" | "ts" | "m2ts" | "mts" | "mpg" | "mpeg" | "wmv" | "flv" | "ogv" | "3gp" | "vob" | "strm" => Kind::Film,
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

    /// Whether a tile of it can have a picture of what is in it.
    pub fn pictured(self) -> bool {
        matches!(self, Kind::Picture | Kind::Film)
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
}

/// Something done to files, which takes as long as it takes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    Copy { from: Vec<Place>, to: Place },
    Move { from: Vec<Place>, to: Place },
    /// To the trash, where there is one.
    Trash(Vec<Place>),
    /// Gone for good: what a remote, or a phone with no trash, offers.
    Delete(Vec<Place>),
    Rename { item: Place, name: String },
    Mkdir { inside: Place, name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToTab {
    Go(Place),
    Refresh,
    /// Pictures for these items of the listing, at most `size` pixels a side.
    Pictures { indices: Vec<usize>, size: u32 },
    Do { id: u64, op: Op },
    Quit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FromTab {
    Listed { place: Place, items: Vec<Item>, free: Option<u64> },
    Failed { place: Place, error: String },
    Progress { id: u64, done: u64, total: u64, what: String },
    Done { id: u64, error: Option<String> },
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
    fn places_go_up_and_down() {
        let p = Place::Remote("Drive".into(), "Photos/2024".into());
        assert_eq!(p.parent(), Some(Place::Remote("Drive".into(), "Photos".into())));
        assert_eq!(p.parent().unwrap().parent(), Some(Place::Remote("Drive".into(), String::new())));
        assert_eq!(Place::Remote("Drive".into(), String::new()).parent(), None);
        assert_eq!(Place::Remote("Drive".into(), String::new()).join("a"), Place::Remote("Drive".into(), "a".into()));
        assert_eq!(p.text(), "Drive:Photos/2024");
        let trail: Vec<String> = Place::Local("/Users/me/Pictures".into()).trail().into_iter().map(|(n, _)| n).collect();
        assert_eq!(trail, ["/", "Users", "me", "Pictures"]);
    }

    #[test]
    fn messages_and_pictures_come_through_in_pieces() {
        let a = FromTab::Listed { place: Place::Local("/".into()), items: vec![Item { name: "a.jpg".into(), kind: Kind::Picture, size: 3, modified: Some(1), hidden: false }], free: Some(9) };
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
