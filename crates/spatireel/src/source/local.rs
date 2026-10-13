//! This machine's folders.

use std::path::{Path, PathBuf};

use std::sync::{Arc, Mutex};

use super::{Item, Picture, Play, Source, Watching};
use crate::library::{self, Kind};
use crate::store::Store;

pub struct Local {
    /// Where this machine's films were left, which nothing else would remember.
    pub store: Arc<Mutex<Store>>,
    /// Where the top is: the folder films are usually kept in. Above it is reached by going
    /// up, all the way to the root -- what may be read is the file system's to say.
    pub top: PathBuf,
    pub label: String,
}

impl Local {
    pub fn new(top: PathBuf, store: Arc<Mutex<Store>>) -> Local {
        let label = if cfg!(target_os = "macos") {
            "This Mac"
        } else if cfg!(target_os = "android") {
            "This device"
        } else {
            "This computer"
        };
        Local { top, label: label.to_string(), store }
    }

    pub fn item(path: &Path) -> Item {
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        Item { key: path.to_string_lossy().into_owned(), title: library::title(&name), name, ..Item::default() }
    }
}

impl Source for Local {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn list(&self, folder: Option<&Item>) -> Result<Vec<Item>, String> {
        let path = folder.map_or(self.top.clone(), |item| PathBuf::from(&item.key));
        std::fs::read_dir(&path).map_err(|e| format!("{} cannot be opened: {e}", path.display()))?;
        let places = self.store.lock().unwrap().places.clone();
        Ok(library::list(&path)
            .into_iter()
            .map(|entry| {
                let folder = entry.kind == Kind::Folder;
                let place = places.get(entry.path.to_string_lossy().as_ref()).copied();
                Item {
                    resume: place.map_or(0.0, |p| p.seconds),
                    progress: place.filter(|p| p.duration > 0.0).map(|p| (p.seconds / p.duration) as f32),
                    key: entry.path.to_string_lossy().into_owned(),
                    title: if folder { entry.name.clone() } else { library::title(&entry.name) },
                    detail: if folder { Some("Folder".into()) } else { library::told_by_name(&entry.name) },
                    folder,
                    // A film's tile is a frame of it, which is as wide as films are.
                    aspect: (!folder).then_some(16.0 / 9.0),
                    pictured: !folder && !entry.name.to_lowercase().ends_with(".strm"),
                    name: entry.name,
                    ..Item::default()
                }
            })
            .collect())
    }

    fn picture(&self, item: &Item) -> Option<Picture> {
        spatiand_media::still::grab(&item.key, 480).map(Picture::Pixels)
    }

    fn play(&self, item: &Item) -> Result<Play, String> {
        Ok(Play { url: library::source(Path::new(&item.key)), options: Vec::new() })
    }

    fn watching(&self, item: &Item, what: Watching) {
        let mut store = self.store.lock().unwrap();
        match what {
            Watching::Stopped { seconds, duration } => store.leave(&item.key, seconds, duration),
            Watching::Finished => store.leave(&item.key, 0.0, 0.0),
            _ => return,
        }
        store.save();
    }

    fn unfinished(&self) -> Vec<Item> {
        let mut places: Vec<(String, crate::store::Place)> = self.store.lock().unwrap().places.iter().map(|(film, place)| (film.clone(), *place)).collect();
        places.sort_by_key(|(_, place)| std::cmp::Reverse(place.when));
        places
            .into_iter()
            // A film since moved or deleted is not offered.
            .filter(|(film, _)| Path::new(film).is_file())
            .map(|(film, place)| Item {
                resume: place.seconds,
                progress: (place.duration > 0.0).then(|| (place.seconds / place.duration) as f32),
                aspect: Some(16.0 / 9.0),
                pictured: true,
                ..Local::item(Path::new(&film))
            })
            .collect()
    }
}
