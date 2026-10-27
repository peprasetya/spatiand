//! What the address bar suggests while something is typed into it: pages from the history, the
//! bookmarks and the open tabs, and what was typed itself -- as an address to go to or words to
//! search for -- with the likeliest page's address finished inline, as browsers do.
//!
//! A page is suggested when every word typed is found in it: the start of its address (less
//! `https://` and `www.`) counts most, the start of a word of its title or of a part of its
//! address next, anywhere at all least. Among pages that match alike, the ones visited often
//! and lately come first, and bookmarks before history.

use crate::address::{self, SearchEngine, Typed};
use crate::bookmarks::Bookmark;
use crate::engine::TabId;
use crate::history::Entry;

/// What a suggestion is.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// What was typed, as an address.
    Go,
    /// What was typed, as words for the search engine.
    Search,
    History,
    Bookmark,
    /// A tab open already: going to it brings it to the front.
    Tab(TabId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub kind: Kind,
    /// Where it goes.
    pub url: String,
    /// What it is called: a page's title, or the words searched for.
    pub title: String,
}

/// An address finished in the bar: what the bar then says, and where Enter goes.
#[derive(Debug, Clone, PartialEq)]
pub struct Inline {
    pub text: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Suggestions {
    pub list: Vec<Suggestion>,
    pub inline: Option<Inline>,
}

/// An address as it is typed: no scheme, no `www.`, and no slash after a bare host.
pub fn stripped(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    match rest.find('/') {
        Some(i) if i + 1 == rest.len() => &rest[..i],
        _ => rest,
    }
}

fn host(stripped: &str) -> &str {
    stripped.split(['/', '?', '#']).next().unwrap_or(stripped)
}

/// How well one typed word matches a page: 0 for not at all.
fn word_score(word: &str, address: &str, title: &str) -> f32 {
    if address.starts_with(word) {
        return 100.0;
    }
    let starts_a_part = address.split(['/', '.', '-', '_', '?', '&', '=', '#']).any(|part| part.starts_with(word));
    let starts_a_title_word = title.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(word));
    if starts_a_part || starts_a_title_word {
        return 50.0;
    }
    if address.contains(word) || title.contains(word) {
        return 15.0;
    }
    0.0
}

/// How much a page has been wanted: often, and lately.
fn frecency(visits: u32, last_ms: u64, now_ms: u64) -> f32 {
    let days = now_ms.saturating_sub(last_ms) as f32 / 86_400_000.0;
    (visits as f32).ln_1p() * 12.0 + 30.0 * (-days / 14.0).exp()
}

struct Candidate {
    kind: Kind,
    url: String,
    title: String,
    visits: u32,
    last: u64,
}

