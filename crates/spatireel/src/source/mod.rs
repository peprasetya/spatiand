//! Where films come from.
//!
//! This machine's folders, a Vibrefy, Jellyfin or Emby server, a DLNA server on the network:
//! each is a [`Source`], and the library and the player know nothing else about any of them.
//! A source is asked for a folder's contents, for a picture of a thing, for where a film is
//! to be played from, and is told how far a film has been watched -- and each answers in its
//! own way, over its own connection. None of it goes by a paired spatiand-host.
//!
//! Every call here may take as long as a network takes, and is made from a thread that is not
//! the one drawing (see `crate::work`).

pub mod dlna;
pub mod jellyfin;
pub mod local;
pub mod vibrefy;
#[cfg(test)]
mod test_server;

use std::time::Duration;

use spatiand_media::still::Still;
use spatiand_media::VideoLayout;

/// The connection a server source asks over: given up on when the server does not take it in
/// `connect`, or goes `read` without sending anything (each read of the socket, never the whole
/// answer).
///
/// Set out in full, because ureq 3's defaults are not ureq 2's: no proxy from the environment
/// (ureq 2 never read one), and four redirects followed and an error at the fifth, which is what
/// ureq 2's default `redirects(5)` did (it counted the answer that is not a redirect among the
/// five); ureq 3 left to itself follows ten.
fn agent(connect: Duration, read: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(connect))
        .timeout_per_read(Some(read))
        .max_redirects(4)
        .proxy(None)
        .build()
        .into()
}

/// The most of a server's JSON answer that is read. ureq 2 read JSON with no limit; ureq 3
/// stops at 10 MB unless told. A big library's listing is a few megabytes, so this is far past
/// anything real while still not letting a broken server fill memory.
const JSON_LIMIT: u64 = 256 * 1024 * 1024;

/// The most of a text answer (DLNA's XML) that is read: 10 MB, as ureq 2's `into_string` and
/// ureq 3's `read_to_string` both stop at.
const TEXT_LIMIT: u64 = 10 * 1024 * 1024;

/// ureq's error in words fit to show: a socket's own error without the "io: " ureq 3 puts in
/// front of it.
fn words(error: &ureq::Error) -> String {
    match error {
        ureq::Error::Io(e) => e.to_string(),
        ureq::Error::Timeout(_) => format!("no answer in time ({error})"),
        other => other.to_string(),
    }
}

/// One thing in a source: a folder of things, or a film.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Item {
    /// What the source knows it by. Nothing else reads it.
    pub key: String,
    pub title: String,
    /// A second, dimmer line: a year, an episode's number.
    pub detail: Option<String>,
    pub folder: bool,
    /// The file's own name where the source knows it, for what a name says about a film's
    /// layout (`hints`); the title otherwise.
    pub name: String,
    /// Its picture's width over its height, where the source says: what decides whether a
    /// folder is shown as posters or as frames.
    pub aspect: Option<f32>,
    /// How far through it the wearer is, 0 to 1.
    pub progress: Option<f32>,
    /// Where to take it up again, in seconds.
    pub resume: f64,
    /// What the source itself says the film's layout is, which outranks its name.
    pub layout: Option<VideoLayout>,
    /// Whether there is a picture to ask for at all.
    pub pictured: bool,
    /// When it last changed, in any one unit, where the source says: what "last updated" orders by.
    pub modified: i64,
}

/// Numbers in no particular order, which is all a shuffle wants: no dependency for it.
pub struct Dice(u64);

impl Dice {
    pub fn new() -> Dice {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
        Dice(seed | 1)
    }

    pub fn below(&mut self, limit: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % limit.max(1) as u64) as usize
    }

    pub fn shuffle<T>(&mut self, things: &mut [T]) {
        for i in (1..things.len()).rev() {
            things.swap(i, self.below(i + 1));
        }
    }
}

/// A picture of an item, as the source has it.
pub enum Picture {
    /// A JPEG, PNG or WebP, as it came over the wire.
    Encoded(Vec<u8>),
    /// Pixels already.
    Pixels(Still),
}

/// Where a film is to be played from.
#[derive(Debug, Clone, Default)]
pub struct Play {
    pub url: String,
    /// What FFmpeg is to be told about fetching it.
    pub options: Vec<(String, String)>,
}

/// What is happening to a film, for a source that keeps the wearer's place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Watching {
    Started,
    /// Still going, or paused, at this many seconds.
    At { seconds: f64, paused: bool },
    /// Left, at this many seconds of so many.
    Stopped { seconds: f64, duration: f64 },
    /// Played to its end.
    Finished,
}

pub trait Source: Send + Sync {
    /// What the wearer calls it.
    fn label(&self) -> String;
    /// What is in `folder`, or at the top.
    fn list(&self, folder: Option<&Item>) -> Result<Vec<Item>, String>;
    fn picture(&self, item: &Item) -> Option<Picture>;
    fn play(&self, item: &Item) -> Result<Play, String>;
    /// Nothing, for a source that keeps no place: DLNA.
    fn watching(&self, _item: &Item, _what: Watching) {}
    /// The films begun and not finished, most lately watched first: what the home screen
    /// offers to go on with.
    fn unfinished(&self) -> Vec<Item> {
        Vec::new()
    }
    /// Whether the server has said this sign-in is over, so the source is no use until it
    /// is signed in to again.
    fn expired(&self) -> bool {
        false
    }
}

/// A picture as pixels, no wider than a tile needs.
pub fn pixels(picture: Picture) -> Option<Still> {
    match picture {
        Picture::Pixels(still) => Some(still),
        Picture::Encoded(bytes) => {
            let decoded = image::load_from_memory(&bytes).ok()?;
            let small = if decoded.width() > 520 { decoded.thumbnail(520, 1040) } else { decoded };
            let rgba = small.to_rgba8();
            Some(Still { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() })
        }
    }
}
