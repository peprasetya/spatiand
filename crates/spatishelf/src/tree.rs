//! The list's folders opened where they are, as Finder's list has them: a folder's things shown
//! under it, one step in, to any depth, each folder's in the folder's own order; and where
//! things dragged over the list would go.
//!
//! All of it is plain logic over places and listings, with no window and no tab: the window
//! keeps a [`Tree`] for each tab, makes its lines with [`flatten`] whenever anything changes, and
//! asks the tab for the listings [`flatten`] says are wanted. A folder is listed only when it is
//! opened and on show -- never ahead, never all the way down -- which matters on a remote, where
//! each listing is a call over the network that may be slow or fail.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::proto::{Item, Kind, Place};

/// How long something dragged waits over a closed folder before it opens (Finder's
/// spring-loading).
pub const SPRING_OPEN: Duration = Duration::from_millis(1000);

/// What is known of what is in an opened folder.
#[derive(Debug, Clone, PartialEq)]
pub enum Listing {
    /// Asked for; nothing has come yet.
    Loading,
    Failed(String),
    Ready(Vec<Item>),
}

/// A line of the list that is not a thing: what an opened folder has to say instead.
#[derive(Debug, Clone, PartialEq)]
pub enum Note {
    Loading,
    Failed(String),
    Empty,
}

/// Where a thing of the list is: the folder it is in, how far in, and whether it is a note.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub parent: Place,
    pub depth: usize,
    pub note: Option<Note>,
}

/// The folders opened in a tab's list, and what they hold.
///
/// Opened folders are kept by their whole place, so a folder opened, left and come back to is
/// open again; one opened inside a folder that is then closed is open again when that folder
/// is. Listings are kept only while their folder is on show (see [`Tree::prune`]): closing a
/// folder forgets what was in it and stops waiting for a listing asked for.
#[derive(Debug, Default)]
pub struct Tree {
    pub open: HashSet<Place>,
    pub listings: HashMap<Place, Listing>,
}

impl Tree {
    pub fn is_open(&self, place: &Place) -> bool {
        self.open.contains(place)
    }

    /// Open a folder where it is; whether it was closed.
    pub fn expand(&mut self, place: Place) -> bool {
        self.open.insert(place)
    }

    /// Close it; whether it was open. What was in it is forgotten, and a listing on its way is
    /// not waited for.
    pub fn collapse(&mut self, place: &Place) -> bool {
        self.listings.remove(place);
        self.open.remove(place)
    }

    /// A listing of `place` has been asked for. One already there stays on show until the new
    /// one comes; one that failed is tried again.
    pub fn asked(&mut self, place: Place) {
        let listing = self.listings.entry(place).or_insert(Listing::Loading);
        if matches!(listing, Listing::Failed(_)) {
            *listing = Listing::Loading;
        }
    }

    /// A listing came. Whether it is wanted: not if its folder was closed (or went off show)
    /// since it was asked for.
    pub fn arrived(&mut self, place: &Place, result: Result<Vec<Item>, String>) -> bool {
        match self.listings.get_mut(place) {
            Some(listing) => {
                *listing = match result {
                    Ok(items) => Listing::Ready(items),
                    Err(why) => Listing::Failed(why),
                };
                true
            }
            None => false,
        }
    }

    /// The opened folders whose listings are known, to be listed again when the folder on show
    /// is: the same refresh, all the way down what is open.
    pub fn known(&self) -> Vec<Place> {
        self.listings.iter().filter(|(_, l)| !matches!(l, Listing::Loading)).map(|(p, _)| p.clone()).collect()
    }

    /// Forget the listings of folders no longer on show.
    pub fn prune(&mut self, on_show: &HashSet<Place>) {
        self.listings.retain(|place, _| on_show.contains(place));
    }
}

/// The list's lines, made from the folder on show, what is opened in it and what is known.
#[derive(Debug, Default)]
pub struct Flat {
    /// Every thing, then every note: the folder's own listing first and as it came, so that a
    /// thing of it keeps its number (which its tab knows it by); then what opened folders hold.
    pub items: Vec<Item>,
    /// Where each of `items` is.
    pub entries: Vec<Entry>,
    /// The lines in the order shown: numbers into `items`.
    pub lines: Vec<usize>,
    /// Opened folders on show of which nothing is known: to be listed.
    pub wanted: Vec<Place>,
    /// Every opened folder on show.
    pub on_show: HashSet<Place>,
}

