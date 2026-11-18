//! The next episode, played by itself when one ends, as Vibrefy's web player does (the owner,
//! 2026-11-18: "playing video of TV Show, can automatic go to next video when one ended, till the
//! last.. I want spatireel do the same").
//!
//! The rule is Vibrefy's own, already ported (`vibrefy_api::episodes`, from `parseEpisode` and
//! `pickNextEpisode` in its `scriptlib.js`): a name with an `SxxExx` marker is an episode, the text
//! before the marker says which show, and the next is the lowest episode of the same show after
//! it among the names in the same folder -- `s4e9` before `s4e10`, and on into the next season.
//! Vibrefy decides it in the page from the folder listing the server already gives; SpatiReel
//! does the same from the folder listing its source gives, whichever source that is. One rule,
//! by name, everywhere: a Jellyfin's own episode order is not asked for.

use vibrefy_api::episodes;

use crate::source::{Item, Source};

/// A folder's listing as it was held: the folder's key, and what is in it.
pub type Listing = (String, Vec<Item>);

/// The episode after `current` among `beside` (what is in its folder), or nothing: the last
/// one, another show's, or not an episode at all.
pub fn after(current: &Item, beside: &[Item]) -> Option<Item> {
    let episode = episodes::parse(&current.name)?;
    let films: Vec<&Item> = beside.iter().filter(|item| !item.folder && item.key != current.key).collect();
    let names: Vec<&str> = films.iter().map(|item| item.name.as_str()).collect();
    let name = episodes::pick_next(&episode, &names)?;
    let mut next = films.into_iter().find(|item| item.name == name)?.clone();
    // Watched to the end before (a server that says how far): from its beginning again.
    if next.progress.is_some_and(|through| through >= 0.97) {
        next.resume = 0.0;
    }
    Some(next)
}

/// The episode after `current`, asked of its source: the folder it is in is listed, unless
/// `held` is already that folder's listing (it was opened from that folder's screen). Comes
/// back with the listing it used, for the episode after that one. Nothing where the source
/// cannot say what folder the film is in. Waits on the network: never on the drawing thread.
pub fn find(source: &dyn Source, current: &Item, held: Option<Listing>) -> (Option<Item>, Option<Listing>) {
    // Not an episode: nothing to list.
    if episodes::parse(&current.name).is_none() {
        return (None, None);
    }
    let Some(folder) = source.folder_of(current) else {
        log::info!("{}: {} cannot say what folder it is in; no next episode", current.title, source.label());
        return (None, None);
    };
    let beside = match held {
        Some((key, items)) if key == folder.key => items,
        _ => match source.list(Some(&folder)) {
            Ok(items) => items,
            Err(e) => {
                log::warn!("{}: its folder could not be listed for the next episode: {e}", current.title);
                return (None, None);
            }
        },
    };
    let next = after(current, &beside);
    log::info!("{}: the next episode is {}", current.title, next.as_ref().map_or("none", |n| n.name.as_str()));
    (next, Some((folder.key, beside)))
}

/// How long before the end the next episode is offered: the last 3 % of the film, at least the
/// last 20 seconds and at most the last five minutes -- about where the credits are.
pub fn offered_for(duration: f64) -> f64 {
    (duration * 0.03).clamp(20.0, 300.0)
}

/// Whether `position` is that near the end of a film whose length is known (not judged from
/// its size).
pub fn near_end(position: f64, duration: f64, guessed: bool) -> bool {
    duration > 0.0 && !guessed && position >= duration - offered_for(duration)
}

