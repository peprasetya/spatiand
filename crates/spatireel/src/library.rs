//! Films in folders on this machine.

use std::path::{Path, PathBuf};

use spatiand_media::hints::{self, ContainerHints};
use spatiand_media::{Projection, StereoLayout};

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Folder,
    Film,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub kind: Kind,
}

/// What FFmpeg will play that is a film rather than a song or a picture, by how it is named.
const FILMS: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "mov", "avi", "ts", "m2ts", "mts", "mpg", "mpeg", "wmv", "flv", "ogv", "3gp", "vob", "strm",
];

pub fn is_film(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| FILMS.iter().any(|known| e.eq_ignore_ascii_case(known)))
}

fn is_package(name: &str) -> bool {
    let Some((_, ending)) = name.rsplit_once('.') else { return false };
    let ending = ending.to_lowercase();
    ending.ends_with("library") || ["app", "theater", "bundle", "framework", "photoslibrary", "plugin", "kext"].contains(&ending.as_str())
}

/// Where a film really is. A `.strm` file is one line saying where: the convention every
/// media server shares for a film that is a stream somewhere else.
pub fn source(path: &Path) -> String {
    let is_pointer = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("strm"));
    if is_pointer {
        if let Some(line) = std::fs::read_to_string(path).ok().and_then(|text| text.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)) {
            return line;
        }
    }
    path.to_string_lossy().into_owned()
}

/// Where films are kept on this kind of machine, if such a place exists; the home folder if
/// not.
pub fn home() -> PathBuf {
    if let Some(named) = std::env::var_os("SPATIREEL_FOLDER") {
        return named.into();
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into());
    let usual: &[&str] = if cfg!(target_os = "android") {
        &["/sdcard/Movies", "/sdcard"]
    } else if cfg!(target_os = "macos") {
        &["Movies"]
    } else {
        &["Videos"]
    };
    usual.iter().map(|folder| home.join(folder)).find(|path| path.is_dir()).unwrap_or(home)
}

/// A folder's folders, then its films, each by name. Whatever cannot be read is not listed:
/// a folder the wearer may not open is not a reason to show none of the others.
pub fn list(folder: &Path) -> Vec<Entry> {
    let mut entries: Vec<Entry> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let path = entry.path();
            // Through a link, to what it points at: a library kept on another disk is usually
            // reached by one.
            let kind = if path.is_dir() {
                // A Mac keeps some things in folders that are not folders to anyone using
                // one: an application, a library of a program's own.
                if is_package(&name) {
                    return None;
                }
                Kind::Folder
            } else if is_film(&path) {
                Kind::Film
            } else {
                return None;
            };
            Some(Entry { name, path, kind })
        })
        .collect();
    entries.sort_by_key(|entry| (entry.kind != Kind::Folder, entry.name.to_lowercase()));
    entries
}

/// A film's name without what its file is called for the machine's sake.
pub fn title(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, extension)) if FILMS.iter().any(|known| extension.eq_ignore_ascii_case(known)) => stem.to_string(),
        _ => name.to_string(),
    }
}

/// What a film's name says it is, for a word under it before it has ever been opened.
pub fn told_by_name(name: &str) -> Option<String> {
    let layout = hints::detect(name, ContainerHints::default());
    let place = match layout.projection {
        Projection::Flat => None,
        Projection::Dome180 => Some("180\u{b0}"),
        Projection::Sphere360 => Some("360\u{b0}"),
    };
    let eyes = match layout.stereo {
        StereoLayout::Mono => None,
        _ => Some("3D"),
    };
    match (place, eyes) {
        (None, None) => None,
        (Some(place), None) => Some(place.to_string()),
        (None, Some(eyes)) => Some(eyes.to_string()),
        (Some(place), Some(eyes)) => Some(format!("{place} {eyes}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_film_is_known_by_its_ending_whatever_its_case() {
        assert!(is_film(Path::new("/films/Arrival.MKV")));
        assert!(is_film(Path::new("a.mp4")));
        assert!(!is_film(Path::new("cover.jpg")));
        assert!(!is_film(Path::new("README")));
    }

    #[test]
    fn a_title_loses_only_an_ending_that_means_film() {
        assert_eq!(title("Dune.Part.Two.mkv"), "Dune.Part.Two");
        assert_eq!(title("Season 1"), "Season 1");
        assert_eq!(title("notes.v2"), "notes.v2");
    }

    #[test]
    fn folders_come_first_and_names_are_in_order_whatever_their_case() {
        let dir = std::env::temp_dir().join(format!("spatireel-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("shows")).unwrap();
        for name in ["beta.mp4", "Alpha.mkv", ".hidden.mp4", "poster.jpg"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        let names: Vec<String> = list(&dir).into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["shows", "Alpha.mkv", "beta.mp4"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_name_that_says_what_the_film_is_is_passed_on() {
        assert_eq!(told_by_name("Gravity.HSBS.mkv").as_deref(), Some("3D"));
        assert_eq!(told_by_name("surf_vr180_sbs.mp4").as_deref(), Some("180\u{b0} 3D"));
        assert_eq!(told_by_name("Arrival.mkv"), None);
    }
}