impl Flat {
    /// The place of a thing (not of a note).
    pub fn place_of(&self, index: usize) -> Option<Place> {
        place_of(&self.entries, &self.items, index)
    }
}

pub fn place_of(entries: &[Entry], items: &[Item], index: usize) -> Option<Place> {
    let entry = entries.get(index).filter(|e| e.note.is_none())?;
    Some(entry.parent.join(&items.get(index)?.name))
}

/// The lines of `root`, whose own listing is `top`. `nested`: folders opened are shown open
/// (the list); not, and only `top` is shown (the tiles). `keep_top` chooses which of `top` are
/// shown (hidden files, the filter typed), `keep` which of an opened folder's; `order` puts each
/// folder's things in order, under it.
pub fn flatten(root: &Place, top: &[Item], tree: &Tree, nested: bool, keep_top: &dyn Fn(&Item) -> bool, keep: &dyn Fn(&Item) -> bool, order: &dyn Fn(&Item, &Item) -> Ordering) -> Flat {
    let mut flat = Flat { items: top.to_vec(), entries: top.iter().map(|_| Entry { parent: root.clone(), depth: 0, note: None }).collect(), ..Flat::default() };
    let mut first: Vec<usize> = (0..top.len()).filter(|&i| keep_top(&top[i])).collect();
    first.sort_by(|&a, &b| order(&top[a], &top[b]));
    add(&mut flat, first, tree, nested, keep, order);
    flat
}

fn add(flat: &mut Flat, indices: Vec<usize>, tree: &Tree, nested: bool, keep: &dyn Fn(&Item) -> bool, order: &dyn Fn(&Item, &Item) -> Ordering) {
    for index in indices {
        flat.lines.push(index);
        if !nested || flat.items[index].kind != Kind::Folder {
            continue;
        }
        let Some(place) = flat.place_of(index) else { continue };
        if !tree.is_open(&place) {
            continue;
        }
        let depth = flat.entries[index].depth + 1;
        flat.on_show.insert(place.clone());
        let note = |flat: &mut Flat, note: Note, parent: &Place| {
            flat.items.push(Item { name: String::new(), kind: Kind::Other, size: 0, modified: None, hidden: false });
            flat.entries.push(Entry { parent: parent.clone(), depth, note: Some(note) });
            flat.lines.push(flat.items.len() - 1);
        };
        match tree.listings.get(&place) {
            None => {
                flat.wanted.push(place.clone());
                note(flat, Note::Loading, &place);
            }
            Some(Listing::Loading) => note(flat, Note::Loading, &place),
            Some(Listing::Failed(why)) => note(flat, Note::Failed(why.clone()), &place),
            Some(Listing::Ready(children)) => {
                let start = flat.items.len();
                for child in children {
                    flat.items.push(child.clone());
                    flat.entries.push(Entry { parent: place.clone(), depth, note: None });
                }
                let mut kept: Vec<usize> = (start..flat.items.len()).filter(|&i| keep(&flat.items[i])).collect();
                if kept.is_empty() {
                    note(flat, Note::Empty, &place);
                    continue;
                }
                let items = &flat.items;
                kept.sort_by(|&a, &b| order(&items[a], &items[b]));
                add(flat, kept, tree, nested, keep, order);
            }
        }
    }
}

/// The last line of the block a line begins: it and everything shown under it.
pub fn block_end(entries: &[Entry], lines: &[usize], at: usize) -> usize {
    let Some(depth) = lines.get(at).map(|&i| entries[i].depth) else { return at };
    let mut end = at;
    while end + 1 < lines.len() && entries[lines[end + 1]].depth > depth {
        end += 1;
    }
    end
}

/// The line of the folder a line is in, if it is in an opened one (not at the top).
pub fn parent_line(entries: &[Entry], lines: &[usize], at: usize) -> Option<usize> {
    let depth = entries[*lines.get(at)?].depth;
    if depth == 0 {
        return None;
    }
    (0..at).rev().find(|&l| entries[lines[l]].depth == depth - 1)
}

/// Where dragged things let go of over the list would go.
#[derive(Debug, Clone, PartialEq)]
pub struct Aim {
    pub place: Place,
    /// The folder's line, lit while it is aimed at; none for the folder on show.
    pub line: Option<usize>,
    /// Aimed at by what is in it (lit with it), not by its own line.
    pub within: bool,
}