/// Where an episode played next starts: where it was left, unless that was at its credits or
/// past them, when it starts from its beginning.
pub fn start_at(resume: f64, duration: f64) -> f64 {
    if duration > 0.0 && resume >= duration - offered_for(duration) {
        0.0
    } else {
        resume
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::local::Local;

    fn film(name: &str) -> Item {
        Item { key: format!("/shows/{name}"), title: crate::library::title(name), name: name.into(), ..Item::default() }
    }

    fn folder(names: &[&str]) -> Vec<Item> {
        names.iter().map(|name| film(name)).collect()
    }

    #[test]
    fn the_next_in_the_folder_is_the_next_episode_by_number_not_by_text() {
        let beside = folder(&["Show s01e10.mkv", "Show s01e09.mkv", "Show s01e02.mkv", "Show s02e01.mkv", "Show s01e01.mkv"]);
        assert_eq!(after(&film("Show s01e01.mkv"), &beside).unwrap().name, "Show s01e02.mkv");
        assert_eq!(after(&film("Show s01e09.mkv"), &beside).unwrap().name, "Show s01e10.mkv");
        // On into the next season.
        assert_eq!(after(&film("Show s01e10.mkv"), &beside).unwrap().name, "Show s02e01.mkv");
    }

    #[test]
    fn the_last_episode_has_none_after_it() {
        let beside = folder(&["Show s01e01.mp4", "Show s01e02.mp4", "Show s01e03.mp4"]);
        assert_eq!(after(&film("Show s01e03.mp4"), &beside), None);
    }

    #[test]
    fn another_show_in_the_same_folder_is_not_played_next() {
        let beside = folder(&["Show s01e01.mp4", "Other Show s01e02.mp4", "Show s01e03.mp4", "Extras.mp4"]);
        assert_eq!(after(&film("Show s01e01.mp4"), &beside).unwrap().name, "Show s01e03.mp4");
        let beside = folder(&["Show s01e01.mp4", "Other Show s01e02.mp4"]);
        assert_eq!(after(&film("Show s01e01.mp4"), &beside), None);
    }

    #[test]
    fn a_film_that_is_no_episode_has_nothing_after_it() {
        let beside = folder(&["Arrival.mkv", "Show s01e01.mkv", "Show s01e02.mkv"]);
        assert_eq!(after(&film("Arrival.mkv"), &beside), None);
    }

    #[test]
    fn a_folder_is_not_an_episode_and_one_watched_to_its_end_starts_again() {
        let mut beside = folder(&["Show s01e02", "Show s01e03.mkv"]);
        beside[0].folder = true;
        beside[1].resume = 1500.0;
        beside[1].progress = Some(0.99);
        let next = after(&film("Show s01e01.mkv"), &beside).unwrap();
        assert_eq!((next.name.as_str(), next.resume), ("Show s01e03.mkv", 0.0));
    }

    #[test]
    fn the_button_is_offered_in_the_last_three_percent_within_bounds() {
        // An hour: the last 108 seconds.
        assert!(!near_end(3600.0 - 109.0, 3600.0, false));
        assert!(near_end(3600.0 - 107.0, 3600.0, false));
        // Short: at least the last 20 seconds. Long: at most the last five minutes.
        assert_eq!(offered_for(120.0), 20.0);
        assert_eq!(offered_for(4.0 * 3600.0), 300.0);
        // Not when the length is not known, or only judged.
        assert!(!near_end(100.0, 0.0, false));
        assert!(!near_end(3590.0, 3600.0, true));
    }

    #[test]
    fn a_place_kept_at_the_credits_starts_the_episode_from_its_beginning() {
        assert_eq!(start_at(600.0, 2400.0), 600.0);
        assert_eq!(start_at(2380.0, 2400.0), 0.0);
        // Its length not known yet: where it was left.
        assert_eq!(start_at(600.0, 0.0), 600.0);
    }

    /// The whole of it over this machine's folders, as SpatiReel asks: the folder listed, or the
    /// listing held used instead.
    #[test]
    fn a_folder_on_this_machine_gives_the_next_episode() {
        let dir = std::env::temp_dir().join(format!("spatireel-next-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["Show s01e01.mp4", "Show s01e02.mp4", "Show s01e03.mp4", "Show s01e02.srt", "Notes.txt"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        let local = Local::new(dir.clone());
        let (next, listing) = find(&local, &Local::item(&dir.join("Show s01e01.mp4")), None);
        let next = next.unwrap();
        assert_eq!(next.name, "Show s01e02.mp4");
        assert_eq!(next.path.as_deref(), Some(dir.join("Show s01e02.mp4").as_path()));
        // The listing it used is the folder's, and given again it is not listed: a file gone
        // since is still in it.
        let listing = listing.unwrap();
        assert_eq!(listing.0, dir.to_string_lossy());
        std::fs::remove_file(dir.join("Show s01e03.mp4")).unwrap();
        let (held, _) = find(&local, &next, Some(listing));
        assert_eq!(held.unwrap().name, "Show s01e03.mp4");
        // Listed afresh, it is gone, and the second is the last.
        assert_eq!(find(&local, &next, None).0, None);
        // An address with no folder has no next.
        let mut address = Local::item(std::path::Path::new("http://example.com/Show s01e01.mp4"));
        address.path = None;
        assert_eq!(find(&local, &address, None).0, None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
