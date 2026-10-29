//! What SpatiKey's screens share: what they are lent to draw from and what they ask for, the
//! words for every refusal, and the pieces set large -- the recovery code, the six digits of a
//! pairing -- or laid over a page, the yes-or-no card.

use std::time::Instant;

use spatiand_app::ui::{draw_icon, look, theme, Button, Color, Icon, Input, List, Painter, Rect};
use spatiand_app::{Gl, Key};
use spatikey_core::entry::{Entry, Summary};
use spatikey_core::wire::{Code, DevicesAnswer, Field, Found, Refusal, Settings, StatusAnswer, TargetAnswer};
use zeroize::Zeroizing;

use crate::locked::{Away, Locked, Pair, Recover, SetUp, ShowCode};
use crate::open::{Edit, EntryPage, ListPage, SettingsPage};

/// What the window knows, lent to the screen each time it is drawn or touched.
pub struct View<'a> {
    pub status: &'a StatusAnswer,
    /// The Mac that opens the vault, by name: this one, or the paired Mac this device asks.
    pub mac: &'a str,
    /// Every entry's summary, while SpatiKey is open.
    pub entries: &'a [Summary],
    pub settings: Settings,
    /// What is being waited on, since when, in words: "Touch ID on darkPhoenix".
    pub waiting: Option<(Instant, &'a str)>,
    pub trouble: Option<&'a str>,
    /// What was just done, said for a moment: "Typed into Steam."
    pub said: Option<&'a str>,
    /// The window SpatiKey types into: the one the wearer was in before it.
    pub target: Option<&'a TargetAnswer>,
    pub devices: Option<&'a DevicesAnswer>,
    /// The last refusal was Touch ID's fingers having changed: the recovery code is the way on.
    pub fingers_changed: bool,
}

impl View<'_> {
    /// Whether this is a Mac with Touch ID: one that opens the vault itself, or could.
    pub fn on_mac(&self) -> bool {
        self.status.opens_here || self.status.can_set_up
    }

    pub fn busy(&self) -> bool {
        self.waiting.is_some()
    }
}

/// What the wearer asked for, for the window to do.
pub enum Want {
    Nothing,
    /// Ask how things stand, and show the screen that follows from it.
    Status,
    SetUp,
    /// The recovery code has been seen to: after setting up (`first`), or a new one from settings.
    CodeSaved { first: bool },
    Open,
    Lock,
    /// The screen that takes the recovery code.
    RecoverPage,
    Recover(Zeroizing<String>),
    Find,
    PairWith(Found),
    PairConfirm(String, bool),
    /// An entry, by its id.
    Show(String),
    List,
    Add,
    Edit(Box<Entry>),
    Save(Box<Entry>),
    Delete(String),
    Type { id: String, fields: Vec<Field> },
    Settings,
    SetSettings(Settings),
    Forget(String),
    NewCode,
}

/// The screen being shown: one at a time.
pub enum Screen {
    /// Before the service has said how things stand.
    Waiting(Instant),
    Away(Away),
    SetUp(SetUp),
    Code(ShowCode),
    Recover(Recover),
    Pair(Pair),
    Locked(Locked),
    List(ListPage),
    Entry(EntryPage),
    Edit(Edit),
    Settings(SettingsPage),
}

impl Screen {
    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, view: &View) {
        match self {
            Screen::Waiting(since) => {
                let mut all = page(p);
                look::header(p, gl, &mut all, "SpatiKey", "");
                let row = Rect { y: all.y + all.h * 0.4, h: theme::ROW_HEIGHT, ..all };
                look::waiting(p, gl, row, *since);
            }
            Screen::Away(screen) => screen.draw(p, gl, view),
            Screen::SetUp(screen) => screen.draw(p, gl, view),
            Screen::Code(screen) => screen.draw(p, gl, view),
            Screen::Recover(screen) => screen.draw(p, gl, view),
            Screen::Pair(screen) => screen.draw(p, gl, view),
            Screen::Locked(screen) => screen.draw(p, gl, view),
            Screen::List(screen) => screen.draw(p, gl, view),
            Screen::Entry(screen) => screen.draw(p, gl, view),
            Screen::Edit(screen) => screen.draw(p, gl, view),
            Screen::Settings(screen) => screen.draw(p, gl, view),
        }
    }

    pub fn input(&mut self, input: &Input, view: &View) -> Want {
        match self {
            Screen::Waiting(_) => Want::Nothing,
            Screen::Away(screen) => screen.input(input),
            Screen::SetUp(screen) => screen.input(input, view),
            Screen::Code(screen) => screen.input(input),
            Screen::Recover(screen) => screen.input(input, view),
            Screen::Pair(screen) => screen.input(input, view),
            Screen::Locked(screen) => screen.input(input, view),
            Screen::List(screen) => screen.input(input, view),
            Screen::Entry(screen) => screen.input(input, view),
            Screen::Edit(screen) => screen.input(input, view),
            Screen::Settings(screen) => screen.input(input, view),
        }
    }

    /// Whether it changes with nothing done: a code's seconds running down.
    pub fn ticking(&self) -> bool {
        matches!(self, Screen::Entry(page) if page.totp.is_some())
    }
}

