//! A profile's cookies, the same on every machine: log in on one, be logged in on the others.
//!
//! `apps/spatimesh/profiles/<profile>/cookies/<site>.json`, a file per site (the cookie's
//! domain without its leading dot), so that two machines using different sites while apart
//! both keep theirs; the synchroniser keeps the newest of one site's file.
//!
//! Each engine keeps its cookies in its own format, so the window asks its engine for all of
//! them now and then (after pages load, and when the synchroniser brought something) and
//! merges three ways, site by site: what the engine has, what the file says, and what the two
//! last agreed on (kept on this machine, `local/spatimesh/profiles/<profile>/cookies.json`). A
//! cookie changed here since is written out; one changed elsewhere is given to the engine;
//! one gone from either side is gone from both.
//!
//! Only cookies that outlive the session cross: an engine forgets its session cookies when it
//! ends, which must not log the other machines out. A cookie whose only change is its expiry
//! moved by less than a day is not a change, so that sites renewing theirs on every page do not
//! write a file every time.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::engine::Cookie;
use crate::store::Store;

const DAY: f64 = 86_400.0;

/// The site a cookie's file is named for.
pub fn site(domain: &str) -> String {
    domain.trim_start_matches('.').to_ascii_lowercase()
}

fn folder(profile: &str) -> String {
    format!("profiles/{profile}/cookies")
}

fn file(profile: &str, site: &str) -> String {
    format!("profiles/{profile}/cookies/{site}.json")
}

fn agreed_file(profile: &str) -> String {
    format!("profiles/{profile}/cookies.json")
}

