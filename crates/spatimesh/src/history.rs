//! A profile's history: the pages it showed, on every machine.
//!
//! `apps/spatimesh/profiles/<profile>/history/<machine>-<year>-<month>.json`: each machine
//! writes only its own files, a month to a file, so the synchroniser never has two versions of
//! one to choose between. A machine's files older than a year are removed by that machine.
//! Every machine reads all of them: a page visited on one is a visited link on the others.
//!
//! What the wearer takes out of the history is said the same way: each machine keeps a list of
//! its own, `forgotten-<machine>.json` (an address, or everything, and up to when), and every
//! machine leaves out of what it shows whatever any machine's list says. This machine's own
//! files are also rewritten without it.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::store::{new_id, now_ms, Store};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Visit {
    pub url: String,
    #[serde(default)]
    pub title: String,
    /// Milliseconds since 1970.
    pub at: u64,
}

/// This machine's name for its own files, made once and kept on this machine only.
pub fn machine(store: &Store) -> String {
    if let Some(id) = store.read_local::<String>("machine.json") {
        return id;
    }
    let id = new_id();
    let _ = store.write_local("machine.json", &id);
    id
}

fn folder(profile: &str) -> String {
    format!("profiles/{profile}/history")
}

/// The year and month of a time, in UTC: "2026-10".
fn month(at_ms: u64) -> String {
    // Days since 1970 to a civil date (Howard Hinnant's algorithm).
    let z = (at_ms / 86_400_000) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}")
}

/// Whether an address is worth remembering as a visit.
fn remembered(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// One address's visits, on every machine: what the history page and the address bar show.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub url: String,
    /// The title it was last seen with.
    pub title: String,
    /// The newest visit, in milliseconds since 1970.
    pub last: u64,
    pub visits: u32,
}

/// What was forgotten: an address (none for everything), and the visits up to when.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Forgotten {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    /// Only visits after this too, for "the last hour" (milliseconds since 1970; 0 for all).
    #[serde(default)]
    from: u64,
    until: u64,
}

impl Forgotten {
    fn hides(&self, visit: &Visit) -> bool {
        visit.at <= self.until && visit.at >= self.from && self.url.as_deref().map_or(true, |u| u == visit.url)
    }
}

pub struct History {
    profile: String,
    machine: String,
    /// This month's file of this machine's, as last written.
    name: String,
    visits: Vec<Visit>,
    /// Every machine's visits, newest first, with what was forgotten left out: read when first
    /// asked for, and again after anything changed it.
    everything: Option<Vec<Visit>>,
}

impl History {
    pub fn load(store: &Store, profile: &str) -> History {
        let machine = machine(store);
        let name = format!("{machine}-{}", month(now_ms()));
        let visits = store.read(&format!("{}/{name}.json", folder(profile))).unwrap_or_default();
        let history = History { profile: profile.to_string(), machine, name, visits, everything: None };
        history.forget_old(store);
        history
    }

    /// This machine's files of more than a year ago, removed.
    fn forget_old(&self, store: &Store) {
        let year_ago = month(now_ms().saturating_sub(366 * 86_400_000));
        let mine = format!("{}-", self.machine);
        for name in store.list(&folder(&self.profile)) {
            if let Some(m) = name.strip_prefix(&mine) {
                if m < year_ago.as_str() {
                    store.remove(&format!("{}/{name}.json", folder(&self.profile)));
                }
            }
        }
    }

    /// A page shown: added, or, if it is the page just added, its title brought up to date.
    pub fn visited(&mut self, store: &Store, url: &str, title: &str) {
        if !remembered(url) {
            return;
        }
        let now = now_ms();
        let name = format!("{}-{}", self.machine, month(now));
        if name != self.name {
            self.name = name;
            self.visits.clear();
        }
        match self.visits.last_mut() {
            Some(last) if last.url == url && now.saturating_sub(last.at) < 30 * 60_000 => {
                if last.title == title || title.is_empty() {
                    return;
                }
                last.title = title.to_string();
            }
            _ => {
                let visit = Visit { url: url.to_string(), title: title.to_string(), at: now };
                if let Some(everything) = self.everything.as_mut() {
                    everything.insert(0, visit.clone());
                }
                self.visits.push(visit);
            }
        }
        if let (Some(everything), Some(last)) = (self.everything.as_mut(), self.visits.last()) {
            if let Some(first) = everything.first_mut().filter(|v| v.url == last.url) {
                first.title = last.title.clone();
            }
        }
        if let Err(e) = store.write(&format!("{}/{}.json", folder(&self.profile), self.name), &self.visits) {
            log::warn!("history not written: {e}");
        }
    }

