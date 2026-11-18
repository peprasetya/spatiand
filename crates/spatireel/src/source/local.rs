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
        Item { key: path.to_string_lossy().into_owned(), title: library::title(&name), name, path: Some(path.to_path_buf()), ..Item::default() }
    }

    /// Where an item of this machine's is.
    fn path_of(item: &Item) -> PathBuf {
        item.path.clone().unwrap_or_else(|| PathBuf::from(&item.key))
    }
}

impl Source for Local {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn list(&self, folder: Option<&Item>) -> Result<Vec<Item>, String> {
        let path = folder.map_or(self.top.clone(), Local::path_of);
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
                    path: Some(entry.path),
                    ..Item::default()
                }
            })
            .collect())
    }

    fn picture(&self, item: &Item) -> Option<Picture> {
        spatiand_media::still::grab_file(&Local::path_of(item), 480).map(Picture::Pixels)
    }

    fn play(&self, item: &Item) -> Result<Play, String> {
        let path = Local::path_of(item);
        // A `.strm` says where the film is: an address, played as it is.
        Ok(match library::pointed_to(&path) {
            Some(url) => Play { url, options: Vec::new(), path: None },
            None => Play { url: path.to_string_lossy().into_owned(), options: Vec::new(), path: Some(path) },
        })
    }

    // Where this machine's films were left is not kept: `watching` and `unfinished` are the
    // trait's own, which do nothing.
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name that is not UTF-8 (Latin-1's "é", as an old card or a copied disc has it): listed,
    /// and what is played is the file itself, not its name made text.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_film_whose_name_is_not_utf8_is_played_by_its_own_name() {
        use std::os::unix::ffi::OsStrExt;
        let dir = std::env::temp_dir().join(format!("spatireel-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let name = std::ffi::OsStr::from_bytes(b"Caf\xe9.mkv");
        std::fs::write(dir.join(name), b"").unwrap();
        let local = Local::new(dir.clone());
        let items = local.list(None).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Caf\u{fffd}");
        let play = local.play(&items[0]).unwrap();
        let path = play.path.expect("the file's own name");
        assert!(path.exists() && path.file_name() == Some(name));
        // Not a name made text, which is the name of nothing.
        assert!(!std::path::Path::new(&play.url).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