/// Over the line `row` (none: below the last, or on the heads): onto a folder's line, into
/// that folder; onto a file or a note in an opened folder, into the folder it is in, as onto
/// the space that folder's things take; onto a file at the top, or anywhere else, into the
/// folder on show.
pub fn aim(entries: &[Entry], items: &[Item], lines: &[usize], row: Option<usize>, root: &Place) -> Aim {
    let whole = Aim { place: root.clone(), line: None, within: false };
    let Some(at) = row.filter(|&r| r < lines.len()) else { return whole };
    let index = lines[at];
    let entry = &entries[index];
    if entry.note.is_none() && items[index].kind == Kind::Folder {
        if let Some(place) = place_of(entries, items, index) {
            return Aim { place, line: Some(at), within: false };
        }
    }
    match parent_line(entries, lines, at) {
        Some(line) => Aim { place: entry.parent.clone(), line: Some(line), within: true },
        None => whole,
    }
}

/// Whether `items` can go into `target` at all: not into one of themselves or anything inside
/// one, and not where every one of them already is.
pub fn allowed(items: &[Place], target: &Place) -> bool {
    !items.is_empty() && items.iter().all(|i| i != target && !i.holds(target)) && items.iter().any(|i| i.parent().as_ref() != Some(target))
}

/// A closed folder opening under something held over it long enough, once.
#[derive(Debug, Default)]
pub struct Spring {
    over: Option<(Place, Instant)>,
    sprung: bool,
}

impl Spring {
    /// What is under the drag now: a closed folder that could open, or nothing that could.
    pub fn over(&mut self, folder: Option<&Place>, now: Instant) {
        match (folder, &self.over) {
            (Some(f), Some((o, _))) if f == o => {}
            (Some(f), _) => {
                self.over = Some((f.clone(), now));
                self.sprung = false;
            }
            (None, _) => self.over = None,
        }
    }

    /// The folder to open now, once it has been held over for [`SPRING_OPEN`].
    pub fn due(&mut self, now: Instant) -> Option<Place> {
        let (place, since) = self.over.as_ref()?;
        if self.sprung || now.duration_since(*since) < SPRING_OPEN {
            return None;
        }
        self.sprung = true;
        Some(place.clone())
    }
}

