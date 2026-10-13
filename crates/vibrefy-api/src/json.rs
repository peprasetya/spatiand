//! Turns Vibrefy's JSON answers into the client's models.
//!
//! Ported from `app/src/main/java/id/prasetya/vibrefy/app/data/VibrefyJson.kt`.
//!
//! What the server deliberately does *not* send is the derived shelves - "Watching" and
//! "Recent Update" are re-orderings of the very item list it has already sent, so it sends
//! that list once and leaves the sorting here. [`folder`] is where that happens, and it
//! has to agree with `dynamic/files.jsp` or the web page and this client disagree about
//! what a folder contains.

use serde_json::Value;

use crate::models::{Crumb, FolderListing, ItemGroup, ItemType, MediaItem};

pub const WATCHING: &str = "Watching";
pub const RECENT: &str = "Recent Update";

/// How many tiles the "Recent Update" shelf carries, matching `dynamic/files.jsp`.
const RECENT_LIMIT: usize = 20;

pub fn item(json: &Value) -> Option<MediaItem> {
    let path = json
        .get("path")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    let kind = ItemType::of(json.get("type").and_then(Value::as_i64).unwrap_or(-1));
    let resume = json.get("time").and_then(Value::as_i64).unwrap_or(0);
    Some(MediaItem {
        name: json
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).to_string()),
        path: path.to_string(),
        kind,
        // A directory carries the progress of nothing; only a file can be resumed.
        resume_seconds: if kind == ItemType::File { resume.max(0) } else { 0 },
        modified: json.get("modified").and_then(Value::as_i64).unwrap_or(0),
    })
}

pub fn items(array: Option<&Value>) -> Vec<MediaItem> {
    array
        .and_then(Value::as_array)
        .map(|rows| rows.iter().filter_map(item).collect())
        .unwrap_or_default()
}

/// The watch list from `/home`, which is the only shelf that command emits.
///
/// Note that `/home` carries no `session` field of its own, so nothing here can be played
/// until `/files` has been asked once for the session id the stream URLs need.
pub fn watching(json: &Value) -> Vec<MediaItem> {
    items(json.get("watching"))
}

/// A folder listing from `/files/...`, shelved the way the web page shelves it.
///
/// Items arrive already sorted by type and then name, so a shelf boundary is simply a
/// change of type - the same walk `dynamic/files.jsp` does. Around that sit the two
/// derived shelves: what is part-watched here, and what arrived most recently.
pub fn folder(json: &Value) -> FolderListing {
    let items = items(json.get("items"));
    FolderListing {
        crumbs: crumbs(json),
        groups: shelves(items),
    }
}

/// The breadcrumb. The server sends bare names plus the joined path; the clickable target
/// for each step is the running join up to it, and the leading "Library" step addresses
/// the root, which has no name of its own.
fn crumbs(json: &Value) -> Vec<Crumb> {
    let Some(names) = json.get("crumbs").and_then(Value::as_array) else {
        return Vec::new();
    };
    if names.is_empty() {
        return Vec::new();
    }
    let mut steps = vec![Crumb {
        label: "Library".into(),
        path: String::new(),
    }];
    let mut running = String::new();
    for name in names {
        let Some(name) = name.as_str().filter(|s| !s.is_empty()) else {
            continue;
        };
        if !running.is_empty() {
            running.push('/');
        }
        running.push_str(name);
        steps.push(Crumb {
            label: name.to_string(),
            path: running.clone(),
        });
    }
    steps
}

fn shelves(items: Vec<MediaItem>) -> Vec<ItemGroup> {
    let mut groups: Vec<ItemGroup> = Vec::new();

    // Ahead of the listing itself, because picking up where you left off is the reason
    // this folder was opened often enough to be worth the top of the screen.
    let watching: Vec<MediaItem> = items.iter().filter(|i| i.is_resumable()).cloned().collect();
    if !watching.is_empty() {
        groups.push(ItemGroup {
            title: WATCHING.into(),
            items: watching,
        });
    }

    let mut run: Vec<MediaItem> = Vec::new();
    for item in &items {
        if let Some(first) = run.first() {
            if first.kind != item.kind {
                groups.push(ItemGroup {
                    title: title(first.kind).into(),
                    items: std::mem::take(&mut run),
                });
            }
        }
        run.push(item.clone());
    }
    if let Some(first) = run.first() {
        groups.push(ItemGroup {
            title: title(first.kind).into(),
            items: run,
        });
    }

    // The same media again, newest first. Worth a shelf only when there is more than one
    // file to order - with one, it would just be the Media shelf repeated.
    let mut videos: Vec<MediaItem> = items.iter().filter(|i| i.is_video()).cloned().collect();
    if videos.len() > 1 {
        videos.sort_by(|a, b| b.modified.cmp(&a.modified));
        videos.truncate(RECENT_LIMIT);
        groups.push(ItemGroup {
            title: RECENT.into(),
            items: videos,
        });
    }

    groups
}

