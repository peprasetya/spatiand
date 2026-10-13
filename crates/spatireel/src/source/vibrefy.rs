//! A Vibrefy server, through the client the Vibrefy port already had (`vibrefy-api`).
//!
//! Vibrefy says nothing about a film but its name and where the wearer left it, so a film's
//! layout is read from its name, as the port read it. Signing in is by pairing: this device
//! shows eight digits, and a device already signed in approves them.

use std::collections::BTreeMap;

use vibrefy_api::client::VibrefyClient;
use vibrefy_api::models::MediaItem;

use super::{Item, Picture, Play, Source, Watching};

/// What is kept of a server between sessions: its address and the session's cookies.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Account {
    pub address: String,
    pub account: Option<String>,
    pub cookies: BTreeMap<String, String>,
}

pub struct Server {
    client: VibrefyClient,
    account: Account,
}

const WATCHING: &str = "\u{1}watching";
const RANDOM: &str = "\u{1}random";

/// Numbers in no particular order, which is all a shuffle wants: no dependency for it.
struct Dice(u64);

impl Dice {
    fn new() -> Dice {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
        Dice(seed | 1)
    }

    fn below(&mut self, limit: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % limit.max(1) as u64) as usize
    }

    fn shuffle<T>(&mut self, things: &mut [T]) {
        for i in (1..things.len()).rev() {
            things.swap(i, self.below(i + 1));
        }
    }
}

fn item(media: &MediaItem) -> Item {
    let folder = media.kind.is_browsable();
    Item {
        key: media.path.clone(),
        title: if folder { media.name.clone() } else { crate::library::title(&media.name) },
        detail: if folder { None } else { crate::library::told_by_name(&media.name) },
        folder,
        name: media.name.clone(),
        aspect: (!folder).then_some(16.0 / 9.0),
        // The server keeps where a film was left and not how long it is.
        progress: (media.resume_seconds > 0).then_some(0.5),
        resume: media.resume_seconds.max(0) as f64,
        layout: None,
        pictured: media.has_artwork(),
    }
}

impl Server {
    pub fn new(account: Account) -> Server {
        let client = VibrefyClient::new(&account.address);
        client.restore_cookies(account.cookies.clone());
        Server { client, account }
    }

    /// Films from here and there in the library, in no order: the server keeps no such list
    /// (its web page shuffles one folder), so this wanders -- a folder chosen at random, and
    /// from it another, for a dozen folders or so -- and shuffles what it passed.
    fn random(&self) -> Result<Vec<Item>, String> {
        let mut dice = Dice::new();
        let mut films: Vec<Item> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let top: Vec<MediaItem> = self.client.browse("").map_err(|e| e.to_string())?.items().cloned().collect();
        let mut folders: Vec<MediaItem> = top.iter().filter(|m| m.kind.is_browsable()).cloned().collect();
        films.extend(top.iter().filter(|m| m.is_video()).map(item));
        for _ in 0..12 {
            if folders.is_empty() || films.len() >= 60 {
                break;
            }
            let folder = folders.swap_remove(dice.below(folders.len()));
            if !seen.insert(folder.path.clone()) {
                continue;
            }
            // A folder that will not open is one fewer to choose from, not the end of it.
            let Ok(listing) = self.client.browse(&folder.path) else { continue };
            for media in listing.items() {
                if media.kind.is_browsable() {
                    folders.push(media.clone());
                } else if media.is_video() {
                    films.push(item(media));
                }
            }
        }
        let mut unique = std::collections::HashSet::new();
        films.retain(|film| unique.insert(film.key.clone()));
        dice.shuffle(&mut films);
        films.truncate(24);
        Ok(films)
    }

    /// A client not yet signed in, to pair with.
    pub fn client(address: &str) -> VibrefyClient {
        VibrefyClient::new(address)
    }

    /// What to keep of a client that has just been paired.
    pub fn account_of(client: &VibrefyClient, account: Option<String>) -> Account {
        Account { address: client.base_url().to_string(), account, cookies: client.cookies() }
    }
}

impl Source for Server {
    fn label(&self) -> String {
        let address = self.account.address.trim_end_matches('/');
        let host = address.split_once("://").map_or(address, |(_, rest)| rest);
        format!("Vibrefy \u{b7} {}", host.split([':', '/']).next().unwrap_or(host))
    }

    fn list(&self, folder: Option<&Item>) -> Result<Vec<Item>, String> {
        match folder {
            Some(folder) if folder.key == RANDOM => self.random(),
            Some(folder) if folder.key == WATCHING => {
                // The stream addresses need the session's number, which only a listing gives.
                self.client.browse("").map_err(|e| e.to_string())?;
                Ok(self.client.watching().map_err(|e| e.to_string())?.iter().filter(|m| m.is_video()).map(item).collect())
            }
            _ => {
                let listing = self.client.browse(folder.map_or("", |f| f.key.as_str())).map_err(|e| e.to_string())?;
                let mut seen = std::collections::HashSet::new();
                let mut items: Vec<Item> = Vec::new();
                if folder.is_none() && self.client.watching().is_ok_and(|w| !w.is_empty()) {
                    items.push(Item { key: WATCHING.into(), title: "Continue watching".into(), folder: true, ..Item::default() });
                }
                if folder.is_none() {
                    items.push(Item { key: RANDOM.into(), title: "Something at random".into(), folder: true, ..Item::default() });
                }
                // A thing may be on more than one of the server's shelves; here it is once.
                items.extend(listing.items().filter(|m| m.kind.is_browsable() || m.is_video()).filter(|m| seen.insert(m.path.clone())).map(item));
                Ok(items)
            }
        }
    }

    fn unfinished(&self) -> Vec<Item> {
        if self.client.session_id().is_none() && self.client.browse("").is_err() {
            return Vec::new();
        }
        self.client.watching().map(|films| films.iter().filter(|m| m.is_video()).map(item).collect()).unwrap_or_default()
    }

    fn picture(&self, item: &Item) -> Option<Picture> {
        let bytes = self.client.image(&self.client.thumb_url(&item.key)).ok().flatten()?;
        Some(Picture::Encoded(bytes))
    }

    fn play(&self, item: &Item) -> Result<Play, String> {
        if self.client.session_id().is_none() {
            self.client.browse("").map_err(|e| e.to_string())?;
        }
        Ok(Play { url: self.client.stream_url(&item.key).map_err(|e| e.to_string())?, options: Vec::new() })
    }

    fn watching(&self, item: &Item, what: Watching) {
        let key = format!("files/{}", item.key);
        let seconds = match what {
            Watching::Started => return,
            Watching::At { seconds, .. } | Watching::Stopped { seconds, .. } => seconds as i64,
            Watching::Finished => vibrefy_api::progress::FINISHED,
        };
        if let Err(e) = self.client.save_progress(&key, seconds) {
            log::warn!("{}: {e}", self.label());
        }
    }
}
