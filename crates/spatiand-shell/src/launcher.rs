//! The home screen — a grid of applications as glass bubbles.
//!
//! Opened with the `⋯` button, in the manner of visionOS or NebulaOS: a floating arc of icons
//! in front of you rather than a window containing a list. The apps come from the system's own
//! desktop entries, so whatever is installed shows up without Spatiand keeping a catalogue of
//! its own.
//!
//! The layout is an **arc, not a plane**. Icons are placed at a fixed distance around the
//! viewer, which keeps every bubble the same size and the same focal distance — on a plane the
//! outer icons are further away and smaller, and the eyes have to re-converge as the cursor
//! travels. That is tiring in a way that is hard to attribute to layout.

use crate::grid::{Direction, Grid};

/// One launchable application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppEntry {
    pub name: String,
    /// Command line, already stripped of desktop-entry field codes.
    pub exec: String,
    /// Absolute path to an icon, if one was found. Absent is normal and not an error: the
    /// bubble falls back to the app's initial, which is legible at bubble size anyway.
    pub icon: Option<String>,
    /// Freedesktop categories, used to file it under a group.
    pub categories: Vec<String>,
}

/// The icon a remote computer's bubble looks up in the theme.
pub const HOST_ICON: &str = "network-server";
/// The icon this machine's own bubble looks up.
pub const LOCAL_ICON: &str = "computer";

/// Angular spacing between adjacent bubbles, degrees.
///
/// A bubble subtends about 4.6°, so 9° leaves nearly a full bubble of space between
/// neighbours.
///
/// The numbers here are set by a hard constraint rather than by taste: one eye sees **40°
/// across and only 23° vertically**. Four columns at 9° reach ±15.8° including the glass,
/// inside the 20° half-width. Earlier attempts at 11° and 13° looked reasonable written down
/// and put the outer column past the edge of the field.
pub const COLUMN_SPACING_DEG: f32 = 9.0;
/// Rows are tighter than columns because the vertical field is half the horizontal one, and
/// each bubble still has to fit a label underneath it. Three rows at 7° reach ±10.7°
/// including the label, against a half-height of 11.57°.
pub const ROW_SPACING_DEG: f32 = 7.0;
/// How far out the arc sits, metres. Matches the default window radius so switching between
/// the launcher and a window does not change focal distance.
pub const ARC_RADIUS_M: f32 = 2.0;
/// Bubbles per row.
pub const COLUMNS: usize = 4;
/// Rows shown at once.
///
/// Three, capped deliberately. More than that and the outer rows are past the vertical field
/// and have to be hunted for by tilting your head, which is far worse than paging.
pub const ROWS_PER_PAGE: usize = 3;
/// Bubbles on one page.
pub const PAGE_SIZE: usize = COLUMNS * ROWS_PER_PAGE;

/// Where one bubble goes, in the viewer-centred frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BubblePlacement {
    /// Radians, positive to the left — the same convention as `spatiand::window::Placement`.
    pub yaw: f32,
    pub pitch: f32,
    pub radius: f32,
    /// 1.0 for a normal bubble; the focused one is grown slightly.
    pub scale: f32,
}

/// What the launcher is currently showing.
///
/// Two levels: a machine, then its applications. The applications used to be filed under
/// categories first ("Internet, then Chrome"), which is two decisions where the answer to the
/// first is a guess -- is a terminal a System thing or a Utility? -- and on a Mac, which files
/// nothing under categories, there was nothing to file by. One list by name, narrowed by typing,
/// asks only what the application is called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Level {
    /// The machines: this one, then each remote computer.
    Machines,
    /// This machine's applications.
    Local,
    /// The applications another computer offers. Carries the index into the computers.
    Host(usize),
}

/// One application a remote computer offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    /// The computer's own name for it, which is what launching it sends.
    pub id: String,
    pub name: String,
    /// A picture of it: a path, which the renderer loads like any other icon.
    pub icon: Option<String>,
}

/// A remote computer, as a tab in the launcher.
///
/// A tab rather than its applications mixed in with this machine's: the same name can be on
/// both — a browser here and a browser there are not the same browser — and which machine
/// something runs on is the first thing you choose, not the last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostTab {
    pub label: String,
    /// What it is known by; launching names it.
    pub address: String,
    pub online: bool,
    pub apps: Vec<RemoteEntry>,
}