/// How wide prose and forms are let go: wider than this, a line is hard to follow back.
pub const COLUMN: f32 = 1040.0;
/// Prose: a little smaller than a row's label.
pub const PROSE_EM: f32 = theme::ROW_EM * 0.8;

/// Where a screen's content goes: the window, less its margin.
pub fn page(p: &Painter) -> Rect {
    p.bounds().inset(theme::PAD * 0.8)
}

/// The left part of `all`, no wider than `width`.
pub fn column(all: Rect, width: f32) -> Rect {
    Rect { w: all.w.min(width), ..all }
}

/// Words wrapped to `all`'s width, taken off its top with a little room after them.
pub fn prose(p: &mut Painter, gl: &Gl, all: &mut Rect, text: &str, color: Color) {
    let (_, h) = p.text_left(gl, text, all.x, all.y, PROSE_EM, all.w, color);
    all.take_top(h + PROSE_EM * 0.7);
}

/// A screen's head with a way back: a Back (or Cancel) button, its title, and the rule under.
/// Returns the row's right-hand part, for buttons of the screen's own.
pub fn top(p: &mut Painter, gl: &Gl, all: &mut Rect, back: &mut Button, back_label: &str, back_icon: Icon, title: &str) -> Rect {
    let mut row = all.take_top(Button::HEIGHT);
    let width = Button::icon_width(p, gl, back_label);
    back.draw_icon(p, gl, row.take_left(width), back_icon, back_label);
    row.take_left(24.0);
    let taken = p.label(gl, title, Rect { w: (row.w * 0.6).max(0.0), ..row }, theme::TITLE_EM * 0.9, theme::ink(), 0.0);
    all.take_top(14.0);
    look::rule(p, gl, all.take_top(2.0));
    all.take_top(24.0);
    Rect { x: row.x + taken + 24.0, w: (row.w - taken - 24.0).max(0.0), ..row }
}

/// The line at a screen's foot: what went wrong, else what was just done, else which keys do what.
pub fn foot(p: &mut Painter, gl: &Gl, all: &mut Rect, hint: &str, view: &View) {
    match (view.trouble, view.said) {
        (Some(trouble), _) => look::foot(p, gl, all, hint, Some(trouble)),
        (None, Some(said)) => {
            let line = all.take_bottom(theme::FOOTER_EM * 1.9);
            let dot = Rect::new(line.x, line.y + line.h * 0.5 - 5.0, 10.0, 10.0);
            p.fill(gl, dot, 5.0, look::TEAL);
            p.label(gl, said, Rect { x: line.x + 22.0, w: line.w - 22.0, ..line }, theme::FOOTER_EM, theme::ink_detail(), 0.0);
        }
        (None, None) => look::foot(p, gl, all, hint, None),
    }
}

/// Three dots taking turns, and the words for what is being waited on.
pub fn waiting(p: &mut Painter, gl: &Gl, row: Rect, since: Instant, words: &str) {
    look::waiting(p, gl, Rect { w: 120.0, ..row }, since);
    p.label(gl, words, Rect { x: row.x + 136.0, w: (row.w - 136.0).max(0.0), ..row }, PROSE_EM, theme::ink_detail(), 0.0);
}

/// The same, centred in `row`.
pub fn waiting_centred(p: &mut Painter, gl: &Gl, row: Rect, since: Instant, words: &str) {
    let width = (p.measure(gl, words, PROSE_EM, 8192.0 / p.scale()).0 + 136.0).min(row.w);
    waiting(p, gl, Rect { x: row.x + (row.w - width) * 0.5, w: width, ..row }, since, words);
}

