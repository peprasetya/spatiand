//! What the wearer's hand does in the window beyond a click: pressing and dragging things onto
//! a folder, a place along the side, a tab or a step of the path (moved, or copied where they
//! go to another disk or remote, or with Alt held); a band drawn round several; a name renamed
//! where it is, as Finder does (a click on the name of the one thing chosen, F2, or the menu);
//! and the menus of the right button -- for the things under it, or for the folder itself.

use std::time::{Duration, Instant};

use spatiand_app::ui::{look as room, theme, Field, Icon, List, Painter, Rect, Row, Texture};
use spatiand_app::Modifiers;

use crate::proto::{Item, Kind, Op, Place, ToTab};
use crate::{list_columns, look, opener_for, own_program, places, Action, Opener, Overlay, Shelf, SideRow, SortBy, View, ROW, TILE};

/// How far a press moves before it is a drag (or a band), in the window's pixels.
const DRAG_AFTER: f32 = 7.0;
/// How long a press held still opens the menu: the right button for a pad or a hand.
const HOLD_FOR_MENU: Duration = Duration::from_millis(800);
/// How long after the click on a chosen name its renaming starts: a second click before then
/// is a double click, which opens it instead.
const RENAME_AFTER: Duration = Duration::from_millis(550);
/// How long something dragged over a tab waits before that tab comes to the front.
const SPRING_AFTER: Duration = Duration::from_millis(700);

/// A press of the button in the folder's pane, not yet let go of.
pub struct Press {
    pub at: (f32, f32),
    /// What it was on, by its place among those shown; none for the folder itself.
    pub on: Option<usize>,
    pub when: Instant,
    /// What was chosen stays so while it may be dragged: the press is on one of several. Let
    /// go of without a drag, only that one is chosen.
    pub choose_on_release: bool,
    /// The press was on the name of the one thing chosen: let go of, it is renamed.
    pub rename_on_release: bool,
    pub moved: bool,
}

/// Things being dragged.
pub struct Drag {
    pub items: Vec<Place>,
    /// The folder they are in.
    pub from: Place,
    /// The first of them, to draw under the pointer.
    pub kind: Kind,
    pub picture: Option<Texture>,
    pub target: Option<Target>,
    /// Since when the pointer has been over the tab it is over, for bringing that tab forward.
    pub over_tab: Option<(usize, Instant)>,
    /// The keys held, as of the pointer's last move: Alt copies, Shift moves.
    pub held: Modifiers,
}

/// Where dragged things would go if let go of here.
#[derive(Clone, PartialEq)]
pub struct Target {
    pub place: Place,
    pub rect: Rect,
    pub tab: Option<usize>,
}

/// A band being drawn round things to choose them.
pub struct Band {
    /// Where it started, in the folder's own coordinates (scrolled with it).
    pub from: (f32, f32),
    /// What was chosen before it, kept when Control or Shift was held.
    pub before: std::collections::BTreeSet<usize>,
}

/// A name being typed over, in the folder's pane.
pub struct Renaming {
    pub tab: usize,
    pub name: String,
    pub field: Field,
}

/// The part of a name before its ending: what is chosen to be typed over.
fn stem_chars(name: &str, folder: bool) -> usize {
    if folder {
        return name.chars().count();
    }
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.chars().count(),
        _ => name.chars().count(),
    }
}

/// A shortcut as this machine writes it.
pub fn keys(letter: &str) -> String {
    if cfg!(target_os = "macos") { format!("\u{2318}{letter}") } else { format!("Ctrl+{letter}") }
}

