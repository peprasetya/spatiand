//! Jellyfin and Emby, which are one server twice: Jellyfin began as Emby, and what a player
//! needs of either -- sign in, list, picture, stream, say how far -- is still the same calls.
//!
//! The film is played as it is on the server's disk (`static=true`): nothing is transcoded,
//! so a 3D film's frame arrives with both its eyes and a sphere arrives whole.

use std::time::Duration;

use serde_json::{json, Value};

use super::{Item, Picture, Play, Source, Watching};
use spatiand_media::{Packing, Projection, StereoLayout, VideoLayout};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Flavour {
    Jellyfin,
    Emby,
}

impl Flavour {
    pub fn label(self) -> &'static str {
        match self {
            Flavour::Jellyfin => "Jellyfin",
            Flavour::Emby => "Emby",
        }
    }
}

/// What is kept of a server between sessions. A token, never the password.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Account {
    pub flavour: Flavour,
    /// With its scheme and no slash at its end.
    pub address: String,
    pub user: String,
    pub user_id: String,
    pub token: String,
    /// This device's name for itself to the server, made once.
    pub device: String,
}

pub struct Server {
    account: Account,
    agent: ureq::Agent,
}

/// The key of the folder that is not a folder on the server: what is part watched.
const RESUME: &str = "\u{1}resume";

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(8)).timeout_read(Duration::from_secs(25)).build()
}

/// An address as typed, made whole: a scheme if it has none, the usual port if it has none
/// and is not a name that plainly sits behind a proxy.
pub fn address(typed: &str) -> String {
    let typed = typed.trim().trim_end_matches('/');
    let with_scheme = if typed.contains("://") { typed.to_string() } else { format!("http://{typed}") };
    let after = with_scheme.split_once("://").map_or("", |(_, rest)| rest);
    let host = after.split('/').next().unwrap_or("");
    let has_port = host.rsplit_once(':').is_some_and(|(_, port)| port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty());
    if has_port || with_scheme.starts_with("https://") || after.contains('/') {
        with_scheme
    } else {
        format!("{with_scheme}:8096")
    }
}

fn identity(device: &str, token: Option<&str>) -> String {
    let host = std::env::var("HOSTNAME").ok().filter(|h| !h.is_empty()).unwrap_or_else(|| "Spatiand".into());
    let mut said = format!(
        "MediaBrowser Client=\"SpatiReel\", Device=\"{}\", DeviceId=\"{device}\", Version=\"{}\"",
        host.replace('"', ""),
        env!("CARGO_PKG_VERSION")
    );
    if let Some(token) = token {
        said.push_str(&format!(", Token=\"{token}\""));
    }
    said
}

fn prefix(flavour: Flavour) -> &'static str {
    match flavour {
        Flavour::Jellyfin => "",
        Flavour::Emby => "/emby",
    }
}

fn failure(what: &str, error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(401, _) => "the server did not accept that name and password".to_string(),
        ureq::Error::Status(code, _) => format!("{what}: the server answered {code}"),
        ureq::Error::Transport(e) => format!("{what}: {e}"),
    }
}

/// Sign in, and come back with what is kept.
pub fn sign_in(flavour: Flavour, typed_address: &str, user: &str, password: &str, device: &str) -> Result<Account, String> {
    let address = address(typed_address);
    let url = format!("{address}{}/Users/AuthenticateByName", prefix(flavour));
    let said = identity(device, None);
    let reply: Value = agent()
        .post(&url)
        // Both names for the same header: Emby reads the second, Jellyfin either.
        .set("Authorization", &said)
        .set("X-Emby-Authorization", &said)
        .send_json(json!({ "Username": user, "Pw": password }))
        .map_err(|e| failure("could not sign in", e))?
        .into_json()
        .map_err(|e| format!("the server's answer could not be read: {e}"))?;
    let token = reply["AccessToken"].as_str().ok_or("the server gave no token")?.to_string();
    let user_id = reply["User"]["Id"].as_str().ok_or("the server did not say who was signed in")?.to_string();
    let name = reply["User"]["Name"].as_str().unwrap_or(user).to_string();
    Ok(Account { flavour, address, user: name, user_id, token, device: device.to_string() })
}

/// What the server's own word for a 3D film's packing means.
pub fn layout_of(format: &str) -> Option<VideoLayout> {
    let (stereo, packing) = match format {
        "HalfSideBySide" => (StereoLayout::SideBySide, Packing::Half),
        "FullSideBySide" => (StereoLayout::SideBySide, Packing::Full),
        "HalfTopAndBottom" => (StereoLayout::OverUnder, Packing::Half),
        "FullTopAndBottom" => (StereoLayout::OverUnder, Packing::Full),
        _ => return None,
    };
    Some(VideoLayout { projection: Projection::Flat, stereo, packing })
}

