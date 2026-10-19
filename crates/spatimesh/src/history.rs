//! A profile's history: the pages it showed, on every machine.
//!
//! `apps/spatimesh/profiles/<profile>/history/<machine>-<year>-<month>.json`: each machine
//! writes only its own files, a month to a file, so the synchroniser never has two versions of
//! one to choose between. A machine's files older than a year are removed by that machine.
//! Every machine reads all of them: a page visited on one is a visited link on the others.

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

pub struct History {
    profile: String,
    machine: String,
    /// This month's file of this machine's, as last written.
    name: String,
    visits: Vec<Visit>,
}

impl History {
    pub fn load(store: &Store, profile: &str) -> History {
        let machine = machine(store);
        let name = format!("{machine}-{}", month(now_ms()));
        let visits = store.read(&format!("{}/{name}.json", folder(profile))).unwrap_or_default();
        let history = History { profile: profile.to_string(), machine, name, visits };
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
            _ => self.visits.push(Visit { url: url.to_string(), title: title.to_string(), at: now }),
        }
        if let Err(e) = store.write(&format!("{}/{}.json", folder(&self.profile), self.name), &self.visits) {
            log::warn!("history not written: {e}");
        }
    }

    /// Every address visited on any machine, read from all the files.
    pub fn all_urls(&self, store: &Store) -> HashSet<String> {
        let mut urls = HashSet::new();
        for name in store.list(&folder(&self.profile)) {
            let visits: Vec<Visit> = store.read(&format!("{}/{name}.json", folder(&self.profile))).unwrap_or_default();
            urls.extend(visits.into_iter().map(|v| v.url));
        }
        urls
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
}
