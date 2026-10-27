//! Downloads: what the engines were asked for (a link saved, an image, an extension from its
//! store) and what pages began themselves, with their progress, kept on this machine
//! (`local/spatimesh/downloads.json`, never the private profile's) for the Downloads page.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use spatimesh::engine::{Download, TabId, ENGINE_DOWNLOADS};
use spatimesh::profiles::PRIVATE;
use spatimesh::store::{now_ms, Store};
use spatimesh::wire::ToHost;

use super::{Browser, Own};

const FILE: &str = "downloads.json";
/// How many are kept in the list.
const KEPT: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) enum State {
    Going,
    Done,
    Failed(String),
}

/// What a download is for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub(super) enum Purpose {
    /// The wearer's: kept in their Downloads.
    #[default]
    Keep,
    /// An extension's package from its store: installed in the profile when it has come.
    Install,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Item {
    /// The window's number for it.
    pub key: u64,
    /// The engine's, which are numbered per profile.
    #[serde(skip)]
    pub engine_id: u64,
    pub profile: String,
    #[serde(skip)]
    pub tab: TabId,
    pub url: String,
    pub file: String,
    pub received: u64,
    pub total: u64,
    pub state: State,
    pub started: u64,
    #[serde(default)]
    pub purpose: Purpose,
}

impl Item {
    pub fn name(&self) -> String {
        Path::new(&self.file)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| self.url.rsplit('/').next().unwrap_or(&self.url).to_string())
    }

    /// How it is, in a few words: "3.1 MB of 10 MB", "Done, 10 MB", "Failed: ...".
    pub fn status(&self) -> String {
        match &self.state {
            State::Going if self.total > 0 => format!("{} of {}", size(self.received), size(self.total)),
            State::Going => format!("{} so far", size(self.received)),
            State::Done if !Path::new(&self.file).exists() => "Done, moved or deleted since".into(),
            State::Done => format!("Done, {}", size(self.received.max(self.total))),
            State::Failed(why) => format!("Failed: {why}"),
        }
    }

    /// From 0 to 1, while it is going and its size is known.
    pub fn progress(&self) -> Option<f32> {
        (self.state == State::Going && self.total > 0).then(|| (self.received as f32 / self.total as f32).clamp(0.0, 1.0))
    }
}

/// A size as a person reads it.
pub(super) fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{} kB", b >> 10),
        b => format!("{b} bytes"),
    }
}

#[derive(Default)]
pub(super) struct Downloads {
    pub items: Vec<Item>,
    next: u64,
}

impl Downloads {
    pub fn load(&mut self, store: &Store) {
        self.items = store.read_local(FILE).unwrap_or_default();
        // Any left going when SpatiMesh last stopped did not finish.
        for item in &mut self.items {
            if item.state == State::Going {
                item.state = State::Failed("SpatiMesh stopped before it finished".into());
            }
        }
        self.next = self.items.iter().map(|i| i.key).max().unwrap_or(0) + 1;
    }

    fn save(&self, store: &Store) {
        let kept: Vec<&Item> = self.items.iter().filter(|i| i.profile != PRIVATE && i.purpose == Purpose::Keep).collect();
        if let Err(e) = store.write_local(FILE, &kept) {
            log::warn!("the list of downloads not kept: {e}");
        }
    }

    pub fn going(&self) -> usize {
        self.items.iter().filter(|i| i.state == State::Going).count()
    }

    /// All of them going, as one fraction, if any is.
    pub fn progress(&self) -> Option<f32> {
        let going: Vec<&Item> = self.items.iter().filter(|i| i.state == State::Going).collect();
        if going.is_empty() {
            return None;
        }
        let (got, of) = going.iter().fold((0u64, 0u64), |(g, o), i| (g + i.received, o + i.total.max(i.received)));
        Some(if of == 0 { 0.0 } else { got as f32 / of as f32 })
    }

    pub fn get(&self, key: u64) -> Option<&Item> {
        self.items.iter().find(|i| i.key == key)
    }