const TICKS: f64 = 10_000_000.0;

/// One of the server's items as one of ours.
pub fn item(value: &Value) -> Option<Item> {
    let key = value["Id"].as_str()?.to_string();
    let kind = value["Type"].as_str().unwrap_or("");
    let folder = value["IsFolder"].as_bool().unwrap_or(false) || matches!(kind, "Series" | "Season" | "BoxSet" | "Folder" | "CollectionFolder" | "UserView");
    let name = value["Name"].as_str().unwrap_or("").to_string();
    let title = match (kind, value["IndexNumber"].as_i64()) {
        ("Episode", Some(number)) => format!("{number}. {name}"),
        _ => name.clone(),
    };
    let detail = match kind {
        "Episode" => value["SeriesName"].as_str().map(|series| match value["ParentIndexNumber"].as_i64() {
            Some(season) => format!("{series} \u{b7} Season {season}"),
            None => series.to_string(),
        }),
        _ => value["ProductionYear"].as_i64().map(|year| year.to_string()),
    };
    let ticks = value["RunTimeTicks"].as_f64().unwrap_or(0.0);
    let at = value["UserData"]["PlaybackPositionTicks"].as_f64().unwrap_or(0.0);
    let played = value["UserData"]["Played"].as_bool().unwrap_or(false);
    let progress = if !folder && played && at == 0.0 {
        Some(1.0)
    } else if ticks > 0.0 && at > 0.0 {
        Some((at / ticks) as f32)
    } else {
        None
    };
    // The file's own name says more about a film's layout than its title does.
    let file = value["Path"].as_str().and_then(|path| path.rsplit(['/', '\\']).next()).map(str::to_string);
    Some(Item {
        key,
        title,
        detail,
        folder,
        name: file.unwrap_or(name),
        aspect: value["PrimaryImageAspectRatio"].as_f64().map(|a| a as f32),
        progress,
        resume: at / TICKS,
        layout: value["Video3DFormat"].as_str().and_then(layout_of),
        pictured: value["ImageTags"]["Primary"].is_string(),
        modified: 0,
    })
}

impl Server {
    pub fn new(account: Account) -> Server {
        Server { account, agent: agent() }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}{path}", self.account.address, prefix(self.account.flavour))
    }

    fn request(&self, method: &str, path: &str) -> ureq::Request {
        let said = identity(&self.account.device, Some(&self.account.token));
        self.agent
            .request(method, &self.url(path))
            .set("Authorization", &said)
            .set("X-Emby-Authorization", &said)
            .set("X-Emby-Token", &self.account.token)
    }

    fn items(&self, path: &str) -> Result<Vec<Item>, String> {
        let reply: Value = self
            .request("GET", path)
            .call()
            .map_err(|e| failure("could not read the library", e))?
            .into_json()
            .map_err(|e| format!("the server's answer could not be read: {e}"))?;
        Ok(reply["Items"].as_array().map(|items| items.iter().filter_map(item).collect()).unwrap_or_default())
    }

    fn told(&self, path: &str, body: Value) {
        if let Err(e) = self.request("POST", path).send_json(body) {
            log::warn!("{}: the place in a film was not kept: {}", self.label(), failure(path, e));
        }
    }
}

const FIELDS: &str = "Fields=PrimaryImageAspectRatio,Path,Video3DFormat&EnableImageTypes=Primary&ImageTypeLimit=1";

impl Source for Server {
    fn label(&self) -> String {
        let host = self.account.address.split_once("://").map_or(self.account.address.as_str(), |(_, rest)| rest);
        format!("{} \u{b7} {}", self.account.flavour.label(), host.split([':', '/']).next().unwrap_or(host))
    }

    fn list(&self, folder: Option<&Item>) -> Result<Vec<Item>, String> {
        let user = &self.account.user_id;
        match folder {
            None => {
                let mut top = Vec::new();
                // What is part watched, first, if anything is: the commonest thing to want.
                let resumable = self.items(&format!("/Users/{user}/Items/Resume?MediaTypes=Video&Limit=1&{FIELDS}")).unwrap_or_default();
                if !resumable.is_empty() {
                    top.push(Item { key: RESUME.into(), title: "Continue watching".into(), folder: true, ..Item::default() });
                }
                top.extend(self.items(&format!("/Users/{user}/Views"))?);
                Ok(top)
            }
            Some(item) if item.key == RESUME => self.items(&format!("/Users/{user}/Items/Resume?MediaTypes=Video&Limit=40&{FIELDS}")),
            Some(item) => self.items(&format!("/Users/{user}/Items?ParentId={}&SortBy=IsFolder,SortName&SortOrder=Ascending&{FIELDS}", item.key)),
        }
    }

