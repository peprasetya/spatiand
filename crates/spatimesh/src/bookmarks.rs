//! Bookmarks, a profile's: `apps/spatimesh/profiles/<profile>/bookmarks/<id>.json`.
//!
//! **One file per bookmark or folder**, because the synchroniser keeps the newest version of a
//! file: two machines that each add a bookmark while apart then both keep theirs, where one
//! file of all of them would lose one machine's. Their order is a key of their own (`order`),
//! a fraction written in base 36 that sorts as text, so that putting one between two others
//! writes that one file and no other. Two machines that put two at the same place end with
//! the same key, and the bookmarks' ids settle which comes first.

use serde::{Deserialize, Serialize};

use crate::store::{new_id, now_ms, Store};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    #[serde(skip)]
    pub id: String,
    /// The folder it is in: empty for the top.
    #[serde(default)]
    pub parent: String,
    pub title: String,
    /// What it opens; none for a folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub order: String,
    #[serde(default)]
    pub added: u64,
}

impl Bookmark {
    pub fn is_folder(&self) -> bool {
        self.url.is_none()
    }
}

/// Where a bookmark goes among its folder's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum At<'a> {
    End,
    Start,
    Before(&'a str),
    After(&'a str),
}

fn folder(profile: &str) -> String {
    format!("profiles/{profile}/bookmarks")
}

fn file(profile: &str, id: &str) -> String {
    format!("profiles/{profile}/bookmarks/{id}.json")
}

/// All of a profile's bookmarks and folders.
pub fn all(store: &Store, profile: &str) -> Vec<Bookmark> {
    store
        .list(&folder(profile))
        .into_iter()
        .filter_map(|id| store.read::<Bookmark>(&file(profile, &id)).map(|b| Bookmark { id, ..b }))
        .collect()
}

/// A folder's own, in order. A bookmark whose folder has gone (deleted on another machine
/// while it was added here) is shown at the top rather than lost.
pub fn children<'a>(all: &'a [Bookmark], parent: &str) -> Vec<&'a Bookmark> {
    let folder_exists = |id: &str| all.iter().any(|b| b.id == id && b.is_folder());
    let mut found: Vec<&Bookmark> = all
        .iter()
        .filter(|b| b.parent == parent || (parent.is_empty() && !b.parent.is_empty() && !folder_exists(&b.parent)))
        .collect();
    found.sort_by(|a, b| (a.order.as_str(), a.id.as_str()).cmp(&(b.order.as_str(), b.id.as_str())));
    found
}

/// The bookmark for an address, if there is one.
pub fn find<'a>(all: &'a [Bookmark], url: &str) -> Option<&'a Bookmark> {
    let wanted = url.trim_end_matches('/');
    all.iter().find(|b| b.url.as_deref().is_some_and(|u| u.trim_end_matches('/') == wanted))
}

/// The key that sorts at `at` in a folder.
fn order_at(all: &[Bookmark], parent: &str, at: At) -> String {
    let siblings = children(all, parent);
    let position = |id: &str| siblings.iter().position(|b| b.id == id);
    let (before, after): (Option<&str>, Option<&str>) = match at {
        At::End => (siblings.last().map(|b| b.order.as_str()), None),
        At::Start => (None, siblings.first().map(|b| b.order.as_str())),
        At::Before(id) => match position(id) {
            Some(i) => (i.checked_sub(1).map(|j| siblings[j].order.as_str()), Some(siblings[i].order.as_str())),
            None => (siblings.last().map(|b| b.order.as_str()), None),
        },
        At::After(id) => match position(id) {
            Some(i) => (Some(siblings[i].order.as_str()), siblings.get(i + 1).map(|b| b.order.as_str())),
            None => (siblings.last().map(|b| b.order.as_str()), None),
        },
    };
    // Two that ended up with one key (two machines) have nothing between them; the next one
    // then goes after both.
    match (before, after) {
        (Some(a), Some(b)) if a >= b => key_between(a, None),
        (a, b) => key_between(a.unwrap_or(""), b),
    }
}

pub fn add(store: &Store, profile: &str, parent: &str, title: &str, url: Option<&str>, at: At) -> Result<Bookmark, String> {
    let existing = all(store, profile);
    let bookmark = Bookmark {
        id: new_id(),
        parent: parent.to_string(),
        title: title.trim().to_string(),
        url: url.map(str::to_string),
        order: order_at(&existing, parent, at),
        added: now_ms(),
    };
    store.write(&file(profile, &bookmark.id), &bookmark)?;
    Ok(bookmark)
}

