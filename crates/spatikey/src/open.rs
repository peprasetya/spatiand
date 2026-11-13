//! What SpatiKey shows while the vault is open: the list, one entry, the editor, and settings.
//!
//! The list is drawn from summaries alone; an entry's secrets are asked for when its page opens
//! and overwritten when it closes. A password is dots until the eye is pressed, and nothing is
//! ever put on the clipboard -- the room's clipboard reaches every computer it is linked to.

use std::time::Instant;

use spatiand_app::ui::{look, theme, Button, Field, Icon, Input, List, Painter, Rect, Row, Tabs, Typed};
use spatiand_app::{Gl, Key};
use spatikey_core::entry::{Entry, Kind, Summary, Totp};
use spatikey_core::wire;
use zeroize::{Zeroize, Zeroizing};

use crate::locked::is_copy;
use crate::ui::{self, Confirm, View, Want};

/// What an entry is called: its title, or failing that what it is for.
pub fn title_of(summary: &Summary) -> String {
    if !summary.title.trim().is_empty() {
        return summary.title.clone();
    }
    summary
        .passkey_rp
        .clone()
        .or_else(|| summary.sites.first().map(|s| site_shown(s)))
        .or_else(|| summary.apps.first().cloned())
        .unwrap_or_else(|| "Untitled".into())
}

/// A site as it is read: a whole origin without its scheme, a domain as it is.
fn site_shown(site: &str) -> String {
    site.split_once("://").map_or(site, |(_, rest)| rest).to_string()
}

// --- the list ---

const KINDS: [&str; 4] = ["All", "Logins", "Passkeys", "Codes"];

/// Every entry, by title: a search field over them, and tabs for logins, passkeys and the
/// entries with a two-factor code.
pub struct ListPage {
    search: Field,
    tabs: Tabs,
    list: List,
    add: Button,
    cog: Button,
    lock: Button,
    /// The entries on show, by their place in the summaries, in the order shown.
    shown: Vec<usize>,
}

impl ListPage {
    pub fn new() -> ListPage {
        let mut search = Field::default();
        search.hint = "Search".into();
        search.focused = true;
        ListPage { search, tabs: Tabs::default(), list: ui::keyed_list(), add: Button::default(), cog: Button::default(), lock: Button::default(), shown: Vec::new() }
    }

    /// The entries the tab and the search let through, by title.
    fn choose(&mut self, entries: &[Summary]) {
        let words: Vec<String> = self.search.text.split_whitespace().map(str::to_lowercase).collect();
        let mut shown: Vec<usize> = entries.iter().enumerate().filter(|(_, s)| of_kind(s, self.tabs.active) && says(s, &words)).map(|(i, _)| i).collect();
        shown.sort_by_cached_key(|&i| title_of(&entries[i]).to_lowercase());
        self.shown = shown;
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, view: &View) {
        self.choose(view.entries);
        let mut all = ui::page(p);
        let beside = match view.entries.len() {
            0 => String::new(),
            1 => "1 entry".into(),
            n => format!("{n} entries"),
        };
        look::header(p, gl, &mut all, "SpatiKey", &beside);
        ui::foot(p, gl, &mut all, "[\u{2191}] [\u{2193}] Choose  [Enter] Open  [Tab] Next kind  [Esc] Clear", view);
        let mut top = all.take_top(Field::HEIGHT);
        let lock = top.take_right(Button::icon_width(p, gl, "Lock"));
        top.take_right(12.0);
        let cog = top.take_right(Button::HEIGHT);
        top.take_right(12.0);
        let add = top.take_right(Button::icon_width(p, gl, "Add"));
        top.take_right(20.0);
        self.search.focused = true;
        self.search.draw(p, gl, top);
        self.add.draw_icon(p, gl, add, Icon::Plus, "Add");
        self.cog.draw_icon(p, gl, cog, Icon::Cog, "");
        self.lock.draw_icon(p, gl, lock, Icon::Lock, "Lock");
        all.take_top(16.0);
        self.tabs.draw(p, gl, all.take_top(Tabs::HEIGHT), &KINDS);
        all.take_top(16.0);
        if self.shown.is_empty() {
            let said = if view.entries.is_empty() {
                "Nothing in SpatiKey yet. Add a login with Add.".to_string()
            } else if !self.search.text.trim().is_empty() {
                format!("Nothing here matches \u{201C}{}\u{201D}.", self.search.text.trim())
            } else {
                format!("No {} yet.", KINDS[self.tabs.active.min(3)].to_lowercase())
            };
            look::message(p, gl, Rect { h: all.h * 0.6, ..all }, &said, theme::ink_hint());
            return;
        }
        let rows: Vec<Row> = self.shown.iter().map(|&i| row_of(&view.entries[i])).collect();
        self.list.focused = true;
        self.list.draw(p, gl, all, &rows);
    }

    fn open(&self, row: usize, view: &View) -> Want {
        match self.shown.get(row).and_then(|&i| view.entries.get(i)) {
            Some(summary) => Want::Show(summary.id.clone()),
            None => Want::Nothing,
        }
    }

    pub fn input(&mut self, input: &Input, view: &View) -> Want {
        self.choose(view.entries);
        if self.add.input(input) {
            return Want::Add;
        }
        if self.cog.input(input) {
            return Want::Settings;
        }
        if self.lock.input(input) {
            return Want::Lock;
        }
        // The keys: a row's with the arrows and Enter, the search's otherwise, and Tab the
        // kind's, since Left and Right move the search's caret.
        if let Input::Key { key, modifiers, .. } = input {
            let command = modifiers.control || modifiers.logo;
            match key {
                Key::Tab => {
                    let kinds = KINDS.len();
                    self.tabs.active = if modifiers.shift { (self.tabs.active + kinds - 1) % kinds } else { (self.tabs.active + 1) % kinds };
                    self.list.cursor = 0;
                    return Want::Nothing;
                }
                Key::Escape => {
                    self.search.set("");
                    self.list.cursor = 0;
                    return Want::Nothing;
                }
                Key::Char('n') if command => return Want::Add,
                Key::Char('l') if command => return Want::Lock,
                Key::Up | Key::Down | Key::PageUp | Key::PageDown | Key::Enter => {
                    return match self.list.input(input) {
                        Some(row) => self.open(row, view),
                        None => Want::Nothing,
                    };
                }
                _ => {}
            }
            if self.search.input(input) == Some(Typed::Changed) {
                self.list.cursor = 0;
            }
            return Want::Nothing;
        }
        if self.tabs.input(input).is_some() {
            self.list.cursor = 0;
        }
        if let Some(row) = self.list.input(input) {
            return self.open(row, view);
        }
        Want::Nothing
    }
}