impl Shelf {
    /// Where the thing at `at` (among those shown) is drawn: its whole cell, its face, its name.
    pub(crate) fn cell(&self, at: usize) -> (Rect, Rect, Rect) {
        let tab = &self.tabs[self.active];
        let c = self.content;
        match self.view {
            View::Grid => {
                let columns = self.columns();
                let cell_h = TILE + 64.0;
                let (row, col) = (at / columns, at % columns);
                let cell = Rect::new(c.x + 4.0 + col as f32 * (TILE + theme::TILE_GAP), c.y + row as f32 * cell_h - tab.scroll + 8.0, TILE, cell_h - 8.0);
                let face = Rect { h: TILE, ..cell };
                let name = Rect::new(cell.x + 2.0, face.bottom() + 8.0, cell.w - 4.0, 28.0);
                (cell, face, name)
            }
            View::List => {
                let rect = Rect::new(c.x, c.y + 40.0 + at as f32 * ROW - tab.scroll, c.w, ROW - 4.0);
                let cols = list_columns(rect);
                let face = Rect::new(cols[0].x, rect.y + 7.0, rect.h - 14.0, rect.h - 14.0);
                let name = Rect { x: face.right() + 14.0, w: cols[0].w - face.w - 14.0, ..cols[0] };
                (rect, face, name)
            }
        }
    }

    /// Where a name being renamed is typed: over the name, a little wider.
    pub(crate) fn rename_rect(&self, at: usize) -> Rect {
        let (cell, _, name) = self.cell(at);
        match self.view {
            View::Grid => Rect::new(cell.x - 16.0, name.y - 4.0, cell.w + 32.0, 38.0),
            View::List => Rect { y: cell.y + 6.0, h: cell.h - 12.0, x: name.x - 8.0, w: name.w + 8.0 },
        }
    }

    // --- pressing, dragging, letting go -------------------------------------------------

    pub(crate) fn content_press(&mut self, x: f32, y: f32) {
        let m = self.modifiers;
        // Control-click on a Mac is the right button, as everywhere on a Mac.
        if cfg!(target_os = "macos") && m.control && !m.logo {
            self.context_menu(x, y);
            return;
        }
        // The list's heads sort it.
        if self.view == View::List && y < self.content.y + 40.0 {
            let cols = list_columns(Rect::new(self.content.x, self.content.y, self.content.w, 40.0));
            let by = [SortBy::Name, SortBy::Kind, SortBy::Size, SortBy::Modified];
            if let Some(i) = cols.iter().position(|c| c.contains(x, y)) {
                self.act(Action::Sort(by[i]));
            }
            return;
        }
        let hit = self.hit(x, y);
        let on_name = hit.is_some_and(|at| self.cell(at).2.inset(-4.0).contains(x, y));
        let double = hit.is_some() && self.last_click.is_some_and(|(t, i)| t.elapsed() < Duration::from_millis(450) && Some(i) == hit);
        let tab = &mut self.tabs[self.active];
        let mut press = Press { at: (x, y), on: hit, when: Instant::now(), choose_on_release: false, rename_on_release: false, moved: false };
        match hit {
            Some(at) => {
                let index = tab.shown[at];
                if m.control || m.logo {
                    if !tab.selected.remove(&index) {
                        tab.selected.insert(index);
                    }
                } else if m.shift {
                    let (a, b) = (tab.anchor.min(at), tab.anchor.max(at));
                    tab.selected = tab.shown[a..=b].iter().copied().collect();
                } else if tab.selected.contains(&index) {
                    press.choose_on_release = tab.selected.len() > 1;
                    press.rename_on_release = tab.selected.len() == 1 && on_name && !double;
                    tab.anchor = at;
                } else {
                    tab.selected.clear();
                    tab.selected.insert(index);
                    tab.anchor = at;
                }
                tab.cursor = at;
                self.last_click = Some((Instant::now(), at));
                if double {
                    self.last_click = None;
                    self.rename_due = None;
                    self.open(None);
                    return;
                }
            }
            None => {
                if !(m.control || m.logo || m.shift) {
                    tab.selected.clear();
                }
            }
        }
        self.press = Some(press);
    }