/// What pressing A on an application asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    Local(AppEntry),
    Remote { host: String, app: String },
}

/// One bubble on show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    /// This machine, at the top level.
    ThisMachine,
    /// A remote computer, at the top level: its index.
    Machine(usize),
    /// One of this machine's applications: its index.
    Local(usize),
    /// A remote computer's application: the computer, then the application.
    Remote(usize, usize),
}

/// The launcher's state.
#[derive(Debug, Clone)]
pub struct Launcher {
    /// This machine's applications, by name.
    apps: Vec<AppEntry>,
    grid: Grid,
    level: Level,
    /// Remote computers, shown after this one.
    hosts: Vec<HostTab>,
    /// What this machine is called.
    local_name: String,
    /// Where the cursor was among the machines, so backing out returns to it rather than to the
    /// top -- opening the wrong one and coming back should not cost you your place.
    machine_cursor: usize,
    /// What has been typed to narrow the bubbles. Empty shows everything.
    query: String,
    /// The bubbles on show: the level's, narrowed by the query.
    shown: Vec<Item>,
}

/// How well a name answers what was typed: lower is better, `None` is not at all.
///
/// A name that starts with it, then a word in the name that does, then anywhere inside, then
/// the initials of its words ("vsc" finds Visual Studio Code). Case is ignored.
fn rank(name: &str, query: &str) -> Option<u32> {
    let name = name.to_lowercase();
    let query = query.to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    if name.starts_with(&query) {
        return Some(0);
    }
    if name
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word.starts_with(&query))
    {
        return Some(1);
    }
    if name.contains(&query) {
        return Some(2);
    }
    // The initials of its words: "vsc" for Visual Studio Code. Not its letters in any order
    // with others between them, which found "Chrome" for "co" and half the list for anything.
    let initials: String = name
        .split(|c: char| !c.is_alphanumeric())
        .filter_map(|word| word.chars().next())
        .collect();
    (query.chars().count() > 1 && initials.starts_with(&query)).then_some(3)
}

impl Launcher {
    pub fn new(mut apps: Vec<AppEntry>) -> Self {
        apps.sort_by_key(|a| a.name.to_lowercase());
        let mut this = Self {
            grid: Grid::new(COLUMNS, 0),
            apps,
            level: Level::Local,
            hosts: Vec::new(),
            local_name: "This computer".into(),
            machine_cursor: 0,
            query: String::new(),
            shown: Vec::new(),
        };
        this.level = this.root();
        this.refill(0);
        this
    }

    /// What this machine is called, for its bubble among the machines.
    pub fn set_local_name(&mut self, name: &str) {
        if !name.trim().is_empty() && name != self.local_name {
            self.local_name = name.trim().to_string();
        }
    }

    pub fn local_name(&self) -> &str {
        &self.local_name
    }

    /// Where the launcher opens, and where B stops climbing.
    ///
    /// The machines, when there is more than one to choose between. With only this one, or only
    /// one remote computer and nothing here (a Beam Pro), choosing among one is a press for
    /// nothing, and the launcher opens on its applications.
    fn root(&self) -> Level {
        match (self.apps.is_empty(), self.hosts.len()) {
            (_, 0) => Level::Local,
            (true, 1) => Level::Host(0),
            _ => Level::Machines,
        }
    }