fn of_kind(summary: &Summary, tab: usize) -> bool {
    match tab {
        1 => summary.kind == Kind::Login,
        2 => summary.kind == Kind::Passkey,
        3 => summary.has_totp,
        _ => true,
    }
}

/// Whether every word searched for is in what the entry is called, who it is for, or where.
fn says(summary: &Summary, words: &[String]) -> bool {
    if words.is_empty() {
        return true;
    }
    let mut said = vec![summary.title.to_lowercase(), summary.username.to_lowercase()];
    said.extend(summary.sites.iter().chain(&summary.apps).map(|s| s.to_lowercase()));
    said.extend(summary.passkey_rp.iter().chain(&summary.passkey_user).map(|s| s.to_lowercase()));
    words.iter().all(|word| said.iter().any(|s| s.contains(word.as_str())))
}

fn row_of(summary: &Summary) -> Row {
    let icon = match summary.kind {
        Kind::Passkey => Icon::Lock,
        Kind::Note => Icon::Document,
        Kind::Login if summary.sites.is_empty() && !summary.apps.is_empty() => Icon::Desktop,
        Kind::Login => Icon::Globe,
    };
    let who = match summary.kind {
        Kind::Passkey => summary.passkey_user.clone().unwrap_or_default(),
        _ => summary.username.clone(),
    };
    let at = summary.passkey_rp.clone().or_else(|| summary.sites.first().map(|s| site_shown(s))).or_else(|| summary.apps.first().cloned()).unwrap_or_default();
    let trailing: Vec<String> = [who, at].into_iter().filter(|s| !s.is_empty()).collect();
    Row { label: title_of(summary), trailing: (!trailing.is_empty()).then(|| trailing.join("  \u{b7}  ")), heading: false, icon: Some(icon) }
}

// --- one entry ---

/// The current one-time code, as the service gave it.
pub struct Current {
    pub code: Zeroizing<String>,
    /// When it stops being the code.
    pub until: Instant,
    pub period: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    TypeBoth,
    TypePassword,
    TypeCode,
    Eye,
    Edit,
    Delete,
}

/// An entry: what it is on the left, and on the right what can be typed of it into the window
/// the wearer was in before SpatiKey.
pub struct EntryPage {
    pub id: String,
    /// The whole entry, once the service has given it.
    pub entry: Option<Entry>,
    opened: Instant,
    shown: bool,
    pub totp: Option<Current>,
    /// A code has been asked for and has not come.
    pub totp_asked: bool,
    /// The service would not give a code: it is not asked again while the page is open.
    pub totp_refused: bool,
    back: Button,
    edit: Button,
    delete: Button,
    eye: Button,
    type_both: Button,
    type_password: Button,
    type_code: Button,
    focus: usize,
    confirm: Option<Confirm>,
}

impl EntryPage {
    pub fn new(id: String) -> EntryPage {
        EntryPage {
            id,
            entry: None,
            opened: Instant::now(),
            shown: false,
            totp: None,
            totp_asked: false,
            totp_refused: false,
            back: Button::default(),
            edit: Button::default(),
            delete: Button::default(),
            eye: Button::default(),
            type_both: Button::default(),
            type_password: Button::default(),
            type_code: Button::default(),
            focus: 0,
            confirm: None,
        }
    }

    /// Whether the code shown has run out (or none has come yet) and another is to be asked for.
    pub fn wants_code(&self) -> bool {
        let has = self.entry.as_ref().is_some_and(|e| e.totp.is_some());
        has && !self.totp_asked && !self.totp_refused && self.totp.as_ref().map_or(true, |c| Instant::now() >= c.until)
    }

    /// What can be done here, in the order the keys go through it.
    fn actions(&self, view: &View) -> Vec<Action> {
        let mut actions = Vec::new();
        if let Some(entry) = &self.entry {
            let typing = view.target.is_some() && entry.kind == Kind::Login;
            if typing && !entry.username.is_empty() && !entry.password.is_empty() {
                actions.push(Action::TypeBoth);
            }
            if typing && !entry.password.is_empty() {
                actions.push(Action::TypePassword);
            }
            if typing && entry.totp.is_some() {
                actions.push(Action::TypeCode);
            }
            if !entry.password.is_empty() {
                actions.push(Action::Eye);
            }
            actions.push(Action::Edit);
            actions.push(Action::Delete);
        }
        actions
    }