    pub(crate) fn pointer_moved(&mut self, x: f32, y: f32) {
        if let Some(press) = &mut self.press {
            let (dx, dy) = (x - press.at.0, y - press.at.1);
            if !press.moved && dx * dx + dy * dy > DRAG_AFTER * DRAG_AFTER {
                press.moved = true;
                match press.on {
                    Some(_) => self.start_drag(),
                    None => {
                        let scroll = self.tabs[self.active].scroll;
                        let before = if self.modifiers.control || self.modifiers.logo || self.modifiers.shift { self.tabs[self.active].selected.clone() } else { Default::default() };
                        self.band = Some(Band { from: (press.at.0, press.at.1 + scroll), before });
                    }
                }
            }
        }
        if self.drag.is_some() {
            let target = self.drop_target(x, y);
            let now = Instant::now();
            let held = self.app().modifiers();
            let drag = self.drag.as_mut().unwrap();
            drag.held = held;
            match target.as_ref().and_then(|t| t.tab) {
                Some(t) if drag.over_tab.is_none_or(|(o, _)| o != t) => drag.over_tab = Some((t, now)),
                Some(_) => {}
                None => drag.over_tab = None,
            }
            drag.target = target;
        }
        if self.band.is_some() {
            self.band_choose(x, y);
        }
    }

    fn start_drag(&mut self) {
        let tab = &self.tabs[self.active];
        let items = tab.chosen();
        let Some(first) = tab.selected.iter().next().copied().or_else(|| tab.item_at_cursor().map(|(i, _)| i)) else { return };
        let Some(item) = tab.items.get(first) else { return };
        self.drag = Some(Drag { items, from: tab.place.clone(), kind: item.kind, picture: tab.pictures.get(&first).copied(), target: None, over_tab: None, held: self.modifiers });
        self.rename_due = None;
    }

    pub(crate) fn release(&mut self) {
        let press = self.press.take();
        if let Some(drag) = self.drag.take() {
            self.drop(drag);
            return;
        }
        if self.band.take().is_some() {
            return;
        }
        let Some(press) = press.filter(|p| !p.moved) else { return };
        let Some(at) = press.on else { return };
        let tab = &mut self.tabs[self.active];
        if press.choose_on_release {
            if let Some(&index) = tab.shown.get(at) {
                tab.selected.clear();
                tab.selected.insert(index);
            }
        }
        if press.rename_on_release {
            if let Some(&index) = tab.shown.get(at) {
                self.rename_due = Some((Instant::now() + RENAME_AFTER, index));
            }
        }
    }

    /// What is under the pointer that dragged things could go into.
    fn drop_target(&self, x: f32, y: f32) -> Option<Target> {
        let drag = self.drag.as_ref()?;
        let tab = &self.tabs[self.active];
        if self.content.contains(x, y) {
            if let Some(at) = self.hit(x, y) {
                let item = &tab.items[tab.shown[at]];
                let place = tab.place.join(&item.name);
                if item.kind == Kind::Folder && !drag.items.contains(&place) {
                    return Some(Target { place, rect: self.cell(at).1, tab: None });
                }
            }
            return Some(Target { place: tab.place.clone(), rect: self.content, tab: None });
        }
        if let Some(i) = self.placed_tabs.iter().position(|(r, _)| r.contains(x, y)) {
            return Some(Target { place: self.tabs[i].place.clone(), rect: self.placed_tabs[i].0, tab: Some(i) });
        }
        if let Some((rect, place)) = self.trail.iter().find(|(r, _)| r.contains(x, y)) {
            return Some(Target { place: place.clone(), rect: *rect, tab: None });
        }
        let row = crate::side_row_at(&self.side, x, y)?;
        match self.side_rows.get(row) {
            Some(SideRow::Go(place)) => Some(Target { place: place.clone(), rect: self.side.row_rect(row).unwrap_or_default(), tab: None }),
            _ => None,
        }
    }