    /// A profile's, newest first.
    pub fn of(&self, profile: &str) -> Vec<&Item> {
        let mut found: Vec<&Item> = self.items.iter().filter(|i| i.profile == profile && i.purpose == Purpose::Keep).collect();
        found.sort_by(|a, b| b.started.cmp(&a.started));
        found
    }
}

/// Open a file or folder with what the system opens it with.
pub(super) fn open_with_system(path: &Path) -> Result<(), String> {
    let program = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    if cfg!(target_os = "android") {
        return Err("files are opened from the Files app on Android".into());
    }
    std::process::Command::new(program).arg(path).spawn().map(|_| ()).map_err(|e| format!("{program}: {e}"))
}

impl Browser {
    /// Download `url` for a tab: into `to`, or the wearer's Downloads. Its number.
    pub(super) fn start_download(&mut self, tab: TabId, url: &str, to: Option<PathBuf>, purpose: Purpose) -> Option<u64> {
        let profile = self.tabs.tab(tab).map(|t| t.profile.clone())?;
        self.host(&profile)?;
        let key = self.downloads.next.max(1);
        self.downloads.next = key + 1;
        self.downloads.items.push(Item {
            key,
            engine_id: key,
            profile: profile.clone(),
            tab,
            url: url.to_string(),
            file: to.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            received: 0,
            total: 0,
            state: State::Going,
            started: now_ms(),
            purpose,
        });
        let to = to.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        self.tell_profile(Some(&profile), ToHost::Download { tab, id: key, url: url.to_string(), to });
        self.dirty_all();
        Some(key)
    }

    pub(super) fn dirty_all(&mut self) {
        for win in &mut self.wins {
            win.dirty = true;
        }
    }

    /// What an engine said of a download.
    pub(super) fn download_news(&mut self, profile: &str, tab: TabId, id: u64, event: Download) {
        let at = self.downloads.items.iter().position(|i| i.profile == profile && i.engine_id == id && (id < ENGINE_DOWNLOADS || i.state == State::Going));
        let at = match at {
            Some(at) => at,
            None if id >= ENGINE_DOWNLOADS => {
                // One the page began: a link with `download`, a file it cannot show.
                let key = self.downloads.next.max(1);
                self.downloads.next = key + 1;
                self.downloads.items.push(Item {
                    key,
                    engine_id: id,
                    profile: profile.to_string(),
                    tab,
                    url: String::new(),
                    file: String::new(),
                    received: 0,
                    total: 0,
                    state: State::Going,
                    started: now_ms(),
                    purpose: Purpose::Keep,
                });
                self.downloads.items.len() - 1
            }
            None => return,
        };
        let mut finished = None;
        {
            let item = &mut self.downloads.items[at];
            match event {
                Download::Started { url, file } => {
                    if !url.is_empty() {
                        item.url = url;
                    }
                    if !file.is_empty() {
                        item.file = file;
                    }
                }
                Download::Progress { received, total } => {
                    item.received = received;
                    item.total = total;
                }
                Download::Finished { file } => {
                    if !file.is_empty() {
                        item.file = file;
                    }
                    item.received = item.received.max(item.total);
                    item.state = State::Done;
                    finished = Some(item.clone());
                }
                Download::Failed { why } => {
                    item.state = State::Failed(why.clone());
                    if item.purpose == Purpose::Install {
                        self.toast = Some((format!("The extension could not be downloaded: {why}"), std::time::Instant::now()));
                    } else if why != "Cancelled" {
                        let name = item.name();
                        self.toast = Some((format!("{name} could not be downloaded: {why}"), std::time::Instant::now()));
                    }
                }
            }
        }
        if let Some(item) = finished {
            self.download_finished(item);
        }
        let excess = self.downloads.items.len().saturating_sub(KEPT);
        self.downloads.items.drain(..excess);
        self.downloads.save(&self.store);
        self.refresh_internal();
        self.dirty_all();
    }