    fn lit(&mut self, actions: &[Action]) {
        let at = actions.get(self.focus).copied();
        self.type_both.focused = at == Some(Action::TypeBoth);
        self.type_password.focused = at == Some(Action::TypePassword);
        self.type_code.focused = at == Some(Action::TypeCode);
        self.eye.focused = at == Some(Action::Eye);
        self.edit.focused = at == Some(Action::Edit);
        self.delete.focused = at == Some(Action::Delete);
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, view: &View) {
        let summary = view.entries.iter().find(|s| s.id == self.id);
        let title = match (&self.entry, summary) {
            (Some(entry), _) if !entry.title.trim().is_empty() => entry.title.clone(),
            (Some(entry), _) => title_of(&entry.summary()),
            (None, Some(summary)) => title_of(summary),
            (None, None) => String::new(),
        };
        let actions = self.actions(view);
        self.focus = self.focus.min(actions.len().saturating_sub(1));
        self.lit(&actions);
        let mut all = ui::page(p);
        let mut right = ui::top(p, gl, &mut all, &mut self.back, "Back", Icon::Back, &title);
        if self.entry.is_some() {
            let delete_w = Button::icon_width(p, gl, "Delete");
            self.delete.draw_icon(p, gl, right.take_right(delete_w), Icon::Trash, "Delete");
            right.take_right(12.0);
            let edit_w = Button::icon_width(p, gl, "Edit");
            self.edit.draw_icon(p, gl, right.take_right(edit_w), Icon::Pencil, "Edit");
        }
        ui::foot(p, gl, &mut all, "[Tab] Next  [Enter] Press  [Esc] Back", view);
        let Some(entry) = self.entry.as_ref() else {
            look::waiting(p, gl, Rect { h: theme::ROW_HEIGHT * 2.0, ..all }, self.opened);
            return;
        };
        let gap = 56.0;
        let mut left = all.take_left((all.w - gap) * 0.44);
        all.take_left(gap);
        let mut right = all;

        match entry.kind {
            Kind::Passkey => {
                let passkey = entry.passkey.clone().unwrap_or_default();
                detail(p, gl, &mut left, "Site", &passkey.rp_id);
                detail(p, gl, &mut left, "Account", &passkey.user_name);
                if !passkey.user_display.is_empty() && passkey.user_display != passkey.user_name {
                    detail(p, gl, &mut left, "Name", &passkey.user_display);
                }
                detail(p, gl, &mut left, "Made", &ui::date(entry.created));
                notes(p, gl, &mut left, &entry.notes);
                look::shelf(p, gl, &mut right, "A passkey", true);
                ui::prose(
                    p,
                    gl,
                    &mut right,
                    &format!("SpatiMesh and the browser extensions sign in to {} with it, once Touch ID says yes. It is never typed or shown.", passkey.rp_id),
                    theme::ink_row(),
                );
            }
            Kind::Note => notes(p, gl, &mut left, &entry.notes),
            Kind::Login => {
                if !entry.username.is_empty() {
                    detail(p, gl, &mut left, "Username", &entry.username);
                }
                if !entry.password.is_empty() {
                    // Dots of one length whatever the password's, which is not to be read off.
                    let shown = if self.shown { entry.password.clone() } else { "\u{2022}".repeat(12) };
                    let mut line = detail(p, gl, &mut left, "Password", "");
                    let eye = line.take_right(theme::ROW_HEIGHT * 0.84);
                    p.label(gl, &shown, Rect { w: (line.w - 12.0).max(0.0), ..line }, VALUE_EM, theme::ink(), 0.0);
                    self.eye.draw_icon(p, gl, Rect { y: eye.y + (eye.h - eye.w) * 0.5, h: eye.w, ..eye }, Icon::Eye, "");
                }
                if entry.totp.is_some() {
                    let mut line = detail(p, gl, &mut left, "2FA code", "");
                    match &self.totp {
                        Some(current) => {
                            let left_s = current.until.saturating_duration_since(Instant::now()).as_secs_f32().ceil() as u64;
                            let taken = p.label(gl, &ui::grouped(&current.code), line, theme::TITLE_EM * 1.05, look::TEAL, 0.0);
                            line.take_left(taken + 24.0);
                            let bar = Rect::new(line.x, line.y + line.h * 0.5 - 3.0, 140.0_f32.min(line.w * 0.5), 6.0);
                            p.fill(gl, bar, 3.0, theme::scroll_track());
                            let part = left_s as f32 / current.period.max(1) as f32;
                            p.fill(gl, Rect { w: bar.w * part.clamp(0.0, 1.0), ..bar }, 3.0, theme::mark());
                            let text = Rect { x: bar.right() + 14.0, w: (line.right() - bar.right() - 14.0).max(0.0), ..line };
                            p.label(gl, &format!("{left_s} s"), text, theme::DETAIL_EM, theme::ink_hint(), 0.0);
                        }
                        None if self.totp_refused => {
                            p.label(gl, "Not to be had now", line, theme::DETAIL_EM, theme::ink_hint(), 0.0);
                        }
                        None => look::waiting(p, gl, Rect { w: 120.0, ..line }, self.opened),
                    }
                }
                if !entry.sites.is_empty() {
                    let sites: Vec<String> = entry.sites.iter().map(|s| site_shown(s)).collect();
                    detail(p, gl, &mut left, if sites.len() == 1 { "Website" } else { "Websites" }, &sites.join(", "));
                }
                if !entry.apps.is_empty() {
                    detail(p, gl, &mut left, if entry.apps.len() == 1 { "App" } else { "Apps" }, &entry.apps.join(", "));
                }
                notes(p, gl, &mut left, &entry.notes);
                draw_typing(p, gl, &mut right, view, &actions, [&mut self.type_both, &mut self.type_password, &mut self.type_code]);
            }
        }
        if entry.changed > 0 && left.h > theme::FOOTER_EM * 2.0 {
            let line = left.take_bottom(theme::FOOTER_EM * 1.5);
            p.label(gl, &format!("Changed {}", ui::date(entry.changed)), line, theme::FOOTER_EM, theme::ink_hint(), 0.0);
        }
        if let Some(confirm) = self.confirm.as_mut() {
            confirm.draw(p, gl);
        }
    }

    pub fn input(&mut self, input: &Input, view: &View) -> Want {
        if let Some(confirm) = self.confirm.as_mut() {
            match confirm.input(input) {
                Some(true) => {
                    self.confirm = None;
                    return Want::Delete(self.id.clone());
                }
                Some(false) => self.confirm = None,
                None => {}
            }
            return Want::Nothing;
        }
        if self.back.input(input) || matches!(input, Input::Key { key: Key::Escape, .. }) {
            return Want::List;
        }
        let actions = self.actions(view);
        if ui::step(&mut self.focus, actions.len(), input) {
            return Want::Nothing;
        }
        self.lit(&actions);
        let Some(entry) = self.entry.as_ref() else { return Want::Nothing };
        if self.edit.input(input) {
            return Want::Edit(Box::new(entry.clone()));
        }
        if self.delete.input(input) {
            let title = if entry.title.is_empty() { "this entry".to_string() } else { entry.title.clone() };
            self.confirm = Some(Confirm::new(format!("Delete {title}? It goes from SpatiKey on every Spatiand, and cannot be had back."), "Delete", "Keep it"));
            return Want::Nothing;
        }
        if actions.contains(&Action::Eye) && self.eye.input(input) {
            self.shown = !self.shown;
            return Want::Nothing;
        }
        if view.busy() {
            return Want::Nothing;
        }
        let id = self.id.clone();
        use wire::Field::{Password, Tab, Totp as Code, Username};
        if actions.contains(&Action::TypeBoth) && self.type_both.input(input) {
            return Want::Type { id, fields: vec![Username, Tab, Password] };
        }
        if actions.contains(&Action::TypePassword) && self.type_password.input(input) {
            return Want::Type { id, fields: vec![Password] };
        }
        if actions.contains(&Action::TypeCode) && self.type_code.input(input) {
            return Want::Type { id, fields: vec![Code] };
        }
        Want::Nothing
    }
}