/// The suggestions for `typed`, at most `max` of them.
pub fn suggest(
    typed: &str,
    history: &[Entry],
    bookmarks: &[Bookmark],
    tabs: &[(TabId, String, String)],
    engine: &SearchEngine,
    now_ms: u64,
    max: usize,
) -> Suggestions {
    let text = typed.trim();
    if text.is_empty() {
        return Suggestions::default();
    }
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();

    let mut candidates: Vec<Candidate> = Vec::new();
    for b in bookmarks.iter().filter(|b| !b.is_folder()) {
        let url = b.url.clone().unwrap_or_default();
        let seen = history.iter().find(|e| e.url == url);
        candidates.push(Candidate { kind: Kind::Bookmark, url, title: b.title.clone(), visits: seen.map_or(0, |e| e.visits) + 3, last: seen.map_or(b.added, |e| e.last) });
    }
    for e in history {
        if !candidates.iter().any(|c| c.url == e.url) {
            candidates.push(Candidate { kind: Kind::History, url: e.url.clone(), title: e.title.clone(), visits: e.visits, last: e.last });
        }
    }
    for (id, url, title) in tabs {
        if url.starts_with("http") {
            candidates.push(Candidate { kind: Kind::Tab(*id), url: url.clone(), title: title.clone(), visits: 0, last: now_ms });
        }
    }

    let mut scored: Vec<(f32, Candidate)> = Vec::new();
    for c in candidates {
        let address = stripped(&c.url).to_lowercase();
        let title = c.title.to_lowercase();
        let mut total = 0.0;
        let mut all = true;
        for w in &words {
            let s = word_score(w, &address, &title);
            if s == 0.0 {
                all = false;
                break;
            }
            total += s;
        }
        if !all {
            continue;
        }
        // The address itself and no more: the likelier than any page under it.
        if address == lower {
            total += 40.0;
        }
        let bonus = if c.kind == Kind::Bookmark { 20.0 } else { 0.0 };
        scored.push((total + frecency(c.visits, c.last, now_ms) + bonus, c));
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));

    // Finished inline: an address the typing is the start of, never words with spaces.
    let mut inline = None;
    if !text.contains(char::is_whitespace) && !typed.ends_with(' ') {
        let best = scored
            .iter()
            .filter(|(_, c)| !matches!(c.kind, Kind::Tab(_)))
            .filter(|(_, c)| stripped(&c.url).to_lowercase().starts_with(&lower))
            .max_by(|a, b| frecency(a.1.visits, a.1.last, now_ms).total_cmp(&frecency(b.1.visits, b.1.last, now_ms)));
        if let Some((_, c)) = best {
            let page = stripped(&c.url);
            let site = host(page);
            // To the site while what is typed is within its name; to the page past that.
            let (finished, url) = if lower.len() <= site.len() {
                let scheme = c.url.split_once("://").map_or("https", |(s, _)| s);
                let www = if c.url.contains("://www.") { "www." } else { "" };
                (site.to_string(), format!("{scheme}://{www}{site}/"))
            } else {
                (page.to_string(), c.url.clone())
            };
            if finished.len() > text.len() || finished.to_lowercase() == lower {
                // What was typed stays as it was typed; the rest is the address's.
                let rest: String = finished.chars().skip(text.chars().count()).collect();
                inline = Some(Inline { text: format!("{text}{rest}"), url });
            }
        }
    }

    let mut list = Vec::new();
    match (&inline, address::understand(text)) {
        (Some(i), _) => {
            list.push(Suggestion { kind: Kind::Go, url: i.url.clone(), title: stripped(&i.url).to_string() });
            list.push(Suggestion { kind: Kind::Search, url: engine.url_for(text), title: text.to_string() });
        }
        (None, Some(Typed::Address(url))) => {
            list.push(Suggestion { kind: Kind::Go, url: url.clone(), title: stripped(&url).to_string() });
            list.push(Suggestion { kind: Kind::Search, url: engine.url_for(text), title: text.to_string() });
        }
        (None, Some(Typed::Search(words))) => list.push(Suggestion { kind: Kind::Search, url: engine.url_for(&words), title: words }),
        (None, None) => {}
    }
    for (_, c) in scored {
        if list.len() >= max {
            break;
        }
        let same = |s: &Suggestion| stripped(&s.url).trim_end_matches('/') == stripped(&c.url).trim_end_matches('/');
        // An open tab is suggested as itself even when it is also history.
        if matches!(c.kind, Kind::Tab(_)) {
            list.retain(|s| !(same(s) && matches!(s.kind, Kind::History)));
        } else if list.iter().any(same) {
            continue;
        }
        list.push(Suggestion { kind: c.kind, url: c.url, title: c.title });
    }
    list.truncate(max);
    Suggestions { list, inline }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400_000;
    const NOW: u64 = 1_800_000_000_000;

    fn entry(url: &str, title: &str, visits: u32, days_ago: u64) -> Entry {
        Entry { url: url.into(), title: title.into(), last: NOW - days_ago * DAY, visits }
    }

    fn ddg() -> SearchEngine {
        address::search_engines().remove(0)
    }

    #[test]
    fn addresses_as_typed() {
        assert_eq!(stripped("https://www.github.com/"), "github.com");
        assert_eq!(stripped("https://github.com/rust-lang/rust"), "github.com/rust-lang/rust");
        assert_eq!(stripped("http://localhost:8080/"), "localhost:8080");
    }

    #[test]
    fn the_start_of_a_site_is_finished_inline() {
        let history = vec![entry("https://github.com/rust-lang/rust", "Rust", 5, 1), entry("https://github.com/", "GitHub", 20, 0), entry("https://gitlab.com/", "GitLab", 1, 30)];
        let s = suggest("git", &history, &[], &[], &ddg(), NOW, 8);
        let inline = s.inline.unwrap();
        assert_eq!(inline.text, "github.com");
        assert_eq!(inline.url, "https://github.com/");
        assert_eq!(s.list[0].kind, Kind::Go);
        assert_eq!(s.list[1].kind, Kind::Search);
        assert!(s.list.iter().any(|x| x.url == "https://gitlab.com/"));
        // Typed case kept; past the host, the page itself.
        let s = suggest("GitHub.com/rust", &history, &[], &[], &ddg(), NOW, 8);
        assert_eq!(s.inline.unwrap(), Inline { text: "GitHub.com/rust-lang/rust".into(), url: "https://github.com/rust-lang/rust".into() });
    }

    #[test]
    fn words_find_titles_and_are_never_finished() {
        let history = vec![entry("https://doc.rust-lang.org/book/", "The Rust Programming Language", 3, 2), entry("https://example.com/", "Example", 1, 1)];
        let s = suggest("rust book", &history, &[], &[], &ddg(), NOW, 8);
        assert!(s.inline.is_none());
        assert_eq!(s.list[0].kind, Kind::Search);
        assert_eq!(s.list[0].url, "https://duckduckgo.com/?q=rust+book");
        assert_eq!(s.list[1].url, "https://doc.rust-lang.org/book/");
        assert_eq!(s.list.len(), 2, "example.com has neither word");
    }

    #[test]
    fn bookmarks_and_tabs() {
        let bookmark = Bookmark { id: "b".into(), parent: String::new(), title: "News".into(), url: Some("https://news.ycombinator.com/".into()), order: "m".into(), added: NOW - DAY };
        let tabs = vec![(7, "https://news.ycombinator.com/item?id=1".to_string(), "A story".to_string())];
        let s = suggest("news", &[entry("https://newsweek.com/", "Newsweek", 1, 100)], &[bookmark], &tabs, &ddg(), NOW, 8);
        assert_eq!(s.inline.as_ref().unwrap().url, "https://news.ycombinator.com/");
        assert!(s.list.iter().any(|x| x.kind == Kind::Tab(7)));
        let bm = s.list.iter().position(|x| x.kind == Kind::Bookmark);
        let other = s.list.iter().position(|x| x.url == "https://newsweek.com/");
        assert!(bm.is_none() || other.is_none() || bm < other);
    }

    #[test]
    fn nothing_typed_nothing_suggested() {
        assert_eq!(suggest("  ", &[entry("https://a.org/", "", 1, 0)], &[], &[], &ddg(), NOW, 8), Suggestions::default());
    }
}