/// SpatiKey's mark, large, on the screens with little else on them: a padlock in a glow of the
/// room's three colours.
pub fn mark(p: &mut Painter, gl: &Gl, rect: Rect) {
    let radius = rect.w * 0.26;
    p.glow(gl, rect, radius, [look::BLUE[0], look::BLUE[1], look::BLUE[2], 0.20], 46.0);
    let (a, b) = (look::TEAL, look::VIOLET);
    p.gradient(gl, rect, radius, [a[0], a[1], a[2], 0.30], [b[0], b[1], b[2], 0.26]);
    draw_icon(p, gl, Icon::Lock, rect.inset(rect.w * 0.24), theme::ink());
}

/// The recovery code, large, as its five groups each in a box of its own: it is read off and
/// typed by eye, and a group lost in a line of twenty-five is hard to find again.
pub fn recovery_code(p: &mut Painter, gl: &Gl, all: &mut Rect, code: &str) {
    let groups: Vec<&str> = code.split('-').filter(|g| !g.is_empty()).collect();
    let count = groups.len().max(1) as f32;
    let gap = 20.0;
    let width = ((all.w - gap * (count - 1.0)) / count).min(206.0);
    let row = all.take_top(108.0);
    for (index, group) in groups.iter().enumerate() {
        let cell = Rect::new(row.x + index as f32 * (width + gap), row.y, width, row.h);
        p.fill(gl, cell, theme::ROW_RADIUS, theme::raised());
        p.fill(gl, Rect { y: cell.bottom() - 4.0, h: 3.0, ..cell.inset(theme::ROW_RADIUS) }, 1.5, [look::TEAL[0], look::TEAL[1], look::TEAL[2], 0.6]);
        p.label(gl, group, cell, theme::TITLE_EM * 1.2, theme::ink(), 0.5);
    }
    all.take_top(30.0);
}

/// The six digits a pairing shows, large, to be compared by eye with the Mac's.
pub fn digits(p: &mut Painter, gl: &Gl, all: &mut Rect, code: &str) {
    let line = all.take_top(theme::TITLE_EM * 3.0);
    p.label(gl, code, line, theme::TITLE_EM * 2.4, look::TEAL, 0.0);
    all.take_top(16.0);
}

/// The six digits a pairing shows on both machines, from both fingerprints, as every pairing in
/// Spatiand makes them: the device works them out itself, so that it shows them while the Mac's
/// owner is still being asked. None when either is not a fingerprint.
pub fn pairing_code(a: &str, b: &str) -> Option<String> {
    let a: spatiand_stream::Fingerprint = a.parse().ok()?;
    let b: spatiand_stream::Fingerprint = b.parse().ok()?;
    Some(spatiand_stream::pairing_code(&a, &b))
}

/// What the window SpatiKey types into is called: its title, or failing that its application.
pub fn target_name(target: &TargetAnswer) -> String {
    match (target.title.trim(), target.app.trim()) {
        ("", "") => "the window".into(),
        ("", app) => app.into(),
        (title, _) => title.into(),
    }
}

/// Whether two showings of the same digits are the same, whatever spaces they have.
pub fn same_digits(a: &str, b: &str) -> bool {
    let digits = |s: &str| s.chars().filter(char::is_ascii_digit).collect::<String>();
    !digits(a).is_empty() && digits(a) == digits(b)
}

/// A one-time code as it is read: in two halves, "492 038".
pub fn grouped(code: &str) -> String {
    let half = code.chars().count() / 2;
    if code.chars().count() < 6 {
        return code.to_string();
    }
    let (a, b): (String, String) = (code.chars().take(half).collect(), code.chars().skip(half).collect());
    format!("{a} {b}")
}