    /// Whether things going from `from` to `to` are copied rather than moved: when they go to
    /// another disk or remote, as Finder has it, or with Alt (Option) held; Shift (Command on a
    /// Mac) moves them whatever.
    fn copying(m: Modifiers, from: &Place, to: &Place) -> bool {
        if m.alt {
            return true;
        }
        if m.shift || (cfg!(target_os = "macos") && m.logo) {
            return false;
        }
        match (from, to) {
            (Place::Local(a), Place::Local(b)) => {
                use std::os::unix::fs::MetadataExt;
                let device = |p: &str| std::fs::metadata(p).map(|m| m.dev()).ok();
                device(a).is_none() || device(a) != device(b)
            }
            (Place::Remote(a, _), Place::Remote(b, _)) => a != b,
            _ => true,
        }
    }

    fn drop(&mut self, drag: Drag) {
        let Some(target) = drag.target else { return };
        if target.place == drag.from || drag.items.iter().any(|i| target.place == *i) {
            return;
        }
        let copy = Self::copying(self.app().modifiers(), &drag.from, &target.place);
        self.send(drag.items, target.place, copy);
    }

    /// Things copied or moved into a folder, as a job of the tab's.
    pub(crate) fn send(&mut self, items: Vec<Place>, to: Place, copy: bool) {
        if items.is_empty() {
            return;
        }
        let what = if items.len() == 1 { items[0].name() } else { format!("{} items", items.len()) };
        self.say(format!("{} {what} to {}\u{2026}", if copy { "Copying" } else { "Moving" }, to.name()));
        let id = self.next_op;
        self.next_op += 1;
        let op = if copy { Op::Copy { from: items, to } } else { Op::Move { from: items, to } };
        self.tab().tell(&ToTab::Do { id, op });
    }

    fn band_choose(&mut self, x: f32, y: f32) {
        let Some(band) = &self.band else { return };
        let scroll = self.tabs[self.active].scroll;
        let (x0, y0) = (band.from.0, band.from.1 - scroll);
        let rect = Rect::new(x0.min(x), y0.min(y), (x - x0).abs(), (y - y0).abs());
        let before = band.before.clone();
        let count = self.tabs[self.active].shown.len();
        let inside: Vec<usize> = (0..count).filter(|&at| overlaps(self.cell(at).1, rect) || overlaps(self.cell(at).2, rect)).collect();
        let tab = &mut self.tabs[self.active];
        tab.selected = before;
        for at in inside {
            tab.selected.insert(tab.shown[at]);
        }
        if let Some(&last) = tab.selected.iter().next_back() {
            if let Some(at) = tab.shown.iter().position(|&i| i == last) {
                tab.cursor = at;
            }
        }
    }

    /// What waits on time: a press held for the menu, a rename after a click, a tab coming
    /// forward under a drag, and the pane scrolling under a drag at its edge.
    pub(crate) fn tick(&mut self) {
        if let Some(press) = &self.press {
            if !press.moved && press.on.is_some() && press.when.elapsed() >= HOLD_FOR_MENU {
                let at = press.at;
                self.press = None;
                self.context_menu(at.0, at.1);
                self.dirty = true;
            }
        }
        if let Some((due, index)) = self.rename_due {
            if Instant::now() >= due {
                self.rename_due = None;
                let tab = &self.tabs[self.active];
                if tab.selected.len() == 1 && tab.selected.contains(&index) {
                    self.start_rename(index);
                }
            }
        }
        if let Some(drag) = &self.drag {
            if let Some((t, since)) = drag.over_tab {
                if t != self.active && since.elapsed() >= SPRING_AFTER && t < self.tabs.len() {
                    self.active = t;
                    self.dirty = true;
                }
            }
        }
        if self.drag.is_some() || self.band.is_some() {
            let (x, y) = self.pointer;
            let c = self.content;
            let by = if y < c.y + 30.0 && y > c.y - 40.0 { -18.0 } else if y > c.bottom() - 30.0 && y < c.bottom() + 40.0 { 18.0 } else { 0.0 };
            if by != 0.0 && x > c.x && x < c.right() {
                let tab = self.tab();
                tab.scroll = (tab.scroll + by).max(0.0);
                self.pointer_moved(x, y);
                self.dirty = true;
            }
        }
    }