    fn unfinished(&self) -> Vec<Item> {
        self.items(&format!("/Users/{}/Items/Resume?MediaTypes=Video&Limit=20&{FIELDS}", self.account.user_id)).unwrap_or_default()
    }

    fn picture(&self, item: &Item) -> Option<Picture> {
        let reply = self.request("GET", &format!("/Items/{}/Images/Primary?maxWidth=480&quality=85", item.key)).call().ok()?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(reply.into_reader(), 8 * 1024 * 1024), &mut bytes).ok()?;
        Some(Picture::Encoded(bytes))
    }

    fn play(&self, item: &Item) -> Result<Play, String> {
        // The file as it is, with the token in the address: FFmpeg asks for ranges of it, and
        // an address is all it is given.
        Ok(Play {
            url: self.url(&format!("/Videos/{}/stream?static=true&MediaSourceId={}&api_key={}", item.key, item.key, self.account.token)),
            options: Vec::new(),
        })
    }

    fn watching(&self, item: &Item, what: Watching) {
        let ticks = |seconds: f64| (seconds.max(0.0) * TICKS) as i64;
        match what {
            Watching::Started => self.told(
                "/Sessions/Playing",
                json!({ "ItemId": item.key, "MediaSourceId": item.key, "PlayMethod": "DirectPlay", "CanSeek": true, "PositionTicks": ticks(item.resume) }),
            ),
            Watching::At { seconds, paused } => self.told(
                "/Sessions/Playing/Progress",
                json!({ "ItemId": item.key, "MediaSourceId": item.key, "PlayMethod": "DirectPlay", "CanSeek": true, "IsPaused": paused, "PositionTicks": ticks(seconds) }),
            ),
            Watching::Stopped { seconds, .. } => {
                self.told("/Sessions/Playing/Stopped", json!({ "ItemId": item.key, "MediaSourceId": item.key, "PositionTicks": ticks(seconds) }))
            }
            // The server marks a film watched when it is stopped near enough its end, and
            // is told it is exactly there.
            Watching::Finished => self.told("/Sessions/Playing/Stopped", json!({ "ItemId": item.key, "MediaSourceId": item.key, "PositionTicks": i64::MAX / 2 })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_made_whole_without_being_second_guessed() {
        assert_eq!(address("deepmagpie"), "http://deepmagpie:8096");
        assert_eq!(address("192.168.1.9:8920/"), "http://192.168.1.9:8920");
        // Behind a proxy, on the port the scheme says.
        assert_eq!(address("https://films.example.org"), "https://films.example.org");
        assert_eq!(address("example.org/jellyfin"), "http://example.org/jellyfin");
    }

    #[test]
    fn an_episode_is_numbered_and_says_what_it_is_of() {
        let episode = item(&json!({
            "Id": "e1", "Name": "Pilot", "Type": "Episode", "IndexNumber": 1, "ParentIndexNumber": 2, "SeriesName": "The Show",
            "RunTimeTicks": 6000000000i64, "UserData": { "PlaybackPositionTicks": 1500000000i64 },
            "Path": "/media/The Show/S02E01.HSBS.mkv", "ImageTags": { "Primary": "abc" }, "PrimaryImageAspectRatio": 1.7777
        }))
        .unwrap();
        assert_eq!(episode.title, "1. Pilot");
        assert_eq!(episode.detail.as_deref(), Some("The Show \u{b7} Season 2"));
        assert!(!episode.folder && episode.pictured);
        assert_eq!(episode.resume, 150.0);
        assert_eq!(episode.progress, Some(0.25));
        assert_eq!(episode.name, "S02E01.HSBS.mkv");
    }

    #[test]
    fn a_series_is_a_folder_and_a_watched_film_is_full() {
        let series = item(&json!({ "Id": "s", "Name": "The Show", "Type": "Series", "ProductionYear": 2019 })).unwrap();
        assert!(series.folder);
        assert_eq!(series.detail.as_deref(), Some("2019"));
        let film = item(&json!({ "Id": "m", "Name": "Film", "Type": "Movie", "RunTimeTicks": 10, "UserData": { "Played": true, "PlaybackPositionTicks": 0 } })).unwrap();
        assert_eq!(film.progress, Some(1.0));
    }

    #[test]
    fn the_servers_word_for_a_packing_is_believed() {
        let half = layout_of("HalfSideBySide").unwrap();
        assert_eq!((half.stereo, half.packing), (StereoLayout::SideBySide, Packing::Half));
        assert_eq!(layout_of("FullTopAndBottom").unwrap().stereo, StereoLayout::OverUnder);
        assert!(layout_of("MVC").is_none());
    }
}