/// The buttons that type an entry into the window the wearer was in before SpatiKey: both, the
/// password, the code.
fn draw_typing(p: &mut Painter, gl: &Gl, right: &mut Rect, view: &View, actions: &[Action], buttons: [&mut Button; 3]) {
    let [type_both, type_password, type_code] = buttons;
    let Some(target) = view.target else {
        look::shelf(p, gl, right, "Type it", false);
        ui::prose(
            p,
            gl,
            right,
            "There is no window to type into. SpatiKey types into the window you were in just before it: go to that window, then open SpatiKey.",
            theme::ink_hint(),
        );
        return;
    };
    let name = ui::target_name(target);
    look::shelf(p, gl, right, &format!("Type into {name}"), true);
    right.take_top(6.0);
    for action in actions {
        let (button, label, icon) = match action {
            Action::TypeBoth => (&mut *type_both, format!("Type username and password into {name}"), Icon::Forward),
            Action::TypePassword => (&mut *type_password, "Type password".to_string(), Icon::Lock),
            Action::TypeCode => (&mut *type_code, "Type code".to_string(), Icon::SortRecent),
            _ => continue,
        };
        let row = right.take_top(Button::HEIGHT);
        let width = Button::icon_width(p, gl, &label).min(row.w);
        button.draw_icon(p, gl, Rect { w: width, ..row }, icon, &label);
        right.take_top(12.0);
    }
    right.take_top(8.0);
    match view.waiting {
        Some((since, words)) => ui::waiting(p, gl, right.take_top(Button::HEIGHT), since, words),
        None => ui::prose(
            p,
            gl,
            right,
            &format!("{name} is the window you were in before SpatiKey. Nothing is typed unless it still has the keys when SpatiKey begins."),
            theme::ink_hint(),
        ),
    }
}

impl Drop for EntryPage {
    fn drop(&mut self) {
        if let Some(entry) = self.entry.as_mut() {
            entry.wipe();
        }
    }
}

const VALUE_EM: f32 = theme::ROW_EM * 0.9;

/// One thing about an entry: its name, small, over its value. Returns the value's line, for
/// what goes beside it.
fn detail(p: &mut Painter, gl: &Gl, column: &mut Rect, name: &str, value: &str) -> Rect {
    let label = column.take_top(theme::FOOTER_EM * 1.35);
    p.label(gl, name, label, theme::FOOTER_EM, theme::ink_hint(), 0.0);
    let line = column.take_top(theme::ROW_EM * 1.4);
    p.label(gl, value, line, VALUE_EM, theme::ink(), 0.0);
    column.take_top(16.0);
    line
}

fn notes(p: &mut Painter, gl: &Gl, column: &mut Rect, notes: &str) {
    if notes.trim().is_empty() {
        return;
    }
    let label = column.take_top(theme::FOOTER_EM * 1.35);
    p.label(gl, "Notes", label, theme::FOOTER_EM, theme::ink_hint(), 0.0);
    p.clip(gl, *column);
    let (_, h) = p.text_left(gl, notes.trim(), column.x, column.y + 4.0, ui::PROSE_EM, column.w, theme::ink_row());
    p.unclip(gl);
    column.take_top(h + 20.0);
}

// --- adding and changing an entry ---

const TITLE: usize = 0;
const SITES: usize = 1;
const USERNAME: usize = 2;
const PASSWORD: usize = 3;
const TOTP: usize = 4;
const APPS: usize = 5;
const NOTES: usize = 6;
const NAMES: [&str; 7] = ["Title", "Websites", "Username", "Password", "2FA", "Apps", "Notes"];
/// A line's end in the one-line notes field, put back as one when kept.
const NEWLINE: &str = " \u{21b5} ";

/// Where the keys can be in the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spot {
    Field(usize),
    Eye,
    Generate,
    ThisApp,
    Save,
}

/// A new login, or an entry being changed: a field for each thing about it.
pub struct Edit {
    /// The entry being changed; None for a new one.
    original: Option<Entry>,
    fields: [Field; 7],
    /// What each field said when the editor opened: a field left as it was keeps what the entry
    /// had, exactly.
    first: [String; 7],
    shown: bool,
    eye: Button,
    generate: Button,
    this_app: Button,
    save: Button,
    cancel: Button,
    focus: usize,
    /// What the 2FA field holds, checked: what its codes will be, or why it is not a secret.
    checked: Option<Result<String, String>>,
    /// A field the entry cannot be kept with.
    trouble: Option<String>,
    pointer: (f32, f32),
}

impl Edit {
    pub fn new() -> Edit {
        let mut fields: [Field; 7] = Default::default();
        let hints = [
            "What it is called: GitHub",
            "github.com, or https://192.168.1.1:8443 for one address",
            "The name or address you sign in with",
            "The password, or Generate",
            "An otpauth:// link, or the secret the site shows",
            "Window application ids, as steam",
            "Anything else",
        ];
        for (field, hint) in fields.iter_mut().zip(hints) {
            field.hint = hint.into();
        }
        fields[TITLE].focused = true;
        Edit {
            original: None,
            fields,
            first: Default::default(),
            shown: false,
            eye: Button::default(),
            generate: Button::default(),
            this_app: Button::default(),
            save: Button::default(),
            cancel: Button::default(),
            focus: 0,
            checked: None,
            trouble: None,
            pointer: (0.0, 0.0),
        }
    }

    pub fn of(entry: Entry) -> Edit {
        let mut edit = Edit::new();
        let texts = [
            entry.title.clone(),
            entry.sites.join(", "),
            entry.username.clone(),
            entry.password.clone(),
            entry.totp.as_ref().map(totp_text).unwrap_or_default(),
            entry.apps.join(", "),
            entry.notes.replace('\n', NEWLINE),
        ];
        for (index, text) in texts.into_iter().enumerate() {
            edit.fields[index].set(text.as_str());
            edit.first[index] = text;
        }
        edit.checked = entry.totp.as_ref().map(|t| Ok(described(t)));
        edit.original = Some(entry);
        edit
    }