    /// Whether anything waits on time, for the window to wake for it.
    pub(crate) fn waiting(&self) -> bool {
        self.press.is_some() || self.rename_due.is_some() || self.drag.is_some() || self.band.is_some()
    }

    // --- renaming where the name is -----------------------------------------------------

    pub(crate) fn start_rename(&mut self, index: usize) {
        let tab = &self.tabs[self.active];
        let Some(item) = tab.items.get(index) else { return };
        let (name, folder) = (item.name.clone(), item.kind == Kind::Folder);
        let at = tab.shown.iter().position(|&i| i == index);
        let em = if self.view == View::Grid { theme::ROW_EM * 0.62 } else { theme::ROW_EM * 0.7 };
        let mut field = Field::sized(em);
        field.focused = true;
        field.set(name.clone());
        field.select(0, stem_chars(&name, folder));
        if let Some(at) = at {
            self.tabs[self.active].cursor = at;
        }
        self.renaming = Some(Renaming { tab: self.active, name, field });
        self.dirty = true;
    }

    /// The name typed is the thing's new name.
    pub(crate) fn finish_rename(&mut self) {
        let Some(r) = self.renaming.take() else { return };
        let name = r.field.text.trim().to_string();
        if name.is_empty() || name == r.name {
            return;
        }
        if name.contains('/') {
            self.fail("A name may not have a / in it.");
            return;
        }
        let Some(tab) = self.tabs.get_mut(r.tab) else { return };
        let item = tab.place.join(&r.name);
        tab.land_on = Some(name.clone());
        let id = self.next_op;
        self.next_op += 1;
        self.tabs[r.tab].tell(&ToTab::Do { id, op: Op::Rename { item, name } });
    }

    /// The folder was listed again: a name being renamed that is not in it any more is not.
    pub(crate) fn relisted(&mut self, tab: usize) {
        if let Some(r) = &self.renaming {
            if r.tab == tab && !self.tabs[tab].items.iter().any(|i| i.name == r.name) {
                self.renaming = None;
            }
        }
    }

    /// Where on show the name being renamed is, if it is on show in the tab in front.
    pub(crate) fn renaming_at(&self) -> Option<usize> {
        let r = self.renaming.as_ref().filter(|r| r.tab == self.active)?;
        let tab = &self.tabs[self.active];
        tab.shown.iter().position(|&i| tab.items[i].name == r.name)
    }

    // --- menus --------------------------------------------------------------------------

    /// The right button at a point of the window: the menu for what is there.
    pub(crate) fn context_menu(&mut self, x: f32, y: f32) {
        self.finish_rename();
        self.press = None;
        if self.content.contains(x, y) {
            match self.hit(x, y) {
                Some(at) => {
                    let tab = self.tab();
                    tab.cursor = at;
                    if !tab.selected.contains(&tab.shown[at]) {
                        tab.selected.clear();
                        tab.selected.insert(tab.shown[at]);
                        tab.anchor = at;
                    }
                    self.item_menu((x, y));
                }
                None => {
                    self.tab().selected.clear();
                    self.folder_menu((x, y));
                }
            }
        } else if self.side.contains(x, y) {
            if let Some(row) = crate::side_row_at(&self.side, x, y) {
                self.side_menu(row, (x, y));
            }
        }
    }

    /// The menu key, or Shift-F10: the menu for the thing at the cursor, beside it.
    pub(crate) fn menu_at_cursor(&mut self) {
        let tab = &self.tabs[self.active];
        if tab.shown.is_empty() {
            let c = self.content;
            self.folder_menu((c.x + 40.0, c.y + 40.0));
            return;
        }
        let (_, face, _) = self.cell(tab.cursor);
        let tab = self.tab();
        let index = tab.shown[tab.cursor];
        if !tab.selected.contains(&index) {
            tab.selected.clear();
            tab.selected.insert(index);
        }
        self.item_menu((face.right() - 10.0, face.y + 20.0));
    }

