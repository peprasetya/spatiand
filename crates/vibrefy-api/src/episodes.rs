//! Series auto-play, ported from `parseEpisode` / `pickNextEpisode` in `scriptlib.js`
//! by way of `app/src/main/java/id/prasetya/vibrefy/app/player/EpisodeChain.kt`.
//!
//! Pure functions, so the ordering rule - the one that has to put `s4e9` before `s4e10`
//! rather than sorting them as text - can be tested without a player.

/// A name like "Show s03e02.mp4" is an episode. The text before the marker identifies
/// which series it belongs to, so files from different shows in one folder never chain
/// into each other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Episode {
    pub season: u32,
    pub episode: u32,
    pub prefix: String,
}

/// `s(\d+)\s*e(\d+)`, case-insensitive, hand-rolled.
///
/// The Kotlin and JavaScript sides both express this as a regular expression; here it is
/// a scan, to keep this crate free of a regex engine for one pattern. The behaviour has
/// to stay identical, which is what the ported tests are for - in particular that the
/// *first* match wins, so "Show 2 s01e01" takes the marker and not the title's digits.
pub fn parse(name: &str) -> Option<Episode> {
    let bytes = name.as_bytes();
    let mut at = 0usize;
    while at < bytes.len() {
        if !bytes[at].eq_ignore_ascii_case(&b's') {
            at += 1;
            continue;
        }
        if let Some(found) = match_at(bytes, at) {
            let (season, episode) = found;
            return Some(Episode {
                season,
                episode,
                prefix: name[..at].to_lowercase(),
            });
        }
        at += 1;
    }
    None
}

/// `s<digits><spaces?>e<digits>` starting exactly at `at`, or nothing.
fn match_at(bytes: &[u8], at: usize) -> Option<(u32, u32)> {
    let mut i = at + 1;
    let (season, next) = digits(bytes, i)?;
    i = next;
    // `\s*` in the original. Only ASCII whitespace can appear between the two markers in
    // any naming convention this has to survive.
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() || !bytes[i].eq_ignore_ascii_case(&b'e') {
        return None;
    }
    let (episode, _) = digits(bytes, i + 1)?;
    Some((season, episode))
}

/// One or more ASCII digits at `from`, and where they ended. Saturating, because a name
/// with forty digits in it is malformed rather than a reason to panic.
fn digits(bytes: &[u8], from: usize) -> Option<(u32, usize)> {
    let mut i = from;
    let mut value: u32 = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        value = value
            .saturating_mul(10)
            .saturating_add(u32::from(bytes[i] - b'0'));
        i += 1;
    }
    if i == from {
        None
    } else {
        Some((value, i))
    }
}

/// The lowest episode strictly after `current` among `names`, or `None` when it ends here.
pub fn pick_next<'a>(current: &Episode, names: &[&'a str]) -> Option<&'a str> {
    let mut best: Option<(Episode, &'a str)> = None;
    for name in names {
        let Some(candidate) = parse(name) else { continue };
        if candidate.prefix != current.prefix {
            continue;
        }
        if (candidate.season, candidate.episode) <= (current.season, current.episode) {
            continue;
        }
        let better = match &best {
            None => true,
            Some((incumbent, _)) => {
                (candidate.season, candidate.episode) < (incumbent.season, incumbent.episode)
            }
        };
        if better {
            best = Some((candidate, name));
        }
    }
    best.map(|(_, name)| name)
}

/// Swaps the file name on a library path, keeping the folder. Every playback URL ends
/// with this same library-relative path, so one substitution moves stream, subtitles and
/// progress key together - the trick `startNextEpisode` uses in the web player.
pub fn sibling_path(path: &str, sibling_name: &str) -> String {
    match path.rfind('/') {
        None => sibling_name.to_string(),
        Some(slash) => format!("{}{}", &path[..=slash], sibling_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_season_and_episode_with_the_series_prefix() {
        let episode = parse("Some Show s03e02.mp4").expect("an episode");
        assert_eq!(3, episode.season);
        assert_eq!(2, episode.episode);
        assert_eq!("some show ", episode.prefix);
    }

    #[test]
    fn accepts_loose_spacing_and_upper_case() {
        let episode = parse("Show S4 E9.mp4").expect("an episode");
        assert_eq!(4, episode.season);
        assert_eq!(9, episode.episode);
    }

    #[test]
    fn a_film_is_not_an_episode() {
        assert_eq!(None, parse("Blade Runner 2049.mp4"));
    }

    #[test]
    fn orders_numerically_so_s4e9_comes_before_s4e10() {
        let current = parse("Show s4e9.mp4").unwrap();
        let names = ["Show s4e10.mp4", "Show s4e11.mp4", "Show s4e9.mp4"];
        assert_eq!(Some("Show s4e10.mp4"), pick_next(&current, &names));
    }

    #[test]
    fn rolls_into_the_next_season_when_the_current_one_ends() {
        let current = parse("Show s1e10.mp4").unwrap();
        let names = ["Show s2e01.mp4", "Show s1e10.mp4", "Show s3e01.mp4"];
        assert_eq!(Some("Show s2e01.mp4"), pick_next(&current, &names));
    }

    #[test]
    fn never_chains_into_a_different_series_in_the_same_folder() {
        let current = parse("Show A s1e01.mp4").unwrap();
        let names = ["Show B s1e02.mp4", "Show A s1e01.mp4"];
        assert_eq!(None, pick_next(&current, &names));
    }

    #[test]
    fn stops_at_the_last_episode() {
        let current = parse("Show s2e08.mp4").unwrap();
        assert_eq!(
            None,
            pick_next(&current, &["Show s2e07.mp4", "Show s2e08.mp4"])
        );
    }

    #[test]
    fn swaps_only_the_file_name_on_a_library_path() {
        assert_eq!(
            "Series/Show/Season 2/Show s2e02.mp4",
            sibling_path("Series/Show/Season 2/Show s2e01.mp4", "Show s2e02.mp4")
        );
    }

    #[test]
    fn handles_a_file_sitting_at_the_library_root() {
        assert_eq!(
            "Show s1e02.mp4",
            sibling_path("Show s1e01.mp4", "Show s1e02.mp4")
        );
    }

    /// Not in the Kotlin suite. The hand-rolled scan has a failure mode the regex did not:
    /// a false start on an 's' that is not the marker must not stop the search.
    #[test]
    fn keeps_looking_past_an_s_that_leads_nowhere() {
        let episode = parse("Silo s02e05.mp4").expect("an episode");
        assert_eq!((2, 5), (episode.season, episode.episode));
        assert_eq!("silo ", episode.prefix);
    }

    /// The scan walks bytes and slices the name where it matched, which would panic if it
    /// ever stopped inside a multibyte character. It cannot - it only stops on an ASCII
    /// `s`, and no byte of a multibyte UTF-8 sequence is ASCII - and this keeps it so.
    /// Titles in the library are not all Latin.
    #[test]
    fn a_title_in_any_script_keeps_its_prefix_whole() {
        let episode = parse("進撃の巨人 S04E09.mp4").expect("an episode");
        assert_eq!((4, 9), (episode.season, episode.episode));
        assert_eq!("進撃の巨人 ", episode.prefix);

        let episode = parse("Ślepnąc od świateł s01e03.mp4").expect("an episode");
        assert_eq!((1, 3), (episode.season, episode.episode));
        assert_eq!("ślepnąc od świateł ", episode.prefix);

        assert!(parse("사랑의 불시착.mp4").is_none());
    }
}