    /// Every address visited on any machine, read from all the files.
    pub fn all_urls(&mut self, store: &Store) -> HashSet<String> {
        self.visits(store).iter().map(|v| v.url.clone()).collect()
    }

    /// Something another machine brought: read again when next asked for.
    pub fn stale(&mut self) {
        self.everything = None;
    }

    fn forgotten(&self, store: &Store) -> Vec<Forgotten> {
        store
            .list(&folder(&self.profile))
            .into_iter()
            .filter(|n| n.starts_with("forgotten-"))
            .flat_map(|n| store.read::<Vec<Forgotten>>(&format!("{}/{n}.json", folder(&self.profile))).unwrap_or_default())
            .collect()
    }

    /// Every visit on every machine, newest first, less what was forgotten.
    pub fn visits(&mut self, store: &Store) -> &[Visit] {
        if self.everything.is_none() {
            let forgotten = self.forgotten(store);
            let mut all: Vec<Visit> = Vec::new();
            for name in store.list(&folder(&self.profile)).into_iter().filter(|n| !n.starts_with("forgotten-")) {
                let visits: Vec<Visit> = store.read(&format!("{}/{name}.json", folder(&self.profile))).unwrap_or_default();
                all.extend(visits.into_iter().filter(|v| !forgotten.iter().any(|f| f.hides(v))));
            }
            all.sort_by(|a, b| b.at.cmp(&a.at));
            self.everything = Some(all);
        }
        self.everything.as_deref().unwrap_or_default()
    }

    /// Each address once, with how often and when it was last visited: newest first.
    pub fn entries(&mut self, store: &Store) -> Vec<Entry> {
        let mut index: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        let mut entries: Vec<Entry> = Vec::new();
        for v in self.visits(store) {
            match index.get(v.url.as_str()) {
                Some(&i) => {
                    entries[i].visits += 1;
                    if entries[i].title.is_empty() {
                        entries[i].title = v.title.clone();
                    }
                }
                None => {
                    index.insert(&v.url, entries.len());
                    entries.push(Entry { url: v.url.clone(), title: v.title.clone(), last: v.at, visits: 1 });
                }
            }
        }
        entries
    }

    /// Take an address out of the history on every machine (`None`: everything), its visits
    /// from `from` on (0 for all of them).
    pub fn forget(&mut self, store: &Store, url: Option<&str>, from: u64) {
        let forgotten = Forgotten { url: url.map(str::to_string), from, until: now_ms() };
        let name = format!("{}/forgotten-{}.json", folder(&self.profile), self.machine);
        let mut mine: Vec<Forgotten> = store.read(&name).unwrap_or_default();
        // Forgetting everything makes every earlier entry needless.
        if forgotten.url.is_none() && from == 0 {
            mine.clear();
        }
        mine.push(forgotten.clone());
        if let Err(e) = store.write(&name, &mine) {
            log::warn!("history: what was forgotten could not be written: {e}");
        }
        // This machine's own files, without it.
        for file in store.list(&folder(&self.profile)).into_iter().filter(|n| n.starts_with(&format!("{}-", self.machine))) {
            let path = format!("{}/{file}.json", folder(&self.profile));
            let visits: Vec<Visit> = store.read(&path).unwrap_or_default();
            let kept: Vec<Visit> = visits.iter().filter(|v| !forgotten.hides(v)).cloned().collect();
            if kept.len() != visits.len() {
                let _ = if kept.is_empty() { Ok(store.remove(&path)) } else { store.write(&path, &kept) };
            }
        }
        self.visits.retain(|v| !forgotten.hides(v));
        self.everything = None;
    }