    pub(crate) fn open_menu(&mut self, at: (f32, f32), entries: Vec<(Row, Action)>) {
        self.menu_at = at;
        let mut list = List::default();
        list.focused = true;
        self.overlay = Overlay::Menu { at, list, entries };
    }

    fn item_menu(&mut self, at: (f32, f32)) {
        let tab = &self.tabs[self.active];
        let Some((_, item)) = tab.item_at_cursor() else { return };
        let item: Item = item.clone();
        let place = tab.place.join(&item.name);
        let local = matches!(tab.place, Place::Local(_));
        let several = tab.selected.len() > 1;
        let mut entries: Vec<(Row, Action)> = vec![(Row::with("Open", "Enter").icon(look::icon(item.kind)), Action::Open)];
        if item.kind == Kind::Folder {
            entries.push((Row::new("Open in a new tab").icon(Icon::Plus), Action::OpenInTab(place.clone())));
            if local && own_program("spatiquill").is_some() {
                entries.push((Row::new("Open in SpatiQuill").icon(Icon::Terminal), Action::OpenWith(Opener::Own("spatiquill"))));
            }
        } else {
            for (name, program) in [("SpatiLoupe", "spatiloupe"), ("SpatiReel", "spatireel")] {
                if own_program(program).is_some() && opener_for(item.kind) != Some(program) {
                    entries.push((Row::new(format!("Open with {name}")).icon(Icon::Eye), Action::OpenWith(Opener::Own(program))));
                }
            }
            if !cfg!(target_os = "android") {
                let other = if cfg!(target_os = "macos") { "Open with the Mac's app" } else { "Open with another app" };
                entries.push((Row::new(other).icon(Icon::Link), Action::OpenWith(Opener::Native)));
            }
        }
        let count = if several { format!(" {} items", tab.selected.len()) } else { String::new() };
        entries.push((Row::with(format!("Copy{count}"), keys("C")).icon(Icon::Copy), Action::Copy));
        entries.push((Row::with(format!("Cut{count}"), keys("X")).icon(Icon::Cut), Action::Cut));
        if item.kind == Kind::Folder && self.clipboard.is_some() {
            entries.push((Row::new("Paste into this folder").icon(Icon::Paste), Action::PasteInto(place.clone())));
        }
        entries.push((Row::new("Move to\u{2026}").icon(Icon::Forward), Action::MoveToMenu));
        entries.push((Row::new("Copy to\u{2026}").icon(Icon::Copy), Action::CopyToMenu));
        if local {
            entries.push((Row::with("Duplicate", keys("D")).icon(Icon::Copy), Action::Duplicate));
        }
        if !several {
            entries.push((Row::with("Rename", "F2").icon(Icon::Pencil), Action::Rename));
        }
        let remote = !local;
        let away = if remote || cfg!(target_os = "android") { "Delete\u{2026}" } else { "Move to the trash" };
        entries.push((Row::with(away, if cfg!(target_os = "macos") { "\u{2318}\u{232b}" } else { "Delete" }).icon(Icon::Trash), Action::Trash));
        entries.push((Row::new("Copy the path").icon(Icon::Link), Action::CopyPath));
        self.open_menu(at, entries);
    }