/// How far to scroll, a frame, under something dragged (or a band drawn) at `y`: up near the
/// top of the pane `top..bottom`, down near its foot, a little past either edge too.
pub fn edge_scroll(y: f32, top: f32, bottom: f32) -> f32 {
    if y < top + 30.0 && y > top - 40.0 {
        -18.0
    } else if y > bottom - 30.0 && y < bottom + 40.0 {
        18.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: u64) -> Item {
        Item { name: name.into(), kind: Kind::of(name, false), size, modified: Some(size as i64), hidden: name.starts_with('.') }
    }

    fn folder(name: &str) -> Item {
        Item { name: name.into(), kind: Kind::Folder, size: 0, modified: Some(0), hidden: false }
    }

    fn at(path: &str) -> Place {
        Place::Local(path.into())
    }

    /// Folders first, then by name; or by size, biggest first.
    fn by_name(a: &Item, b: &Item) -> Ordering {
        (b.kind == Kind::Folder).cmp(&(a.kind == Kind::Folder)).then_with(|| a.name.cmp(&b.name))
    }

    fn by_size(a: &Item, b: &Item) -> Ordering {
        (b.kind == Kind::Folder).cmp(&(a.kind == Kind::Folder)).then_with(|| b.size.cmp(&a.size))
    }

    fn shown(tree: &Tree, top: &[Item], order: &dyn Fn(&Item, &Item) -> Ordering) -> Flat {
        flatten(&at("/r"), top, tree, true, &|i| !i.hidden, &|i| !i.hidden, order)
    }

    /// Each line as its depth in dots and its name, or its note.
    fn lines(flat: &Flat) -> Vec<String> {
        flat.lines
            .iter()
            .map(|&i| {
                let e = &flat.entries[i];
                let what = match &e.note {
                    None => flat.items[i].name.clone(),
                    Some(Note::Loading) => "(loading)".into(),
                    Some(Note::Empty) => "(empty)".into(),
                    Some(Note::Failed(why)) => format!("(failed: {why})"),
                };
                format!("{}{what}", ".".repeat(e.depth))
            })
            .collect()
    }

    fn top() -> Vec<Item> {
        vec![file("b.txt", 5), folder("Photos"), file("a.txt", 50), folder("Music"), file(".hidden", 1)]
    }

    #[test]
    fn a_closed_list_is_the_folder_alone() {
        let flat = shown(&Tree::default(), &top(), &by_name);
        assert_eq!(lines(&flat), ["Music", "Photos", "a.txt", "b.txt"]);
        assert!(flat.wanted.is_empty() && flat.on_show.is_empty());
        // The folder's own things keep their numbers: the tab knows them by these.
        assert_eq!(flat.items[..5], top()[..]);
    }

    #[test]
    fn opening_asks_once_shows_loading_then_the_things_one_step_in() {
        let mut tree = Tree::default();
        assert!(tree.expand(at("/r/Photos")));
        let flat = shown(&tree, &top(), &by_name);
        assert_eq!(lines(&flat), ["Music", "Photos", ".(loading)", "a.txt", "b.txt"]);
        assert_eq!(flat.wanted, [at("/r/Photos")]);
        tree.asked(at("/r/Photos"));
        // Asked for: not wanted again while it is on its way.
        assert!(shown(&tree, &top(), &by_name).wanted.is_empty());
        assert!(tree.arrived(&at("/r/Photos"), Ok(vec![file("z.jpg", 1), folder("2024"), file("y.jpg", 9)])));
        let flat = shown(&tree, &top(), &by_name);
        assert_eq!(lines(&flat), ["Music", "Photos", ".2024", ".y.jpg", ".z.jpg", "a.txt", "b.txt"]);
        let y = flat.lines[3];
        assert_eq!(flat.place_of(y), Some(at("/r/Photos/y.jpg")));
    }

    #[test]
    fn to_any_depth_and_each_folder_sorted_under_itself() {
        let mut tree = Tree::default();
        for p in ["/r/Photos", "/r/Photos/2024"] {
            tree.expand(at(p));
            tree.asked(at(p));
        }
        tree.arrived(&at("/r/Photos"), Ok(vec![file("small.jpg", 1), folder("2024"), file("big.jpg", 900)]));
        tree.arrived(&at("/r/Photos/2024"), Ok(vec![file("x.jpg", 3), file("w.jpg", 30)]));
        let flat = shown(&tree, &top(), &by_name);
        assert_eq!(lines(&flat), ["Music", "Photos", ".2024", "..w.jpg", "..x.jpg", ".big.jpg", ".small.jpg", "a.txt", "b.txt"]);
        // By size: every folder's things by size, and still under their folder.
        let flat = shown(&tree, &top(), &by_size);
        assert_eq!(lines(&flat), ["Photos", ".2024", "..w.jpg", "..x.jpg", ".big.jpg", ".small.jpg", "Music", "a.txt", "b.txt"]);
        assert_eq!(flat.on_show.len(), 2);
    }

    #[test]
    fn the_tiles_open_nothing() {
        let mut tree = Tree::default();
        tree.expand(at("/r/Photos"));
        let flat = flatten(&at("/r"), &top(), &tree, false, &|i| !i.hidden, &|i| !i.hidden, &by_name);
        assert_eq!(lines(&flat), ["Music", "Photos", "a.txt", "b.txt"]);
        assert!(flat.wanted.is_empty(), "nothing is listed for the tiles");
    }

    #[test]
    fn empty_and_failed_say_so() {
        let mut tree = Tree::default();
        for p in ["/r/Photos", "/r/Music"] {
            tree.expand(at(p));
            tree.asked(at(p));
        }
        tree.arrived(&at("/r/Photos"), Err("This folder may not be opened.".into()));
        // Only hidden files: empty as shown.
        tree.arrived(&at("/r/Music"), Ok(vec![file(".DS_Store", 1)]));
        let flat = shown(&tree, &top(), &by_name);
        assert_eq!(lines(&flat), ["Music", ".(empty)", "Photos", ".(failed: This folder may not be opened.)", "a.txt", "b.txt"]);
        // A note is not a thing: it has no place.
        assert_eq!(flat.place_of(flat.lines[1]), None);
        // Asked again, a failed folder is loading again.
        tree.asked(at("/r/Photos"));
        assert_eq!(tree.listings[&at("/r/Photos")], Listing::Loading);
    }

    #[test]
    fn closing_forgets_and_stops_waiting_but_inner_folders_stay_open() {
        let mut tree = Tree::default();
        for p in ["/r/Photos", "/r/Photos/2024"] {
            tree.expand(at(p));
            tree.asked(at(p));
        }
        tree.arrived(&at("/r/Photos"), Ok(vec![folder("2024")]));
        assert!(tree.collapse(&at("/r/Photos")));
        let flat = shown(&tree, &top(), &by_name);
        assert_eq!(lines(&flat), ["Music", "Photos", "a.txt", "b.txt"]);
        tree.prune(&flat.on_show);
        // The inner folder's listing, still on its way, is not wanted when it comes.
        assert!(!tree.arrived(&at("/r/Photos/2024"), Ok(vec![file("late.jpg", 1)])));
        assert!(tree.listings.is_empty());
        // Opened again: listed again (it may have changed), and its inner folder is open again.
        tree.expand(at("/r/Photos"));
        let flat = shown(&tree, &top(), &by_name);
        assert_eq!(flat.wanted, [at("/r/Photos")]);
        tree.asked(at("/r/Photos"));
        tree.arrived(&at("/r/Photos"), Ok(vec![folder("2024"), file("new.jpg", 1)]));
        let flat = shown(&tree, &top(), &by_name);
        assert_eq!(lines(&flat), ["Music", "Photos", ".2024", "..(loading)", ".new.jpg", "a.txt", "b.txt"]);
        assert_eq!(flat.wanted, [at("/r/Photos/2024")]);
    }

    #[test]
    fn a_refresh_keeps_what_is_open_and_shows_the_old_until_the_new_comes() {
        let mut tree = Tree::default();
        tree.expand(at("/r/Photos"));
        tree.asked(at("/r/Photos"));
        tree.arrived(&at("/r/Photos"), Ok(vec![file("a.jpg", 1)]));
        // The folder on show listed again: what is open is listed again too.
        assert_eq!(tree.known(), [at("/r/Photos")]);
        tree.asked(at("/r/Photos"));
        assert_eq!(lines(&shown(&tree, &top(), &by_name))[2], ".a.jpg", "the old listing stays on show meanwhile");
        // The new top listing lost a file and kept the folder: still open.
        let new_top = vec![folder("Photos"), folder("Music")];
        tree.arrived(&at("/r/Photos"), Ok(vec![file("a.jpg", 1), file("moved-in.jpg", 2)]));
        let flat = shown(&tree, &new_top, &by_name);
        assert_eq!(lines(&flat), ["Music", "Photos", ".a.jpg", ".moved-in.jpg"]);
        tree.prune(&flat.on_show);
        assert!(tree.listings.contains_key(&at("/r/Photos")));
        // Gone to another folder, what was open there is off show: forgotten, but still open
        // for coming back.
        let elsewhere = flatten(&at("/x"), &[folder("y")], &tree, true, &|_| true, &|_| true, &by_name);
        tree.prune(&elsewhere.on_show);
        assert!(tree.listings.is_empty() && tree.is_open(&at("/r/Photos")));
        assert_eq!(shown(&tree, &new_top, &by_name).wanted, [at("/r/Photos")]);
    }

    /// The lines of a tree: Photos open with 2024 open in it.
    fn opened() -> Flat {
        let mut tree = Tree::default();
        for p in ["/r/Photos", "/r/Photos/2024"] {
            tree.expand(at(p));
            tree.asked(at(p));
        }
        tree.arrived(&at("/r/Photos"), Ok(vec![folder("2024"), file("big.jpg", 9)]));
        tree.arrived(&at("/r/Photos/2024"), Ok(vec![]));
        let flat = shown(&tree, &top(), &by_name);
        assert_eq!(lines(&flat), ["Music", "Photos", ".2024", "..(empty)", ".big.jpg", "a.txt", "b.txt"]);
        flat
    }

    #[test]
    fn where_a_drop_goes() {
        let f = opened();
        let aim_at = |row: Option<usize>| aim(&f.entries, &f.items, &f.lines, row, &at("/r"));
        // Onto a folder's line: into it, that line lit.
        assert_eq!(aim_at(Some(0)), Aim { place: at("/r/Music"), line: Some(0), within: false });
        assert_eq!(aim_at(Some(2)), Aim { place: at("/r/Photos/2024"), line: Some(2), within: false });
        // Onto a thing in an opened folder, or its note: into that folder, lit with what is in it.
        assert_eq!(aim_at(Some(4)), Aim { place: at("/r/Photos"), line: Some(1), within: true });
        assert_eq!(aim_at(Some(3)), Aim { place: at("/r/Photos/2024"), line: Some(2), within: true });
        // Onto a file at the top, below the last line, or on the heads: the folder on show.
        let whole = Aim { place: at("/r"), line: None, within: false };
        assert_eq!(aim_at(Some(5)), whole);
        assert_eq!(aim_at(Some(99)), whole);
        assert_eq!(aim_at(None), whole);
        // What a folder's line lights with what is in it.
        assert_eq!(block_end(&f.entries, &f.lines, 1), 4);
        assert_eq!(block_end(&f.entries, &f.lines, 2), 3);
        assert_eq!(block_end(&f.entries, &f.lines, 5), 5);
    }

    #[test]
    fn drops_that_do_nothing_are_not_allowed() {
        let photo = at("/r/Photos/big.jpg");
        // Onto itself, into its own folder, into a folder inside itself.
        assert!(!allowed(&[at("/r/Photos")], &at("/r/Photos")));
        assert!(!allowed(&[photo.clone()], &at("/r/Photos")));
        assert!(!allowed(&[at("/r/Photos")], &at("/r/Photos/2024")));
        assert!(!allowed(&[at("/r/Photos")], &at("/r/Photos/2024/deeper")));
        assert!(!allowed(&[], &at("/r")));
        // Elsewhere: up to the folder on show, or into a sibling.
        assert!(allowed(&[photo.clone()], &at("/r")));
        assert!(allowed(&[photo.clone()], &at("/r/Music")));
        // A name that only begins the same is not inside.
        assert!(allowed(&[at("/r/Photos")], &at("/r/Photos 2")));
        // Several, from different folders: anywhere none of them is, or that not all are in.
        let several = [photo.clone(), at("/r/a.txt")];
        assert!(allowed(&several, &at("/r/Music")));
        assert!(allowed(&several, &at("/r")), "the photo moves up; a.txt is already there");
        assert!(!allowed(&[photo, at("/r/Photos")], &at("/r/Photos/2024")), "one of them would go inside itself");
        // Remotes alike.
        let remote = |p: &str| Place::Remote("drive".into(), p.into());
        assert!(!allowed(&[remote("a")], &remote("a/b")));
        assert!(allowed(&[remote("a/b")], &remote("")));
        assert!(allowed(&[remote("a")], &at("/r")));
    }

    #[test]
    fn a_closed_folder_springs_open_after_a_second_held_over_it_once() {
        let t0 = Instant::now();
        let mut s = Spring::default();
        let music = at("/r/Music");
        s.over(Some(&music), t0);
        assert_eq!(s.due(t0 + Duration::from_millis(500)), None);
        // Moving about over the same folder does not start it again.
        s.over(Some(&music), t0 + Duration::from_millis(600));
        assert_eq!(s.due(t0 + SPRING_OPEN), Some(music.clone()));
        assert_eq!(s.due(t0 + Duration::from_secs(5)), None, "once");
        // Off it and onto another: that one's own second.
        let other = at("/r/Photos");
        s.over(None, t0 + Duration::from_millis(1100));
        assert_eq!(s.due(t0 + Duration::from_secs(9)), None);
        s.over(Some(&other), t0 + Duration::from_millis(1200));
        assert_eq!(s.due(t0 + Duration::from_millis(2000)), None);
        assert_eq!(s.due(t0 + Duration::from_millis(2200)), Some(other));
        // Left before its second is up: nothing.
        s.over(Some(&music), t0 + Duration::from_secs(10));
        s.over(None, t0 + Duration::from_millis(10500));
        assert_eq!(s.due(t0 + Duration::from_secs(12)), None);
    }

    #[test]
    fn scrolling_at_the_edges() {
        let (top, bottom) = (100.0, 700.0);
        assert_eq!(edge_scroll(110.0, top, bottom), -18.0);
        assert_eq!(edge_scroll(80.0, top, bottom), -18.0, "a little above the pane too");
        assert_eq!(edge_scroll(40.0, top, bottom), 0.0, "but not from the bar of tabs");
        assert_eq!(edge_scroll(400.0, top, bottom), 0.0);
        assert_eq!(edge_scroll(690.0, top, bottom), 18.0);
        assert_eq!(edge_scroll(730.0, top, bottom), 18.0);
        assert_eq!(edge_scroll(760.0, top, bottom), 0.0);
    }
}