/// A bookmark's title or address changed.
pub fn update(store: &Store, profile: &str, bookmark: &Bookmark) -> Result<(), String> {
    store.write(&file(profile, &bookmark.id), bookmark)
}

/// Put a bookmark somewhere else: another folder, another place in its own.
pub fn move_to(store: &Store, profile: &str, id: &str, parent: &str, at: At) -> Result<(), String> {
    let existing = all(store, profile);
    let mut bookmark = existing.iter().find(|b| b.id == id).cloned().ok_or("no such bookmark")?;
    // A folder cannot go inside itself.
    let mut up = parent.to_string();
    while !up.is_empty() {
        if up == id {
            return Err("a folder cannot go inside itself".into());
        }
        up = existing.iter().find(|b| b.id == up).map(|b| b.parent.clone()).unwrap_or_default();
    }
    let others: Vec<Bookmark> = existing.into_iter().filter(|b| b.id != id).collect();
    bookmark.parent = parent.to_string();
    bookmark.order = order_at(&others, parent, at);
    update(store, profile, &bookmark)
}

/// A bookmark gone; a folder with everything in it.
pub fn remove(store: &Store, profile: &str, id: &str) {
    let existing = all(store, profile);
    let mut going = vec![id.to_string()];
    let mut i = 0;
    while i < going.len() {
        let parent = going[i].clone();
        going.extend(existing.iter().filter(|b| b.parent == parent).map(|b| b.id.clone()));
        i += 1;
    }
    for id in going {
        store.remove(&file(profile, &id));
    }
}

const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

fn digit(c: u8) -> usize {
    DIGITS.iter().position(|&d| d == c).unwrap_or(0)
}

/// A key that sorts after `a` (empty: the lowest there is) and before `b` (none: the highest).
/// Keys are fractions in base 36 with no trailing zero, compared as text.
pub fn key_between(a: &str, b: Option<&str>) -> String {
    // A key from elsewhere with a trailing zero, or out of order, is not trusted.
    let a = a.trim_end_matches('0');
    let b = b.map(|b| b.trim_end_matches('0')).filter(|b| !b.is_empty() && *b > a);
    midpoint(a.as_bytes(), b.map(str::as_bytes))
}