    fn folder_menu(&mut self, at: (f32, f32)) {
        let local = matches!(self.tabs[self.active].place, Place::Local(_));
        let mut entries: Vec<(Row, Action)> = vec![(Row::with("New folder", if cfg!(target_os = "macos") { "\u{21e7}\u{2318}N" } else { "Ctrl+Shift+N" }).icon(Icon::Folder), Action::NewFolder)];
        if self.clipboard.is_some() {
            entries.push((Row::with("Paste here", keys("V")).icon(Icon::Paste), Action::Paste));
        }
        let (view, icon) = if self.view == View::Grid { ("Show as a list", Icon::List) } else { ("Show as tiles", Icon::Grid) };
        entries.push((Row::new(view).icon(icon), Action::SetView(if self.view == View::Grid { View::List } else { View::Grid })));
        entries.push((Row::with("Sort by\u{2026}", self.sort.0.word()).icon(Icon::SortName), Action::SortMenu));
        entries.push((Row::with(if self.hidden { "Hide hidden files" } else { "Show hidden files" }, if cfg!(target_os = "macos") { "\u{21e7}\u{2318}." } else { "Ctrl+H" }).icon(Icon::Eye), Action::ToggleHidden));
        if local && own_program("spatiquill").is_some() {
            entries.push((Row::new("Open SpatiQuill here").icon(Icon::Terminal), Action::TerminalHere));
        }
        entries.push((Row::new("Open in a new tab").icon(Icon::Plus), Action::OpenInTab(self.tabs[self.active].place.clone())));
        entries.push((Row::with("Refresh", keys("R")).icon(Icon::Refresh), Action::Refresh));
        entries.push((Row::new("Copy the path").icon(Icon::Link), Action::CopyPath));
        entries.push((Row::new("About SpatiShelf").icon(Icon::Info), Action::About));
        self.open_menu(at, entries);
    }

    pub(crate) fn side_menu(&mut self, row: usize, at: (f32, f32)) {
        let Some(SideRow::Go(place)) = self.side_rows.get(row).cloned() else { return };
        let mut entries = vec![(Row::new("Open in a new tab").icon(Icon::Plus), Action::OpenInTab(place.clone()))];
        if self.clipboard.is_some() {
            entries.push((Row::new("Paste here").icon(Icon::Paste), Action::PasteInto(place.clone())));
        }
        if let Place::Remote(name, _) = &place {
            entries.push((Row::new(format!("Forget {name}\u{2026}")).icon(Icon::Close), Action::Forget(name.clone())));
        }
        self.open_menu(at, entries);
    }

    /// Where chosen things can be sent: the places along the side, the folders open in other
    /// tabs, and the folder above, but not where they are.
    pub(crate) fn destinations_menu(&mut self, at: (f32, f32), copy: bool) {
        let here = self.tabs[self.active].place.clone();
        let mut seen = vec![here.clone()];
        let mut entries = Vec::new();
        let mut add = |name: String, place: Place, icon: Icon, entries: &mut Vec<(Row, Action)>| {
            if seen.contains(&place) {
                return;
            }
            seen.push(place.clone());
            let within = place.parent().map(|p| short(&p)).unwrap_or_default();
            entries.push((Row::with(name, within).icon(icon), Action::Send { to: place, copy }));
        };
        if let Some(up) = here.parent() {
            add(up.name(), up, Icon::Up, &mut entries);
        }
        for tab in &self.tabs {
            add(tab.place.name(), tab.place.clone(), Icon::Grid, &mut entries);
        }
        for (name, place) in places() {
            add(name, place, Icon::Folder, &mut entries);
        }
        for remote in &self.remotes {
            add(remote.name.clone(), Place::Remote(remote.name.clone(), String::new()), Icon::Cloud, &mut entries);
        }
        self.open_menu(at, entries);
    }

    pub(crate) fn sort_menu(&mut self, at: (f32, f32)) {
        let entries = [SortBy::Name, SortBy::Kind, SortBy::Size, SortBy::Modified]
            .into_iter()
            .map(|by| {
                let mark = if self.sort.0 == by { if self.sort.1 { "\u{25bc}" } else { "\u{25b2}" } } else { "" };
                (Row::with(by.word(), mark).icon(by.icon()), Action::Sort(by))
            })
            .collect();
        self.open_menu(at, entries);
    }

    // --- drawing ------------------------------------------------------------------------