    fn download_finished(&mut self, item: Item) {
        match item.purpose {
            Purpose::Install => {
                match self.install_extension(&item.profile, &item.file) {
                    Ok(name) => self.say(format!("{name} is installed")),
                    Err(e) => self.say(format!("Not installed: {e}")),
                }
                let _ = std::fs::remove_file(&item.file);
                self.downloads.items.retain(|i| i.key != item.key);
            }
            Purpose::Keep if spatimesh::extensions::store::is_package(&item.file) && item.profile != PRIVATE => {
                // An extension's package, downloaded from a link: installed if the wearer says.
                let window = self.tabs.window_of(item.tab).and_then(|k| self.win_of_key(k)).or_else(|| self.last_focused.and_then(|k| self.win_of_key(k))).or((!self.wins.is_empty()).then_some(0));
                if let Some(w) = window {
                    let message = format!("{} is an extension. Install it in this profile? It can then read and change the pages it asks for.", item.name());
                    self.ask_own_with(w, Own::InstallPackage(item.profile.clone(), item.file.clone()), &message, false, "");
                }
            }
            Purpose::Keep => self.say(format!("{} is in {}", item.name(), Path::new(&item.file).parent().map(|p| p.display().to_string()).unwrap_or_default())),
        }
    }

    pub(super) fn cancel_download(&mut self, key: u64) {
        if let Some(item) = self.downloads.get(key).filter(|i| i.state == State::Going).cloned() {
            self.tell_profile(Some(&item.profile), ToHost::CancelDownload { id: item.engine_id });
        }
    }

    pub(super) fn retry_download(&mut self, w: usize, key: u64) {
        let Some(item) = self.downloads.get(key).cloned() else { return };
        self.downloads.items.retain(|i| i.key != key);
        let tab = if self.tabs.tab(item.tab).is_some() { item.tab } else { self.tabs.active(self.wins[w].key).unwrap_or(0) };
        self.start_download(tab, &item.url, None, Purpose::Keep);
    }

    pub(super) fn forget_download(&mut self, key: u64) {
        self.downloads.items.retain(|i| i.key != key || i.state == State::Going);
        self.downloads.save(&self.store);
        self.dirty_all();
    }

    pub(super) fn clear_downloads(&mut self, profile: &str) {
        self.downloads.items.retain(|i| i.profile != profile || i.state == State::Going);
        self.downloads.save(&self.store);
        self.dirty_all();
    }

    /// The extension of the store page in front of a window: downloaded into the profile's
    /// cache, then installed.
    pub(super) fn add_from_store(&mut self, w: usize) {
        let key = self.wins[w].key;
        let Some(tab) = self.tabs.active(key).and_then(|t| self.tabs.tab(t)).cloned() else { return };
        if tab.profile == PRIVATE {
            self.say("Extensions are installed in a profile, not in a private window".into());
            return;
        }
        let Some(listing) = spatimesh::extensions::store::listing(&tab.url) else { return };
        let to = self.store.cache_dir(&tab.profile).join("extension-packages").join(listing.file_name());
        let _ = std::fs::remove_file(&to);
        self.start_download(tab.id, &listing.package, Some(to), Purpose::Install);
        self.say("Downloading the extension\u{2026}".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_status() {
        assert_eq!(size(512), "512 bytes");
        assert_eq!(size(2048), "2 kB");
        assert_eq!(size(3 << 20), "3.0 MB");
        let mut item = Item {
            key: 1,
            engine_id: 1,
            profile: "p".into(),
            tab: 1,
            url: "https://a.example/files/thing.zip".into(),
            file: String::new(),
            received: 1 << 20,
            total: 4 << 20,
            state: State::Going,
            started: 0,
            purpose: Purpose::Keep,
        };
        assert_eq!(item.name(), "thing.zip");
        assert_eq!(item.status(), "1.0 MB of 4.0 MB");
        assert_eq!(item.progress(), Some(0.25));
        item.state = State::Failed("no".into());
        assert_eq!(item.status(), "Failed: no");
        assert_eq!(item.progress(), None);
    }
}
