//! This machine's folders.

use std::path::{Path, PathBuf};

use super::{Item, Picture, Play, Source};
use crate::library::{self, Kind};

pub struct Local {
    /// Where the top is: the folder films are usually kept in. Above it is reached by going
    /// up, all the way to the root -- what may be read is the file system's to say.
    pub top: PathBuf,
    pub label: String,
}

impl Local {
    pub fn new(top: PathBuf) -> Local {
        let label = if cfg!(target_os = "macos") {
            "This Mac"
        } else if cfg!(target_os = "android") {
            "This device"
        } else {
            "This computer"
        };
        Local { top, label: label.to_string() }
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
        Ok(library::list(&path)
            .into_iter()
            .map(|entry| {
                let folder = entry.kind == Kind::Folder;
                Item {
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

    // Where this machine's films were left is not kept: `watching` and `unfinished` are the
    // trait's own, which do nothing.
}
