//! Where the chooser is, or what a line of it is: a path of this machine, or a place in one of the
//! storage service's remotes, written as every Spatiand application writes it (`remote:folder/file`,
//! `spatiand_storage::virtual_place`).

use std::path::PathBuf;

use spatiand_storage::Remote;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Place {
    Local(PathBuf),
    /// A remote by its name, and the path in it: no slash at either end, empty for its top.
    Remote(String, String),
}

impl Place {
    pub fn remote(name: &str, path: &str) -> Place {
        Place::Remote(name.to_string(), path.trim_matches('/').to_string())
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, Place::Remote(..))
    }

    /// The folder it is in: none for the top of the machine or of a remote.
    pub fn parent(&self) -> Option<Place> {
        match self {
            Place::Local(path) => path.parent().map(|p| Place::Local(p.to_path_buf())),
            Place::Remote(_, path) if path.is_empty() => None,
            Place::Remote(remote, path) => Some(Place::Remote(remote.clone(), path.rsplit_once('/').map_or(String::new(), |(dir, _)| dir.to_string()))),
        }
    }

    /// What is called `name` in it; a name may be a path further in (`sub/new.txt`), as a path
    /// of this machine joins one.
    pub fn join(&self, name: &str) -> Place {
        match self {
            Place::Local(path) => Place::Local(path.join(name)),
            Place::Remote(remote, path) if path.is_empty() => Place::remote(remote, name),
            Place::Remote(remote, path) => Place::remote(remote, &format!("{path}/{name}")),
        }
    }

    /// Its own name, a file's or a folder's; the top of a remote is called by the remote's.
    pub fn name(&self) -> String {
        match self {
            Place::Local(path) => path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into()),
            Place::Remote(remote, path) => path.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or(remote).to_string(),
        }
    }

    /// As it is written, and typed: a path, or `remote:path`.
    pub fn shown(&self) -> String {
        match self {
            Place::Local(path) => path.display().to_string(),
            Place::Remote(remote, path) => format!("{remote}:{path}"),
        }
    }

    /// A place written `remote:path` for a remote that is there; anything else is not one, so a
    /// name with a colon in it is still a name.
    pub fn typed(text: &str, remotes: &[Remote]) -> Option<Place> {
        let (remote, path) = spatiand_storage::virtual_place(text)?;
        remotes.iter().any(|r| r.name == remote).then(|| Place::remote(&remote, &path))
    }

    /// Read back from [`Place::shown`]: a path of this machine starts with a slash.
    pub fn parse(text: &str) -> Option<Place> {
        let text = text.trim();
        if text.starts_with('/') {
            return Some(Place::Local(PathBuf::from(text)));
        }
        spatiand_storage::virtual_place(text).map(|(remote, path)| Place::remote(&remote, &path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_place_goes_up_and_in_as_a_path_does() {
        let top = Place::remote("Drive", "");
        let photos = top.join("Photos").join("2024");
        assert_eq!(photos, Place::Remote("Drive".into(), "Photos/2024".into()));
        assert_eq!(photos.name(), "2024");
        assert_eq!(photos.shown(), "Drive:Photos/2024");
        assert_eq!(photos.parent(), Some(Place::remote("Drive", "Photos")));
        assert_eq!(photos.parent().and_then(|p| p.parent()), Some(top.clone()));
        assert_eq!(top.parent(), None, "a remote's top has nothing over it");
        assert_eq!(top.name(), "Drive");
        assert_eq!(top.join("sub/new.txt"), Place::remote("Drive", "sub/new.txt"));
        assert_eq!(Place::Local("/".into()).parent(), None);
        assert_eq!(Place::Local("/".into()).name(), "/");
        assert_eq!(Place::Local("/home/a".into()).join("b.txt"), Place::Local("/home/a/b.txt".into()));
        for place in [photos, top, Place::Local("/home/a b/c:d.txt".into())] {
            assert_eq!(Place::parse(&place.shown()), Some(place.clone()), "read back as written");
        }
    }

    #[test]
    fn only_a_known_remote_is_typed() {
        let remotes = vec![Remote { name: "Drive".into(), kind: "drive".into() }];
        assert_eq!(Place::typed("Drive:Photos/", &remotes), Some(Place::remote("Drive", "Photos")));
        assert_eq!(Place::typed("Drive:", &remotes), Some(Place::remote("Drive", "")));
        assert_eq!(Place::typed("notes:2024", &remotes), None, "words to find, not a remote");
        assert_eq!(Place::typed("/home/a:b", &remotes), None);
        assert_eq!(Place::typed("draft", &remotes), None);
    }
}