/// A name the synchroniser and every file system take: a site is letters, digits, dots and
/// dashes already; anything else (an IPv6 host's colons) is made a dash.
fn safe(site: &str) -> bool {
    !site.is_empty() && !site.starts_with('.') && site.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SiteFile {
    cookies: Vec<Cookie>,
}

type Key = (String, String, String);

fn key(c: &Cookie) -> Key {
    (c.name.clone(), c.domain.clone(), c.path.clone())
}

/// The same cookie, for syncing: its expiry may have moved, by less than a day.
fn same(a: &Cookie, b: &Cookie) -> bool {
    let expires = match (a.expires, b.expires) {
        (Some(x), Some(y)) => (x - y).abs() < DAY,
        (x, y) => x == y,
    };
    expires && Cookie { expires: None, ..a.clone() } == Cookie { expires: None, ..b.clone() }
}

/// The cookies worth syncing, by site: those that outlive the session and are not over.
fn by_site(cookies: impl IntoIterator<Item = Cookie>, now: f64) -> BTreeMap<String, BTreeMap<Key, Cookie>> {
    let mut sites: BTreeMap<String, BTreeMap<Key, Cookie>> = BTreeMap::new();
    for c in cookies {
        if c.expires.is_none_or(|e| e <= now) || c.name.is_empty() {
            continue;
        }
        let s = site(&c.domain);
        if !safe(&s) {
            continue;
        }
        let c = Cookie { expires: c.expires.map(f64::round), ..c };
        sites.entry(s).or_default().insert(key(&c), c);
    }
    sites
}

/// What the engine is to be told: cookies to make or replace, and cookies to remove.
#[derive(Debug, Default, PartialEq)]
pub struct ForEngine {
    pub set: Vec<Cookie>,
    pub remove: Vec<Cookie>,
}

impl ForEngine {
    pub fn is_empty(&self) -> bool {
        self.set.is_empty() && self.remove.is_empty()
    }
}

/// One profile's cookies as last agreed between its engine and the shared files.
pub struct CookieSync {
    profile: String,
    agreed: BTreeMap<String, Vec<Cookie>>,
}

impl CookieSync {
    pub fn load(store: &Store, profile: &str) -> CookieSync {
        let agreed = store.read_local(&agreed_file(profile)).unwrap_or_default();
        CookieSync { profile: profile.to_string(), agreed }
    }

    /// The engine's cookies, all of them, merged with the shared files: those files written
    /// where this machine changed something, and what the engine must be told.
    pub fn merge(&mut self, store: &Store, jar: Vec<Cookie>, now: f64) -> ForEngine {
        let local = by_site(jar, now);
        let shared_sites = store.list(&folder(&self.profile));
        let mut sites: Vec<String> = local.keys().cloned().collect();
        sites.extend(self.agreed.keys().cloned());
        sites.extend(shared_sites.iter().cloned());
        sites.sort();
        sites.dedup();

        let mut out = ForEngine::default();
        let mut changed = false;
        for s in sites {
            let base: BTreeMap<Key, Cookie> = by_site(self.agreed.get(&s).cloned().unwrap_or_default(), now).remove(&s).unwrap_or_default();
            let shared: BTreeMap<Key, Cookie> = if shared_sites.contains(&s) {
                let read: SiteFile = store.read(&file(&self.profile, &s)).unwrap_or_default();
                by_site(read.cookies, now).remove(&s).unwrap_or_default()
            } else {
                BTreeMap::new()
            };
            let mine = local.get(&s).cloned().unwrap_or_default();

            // Three ways: begin from the file; what changed here since the last agreement goes
            // over it, what was removed here goes from it.
            let mut result = shared.clone();
            for (k, c) in &mine {
                if !base.get(k).is_some_and(|b| same(b, c)) {
                    result.insert(k.clone(), c.clone());
                }
            }
            for k in base.keys() {
                if !mine.contains_key(k) {
                    result.remove(k);
                }
            }

            let differs = |a: &BTreeMap<Key, Cookie>, b: &BTreeMap<Key, Cookie>| a.len() != b.len() || a.iter().any(|(k, c)| !b.get(k).is_some_and(|d| same(c, d)));
            if differs(&result, &shared) {
                if result.is_empty() {
                    store.remove(&file(&self.profile, &s));
                } else if let Err(e) = store.write(&file(&self.profile, &s), &SiteFile { cookies: result.values().cloned().collect() }) {
                    log::warn!("cookies of {s} not written: {e}");
                }
            }
            for (k, c) in &result {
                if !mine.get(k).is_some_and(|m| same(m, c)) {
                    out.set.push(c.clone());
                }
            }
            for (k, c) in &mine {
                if !result.contains_key(k) {
                    out.remove.push(c.clone());
                }
            }
            let agreed: Vec<Cookie> = result.into_values().collect();
            if self.agreed.get(&s) != Some(&agreed) {
                changed = true;
                if agreed.is_empty() {
                    self.agreed.remove(&s);
                } else {
                    self.agreed.insert(s, agreed);
                }
            }
        }
        if changed {
            if let Err(e) = store.write_local(&agreed_file(&self.profile), &self.agreed) {
                log::warn!("the cookies agreed on not kept: {e}");
            }
        }
        out
    }

    /// Where an engine that cannot list its cookies is to look: each cookie known, at its own
    /// address, and the hosts of the pages it has shown.
    pub fn addresses(&self, hosts: &[String]) -> Vec<String> {
        let mut all: Vec<String> = self.agreed.values().flatten().map(address_of).collect();
        all.extend(hosts.iter().filter(|h| !h.is_empty()).map(|h| format!("https://{h}/")));
        all.sort();
        all.dedup();
        all
    }

    /// The jar of an engine that only says what it would send to each address (Android's), made
    /// from that and from what was last agreed: a known cookie keeps what is known of it with the
    /// value sent now, and is gone if its own address no longer gets it; one not known is taken
    /// as the host's own, kept a week from today.
    pub fn jar_from_sent(&self, sent: &[(String, String)], now: f64) -> Vec<Cookie> {
        let known: Vec<&Cookie> = self.agreed.values().flatten().collect();
        let asked: HashMap<&str, &str> = sent.iter().map(|(u, h)| (u.as_str(), h.as_str())).collect();
        let mut jar: BTreeMap<Key, Cookie> = BTreeMap::new();
        // Known cookies first: kept, unless their own address was asked and did not get them.
        for c in &known {
            match asked.get(address_of(c).as_str()) {
                Some(header) if !pairs(header).iter().any(|(n, _)| *n == c.name) => {}
                _ => {
                    jar.insert(key(c), (*c).clone());
                }
            }
        }
        let week = ((now / DAY).floor() + 7.0) * DAY;
        for (url, header) in sent {
            let Some((secure, host, path)) = parts(url) else { continue };
            for (name, value) in pairs(header) {
                let best = known
                    .iter()
                    .filter(|c| c.name == name && domain_matches(&c.domain, &host) && path.starts_with(&c.path))
                    .max_by_key(|c| (c.path.len(), c.domain.len()));
                let cookie = match best {
                    Some(c) => Cookie { value: value.to_string(), ..(*c).clone() },
                    None => Cookie {
                        name: name.to_string(),
                        value: value.to_string(),
                        domain: host.clone(),
                        path: "/".into(),
                        expires: Some(week),
                        secure,
                        http_only: false,
                        same_site: String::new(),
                    },
                };
                jar.insert(key(&cookie), cookie);
            }
        }
        jar.into_values().collect()
    }
}

/// The address a cookie is sent to, and only it and those below it.
fn address_of(c: &Cookie) -> String {
    format!("https://{}{}", site(&c.domain), if c.path.starts_with('/') { c.path.as_str() } else { "/" })
}

fn domain_matches(domain: &str, host: &str) -> bool {
    match domain.strip_prefix('.') {
        Some(d) => host == d || host.ends_with(&format!(".{d}")),
        None => host == domain,
    }
}

/// Whether an address is HTTPS, its host and its path.
fn parts(url: &str) -> Option<(bool, String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = rest[..end].rsplit('@').next()?.split(':').next()?.to_ascii_lowercase();
    let path = rest[end..].split(['?', '#']).next().filter(|p| !p.is_empty()).unwrap_or("/");
    Some((scheme == "https", host, path.to_string()))
}

/// `a=1; b=2`, as pairs.
fn pairs(header: &str) -> Vec<(&str, &str)> {
    header
        .split(';')
        .filter_map(|p| {
            let (n, v) = p.trim().split_once('=')?;
            (!n.trim().is_empty()).then(|| (n.trim(), v.trim()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Scratch;

    const NOW: f64 = 1_800_000_000.0;

    fn c(name: &str, value: &str, domain: &str) -> Cookie {
        Cookie {
            name: name.into(),
            value: value.into(),
            domain: domain.into(),
            path: "/".into(),
            expires: Some(NOW + 100.0 * DAY),
            secure: true,
            http_only: true,
            same_site: String::new(),
        }
    }

    /// Two machines' folders, one shared: what the synchroniser would make of them.
    fn two() -> (Scratch, Store, Store) {
        let scratch = Scratch::new("cookies");
        let a = Store::at(&scratch.0.join("a"));
        let b = Store::at(&scratch.0.join("b"));
        std::fs::create_dir_all(scratch.0.join("a/apps")).unwrap();
        std::fs::create_dir_all(scratch.0.join("b")).unwrap();
        std::os::unix::fs::symlink(scratch.0.join("a/apps"), scratch.0.join("b/apps")).unwrap();
        (scratch, a, b)
    }

    #[test]
    fn a_login_on_one_machine_reaches_the_other() {
        let (_s, a, b) = two();
        let mut on_a = CookieSync::load(&a, "p");
        let mut on_b = CookieSync::load(&b, "p");
        let login = c("SID", "secret", ".example.com");
        assert!(on_a.merge(&a, vec![login.clone(), c("x", "1", "other.org")], NOW).is_empty());
        let told = on_b.merge(&b, vec![], NOW);
        assert_eq!(told.set.len(), 2);
        assert!(told.set.contains(&login));
        assert!(told.remove.is_empty());
        // B's engine has them now: nothing more to do on either side.
        let jar_b = told.set.clone();
        assert!(on_b.merge(&b, jar_b.clone(), NOW).is_empty());
        assert!(on_a.merge(&a, vec![login.clone(), c("x", "1", "other.org")], NOW).is_empty());

        // Logged out on B: gone from A too.
        let after: Vec<Cookie> = jar_b.into_iter().filter(|k| k.name != "SID").collect();
        assert!(on_b.merge(&b, after, NOW).is_empty());
        let told = on_a.merge(&a, vec![login.clone(), c("x", "1", "other.org")], NOW);
        assert_eq!(told.remove, vec![login]);
        assert!(told.set.is_empty());
        assert!(a.read::<SiteFile>("profiles/p/cookies/example.com.json").is_none());
    }

    #[test]
    fn both_changed_while_apart_keeps_both() {
        let (_s, a, b) = two();
        let mut on_a = CookieSync::load(&a, "p");
        let mut on_b = CookieSync::load(&b, "p");
        on_a.merge(&a, vec![c("one", "1", "site.com")], NOW);
        on_b.merge(&b, vec![c("one", "1", "site.com")], NOW);
        // Apart: A changes one, B adds another to the same site; B writes last.
        on_a.merge(&a, vec![c("one", "2", "site.com")], NOW);
        let told = on_b.merge(&b, vec![c("one", "1", "site.com"), c("two", "x", "site.com")], NOW);
        assert_eq!(told.set, vec![c("one", "2", "site.com")]);
        let file: SiteFile = a.read("profiles/p/cookies/site.com.json").unwrap();
        assert_eq!(file.cookies.len(), 2);
    }

    #[test]
    fn session_cookies_and_small_renewals_do_not_cross() {
        let (_s, a, b) = two();
        let mut on_a = CookieSync::load(&a, "p");
        let mut on_b = CookieSync::load(&b, "p");
        let session = Cookie { expires: None, ..c("s", "1", "site.com") };
        let kept = c("k", "1", "site.com");
        on_a.merge(&a, vec![session.clone(), kept.clone()], NOW);
        assert_eq!(on_b.merge(&b, vec![], NOW).set, vec![kept.clone()]);
        let path = a.shared_dir().join("profiles/p/cookies/site.com.json");
        let before = std::fs::read(&path).unwrap();
        let renewed = Cookie { expires: Some(NOW + 100.0 * DAY + 600.0), ..kept.clone() };
        assert!(on_a.merge(&a, vec![session, renewed], NOW).is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), before, "a renewal of minutes is not written");
    }

    #[test]
    fn what_android_sends_becomes_a_jar() {
        let (_s, a, _b) = two();
        let mut sync = CookieSync::load(&a, "p");
        sync.merge(&a, vec![c("SID", "old", ".google.com"), c("gone", "1", "mail.google.com")], NOW);
        let addresses = sync.addresses(&["news.site".into()]);
        assert!(addresses.contains(&"https://google.com/".to_string()));
        assert!(addresses.contains(&"https://news.site/".to_string()));
        let sent = vec![
            ("https://google.com/".to_string(), "SID=new".to_string()),
            ("https://mail.google.com/".to_string(), "SID=new".to_string()),
            ("https://news.site/".to_string(), "fresh=1".to_string()),
        ];
        let jar = sync.jar_from_sent(&sent, NOW);
        let sid = jar.iter().find(|k| k.name == "SID").unwrap();
        assert_eq!((sid.value.as_str(), sid.domain.as_str(), sid.http_only), ("new", ".google.com", true));
        assert!(!jar.iter().any(|k| k.name == "gone"), "its own address no longer gets it");
        let fresh = jar.iter().find(|k| k.name == "fresh").unwrap();
        assert_eq!(fresh.domain, "news.site");
        assert!(fresh.expires.unwrap() > NOW + 6.0 * DAY);
    }
}