    /// The band, what a drag would go into, and what is dragged, under the pointer.
    pub(crate) fn paint_hand(&mut self, p: &mut Painter, gl: &spatiand_app::Gl) {
        if let Some(band) = &self.band {
            let scroll = self.tabs[self.active].scroll;
            let (x0, y0) = (band.from.0, band.from.1 - scroll);
            let (x, y) = self.pointer;
            let rect = Rect::new(x0.min(x), y0.min(y), (x - x0).abs(), (y - y0).abs());
            p.clip(gl, self.content);
            p.fill(gl, rect, 4.0, [0.42, 0.68, 1.0, 0.16]);
            for edge in [Rect { h: 1.5, ..rect }, Rect { y: rect.bottom() - 1.5, h: 1.5, ..rect }, Rect { w: 1.5, ..rect }, Rect { x: rect.right() - 1.5, w: 1.5, ..rect }] {
                p.fill(gl, edge, 0.0, [0.45, 0.72, 1.0, 0.8]);
            }
            p.unclip(gl);
        }
        let Some(drag) = self.drag.as_ref() else { return };
        let copy = match &drag.target {
            Some(t) if t.place != drag.from => Self::copying(drag.held, &drag.from, &t.place),
            _ => false,
        };
        let useful = drag.target.as_ref().filter(|t| t.place != drag.from && !drag.items.contains(&t.place));
        if let Some(target) = useful {
            if target.rect != self.content {
                p.glow(gl, target.rect, 16.0, [0.30, 0.62, 1.0, 0.45], 18.0);
                p.fill(gl, target.rect.inset(-3.0), 16.0, [0.42, 0.68, 1.0, 0.22]);
            }
        }
        let (x, y) = self.pointer;
        let ghost = Rect::new(x + 16.0, y + 12.0, 96.0, 96.0);
        p.glow(gl, ghost, 18.0, [0.0, 0.0, 0.0, 0.5], 18.0);
        look::face(p, gl, ghost, drag.kind, drag.picture, 16.0);
        let n = drag.items.len();
        if n > 1 || copy {
            let badge = Rect::new(ghost.right() - 26.0, ghost.y - 12.0, 40.0, 40.0);
            p.fill(gl, badge, 20.0, if copy { room::TEAL } else { theme::mark() });
            let words = if copy && n > 1 { format!("+{n}") } else if copy { "+".into() } else { n.to_string() };
            p.label(gl, &words, badge, theme::FOOTER_EM * 0.9, [1.0, 1.0, 1.0, 1.0], 0.5);
        }
        if let Some(target) = useful {
            let words = format!("{} to {}", if copy { "Copy" } else { "Move" }, target.place.name());
            let w = p.measure(gl, &words, theme::FOOTER_EM, 4096.0).0 + 32.0;
            let pill = Rect::new(ghost.x, ghost.bottom() + 10.0, w, 40.0);
            p.fill(gl, pill, 20.0, theme::ground());
            room::glass(p, gl, pill, 20.0, 1.0);
            p.label(gl, &words, pill.inset(16.0), theme::FOOTER_EM, theme::ink(), 0.0);
        }
    }
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

/// Where a place is, in a few words, for a menu's second column.
fn short(place: &Place) -> String {
    let text = place.text();
    let home = std::env::var("HOME").unwrap_or_default();
    let text = if !home.is_empty() && text.starts_with(&home) { format!("~{}", &text[home.len()..]) } else { text };
    if text.chars().count() > 18 {
        let tail: String = text.chars().rev().take(16).collect::<Vec<_>>().into_iter().rev().collect();
        format!("\u{2026}{tail}")
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_is_chosen_but_its_ending() {
        assert_eq!(stem_chars("holiday.jpeg", false), 7);
        assert_eq!(stem_chars("archive.tar.gz", false), 11);
        assert_eq!(stem_chars(".bashrc", false), 7, "a hidden file is all name");
        assert_eq!(stem_chars("no ending", false), 9);
        assert_eq!(stem_chars("photos.2024", true), 11, "a folder's dot is part of its name");
        assert_eq!(stem_chars("café.txt", false), 4, "counted in characters");
    }

    #[test]
    fn overlapping() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(overlaps(a, Rect::new(5.0, 5.0, 10.0, 10.0)));
        assert!(!overlaps(a, Rect::new(10.0, 0.0, 5.0, 5.0)));
    }
}