    /// Work out the bubbles again, keeping the cursor at `cursor` if there is still one there.
    fn refill(&mut self, cursor: usize) {
        let query = self.query.trim();
        let mut ranked: Vec<(u32, usize, Item)> = Vec::new();
        let mut order = 0usize;
        let mut push = |ranked: &mut Vec<(u32, usize, Item)>, name: &str, item: Item| {
            if let Some(r) = rank(name, query) {
                ranked.push((r, order, item));
            }
            order += 1;
        };
        match &self.level {
            // With nothing typed, the machines. Typing here looks through all of them at once.
            Level::Machines if query.is_empty() => {
                if !self.apps.is_empty() {
                    ranked.push((0, 0, Item::ThisMachine));
                }
                for i in 0..self.hosts.len() {
                    ranked.push((0, i + 1, Item::Machine(i)));
                }
            }
            Level::Machines => {
                for (i, app) in self.apps.iter().enumerate() {
                    push(&mut ranked, &app.name, Item::Local(i));
                }
                for (h, host) in self.hosts.iter().enumerate() {
                    for (i, app) in host.apps.iter().enumerate() {
                        push(&mut ranked, &app.name, Item::Remote(h, i));
                    }
                }
            }
            Level::Local => {
                for (i, app) in self.apps.iter().enumerate() {
                    push(&mut ranked, &app.name, Item::Local(i));
                }
            }
            Level::Host(h) => {
                if let Some(host) = self.hosts.get(*h) {
                    for (i, app) in host.apps.iter().enumerate() {
                        push(&mut ranked, &app.name, Item::Remote(*h, i));
                    }
                }
            }
        }
        // The best answers first; among equals, the order they were in.
        ranked.sort_by_key(|(rank, order, _)| (*rank, *order));
        self.shown = ranked.into_iter().map(|(_, _, item)| item).collect();
        self.grid = Grid::new(COLUMNS, self.shown.len());
        self.grid.set_cursor(cursor.min(self.shown.len().saturating_sub(1)));
    }

    pub fn hosts(&self) -> &[HostTab] {
        &self.hosts
    }

    /// Replace the remote computers.
    ///
    /// Called whenever one comes or goes or changes what it offers. Stays inside a computer's
    /// tab if that computer is still there — a catalogue that grows while you look at it
    /// should not throw you out — and otherwise returns to the top.
    pub fn set_hosts(&mut self, hosts: Vec<HostTab>) {
        if hosts == self.hosts {
            return;
        }
        let was_root = self.level == self.root();
        let open = match &self.level {
            Level::Host(i) => self.hosts.get(*i).map(|h| h.address.clone()),
            _ => None,
        };
        self.hosts = hosts;
        let cursor = self.grid.cursor();
        match &self.level {
            Level::Host(_) => match open.and_then(|a| self.hosts.iter().position(|h| h.address == a)) {
                Some(i) => {
                    self.level = Level::Host(i);
                    self.refill(cursor);
                }
                None => {
                    self.level = self.root();
                    self.query.clear();
                    self.refill(0);
                }
            },
            // At the top, and the top has moved: a first computer has arrived, or the last
            // has gone. Nothing typed and nothing chosen yet, so it is where the top now is.
            Level::Local if was_root && self.query.is_empty() && cursor == 0 => {
                self.level = self.root();
                self.refill(0);
            }
            _ => self.refill(cursor),
        }
    }

    // --- typing ---

    /// What has been typed to narrow the bubbles.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// More letters: the bubbles are narrowed to the names they find, best first.
    pub fn type_query(&mut self, text: &str) {
        let before = self.query.len();
        self.query.extend(text.chars().filter(|c| !c.is_control()));
        if self.query.len() != before {
            self.refill(0);
        }
    }

    pub fn query_backspace(&mut self) {
        if self.query.pop().is_some() {
            self.refill(0);
        }
    }

    /// Forget what was typed. `true` if there was anything.
    pub fn clear_query(&mut self) -> bool {
        if self.query.is_empty() {
            return false;
        }
        self.query.clear();
        self.refill(0);
        true
    }

    /// As it is each time it is opened: at the top, with nothing typed.
    pub fn reset(&mut self) {
        self.query.clear();
        self.level = self.root();
        self.machine_cursor = 0;
        self.refill(0);
    }

    // --- what is on show ---

    fn bubble(&self, item: Item) -> (String, Option<String>) {
        match item {
            Item::ThisMachine => (self.local_name.clone(), Some(LOCAL_ICON.to_string())),
            Item::Machine(i) => {
                let h = &self.hosts[i];
                let label = if h.online { h.label.clone() } else { format!("{} (offline)", h.label) };
                (label, Some(HOST_ICON.to_string()))
            }
            Item::Local(i) => (self.apps[i].name.clone(), self.apps[i].icon.clone()),
            Item::Remote(h, i) => {
                let host = &self.hosts[h];
                let app = &host.apps[i];
                // Looking through every machine at once, a remote one's says whose it is.
                let label = if self.level == Level::Machines {
                    format!("{} \u{00b7} {}", app.name, host.label)
                } else {
                    app.name.clone()
                };
                (label, app.icon.clone())
            }
        }
    }