    fn passkey(&self) -> bool {
        self.original.as_ref().is_some_and(|e| e.kind == Kind::Passkey)
    }

    fn note(&self) -> bool {
        self.original.as_ref().is_some_and(|e| e.kind == Kind::Note)
    }

    fn spots(&self, view: &View) -> Vec<Spot> {
        if self.passkey() || self.note() {
            return vec![Spot::Field(TITLE), Spot::Field(NOTES), Spot::Save];
        }
        let mut spots = vec![Spot::Field(TITLE), Spot::Field(SITES), Spot::Field(USERNAME), Spot::Field(PASSWORD), Spot::Eye, Spot::Generate, Spot::Field(TOTP), Spot::Field(APPS)];
        if view.target.is_some_and(|t| !t.app.is_empty()) {
            spots.push(Spot::ThisApp);
        }
        spots.push(Spot::Field(NOTES));
        spots.push(Spot::Save);
        spots
    }

    fn lit(&mut self, spots: &[Spot]) {
        let at = spots.get(self.focus).copied();
        for (index, field) in self.fields.iter_mut().enumerate() {
            field.focused = at == Some(Spot::Field(index));
        }
        self.eye.focused = at == Some(Spot::Eye);
        self.generate.focused = at == Some(Spot::Generate);
        self.this_app.focused = at == Some(Spot::ThisApp);
        self.save.focused = at == Some(Spot::Save);
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, view: &View) {
        let spots = self.spots(view);
        self.focus = self.focus.min(spots.len() - 1);
        self.lit(&spots);
        let mut all = ui::page(p);
        let title = match &self.original {
            None => "New login".to_string(),
            Some(entry) if entry.title.is_empty() => "Change the entry".to_string(),
            Some(entry) => format!("Change {}", entry.title),
        };
        let mut right = ui::top(p, gl, &mut all, &mut self.cancel, "Cancel", Icon::Close, &title);
        let save_w = Button::width(p, gl, "Save");
        match view.waiting {
            Some((since, words)) => ui::waiting(p, gl, right.take_right(save_w + 200.0), since, words),
            None => self.save.draw(p, gl, right.take_right(save_w), "Save"),
        }
        let trouble = self.trouble.as_deref().or(view.trouble);
        look::foot(p, gl, &mut all, "[Tab] Next  [Enter] Next field  [Esc] Cancel", trouble);
        let label_w = 190.0;
        let field_w = (all.w - label_w).min(900.0);
        let rows: &[usize] = if self.passkey() || self.note() { &[TITLE, NOTES] } else { &[TITLE, SITES, USERNAME, PASSWORD, TOTP, APPS, NOTES] };
        for &index in rows {
            let mut row = all.take_top(Field::HEIGHT);
            p.label(gl, NAMES[index], row.take_left(label_w), theme::ROW_EM * 0.8, theme::ink_detail(), 0.0);
            let mut place = Rect { w: field_w, ..row };
            if index == PASSWORD {
                let generate = place.take_right(Button::icon_width(p, gl, "Generate"));
                place.take_right(10.0);
                let eye = place.take_right(Button::HEIGHT);
                place.take_right(10.0);
                self.eye.draw_icon(p, gl, eye, Icon::Eye, "");
                self.generate.draw_icon(p, gl, generate, Icon::Refresh, "Generate");
            }
            if index == APPS {
                if let Some(target) = view.target.filter(|t| !t.app.is_empty()) {
                    let label = format!("This is for {}", ui::target_name(target));
                    let width = Button::icon_width(p, gl, &label).min(place.w * 0.5);
                    let button = place.take_right(width);
                    place.take_right(10.0);
                    self.this_app.draw_icon(p, gl, button, Icon::Desktop, &label);
                }
            }
            self.fields[index].secret = (index == PASSWORD && !self.shown) || index == TOTP;
            self.fields[index].draw(p, gl, place);
            all.take_top(12.0);
            if index == TOTP {
                if let Some(checked) = &self.checked {
                    let line = all.take_top(theme::DETAIL_EM * 1.3);
                    let line = Rect { x: line.x + label_w + theme::ROW_INSET, w: field_w - theme::ROW_INSET, ..line };
                    match checked {
                        Ok(said) => p.label(gl, said, line, theme::DETAIL_EM, theme::ink_detail(), 0.0),
                        Err(why) => p.label(gl, why, line, theme::DETAIL_EM, look::trouble(), 0.0),
                    };
                    all.take_top(10.0);
                }
            }
        }
    }

    /// The field after `index` among the spots, if there is one: where Enter goes.
    fn next_field(&self, spots: &[Spot]) -> Option<usize> {
        spots.iter().enumerate().skip(self.focus + 1).find(|(_, s)| matches!(s, Spot::Field(_))).map(|(at, _)| at)
    }

    pub fn input(&mut self, input: &Input, view: &View) -> Want {
        if view.busy() {
            return Want::Nothing;
        }
        if self.cancel.input(input) || matches!(input, Input::Key { key: Key::Escape, .. }) {
            return match &self.original {
                Some(entry) => Want::Show(entry.id.clone()),
                None => Want::List,
            };
        }
        let spots = self.spots(view);
        if let Input::Pointer { x, y } = input {
            self.pointer = (*x, *y);
        }
        // A press on a field gives it the keys.
        if let Input::Press { pressed: true } = input {
            let (x, y) = self.pointer;
            if let Some(at) = spots.iter().position(|s| matches!(s, Spot::Field(i) if self.fields[*i].contains(x, y))) {
                self.focus = at;
            }
        }
        if ui::step(&mut self.focus, spots.len(), input) {
            return Want::Nothing;
        }
        self.lit(&spots);
        if self.save.input(input) {
            return self.save();
        }
        if spots.contains(&Spot::Eye) && self.eye.input(input) {
            self.shown = !self.shown;
            return Want::Nothing;
        }
        if spots.contains(&Spot::Generate) && self.generate.input(input) {
            match crate::generate::password() {
                Ok(made) => {
                    self.fields[PASSWORD].text.zeroize();
                    self.fields[PASSWORD].set(made.as_str());
                    self.shown = true;
                    self.trouble = None;
                }
                Err(why) => self.trouble = Some(format!("No password could be made: {why}")),
            }
            return Want::Nothing;
        }
        if spots.contains(&Spot::ThisApp) && self.this_app.input(input) {
            if let Some(target) = view.target {
                let mut apps = list(&self.fields[APPS].text);
                if !apps.contains(&target.app) {
                    apps.push(target.app.clone());
                }
                self.fields[APPS].set(apps.join(", "));
            }
            return Want::Nothing;
        }
        let Some(Spot::Field(index)) = spots.get(self.focus).copied() else { return Want::Nothing };
        // A password, shown or not, a code's secret and the notes (where a PIN is kept) do not go
        // onto the room's clipboard.
        if matches!(index, PASSWORD | TOTP | NOTES) && is_copy(input) {
            return Want::Nothing;
        }
        match self.fields[index].input(input) {
            Some(Typed::Entered) => match self.next_field(&spots) {
                Some(next) => self.focus = next,
                None => return self.save(),
            },
            Some(Typed::Changed) => {
                self.trouble = None;
                if index == TOTP {
                    self.check_totp();
                }
            }
            None => {}
        }
        Want::Nothing
    }