/// What a refusal means, in words for the wearer: what happened, and what to do about it. `mac`
/// is the Mac that approves -- this one, on a Mac (`on_mac`).
pub fn meaning(refusal: &Refusal, mac: &str, on_mac: bool) -> String {
    let theirs = if on_mac { "this Mac".to_string() } else { mac.to_string() };
    let said = refusal.message.trim();
    let with_said = |plain: &str| if said.is_empty() { plain.to_string() } else { format!("{plain} ({said})") };
    match refusal.code {
        Code::NoVault if on_mac => "There is no SpatiKey vault yet: set it up first.".into(),
        Code::NoVault => format!("There is no SpatiKey vault yet: set it up in SpatiKey on {mac} first."),
        Code::VaultExists => "There is a SpatiKey vault already. Its recovery code sets Touch ID up for it on this Mac.".into(),
        Code::Denied if on_mac => "Touch ID was cancelled, or did not know the finger. Nothing was opened.".into(),
        Code::Denied => format!("Touch ID on {mac} was cancelled, or did not know the finger. Nothing was opened."),
        Code::TouchIdUnavailable => format!("Touch ID cannot be used on {theirs} now: its lid may be shut, it may be locked, or no finger is enrolled."),
        Code::FingerprintsChanged if on_mac => {
            "The fingers enrolled in Touch ID have changed, so this Mac's key to SpatiKey was retired. The recovery code sets Touch ID up again.".into()
        }
        Code::FingerprintsChanged => {
            format!("The fingers enrolled in Touch ID on {mac} have changed, so its key to SpatiKey was retired. Set Touch ID up again on {mac} with the recovery code.")
        }
        Code::NoApprover => "This device has no Mac to ask yet: pair it with SpatiKey on your Mac first.".into(),
        Code::NotReachable => format!(
            "{mac} did not answer on this network. It answers only on the same network: awake, with Spatiand running, on the same Wi-Fi as this device. Tailscale does not count."
        ),
        Code::NotSameLan => format!(
            "{mac} refused: it does not see this device on its own network, and SpatiKey never answers over Tailscale or across a router. If Tailscale is on here, leave Spatiand out of it or turn its subnet routes off."
        ),
        Code::NoMatch => "SpatiKey has nothing for that.".into(),
        Code::Excluded => "The site already has a passkey from SpatiKey for this account.".into(),
        Code::BadRequest => with_said("SpatiKey could not do what was asked"),
        Code::BadSession => "SpatiKey closed: it sat unused, or Spatiand restarted. Open it again.".into(),
        Code::NotHere => with_said("That cannot be done on this computer"),
        Code::Timeout => format!("{theirs} did not answer in time."),
        Code::Failed => with_said("Something went wrong in SpatiKey"),
    }
}

/// How long, in words: "every use", "1 minute", "5 minutes".
pub fn span(secs: u64) -> String {
    match secs {
        0 => "Every use".into(),
        s if s % 60 == 0 && s / 60 == 1 => "1 minute".into(),
        s if s % 60 == 0 => format!("{} minutes", s / 60),
        s => format!("{s} seconds"),
    }
}

/// A day, as the wearer's clock has it: "29 Oct 2026".
pub fn date(ms: u64) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let secs = (ms / 1000) as libc::time_t;
    // SAFETY: localtime_r writes only into `tm`, and both live through the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return String::new();
    }
    format!("{} {} {}", tm.tm_mday, MONTHS[tm.tm_mon.clamp(0, 11) as usize], tm.tm_year + 1900)
}

/// A button that has the keys from the start: a screen's one thing to do.
pub fn keyed_button() -> Button {
    let mut button = Button::default();
    button.focused = true;
    button
}

/// A list that has the keys from the start.
pub fn keyed_list() -> List {
    let mut list = List::default();
    list.focused = true;
    list
}

/// Moves which of a page's `count` things has the keys: Tab and Down onwards, Shift-Tab and Up
/// back, round at the ends. Whether it moved.
pub fn step(focus: &mut usize, count: usize, input: &Input) -> bool {
    let Input::Key { key, modifiers, .. } = input else { return false };
    if count == 0 {
        return false;
    }
    match key {
        Key::Tab if modifiers.shift => *focus = (*focus + count - 1) % count,
        Key::Tab | Key::Down => *focus = (*focus + 1) % count,
        Key::Up => *focus = (*focus + count - 1) % count,
        _ => return false,
    }
    true
}

/// A question laid over the page, answered yes or no: deleting an entry, forgetting a device,
/// replacing the recovery code. The keys start on the answer that changes nothing.
pub struct Confirm {
    text: String,
    yes_label: String,
    no_label: String,
    yes: Button,
    no: Button,
    on_yes: bool,
}