    /// Every bubble on show: its label, and the icon to look up for it.
    ///
    /// One list so the renderer does not need to know what kinds of bubble there are.
    pub fn bubbles(&self) -> Vec<(String, Option<String>)> {
        self.shown.iter().map(|item| self.bubble(*item)).collect()
    }

    pub fn level(&self) -> &Level {
        &self.level
    }

    /// A heading for what is on show: the machine whose applications these are, or nothing at
    /// the top.
    pub fn heading(&self) -> Option<String> {
        match &self.level {
            Level::Machines => None,
            Level::Local => (!self.hosts.is_empty()).then(|| self.local_name.clone()),
            Level::Host(i) => self.hosts.get(*i).map(|h| h.label.clone()),
        }
    }

    /// How many bubbles are on show.
    pub fn len(&self) -> usize {
        self.shown.len()
    }

    /// Enter the focused machine, or return the focused application to launch.
    ///
    /// `Some(launch)` means launch it; `None` means the level changed, or there is nothing
    /// under the cursor.
    pub fn activate(&mut self) -> Option<Launch> {
        let cursor = self.grid.cursor();
        match *self.shown.get(cursor)? {
            Item::ThisMachine => {
                self.machine_cursor = cursor;
                self.level = Level::Local;
                self.refill(0);
                None
            }
            Item::Machine(i) => {
                self.machine_cursor = cursor;
                self.level = Level::Host(i);
                self.refill(0);
                None
            }
            Item::Local(i) => Some(Launch::Local(self.apps[i].clone())),
            Item::Remote(h, i) => Some(Launch::Remote {
                host: self.hosts[h].address.clone(),
                app: self.hosts[h].apps[i].id.clone(),
            }),
        }
    }

    /// Back out one step. `true` if there was somewhere to go.
    ///
    /// What was typed goes first, then the machine is climbed out of. `false` at the top means
    /// the caller should close the launcher entirely — B has to keep working rather than
    /// becoming inert once you are already at the root.
    pub fn back(&mut self) -> bool {
        if self.clear_query() {
            return true;
        }
        if self.level == self.root() {
            return false;
        }
        self.level = Level::Machines;
        self.refill(self.machine_cursor);
        true
    }

    /// Label for whatever is focused, for the caption under the grid.
    pub fn focused_label(&self) -> Option<String> {
        self.shown.get(self.grid.cursor()).map(|item| self.bubble(*item).0)
    }

    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    pub fn apps(&self) -> &[AppEntry] {
        &self.apps
    }

    pub fn cursor(&self) -> usize {
        self.grid.cursor()
    }

    /// The focused application of this machine's, or `None` on anything else.
    pub fn focused(&self) -> Option<AppEntry> {
        match *self.shown.get(self.grid.cursor())? {
            Item::Local(i) => Some(self.apps[i].clone()),
            _ => None,
        }
    }

    /// Replace this machine's applications, returning to the top.
    pub fn set_apps(&mut self, mut apps: Vec<AppEntry>) {
        apps.sort_by_key(|a| a.name.to_lowercase());
        self.apps = apps;
        self.reset();
    }

    /// Put the cursor on a bubble the pointer is over. Only the page on show: a bubble on
    /// another page is not in front of anyone, and jumping to it would turn the page under
    /// the pointer.
    pub fn select(&mut self, index: usize) -> bool {
        if !self.visible().contains(&index) || index == self.grid.cursor() {
            return false;
        }
        self.grid.set_cursor(index);
        true
    }

    pub fn step(&mut self, direction: Direction) -> bool {
        self.grid.step(direction)
    }

    /// Which page the cursor is on. Pages exist so the grid never spills past the field of
    /// view; the alternative is rows you have to find by tilting your head.
    pub fn page(&self) -> usize {
        self.grid.cursor() / PAGE_SIZE
    }

    pub fn pages(&self) -> usize {
        self.len().div_ceil(PAGE_SIZE).max(1)
    }

    /// Indices visible on the current page.
    pub fn visible(&self) -> std::ops::Range<usize> {
        let start = self.page() * PAGE_SIZE;
        start..(start + PAGE_SIZE).min(self.len())
    }