/// The shelf headings `dynamic/files.jsp` writes. Folders deliberately go under a blank
/// one: they are the body of the page, not a section of it.
fn title(kind: ItemType) -> &'static str {
    match kind {
        ItemType::Library => "Libraries",
        ItemType::Cloud => "Cloud",
        ItemType::File => "Media",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).expect("valid fixture")
    }

    /// The shape `BrowseBean.getJson()` emits, item for item.
    fn fixture() -> Value {
        parse(
            r#"{
              "session":"6E3A9F1CSESSIONID",
              "library":"Movies",
              "path":"Movies/Season 1",
              "crumbs":["Movies","Season 1"],
              "admin":false,
              "cloud":false,
              "items":[
                {"name":"Extras","path":"Movies/Season 1/Extras","type":2,"time":0,"modified":900},
                {"name":"A.mp4","path":"Movies/Season 1/A.mp4","type":3,"time":742,"modified":100},
                {"name":"B.mp4","path":"Movies/Season 1/B.mp4","type":3,"time":0,"modified":300},
                {"name":"C.mp4","path":"Movies/Season 1/C.mp4","type":3,"time":0,"modified":200}
              ]
            }"#,
        )
    }

    fn names(group: &ItemGroup) -> Vec<&str> {
        group.items.iter().map(|i| i.name.as_str()).collect()
    }

    #[test]
    fn crumbs_address_every_step_and_the_first_one_is_the_library_root() {
        let crumbs = folder(&fixture()).crumbs;
        assert_eq!(
            vec!["Library", "Movies", "Season 1"],
            crumbs.iter().map(|c| c.label.as_str()).collect::<Vec<_>>()
        );
        // Each step is the running join, which is what /files/<path> takes.
        assert_eq!(
            vec!["", "Movies", "Movies/Season 1"],
            crumbs.iter().map(|c| c.path.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn items_are_shelved_by_type_in_the_order_the_server_sorted_them() {
        let groups = folder(&fixture()).groups;
        // Watching first, then the listing itself, then the same media newest-first.
        assert_eq!(
            vec!["Watching", "", "Media", "Recent Update"],
            groups.iter().map(|g| g.title.as_str()).collect::<Vec<_>>()
        );
        assert_eq!(vec!["Extras"], names(&groups[1]));
        assert_eq!(vec!["A.mp4", "B.mp4", "C.mp4"], names(&groups[2]));
    }

    #[test]
    fn the_watching_shelf_is_what_this_folder_has_a_position_for() {
        let listing = folder(&fixture());
        let watching = listing
            .groups
            .iter()
            .find(|g| g.title == WATCHING)
            .expect("a watching shelf");
        assert_eq!(1, watching.items.len());
        let item = &watching.items[0];
        assert_eq!("A.mp4", item.name);
        assert_eq!(742, item.resume_seconds);
        assert!(item.is_resumable());
        assert_eq!("files/Movies/Season 1/A.mp4", item.progress_key());
    }

    #[test]
    fn recent_update_is_the_same_media_newest_first() {
        let listing = folder(&fixture());
        let recent = listing
            .groups
            .iter()
            .find(|g| g.title == RECENT)
            .expect("a recent shelf");
        assert_eq!(vec!["B.mp4", "C.mp4", "A.mp4"], names(recent));
    }

    /// With one file there is nothing to re-order, and the shelf would just be the Media
    /// shelf printed twice - the same guard `dynamic/files.jsp` puts on it.
    #[test]
    fn a_folder_with_a_single_video_has_nothing_to_reorder() {
        let single = parse(
            r#"{"session":"S","crumbs":["Movies"],"items":[
                {"name":"A.mp4","path":"Movies/A.mp4","type":3,"time":0,"modified":100}]}"#,
        );
        assert_eq!(
            vec!["Media"],
            folder(&single)
                .groups
                .iter()
                .map(|g| g.title.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_library_root_lists_volumes_and_cloud_mounts_with_no_breadcrumb() {
        let root = parse(
            r#"{"session":"S","library":null,"path":"","crumbs":[],"admin":false,"cloud":false,"items":[
                {"name":"Movies","path":"Movies/","type":1,"time":0,"modified":0},
                {"name":"gdrive (me@example.com)","path":"~gdrive/","type":5,"time":0,"modified":0}]}"#,
        );
        let listing = folder(&root);
        assert!(listing.crumbs.is_empty());
        assert_eq!(
            vec!["Libraries", "Cloud"],
            listing
                .groups
                .iter()
                .map(|g| g.title.as_str())
                .collect::<Vec<_>>()
        );
        assert!(listing.groups[0].items[0].kind.is_browsable());
        assert_eq!(ItemType::Cloud, listing.groups[1].items[0].kind);
    }

    /// `time` is written on every row, but only a file can be resumed - a directory
    /// carries the progress of nothing, and treating its 0 as a position would shelve it.
    #[test]
    fn only_files_carry_a_resume_position() {
        let listing = folder(&fixture());
        let extras = &listing.groups[1].items[0];
        assert_eq!(0, extras.resume_seconds);
        assert!(!extras.is_resumable());
    }

    #[test]
    fn the_watch_list_is_read_the_same_way_as_a_listing_row() {
        let home = parse(
            r#"{"watching":[{"name":"Film.mp4","path":"Movies/Film.mp4","type":3,"time":12,"modified":5}]}"#,
        );
        let list = watching(&home);
        assert_eq!(1, list.len());
        assert_eq!("Movies/Film.mp4", list[0].path);
        assert_eq!(12, list[0].resume_seconds);
        assert!(list[0].is_video());
    }

    #[test]
    fn an_empty_watch_list_is_empty_rather_than_absent() {
        assert!(watching(&parse(r#"{"watching":[]}"#)).is_empty());
        assert!(watching(&parse("{}")).is_empty());
    }

    /// A row with no path addresses nothing, so it is dropped rather than half-rendered.
    #[test]
    fn a_row_the_server_could_not_name_is_skipped() {
        let listing = folder(&parse(r#"{"crumbs":[],"items":[{"name":"orphan","type":3}]}"#));
        assert_eq!(0, listing.items().count());
    }
}