fn midpoint(a: &[u8], b: Option<&[u8]>) -> String {
    if let Some(b) = b {
        // The common start, `a` read as if it went on in zeros.
        let mut n = 0;
        while n < b.len() && a.get(n).copied().unwrap_or(b'0') == b[n] {
            n += 1;
        }
        if n > 0 {
            let rest_a = if n < a.len() { &a[n..] } else { &[][..] };
            return format!("{}{}", String::from_utf8_lossy(&b[..n]), midpoint(rest_a, Some(&b[n..])));
        }
    }
    if a.is_empty() && b.is_none() {
        // The first: the middle, room either side.
        return "i".to_string();
    }
    let da = a.first().map_or(0, |&c| digit(c));
    let db = b.map_or(DIGITS.len(), |b| b.first().map_or(DIGITS.len(), |&c| digit(c)));
    // At the end, the next digit up, and at the start the next one down, rather than half way:
    // bookmarks are mostly added at the end, and half way there runs out of digits five times
    // as fast.
    if b.is_none() && da + 1 < DIGITS.len() {
        return (DIGITS[da + 1] as char).to_string();
    }
    if a.is_empty() && db >= 2 {
        return (DIGITS[db - 1] as char).to_string();
    }
    if db - da > 1 {
        let mid = (da + db).div_ceil(2);
        return (DIGITS[mid] as char).to_string();
    }
    match b {
        // The first digits are next to each other and `b` goes on: its first digit alone is
        // between them.
        Some(b) if b.len() > 1 => (b[0] as char).to_string(),
        // `b` is a single digit, or there is none: keep `a`'s digit and go on after the rest.
        _ => {
            let rest = if a.len() > 1 { &a[1..] } else { &[][..] };
            format!("{}{}", DIGITS[da] as char, midpoint(rest, None))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Scratch;

    #[test]
    fn keys_go_between() {
        let first = key_between("", None);
        assert_eq!(first, "i");
        let after = key_between(&first, None);
        assert!(after > first);
        let before = key_between("", Some(&first));
        assert!(before < first && !before.is_empty());
        let mid = key_between(&first, Some(&after));
        assert!(first < mid && mid < after, "{first} < {mid} < {after}");
        // Neighbouring digits, and one key the start of the other.
        for (a, b) in [("a", "b"), ("a", "a1"), ("az", "b"), ("49", "5"), ("", "1"), ("zz", "zz1")] {
            let k = key_between(a, Some(b));
            assert!(a < k.as_str() && k.as_str() < b, "{a} < {k} < {b}");
            assert!(!k.ends_with('0'), "{k}");
        }
    }

    #[test]
    fn many_inserts_stay_in_order() {
        // Always between the first two: the worst case for key length.
        let mut keys = vec![key_between("", None), key_between(&key_between("", None), None)];
        for _ in 0..200 {
            let k = key_between(&keys[0], Some(&keys[1]));
            assert!(keys[0] < k && k < keys[1]);
            keys.insert(1, k);
        }
        assert!(keys.windows(2).all(|w| w[0] < w[1]));
        // And at either end.
        let mut end = key_between("", None);
        let mut start = end.clone();
        for _ in 0..500 {
            let next = key_between(&end, None);
            assert!(next > end);
            end = next;
            let earlier = key_between("", Some(&start));
            assert!(earlier < start);
            start = earlier;
        }
        assert!(end.len() < 40 && start.len() < 40, "{} {}", end.len(), start.len());
    }

    #[test]
    fn add_order_move_remove() {
        let scratch = Scratch::new("bookmarks");
        let store = scratch.store();
        let p = "default";
        let a = add(&store, p, "", "A", Some("https://a.example/"), At::End).unwrap();
        let c = add(&store, p, "", "C", Some("https://c.example/"), At::End).unwrap();
        let b = add(&store, p, "", "B", Some("https://b.example/"), At::After(&a.id)).unwrap();
        let first = add(&store, p, "", "Zero", Some("https://0.example/"), At::Start).unwrap();
        let now = all(&store, p);
        let titles: Vec<&str> = children(&now, "").iter().map(|b| b.title.as_str()).collect();
        assert_eq!(titles, vec!["Zero", "A", "B", "C"]);
        assert_eq!(find(&now, "https://b.example").map(|x| x.id.as_str()), Some(b.id.as_str()));

        let folder = add(&store, p, "", "Folder", None, At::End).unwrap();
        move_to(&store, p, &c.id, &folder.id, At::End).unwrap();
        move_to(&store, p, &first.id, "", At::End).unwrap();
        let now = all(&store, p);
        let titles: Vec<&str> = children(&now, "").iter().map(|b| b.title.as_str()).collect();
        assert_eq!(titles, vec!["A", "B", "Folder", "Zero"]);
        assert_eq!(children(&now, &folder.id).len(), 1);
        assert!(move_to(&store, p, &folder.id, &folder.id, At::End).is_err());

        remove(&store, p, &folder.id);
        let now = all(&store, p);
        assert_eq!(now.len(), 3, "the folder went with what was in it");
        assert!(find(&now, "https://c.example/").is_none());
    }

    #[test]
    fn two_machines_adding_apart_keep_both() {
        // Two machines' folders, each adding at the end while apart, then each taking the
        // other's files as the synchroniser would: both bookmarks are there, in one order.
        let one = Scratch::new("bookmarks-one");
        let two = Scratch::new("bookmarks-two");
        let (s1, s2) = (one.store(), two.store());
        let base = add(&s1, "default", "", "Base", Some("https://base.example/"), At::End).unwrap();
        update(&s2, "default", &base).unwrap();
        add(&s1, "default", "", "From one", Some("https://one.example/"), At::End).unwrap();
        add(&s2, "default", "", "From two", Some("https://two.example/"), At::End).unwrap();
        for (from, to) in [(&s1, &s2), (&s2, &s1)] {
            for b in all(from, "default") {
                update(to, "default", &b).unwrap();
            }
        }
        let (a, b) = (all(&s1, "default"), all(&s2, "default"));
        let order = |all: &[Bookmark]| children(all, "").iter().map(|b| b.title.clone()).collect::<Vec<_>>();
        assert_eq!(order(&a).len(), 3);
        assert_eq!(order(&a), order(&b), "one order on both");
        assert_eq!(order(&a)[0], "Base");
        // And one more after them lands after both.
        add(&s1, "default", "", "Later", Some("https://later.example/"), At::End).unwrap();
        assert_eq!(order(&all(&s1, "default")).last().unwrap(), "Later");
    }

    #[test]
    fn orphans_are_shown_at_the_top() {
        let scratch = Scratch::new("bookmarks-orphan");
        let store = scratch.store();
        let mut lost = add(&store, "p", "", "Lost", Some("https://lost.example/"), At::End).unwrap();
        lost.parent = "gone-folder".into();
        update(&store, "p", &lost).unwrap();
        let now = all(&store, "p");
        assert_eq!(children(&now, "").len(), 1);
    }
}