    fn save(&mut self) -> Want {
        match self.entry() {
            Ok(entry) => Want::Save(Box::new(entry)),
            Err(why) => {
                self.trouble = Some(why);
                Want::Nothing
            }
        }
    }

    /// The 2FA field, read as it is typed: what codes it gives, and the one it gives now, to
    /// compare with the site's -- or why it is not a secret at all.
    fn check_totp(&mut self) {
        let text = self.fields[TOTP].text.trim().to_string();
        if text.is_empty() {
            self.checked = None;
            return;
        }
        if self.fields[TOTP].text == self.first[TOTP] {
            self.checked = self.original.as_ref().and_then(|e| e.totp.as_ref()).map(|t| Ok(described(t)));
            return;
        }
        self.checked = Some(match read_totp(&text) {
            Ok(totp) => {
                let now = spatikey_core::now_ms() / 1000;
                let code = spatikey_core::totp::code(&totp, now).map(|(code, _)| format!(" Now {}.", ui::grouped(&code))).unwrap_or_default();
                Ok(format!("{}{code}", described(&totp)))
            }
            Err(why) => Err(format!("Not a 2FA secret: {why}")),
        });
    }

    /// The entry as the fields have it, or why it cannot be kept.
    fn entry(&self) -> Result<Entry, String> {
        let mut entry = self.original.clone().unwrap_or_default();
        let changed = |index: usize| self.fields[index].text != self.first[index];
        let text = |index: usize| self.fields[index].text.trim().to_string();
        entry.title = text(TITLE);
        if changed(NOTES) {
            entry.notes = self.fields[NOTES].text.replace(NEWLINE, "\n").replace('\u{21b5}', "\n").trim().to_string();
        }
        if entry.kind == Kind::Passkey {
            if entry.title.is_empty() {
                entry.title = entry.passkey.as_ref().map(|p| p.rp_id.clone()).unwrap_or_default();
            }
            return Ok(entry);
        }
        if changed(SITES) {
            let mut sites: Vec<String> = Vec::new();
            for typed in list(&text(SITES)) {
                // Said, rather than kept as something no page's address will ever match.
                let site = site_of(&typed).ok_or_else(|| format!("\u{201C}{typed}\u{201D} is not a website: give its domain, like example.com, or its whole address."))?;
                if !sites.contains(&site) {
                    sites.push(site);
                }
            }
            entry.sites = sites;
        }
        if changed(APPS) {
            entry.apps.clear();
            for app in list(&text(APPS)) {
                if !entry.apps.contains(&app) {
                    entry.apps.push(app);
                }
            }
        }
        entry.username = text(USERNAME);
        // Not trimmed: a password may begin or end with a space.
        entry.password = self.fields[PASSWORD].text.clone();
        if changed(TOTP) {
            entry.totp = match text(TOTP).as_str() {
                "" => None,
                typed => Some(read_totp(typed).map_err(|why| format!("The 2FA field is not a secret SpatiKey can use: {why}"))?),
            };
        }
        if entry.title.is_empty() {
            entry.title = entry.sites.first().map(|s| site_shown(s)).or_else(|| entry.apps.first().cloned()).unwrap_or_default();
        }
        if entry.title.is_empty() {
            return Err("Give it a title, or the website it is for.".into());
        }
        if self.original.is_none() {
            let bare = entry.username.is_empty() && entry.password.is_empty() && entry.sites.is_empty() && entry.apps.is_empty() && entry.totp.is_none();
            entry.kind = if bare { Kind::Note } else { Kind::Login };
        }
        Ok(entry)
    }
}

impl Drop for Edit {
    fn drop(&mut self) {
        for field in self.fields.iter_mut() {
            field.text.zeroize();
        }
        for first in self.first.iter_mut() {
            first.zeroize();
        }
        if let Some(entry) = self.original.as_mut() {
            entry.wipe();
        }
    }
}