    /// Where a bubble sits.
    ///
    /// Rows are laid out downward from a little above the horizon, so a single-row launcher
    /// sits at a comfortable reading height rather than at your feet.
    pub fn placement(&self, index: usize) -> BubblePlacement {
        // Everything is relative to the page, so bubble 13 on page 2 sits where bubble 1 does.
        let local = index % PAGE_SIZE;
        let row = local / COLUMNS;
        let page_start = (index / PAGE_SIZE) * PAGE_SIZE;
        let on_this_page = (self.len().saturating_sub(page_start)).min(PAGE_SIZE);
        let rows_here = on_this_page.div_ceil(COLUMNS).max(1);
        let in_this_row = if row + 1 < rows_here {
            COLUMNS
        } else {
            on_this_page - row * COLUMNS
        };
        let column_offset = (local % COLUMNS) as f32 - (in_this_row as f32 - 1.0) * 0.5;
        // Centre the block of rows vertically about the eye line.
        let row_offset = row as f32 - (rows_here as f32 - 1.0) * 0.5;
        BubblePlacement {
            // Positive yaw is to the left, and column 0 is the leftmost, so the offset runs
            // against the column index. Getting this backwards mirrors the whole grid and
            // makes the D-pad appear to move the cursor the wrong way.
            yaw: -column_offset * COLUMN_SPACING_DEG.to_radians(),
            pitch: -row_offset * ROW_SPACING_DEG.to_radians(),
            radius: ARC_RADIUS_M,
            scale: if index == self.grid.cursor() {
                1.18
            } else {
                1.0
            },
        }
    }

    /// The bubbles on the current page, in draw order.
    ///
    /// The focused bubble is emitted **last** so it draws over its neighbours: it is scaled up
    /// and would otherwise be clipped by whatever happens to come after it.
    pub fn placements(&self) -> Vec<(usize, BubblePlacement)> {
        let mut out: Vec<(usize, BubblePlacement)> =
            self.visible().map(|i| (i, self.placement(i))).collect();
        out.sort_by_key(|(i, _)| *i == self.grid.cursor());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str) -> AppEntry {
        AppEntry {
            name: name.into(),
            exec: format!("/usr/bin/{}", name.to_lowercase()),
            icon: None,
            categories: vec!["Utility".into()],
        }
    }

    /// A launcher on this machine's applications, which with no other computer is where it opens.
    fn launcher_of(n: usize) -> Launcher {
        Launcher::new((0..n).map(|i| app(&format!("App{i:02}"))).collect())
    }

    #[test]
    fn an_empty_launcher_has_nothing_focused_and_does_not_panic() {
        let mut l = Launcher::new(Vec::new());
        assert!(l.is_empty());
        assert_eq!(l.focused(), None);
        assert_eq!(l.activate(), None);
        assert!(l.placements().is_empty());
    }

    #[test]
    fn the_cursor_starts_on_the_first_app() {
        let l = launcher_of(5);
        assert_eq!(l.focused().unwrap().name, "App00");
    }

