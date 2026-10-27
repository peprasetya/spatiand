//! The window's side of what a profile keeps on every machine beyond its settings: its cookies
//! (`spatimesh::cookies`) and its history (`spatimesh::history`). See docs/spatimesh.md,
//! "Logins and history on every machine".

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

use spatimesh::cookies::CookieSync;
use spatimesh::engine::{Cookie, TabId};
use spatimesh::history::History;
use spatimesh::profiles::PRIVATE;
use spatimesh::wire::ToHost;

use super::Browser;

/// How long after a page has loaded its cookies are looked at: a login's redirects done.
const AFTER_LOAD: Duration = Duration::from_secs(3);
/// How often they are looked at anyway, while the profile has an engine.
const EVERY: Duration = Duration::from_secs(60);

/// One profile's, while its engine runs.
pub(super) struct Kept {
    cookies: CookieSync,
    pub(super) history: History,
    /// When to ask the engine for its cookies next.
    due: Option<Instant>,
    asked: Instant,
    /// Each ask not yet answered, by the number of cookie changes sent before it: an answer to
    /// an ask made before the engine was given cookies does not have them yet, and is not
    /// taken for this machine having removed them.
    asks: VecDeque<u64>,
    sent: u64,
    /// The addresses the engine has been told are visited.
    visited: HashSet<String>,
}

fn now_s() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

impl Browser {
    /// A profile's engine has started: its cookies are looked at soon, and it is told what
    /// every machine visited.
    pub(super) fn kept_started(&mut self, profile: &str) {
        if profile == PRIVATE {
            return;
        }
        let mut kept = Kept {
            cookies: CookieSync::load(&self.store, profile),
            history: History::load(&self.store, profile),
            due: Some(Instant::now()),
            asked: Instant::now(),
            asks: VecDeque::new(),
            sent: 0,
            visited: HashSet::new(),
        };
        let urls: Vec<String> = kept.history.all_urls(&self.store).into_iter().collect();
        kept.visited.extend(urls.iter().cloned());
        self.kept.insert(profile.to_string(), kept);
        if !urls.is_empty() {
            self.tell_profile(Some(profile), ToHost::Visited { urls });
        }
    }

    pub(super) fn kept_ended(&mut self, profile: &str) {
        self.kept.remove(profile);
    }

    /// A tab's page has loaded, or its title came after: remembered, and its cookies looked at.
    pub(super) fn kept_page(&mut self, tab: TabId, loaded: bool) {
        let Some(t) = self.tabs.tab(tab).filter(|t| t.inspects.is_none()) else { return };
        let (profile, url, title) = (t.profile.clone(), t.url.clone(), t.title.clone());
        let Some(kept) = self.kept.get_mut(&profile) else { return };
        kept.history.visited(&self.store, &url, &title);
        if loaded {
            kept.due = Some(Instant::now() + AFTER_LOAD);
        }
        // Engines that keep no visited links of their own (WebKit on a Mac) are told this one.
        if (url.starts_with("https://") || url.starts_with("http://")) && kept.visited.insert(url.clone()) {
            self.tell_profile(Some(&profile), ToHost::Visited { urls: vec![url] });
        }
    }

    /// Pages were taken out of the profile's history: its engine forgets every visited link and
    /// is told again what is left.
    pub(super) fn kept_forgot(&mut self, profile: &str) {
        let Some(kept) = self.kept.get_mut(profile) else { return };
        let urls: Vec<String> = kept.history.all_urls(&self.store).into_iter().collect();
        kept.visited = urls.iter().cloned().collect();
        self.tell_profile(Some(profile), ToHost::ForgetVisited);
        if !urls.is_empty() {
            self.tell_profile(Some(profile), ToHost::Visited { urls });
        }
    }

    /// The engine's cookies, or what Android's sends to each address it was asked about.
    pub(super) fn kept_cookies(&mut self, profile: &str, jar: Option<Vec<Cookie>>, sent: Option<Vec<(String, String)>>) {
        let Some(kept) = self.kept.get_mut(profile) else { return };
        let asked_after = kept.asks.pop_front();
        if asked_after != Some(kept.sent) {
            // Asked before the engine was last given cookies: ask again.
            kept.due = Some(Instant::now());
            return;
        }
        let now = now_s();
        let jar = match (jar, sent) {
            (Some(jar), _) => jar,
            (None, Some(sent)) => kept.cookies.jar_from_sent(&sent, now),
            (None, None) => return,
        };
        let told = kept.cookies.merge(&self.store, jar, now);
        if !told.is_empty() {
            log::info!("{profile}: cookies from other machines: {} set, {} removed", told.set.len(), told.remove.len());
            kept.sent += 1;
            self.tell_profile(Some(profile), ToHost::SetCookies { set: told.set, remove: told.remove });
        }
    }

    /// The synchroniser brought something: cookies looked at again, and new visits told.
    pub(super) fn kept_synced(&mut self) {
        let profiles: Vec<String> = self.kept.keys().cloned().collect();
        for profile in profiles {
            let Some(kept) = self.kept.get_mut(&profile) else { continue };
            kept.due = Some(Instant::now());
            kept.history.stale();
            let all = kept.history.all_urls(&self.store);
            // Pages another machine took out of the history: forgotten here too.
            if kept.visited.iter().any(|u| !all.contains(u)) {
                self.kept_forgot(&profile);
                continue;
            }
            let new: Vec<String> = all.into_iter().filter(|u| !kept.visited.contains(u)).collect();
            if !new.is_empty() {
                kept.visited.extend(new.iter().cloned());
                self.tell_profile(Some(&profile), ToHost::Visited { urls: new });
            }
        }
    }

    /// Asks each engine for its cookies when it is time.
    pub(super) fn kept_tick(&mut self) {
        let now = Instant::now();
        let mut asks = Vec::new();
        for (profile, kept) in &mut self.kept {
            if kept.asked.elapsed() >= EVERY {
                kept.due.get_or_insert(now);
            }
            // One ask at a time.
            if kept.due.is_some_and(|d| d <= now) && kept.asks.is_empty() {
                kept.due = None;
                kept.asked = now;
                kept.asks.push_back(kept.sent);
                asks.push((profile.clone(), kept.cookies.addresses(&kept.history.recent_hosts(200))));
            } else if !kept.asks.is_empty() && kept.asked.elapsed() >= EVERY {
                // Never answered (an engine that does not know the question): asked again.
                kept.asks.clear();
            }
        }
        for (profile, addresses) in asks {
            self.tell_profile(Some(&profile), ToHost::AskCookies { addresses });
        }
    }
}