/// A list typed as words with commas or spaces between them.
fn list(text: &str) -> Vec<String> {
    text.split([',', ' ', '\n', ';']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// A website as typed, as it is kept; None for what is not one. With its scheme it is an
/// address, and what is kept is what SpatiKey keeps for a login first used there
/// (`site::site_for`): its registrable domain, or the whole origin where that is what must
/// match. Without, it is the domain as typed, in small letters and without a "www." -- and
/// without the path an address copied from a browser's bar (which hides the "https://") brings
/// with it: "accounts.example.com/signin" is accounts.example.com. With a port
/// ("192.168.1.1:8443") it is read as the https address it would be, since a port is part of an
/// origin and no domain has one.
fn site_of(typed: &str) -> Option<String> {
    use spatikey_core::site::site_for;
    let typed = typed.trim();
    let kept = |site: String| (!site.is_empty()).then_some(site);
    if let Some(at) = typed.find("://") {
        let rest = &typed[at + 3..];
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        return kept(site_for(&typed[..at + 3 + end]));
    }
    let host = &typed[..typed.find(['/', '?', '#']).unwrap_or(typed.len())];
    // An IPv6 address written bare is all colons and no port; `site_for` wants its brackets.
    let v6 = host.parse::<std::net::Ipv6Addr>().is_ok();
    if host.contains(':') && !v6 {
        return kept(site_for(&format!("https://{host}")));
    }
    // Only what a page's host can be compared with (`site::site_matches`) is a site.
    let as_address = if v6 { format!("https://[{host}]") } else { format!("https://{host}") };
    if site_for(&as_address).is_empty() {
        return None;
    }
    let lower = host.to_ascii_lowercase();
    Some(lower.strip_prefix("www.").map(str::to_string).unwrap_or(lower))
}

/// The 2FA field as the wearer pastes it: an `otpauth://` link, or the bare secret.
fn read_totp(text: &str) -> Result<Totp, String> {
    if text.get(..10).is_some_and(|s| s.eq_ignore_ascii_case("otpauth://")) {
        spatikey_core::totp::parse_uri(text)
    } else {
        spatikey_core::totp::from_secret(text)
    }
}

/// A kept secret as the field shows it: the secret alone when it makes the usual codes (six
/// digits, thirty seconds, SHA-1), else the whole link that says how.
fn totp_text(totp: &Totp) -> String {
    if totp.digits == 6 && totp.period == 30 && totp.algorithm.eq_ignore_ascii_case("SHA1") {
        return totp.secret.clone();
    }
    format!("otpauth://totp/?secret={}&digits={}&period={}&algorithm={}", totp.secret, totp.digits, totp.period, totp.algorithm)
}

fn described(totp: &Totp) -> String {
    let from = if totp.issuer.is_empty() { String::new() } else { format!(", for {}", totp.issuer) };
    format!("Codes of {} digits every {} seconds{from}.", totp.digits, totp.period)
}

// --- settings ---

/// What a line of settings is.
#[derive(Debug, Clone, PartialEq)]
enum Line {
    Heading,
    Touch,
    Open,
    Device { fp: String, name: String },
    Mac,
    NewCode,
    Recover,
}

/// What a yes on the question over the page goes on to do.
enum Then {
    Forget(String),
    NewCode,
}

/// How long a touch lasts, how long SpatiKey stays open, who may ask this Mac, and the
/// recovery code.
pub struct SettingsPage {
    back: Button,
    lines: List,
    confirm: Option<(Confirm, Then)>,
}

const TOUCH: [u64; 3] = [0, 60, 300];
const OPEN: [u64; 3] = [60, 300, 900];

impl SettingsPage {
    pub fn new() -> SettingsPage {
        SettingsPage { back: Button::default(), lines: { let mut lines = ui::keyed_list(); lines.cursor = 1; lines }, confirm: None }
    }

    fn lines(view: &View) -> Vec<(Line, Row)> {
        let heading = |text: String| (Line::Heading, Row::heading(text));
        let settings = view.settings;
        let mut lines = vec![
            heading(if view.on_mac() { "Touch ID".into() } else { format!("Touch ID on {}", view.mac) }),
            (Line::Touch, Row::with("A touch lasts", ui::span(settings.touch_secs)).icon(Icon::Lock)),
            (Line::Open, Row::with("SpatiKey stays open", format!("{} unused", ui::span(settings.open_secs).to_lowercase())).icon(Icon::SortRecent)),
            heading(format!("Devices that may ask {}", view.mac)),
        ];
        match view.devices {
            None => lines.push(heading("\u{2026}".into())),
            Some(devices) if devices.allowed.is_empty() => lines.push(heading("None yet. A device pairs from SpatiKey on it.".into())),
            Some(devices) => {
                for device in &devices.allowed {
                    lines.push((Line::Device { fp: device.fp.clone(), name: device.name.clone() }, Row::with(device.name.clone(), "Forget").icon(Icon::Desktop)));
                }
            }
        }
        if let Some(devices) = view.devices.filter(|d| !d.touch_ids.is_empty()) {
            lines.push(heading("Macs that open SpatiKey with Touch ID".into()));
            for mac in &devices.touch_ids {
                lines.push((Line::Mac, Row::with(mac.name.clone(), format!("since {}", ui::date(mac.added))).icon(Icon::Lock)));
            }
        }
        lines.push(heading("Recovery code".into()));
        lines.push((Line::NewCode, Row::with("New recovery code", "replaces the old one").icon(Icon::Refresh)));
        if view.on_mac() {
            lines.push((Line::Recover, Row::with("Set up Touch ID again", "with the recovery code").icon(Icon::Pencil)));
        }
        lines
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, view: &View) {
        let mut all = ui::page(p);
        ui::top(p, gl, &mut all, &mut self.back, "Back", Icon::Back, "Settings");
        ui::foot(p, gl, &mut all, "[\u{2191}] [\u{2193}] Choose  [Enter] Change  [Esc] Back", view);
        let rows: Vec<Row> = Self::lines(view).into_iter().map(|(_, row)| row).collect();
        self.lines.focused = self.confirm.is_none();
        self.lines.draw(p, gl, ui::column(all, ui::COLUMN.max(1100.0)), &rows);
        if let Some((confirm, _)) = self.confirm.as_mut() {
            confirm.draw(p, gl);
        }
    }

    pub fn input(&mut self, input: &Input, view: &View) -> Want {
        if let Some((confirm, then)) = self.confirm.as_mut() {
            let answer = confirm.input(input);
            let want = match (answer, then) {
                (Some(true), Then::Forget(fp)) => Want::Forget(fp.clone()),
                (Some(true), Then::NewCode) => Want::NewCode,
                _ => Want::Nothing,
            };
            if answer.is_some() {
                self.confirm = None;
            }
            return want;
        }
        if self.back.input(input) || matches!(input, Input::Key { key: Key::Escape, .. }) {
            return Want::List;
        }
        let lines = Self::lines(view);
        let Some(chosen) = self.lines.input(input) else { return Want::Nothing };
        let mut settings = view.settings;
        match lines.get(chosen).map(|(line, _)| line.clone()) {
            Some(Line::Touch) => {
                settings.touch_secs = next(&TOUCH, settings.touch_secs);
                Want::SetSettings(settings)
            }
            Some(Line::Open) => {
                settings.open_secs = next(&OPEN, settings.open_secs);
                Want::SetSettings(settings)
            }
            Some(Line::Device { fp, name }) => {
                let text = format!("Forget {name}? It can no longer ask {} to open SpatiKey, until it pairs again.", view.mac);
                self.confirm = Some((Confirm::new(text, "Forget", "Keep it"), Then::Forget(fp)));
                Want::Nothing
            }
            Some(Line::NewCode) => {
                let text = "A new recovery code replaces the old one, which stops working at once. The new one is shown once, to be saved in Apple Passwords in its place.";
                self.confirm = Some((Confirm::new(text, "Make a new code", "Cancel"), Then::NewCode));
                Want::Nothing
            }
            Some(Line::Recover) => Want::RecoverPage,
            _ => Want::Nothing,
        }
    }
}

/// The choice after `now`, round again at the end; the middle one for a value not among them.
fn next(choices: &[u64], now: u64) -> u64 {
    match choices.iter().position(|c| *c == now) {
        Some(at) => choices[(at + 1) % choices.len()],
        None => choices[choices.len() / 2],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(title: &str, kind: Kind) -> Summary {
        Summary { id: title.to_lowercase(), kind, title: title.into(), ..Summary::default() }
    }

    #[test]
    fn the_search_finds_by_any_word_and_the_tabs_by_kind() {
        let mut github = summary("GitHub", Kind::Login);
        github.username = "paulus".into();
        github.sites = vec!["github.com".into()];
        github.has_totp = true;
        let mut google = summary("Google", Kind::Passkey);
        google.passkey_rp = Some("google.com".into());
        google.passkey_user = Some("paulus@example.com".into());
        let note = summary("Wi-Fi at home", Kind::Note);
        let entries = vec![note, google, github];
        let mut page = ListPage::new();
        page.choose(&entries);
        // By title, whatever order they came in.
        assert_eq!(page.shown, vec![2, 1, 0]);
        page.search.set("PAULUS");
        page.choose(&entries);
        assert_eq!(page.shown, vec![2, 1]);
        page.search.set("paulus example");
        page.choose(&entries);
        assert_eq!(page.shown, vec![1]);
        page.search.set("");
        for (tab, expected) in [(1, vec![2]), (2, vec![1]), (3, vec![2])] {
            page.tabs.active = tab;
            page.choose(&entries);
            assert_eq!(page.shown, expected, "tab {tab}");
        }
    }

    #[test]
    fn a_row_says_who_and_where() {
        let mut github = summary("GitHub", Kind::Login);
        github.username = "paulus".into();
        github.sites = vec!["github.com".into()];
        let row = row_of(&github);
        assert_eq!(row.label, "GitHub");
        assert_eq!(row.trailing.as_deref(), Some("paulus  \u{b7}  github.com"));
        let mut router = summary("", Kind::Login);
        router.sites = vec!["https://192.168.1.1".into()];
        assert_eq!(row_of(&router).label, "192.168.1.1");
    }

    #[test]
    fn a_field_left_alone_keeps_what_the_entry_had() {
        let entry = Entry {
            id: "e1".into(),
            kind: Kind::Login,
            title: "Router".into(),
            sites: vec!["https://192.168.1.1".into()],
            username: "admin".into(),
            password: " spaced ".into(),
            notes: "one\ntwo".into(),
            totp: Some(Totp { secret: "JBSWY3DPEHPK3PXP".into(), digits: 8, period: 60, algorithm: "SHA256".into(), ..Totp::default() }),
            ..Entry::default()
        };
        let mut edit = Edit::of(entry.clone());
        let kept = edit.entry().unwrap();
        assert_eq!(kept.sites, entry.sites);
        assert_eq!(kept.notes, "one\ntwo");
        assert_eq!(kept.password, " spaced ");
        assert!(kept.totp == entry.totp);
        // Changed, a list is read again; the notes' line ends come back.
        edit.fields[APPS].set("steam, steam  ,");
        edit.fields[NOTES].set(format!("one{NEWLINE}two{NEWLINE}three"));
        let kept = edit.entry().unwrap();
        assert_eq!(kept.apps, vec!["steam".to_string()]);
        assert_eq!(kept.notes, "one\ntwo\nthree");
    }

    #[test]
    fn a_new_entry_needs_a_name_and_is_a_note_with_only_notes() {
        let mut edit = Edit::new();
        assert!(edit.entry().is_err());
        edit.fields[SITES].set("www.GitHub.com, github.com");
        let made = edit.entry().unwrap();
        assert_eq!(made.title, "github.com");
        assert_eq!(made.sites, vec!["github.com".to_string()]);
        assert_eq!(made.kind, Kind::Login);
        let mut edit = Edit::new();
        edit.fields[TITLE].set("Door code");
        edit.fields[NOTES].set("4711");
        assert_eq!(edit.entry().unwrap().kind, Kind::Note);
    }

    #[test]
    fn a_website_typed_is_kept_as_what_a_page_matches() {
        use spatikey_core::site::site_matches;
        let site = |typed: &str| site_of(typed);
        assert_eq!(site("www.GitHub.com/").as_deref(), Some("github.com"));
        assert_eq!(site("https://www.github.com/login").as_deref(), Some("github.com"));
        // As copied from a browser's bar, which hides the scheme.
        assert_eq!(site("accounts.example.com/signin?next=1").as_deref(), Some("accounts.example.com"));
        assert!(site_matches(&site("accounts.example.com/signin").unwrap(), "https://accounts.example.com"));
        // A port is part of an origin.
        assert_eq!(site("192.168.1.1:8443/admin").as_deref(), Some("https://192.168.1.1:8443"));
        assert!(site_matches(&site("192.168.1.1:8443").unwrap(), "https://192.168.1.1:8443"));
        assert!(site_matches(&site("192.168.1.1").unwrap(), "https://192.168.1.1"));
        assert!(site_matches(&site("::1").unwrap(), "http://[::1]:5000"));
        assert!(site_matches(&site("nas").unwrap(), "https://nas"));
        // Not websites: said, not kept.
        for bad in ["user@host", "exa mple.com", "https://", "a..b", "host:port"] {
            assert_eq!(site(bad), None, "{bad}");
        }
        let mut edit = Edit::new();
        edit.fields[SITES].set("user@host");
        assert!(edit.entry().unwrap_err().contains("user@host"));
    }

    #[test]
    fn settings_go_round() {
        assert_eq!(next(&TOUCH, 0), 60);
        assert_eq!(next(&TOUCH, 300), 0);
        assert_eq!(next(&OPEN, 900), 60);
        assert_eq!(next(&OPEN, 42), 300);
    }
}
