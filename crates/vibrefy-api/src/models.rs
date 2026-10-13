//! The shapes Vibrefy puts on the wire, and the few rules that travel with them.
//!
//! Ported from `app/src/main/java/id/prasetya/vibrefy/app/data/Models.kt`. The field
//! meanings are the server's, not ours - see `CLIENT-API.md` in the Vibrefy repo.

/// Vibrefy's `FileItem` type codes, as emitted in the `type` field of every JSON item.
/// See `id.prasetya.vibrefy.data.FileItem` in the server project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemType {
    Library,
    Directory,
    File,
    Link,
    Cloud,
    Unknown,
}

impl ItemType {
    pub fn of(code: i64) -> Self {
        match code {
            1 => Self::Library,
            2 => Self::Directory,
            3 => Self::File,
            4 => Self::Link,
            5 => Self::Cloud,
            _ => Self::Unknown,
        }
    }

    /// Anything that can be opened as a folder rather than played.
    pub fn is_browsable(self) -> bool {
        matches!(self, Self::Library | Self::Directory | Self::Cloud)
    }
}

/// One row in a listing. `path` is the library-relative path the server addresses
/// everything by - stream, subtitles, thumbnail and progress key are all built from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaItem {
    pub name: String,
    pub path: String,
    pub kind: ItemType,
    /// Resume position in seconds. A directory carries the progress of nothing.
    pub resume_seconds: i64,
    /// Last-modified, in server milliseconds. What "Recent Update" is ordered by.
    pub modified: i64,
}

impl MediaItem {
    /// The key `ProgressBean` stores against, matching `scriptlib.js`'s `mediaFile`.
    ///
    /// It has to match the web player's exactly or the two disagree about where a file
    /// was left, and the disagreement shows up as a film that restarts from the top.
    pub fn progress_key(&self) -> String {
        format!("files/{}", self.path)
    }

    pub fn is_resumable(&self) -> bool {
        self.kind == ItemType::File && self.resume_seconds > 0
    }

    /// Whether the server has anywhere to look for a picture of this.
    ///
    /// `BrowseBean` only enters its `thumbmp4` branch when the path is a *file*. Asked
    /// about a directory it falls through to the ordinary browse page and answers **200
    /// with `text/html`** - byte for byte what a dead session answers - so asking for a
    /// folder's thumbnail is asking a question whose answer cannot be read.
    pub fn has_artwork(&self) -> bool {
        !self.kind.is_browsable()
    }

    /// What the server will actually stream. `BrowseBean.isMedia` lists only these two
    /// extensions, and `scriptlib.js`'s `itemClass` keys its `video` class on the same test.
    pub fn is_video(&self) -> bool {
        if self.kind != ItemType::File {
            return false;
        }
        let lower = self.name.to_ascii_lowercase();
        lower.ends_with(".mp4") || lower.ends_with(".m4v")
    }
}

/// One shelf: "Libraries", "Media", "Watching", "Recent Update", ...
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemGroup {
    pub title: String,
    pub items: Vec<MediaItem>,
}

/// One step of the breadcrumb. The server sends the crumbs as plain names; `path` is the
/// running join of everything up to and including this one, which is what addresses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crumb {
    pub label: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FolderListing {
    pub crumbs: Vec<Crumb>,
    pub groups: Vec<ItemGroup>,
}

impl FolderListing {
    pub fn items(&self) -> impl Iterator<Item = &MediaItem> {
        self.groups.iter().flat_map(|g| g.items.iter())
    }
}

/// An entry from `GET /subtitles/<sid>/<path>`.
///
/// The server builds these by walking the MP4's `TrackBox`es and keeping only the `sbtl`
/// and `text` handlers, so there is never a bitmap track here - `SubtitleBean` cannot
/// render PGS or VobSub, and there is no sidecar `.srt` support at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleTrack {
    pub index: i64,
    pub label: String,
    pub lang: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, kind: ItemType) -> MediaItem {
        MediaItem {
            name: name.into(),
            path: format!("Films/{name}"),
            kind,
            resume_seconds: 0,
            modified: 0,
        }
    }

    /// The one that signed people out. A folder's thumbnail is not a missing picture -
    /// it is the browse page, 200 and `text/html`, which is exactly what the server sends
    /// when the session has ended. The two cannot be told apart, so the question is never
    /// asked.
    #[test]
    fn only_a_file_is_worth_asking_the_server_for_a_picture_of() {
        assert!(item("A Film.mp4", ItemType::File).has_artwork());
        assert!(!item("Films", ItemType::Directory).has_artwork());
        assert!(!item("Library", ItemType::Library).has_artwork());
        assert!(!item("Drive", ItemType::Cloud).has_artwork());
    }

    #[test]
    fn the_progress_key_is_the_one_the_web_player_writes() {
        assert_eq!("files/Films/A Film.mp4", item("A Film.mp4", ItemType::File).progress_key());
    }
}