    #[test]
    fn applications_are_in_order_of_name_whatever_order_they_were_found_in() {
        let l = Launcher::new(vec![app("zsh"), app("Chrome"), app("alacritty")]);
        let names: Vec<String> = l.bubbles().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["alacritty", "Chrome", "zsh"]);
    }

    #[test]
    fn pressing_right_moves_the_focus_right_in_the_world() {
        let mut l = launcher_of(8);
        let before = l.placement(l.cursor()).yaw;
        l.step(Direction::Right);
        let after = l.placement(l.cursor()).yaw;
        // Positive yaw is left, so moving right must make it smaller.
        assert!(after < before, "{before} -> {after}");
    }

    #[test]
    fn the_focused_bubble_is_larger_than_the_others() {
        let l = launcher_of(4);
        assert!(l.placement(0).scale > l.placement(1).scale);
    }

    #[test]
    fn the_focused_bubble_draws_last() {
        let mut l = launcher_of(8);
        l.step(Direction::Right);
        let order = l.placements();
        assert_eq!(order.last().unwrap().0, l.cursor());
    }

    #[test]
    fn a_full_row_is_centred_on_straight_ahead() {
        let l = launcher_of(COLUMNS);
        let sum: f32 = (0..COLUMNS).map(|i| l.placement(i).yaw).sum();
        assert!(sum.abs() < 1e-5, "row is off-centre by {sum}");
    }

    #[test]
    fn a_single_row_sits_on_the_eye_line() {
        let l = launcher_of(3);
        for i in 0..3 {
            assert!(l.placement(i).pitch.abs() < 1e-6);
        }
    }

    #[test]
    fn every_bubble_is_the_same_distance_away() {
        let l = launcher_of(PAGE_SIZE);
        for i in 0..PAGE_SIZE {
            assert_eq!(l.placement(i).radius, ARC_RADIUS_M);
        }
    }

    #[test]
    fn rescanning_the_app_list_keeps_the_cursor_in_range() {
        let mut l = launcher_of(9);
        for _ in 0..8 {
            l.step(Direction::Right);
        }
        l.set_apps(vec![app("Only")]);
        assert_eq!(l.cursor(), 0);
        assert_eq!(l.focused().unwrap().name, "Only");
    }

    #[test]
    fn a_page_never_spills_past_the_field_of_view() {
        // One eye sees 40 degrees across and 23 down. A bubble is about 4.6 degrees and its
        // label hangs below it.
        let l = launcher_of(PAGE_SIZE);
        for i in 0..PAGE_SIZE {
            let p = l.placement(i);
            assert!(p.yaw.to_degrees().abs() + 2.3 < 20.0, "bubble {i} is past the side");
            assert!(p.pitch.to_degrees().abs() + 3.7 < 11.57, "bubble {i} is past the top or bottom");
        }
    }

    #[test]
    fn only_one_page_is_drawn_at_a_time() {
        let l = launcher_of(PAGE_SIZE * 2 + 3);
        assert_eq!(l.pages(), 3);
        assert_eq!(l.placements().len(), PAGE_SIZE);
    }

    #[test]
    fn moving_past_the_end_of_a_page_turns_to_the_next_one() {
        let mut l = launcher_of(PAGE_SIZE + 2);
        for _ in 0..ROWS_PER_PAGE {
            l.step(Direction::Down);
        }
        assert_eq!(l.page(), 1);
        assert!(l.visible().contains(&l.cursor()));
    }

    #[test]
    fn there_is_real_space_between_neighbouring_bubbles() {
        let l = launcher_of(PAGE_SIZE);
        let gap = (l.placement(0).yaw - l.placement(1).yaw).to_degrees().abs();
        assert!(gap > 4.6 * 1.5, "neighbours are {gap} degrees apart");
    }

    fn tab(address: &str, apps: &[&str]) -> HostTab {
        HostTab {
            label: address.to_string(),
            address: address.to_string(),
            online: true,
            apps: apps
                .iter()
                .map(|a| RemoteEntry { id: a.to_lowercase(), name: a.to_string(), icon: None })
                .collect(),
        }
    }

    // --- machines first ---

    #[test]
    fn with_no_other_computer_it_opens_on_this_ones_applications() {
        let mut l = launcher_of(3);
        assert_eq!(l.level(), &Level::Local);
        assert!(!l.back(), "and there is nowhere above them to go");
    }

    #[test]
    fn with_another_computer_the_machine_is_chosen_first_and_this_one_is_named() {
        let mut l = launcher_of(3);
        l.set_local_name("steamdeck");
        l.set_hosts(vec![tab("deepmagpie", &["Firefox", "Terminal"])]);
        assert_eq!(l.level(), &Level::Machines);
        let names: Vec<String> = l.bubbles().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["steamdeck", "deepmagpie"]);
        assert_eq!(l.activate(), None, "opening a machine launches nothing");
        assert_eq!(l.level(), &Level::Local);
        assert_eq!(l.len(), 3);
        assert!(l.back());
        assert_eq!(l.level(), &Level::Machines);
    }

    #[test]
    fn a_remote_computer_opens_to_its_apps_and_launching_names_it() {
        let mut l = launcher_of(2);
        l.set_hosts(vec![tab("deepmagpie", &["Firefox", "Terminal"])]);
        l.step(Direction::Right);
        assert_eq!(l.activate(), None);
        assert_eq!(l.level(), &Level::Host(0));
        l.step(Direction::Right);
        assert_eq!(
            l.activate(),
            Some(Launch::Remote { host: "deepmagpie".into(), app: "terminal".into() })
        );
        assert!(l.back());
        assert_eq!(l.cursor(), 1, "back on the machine it came out of");
    }

    #[test]
    fn a_device_with_no_applications_of_its_own_and_one_computer_opens_on_that_computer() {
        // A Beam Pro: everything it runs is somewhere else.
        let mut l = Launcher::new(Vec::new());
        l.set_hosts(vec![tab("deepmagpie", &["Firefox"])]);
        assert_eq!(l.level(), &Level::Host(0));
        assert!(!l.back());
    }

    #[test]
    fn an_offline_computer_says_so_on_its_bubble() {
        let mut l = launcher_of(1);
        let mut t = tab("deepmagpie", &[]);
        t.online = false;
        l.set_hosts(vec![t]);
        assert!(l.bubbles().iter().any(|(label, _)| label == "deepmagpie (offline)"));
    }

    #[test]
    fn a_catalogue_that_changes_while_open_keeps_you_in_the_tab() {
        let mut l = launcher_of(1);
        l.set_hosts(vec![tab("a", &["One"]), tab("b", &["Two"])]);
        l.step(Direction::Right);
        l.step(Direction::Right);
        l.activate();
        assert_eq!(l.level(), &Level::Host(1));
        l.set_hosts(vec![tab("x", &[]), tab("a", &["One"]), tab("b", &["Two", "Three"])]);
        assert_eq!(l.level(), &Level::Host(2), "followed by address, not by position");
        assert_eq!(l.len(), 2);
        l.set_hosts(vec![tab("a", &["One"])]);
        assert_eq!(l.level(), &Level::Machines, "and out, once it is gone");
    }

    // --- typing narrows ---

    #[test]
    fn typing_narrows_to_the_names_it_finds_best_first() {
        let mut l = Launcher::new(vec![app("Code"), app("Chrome"), app("Xcode"), app("Visual Studio Code"), app("Notes")]);
        l.type_query("co");
        let names: Vec<String> = l.bubbles().into_iter().map(|(n, _)| n).collect();
        // Starts with it; then a word that does; then anywhere inside.
        assert_eq!(names, ["Code", "Visual Studio Code", "Xcode"]);
        assert_eq!(l.focused().unwrap().name, "Code", "and the best is under the cursor");
    }

    #[test]
    fn initials_find_a_long_name() {
        let mut l = Launcher::new(vec![app("Visual Studio Code"), app("Notes")]);
        l.type_query("vsc");
        assert_eq!(l.len(), 1);
        assert_eq!(l.focused().unwrap().name, "Visual Studio Code");
    }

    #[test]
    fn backspace_widens_again_and_back_clears_what_was_typed_before_leaving() {
        let mut l = launcher_of(5);
        l.type_query("App03");
        assert_eq!(l.len(), 1);
        l.query_backspace();
        assert_eq!(l.len(), 5);
        assert!(l.back(), "the first B forgets what was typed");
        assert_eq!(l.query(), "");
        assert!(!l.back(), "the second leaves");
    }

    #[test]
    fn typing_among_the_machines_looks_through_all_of_them() {
        let mut l = Launcher::new(vec![app("Firefox"), app("Notes")]);
        l.set_hosts(vec![tab("deepmagpie", &["Firefox", "Terminal"])]);
        assert_eq!(l.level(), &Level::Machines);
        l.type_query("fire");
        let names: Vec<String> = l.bubbles().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["Firefox", "Firefox \u{00b7} deepmagpie"]);
        l.step(Direction::Right);
        assert_eq!(
            l.activate(),
            Some(Launch::Remote { host: "deepmagpie".into(), app: "firefox".into() })
        );
    }

    #[test]
    fn opening_again_starts_at_the_top_with_nothing_typed() {
        let mut l = launcher_of(4);
        l.set_hosts(vec![tab("deepmagpie", &["Firefox"])]);
        l.activate();
        l.type_query("app");
        l.reset();
        assert_eq!(l.level(), &Level::Machines);
        assert_eq!(l.query(), "");
    }

    #[test]
    fn nothing_found_is_an_empty_launcher_that_back_recovers_from() {
        let mut l = launcher_of(3);
        l.type_query("zzz");
        assert!(l.is_empty());
        assert_eq!(l.activate(), None);
        assert!(l.back());
        assert_eq!(l.len(), 3);
    }
}