impl Confirm {
    pub fn new(text: impl Into<String>, yes: impl Into<String>, no: impl Into<String>) -> Confirm {
        Confirm { text: text.into(), yes_label: yes.into(), no_label: no.into(), yes: Button::default(), no: Button::default(), on_yes: false }
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl) {
        let all = p.bounds();
        let ground = theme::ground();
        p.fill(gl, all, 0.0, [ground[0], ground[1], ground[2], 0.72]);
        let width = (all.w - theme::PAD * 2.0).min(840.0);
        let inner = width - 88.0;
        let (_, text_h) = p.measure_left(gl, &self.text, PROSE_EM, inner);
        let height = 44.0 + text_h + 36.0 + Button::HEIGHT + 44.0;
        let card = Rect::new((all.w - width) * 0.5, (all.h - height) * 0.5, width, height);
        look::glass(p, gl, card, theme::CARD_RADIUS, 1.0);
        let mut inside = card.inset(44.0);
        p.text_left(gl, &self.text, inside.x, inside.y, PROSE_EM, inside.w, theme::ink());
        let mut row = inside.take_bottom(Button::HEIGHT);
        let yes = row.take_right(Button::width(p, gl, &self.yes_label));
        row.take_right(14.0);
        let no = row.take_right(Button::width(p, gl, &self.no_label));
        self.lit();
        self.yes.draw(p, gl, yes, &self.yes_label);
        self.no.draw(p, gl, no, &self.no_label);
    }

    fn lit(&mut self) {
        self.yes.focused = self.on_yes;
        self.no.focused = !self.on_yes;
    }

    /// `Some(true)` for yes, `Some(false)` for no (or Escape); None while it is not answered.
    pub fn input(&mut self, input: &Input) -> Option<bool> {
        if let Input::Key { key, .. } = input {
            match key {
                Key::Escape => return Some(false),
                Key::Left | Key::Right | Key::Up | Key::Down | Key::Tab => {
                    self.on_yes = !self.on_yes;
                    return None;
                }
                _ => {}
            }
        }
        self.lit();
        if self.yes.input(input) {
            return Some(true);
        }
        if self.no.input(input) {
            return Some(false);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_refusal_has_words_and_none_is_the_code() {
        let codes = [
            Code::NoVault, Code::VaultExists, Code::Denied, Code::TouchIdUnavailable, Code::FingerprintsChanged, Code::NoApprover, Code::NotReachable,
            Code::NotSameLan, Code::NoMatch, Code::Excluded, Code::BadRequest, Code::BadSession, Code::NotHere, Code::Timeout, Code::Failed,
        ];
        for code in codes {
            for on_mac in [true, false] {
                let words = meaning(&Refusal::new(code, ""), "darkPhoenix", on_mac);
                assert!(words.len() > 20, "{code:?}: {words}");
                assert!(!words.contains('_'), "{code:?} reads as a code: {words}");
            }
        }
        // On a device the Mac is named; on the Mac it is "this Mac".
        assert!(meaning(&Refusal::new(Code::NotReachable, ""), "darkPhoenix", false).starts_with("darkPhoenix"));
        assert!(meaning(&Refusal::new(Code::TouchIdUnavailable, ""), "darkPhoenix", true).contains("this Mac"));
    }

    #[test]
    fn a_pairing_shows_the_same_digits_on_both_ends() {
        let mac = "0b8dfc23a4e15b7790c2d6e81f3a5c4b9d0e7f6a5b4c3d2e1f0a9b8c7d6e5f40";
        let deck = "5c1e9a7b3d2f4e6a8b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8c9d0e1f2a";
        let code = pairing_code(deck, mac).unwrap();
        assert_eq!(Some(code.clone()), pairing_code(mac, deck));
        assert_eq!(code.chars().filter(char::is_ascii_digit).count(), 6);
        assert!(same_digits(&code, &code.replace(' ', "")));
        assert!(pairing_code("not a fingerprint", mac).is_none());
        assert!(!same_digits("", ""));
    }

    #[test]
    fn codes_are_read_in_halves() {
        assert_eq!(grouped("492038"), "492 038");
        assert_eq!(grouped("49203817"), "4920 3817");
        assert_eq!(grouped("1234"), "1234");
    }

    #[test]
    fn spans_in_words() {
        assert_eq!(span(0), "Every use");
        assert_eq!(span(60), "1 minute");
        assert_eq!(span(300), "5 minutes");
        assert_eq!(span(90), "90 seconds");
    }
}