    /// The hosts this machine visited lately, newest first, at most `count`.
    pub fn recent_hosts(&self, count: usize) -> Vec<String> {
        let mut hosts: Vec<String> = Vec::new();
        for v in self.visits.iter().rev() {
            let host = v.url.split_once("://").map_or("", |(_, r)| r).split(['/', '?', '#', ':']).next().unwrap_or("").to_string();
            if !host.is_empty() && !hosts.contains(&host) {
                hosts.push(host);
                if hosts.len() == count {
                    break;
                }
            }
        }
        hosts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Scratch;

    #[test]
    fn months() {
        assert_eq!(month(0), "1970-01");
        assert_eq!(month(1_760_000_000_000), "2025-10");
        assert_eq!(month(951_782_400_000), "2000-02"); // 29 February 2000
    }

    #[test]
    fn a_visit_on_one_machine_is_seen_on_the_other() {
        let scratch = Scratch::new("history");
        let a = Store::at(&scratch.0.join("a"));
        let b = Store::at(&scratch.0.join("b"));
        std::fs::create_dir_all(scratch.0.join("a/apps")).unwrap();
        std::fs::create_dir_all(scratch.0.join("b")).unwrap();
        std::os::unix::fs::symlink(scratch.0.join("a/apps"), scratch.0.join("b/apps")).unwrap();
        let mut on_a = History::load(&a, "p");
        let mut on_b = History::load(&b, "p");
        on_a.visited(&a, "https://example.com/x", "");
        on_a.visited(&a, "https://example.com/x", "X");
        on_a.visited(&a, "about:blank", "");
        on_b.visited(&b, "https://other.org/", "O");
        let urls = on_b.all_urls(&b);
        assert!(urls.contains("https://example.com/x") && urls.contains("https://other.org/"));
        assert_eq!(urls.len(), 2);
        assert_eq!(on_a.visits.len(), 1, "a title coming later is the same visit");
        assert_eq!(on_a.visits[0].title, "X");
        assert_eq!(a.list("profiles/p/history").len(), 2, "a file each");
        assert_eq!(on_a.recent_hosts(5), vec!["example.com".to_string()]);
    }

    #[test]
    fn entries_and_forgetting_reach_every_machine() {
        let scratch = Scratch::new("history-forget");
        let a = Store::at(&scratch.0.join("a"));
        let b = Store::at(&scratch.0.join("b"));
        std::fs::create_dir_all(scratch.0.join("a/apps")).unwrap();
        std::fs::create_dir_all(scratch.0.join("b")).unwrap();
        std::os::unix::fs::symlink(scratch.0.join("a/apps"), scratch.0.join("b/apps")).unwrap();
        let mut on_a = History::load(&a, "p");
        let mut on_b = History::load(&b, "p");
        on_a.visited(&a, "https://one.org/", "One");
        on_b.visited(&b, "https://two.org/", "Two");
        on_b.visited(&b, "https://one.org/", "One again");
        let entries = on_a.entries(&a);
        assert_eq!(entries.len(), 2);
        let one = entries.iter().find(|e| e.url == "https://one.org/").unwrap();
        assert_eq!(one.visits, 2);
        // A forgets one.org: B's visit to it goes from what B shows too, though B's file is B's.
        on_a.forget(&a, Some("https://one.org/"), 0);
        on_b.stale();
        assert_eq!(on_b.entries(&b).iter().map(|e| e.url.as_str()).collect::<Vec<_>>(), vec!["https://two.org/"]);
        assert!(!on_a.all_urls(&a).contains("https://one.org/"));
        // A visit after the forgetting is shown again.
        std::thread::sleep(std::time::Duration::from_millis(5));
        on_a.visited(&a, "https://one.org/", "One");
        on_b.stale();
        assert_eq!(on_b.entries(&b).len(), 2);
        // Everything, from B.
        on_b.forget(&b, None, 0);
        on_a.stale();
        assert!(on_a.entries(&a).is_empty());
    }
}
