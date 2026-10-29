//! What SpatiKey shows while the vault is shut: setting it up, the recovery code, pairing this
//! device with a Mac, setting Touch ID up again from the recovery code, and the locked screen
//! with its one button.

use spatiand_app::ui::{look, theme, Button, Field, Icon, Input, List, Painter, Rect, Row, Typed};
use spatiand_app::{Gl, Key};
use spatikey_core::wire::Found;
use zeroize::{Zeroize, Zeroizing};

use crate::ui::{self, View, Want};

// --- the service is not there ---

/// SpatiKey's service cannot be reached: Spatiand is not running on this computer.
pub struct Away {
    pub why: String,
    retry: Button,
}

impl Away {
    pub fn new(why: String) -> Away {
        Away { why, retry: ui::keyed_button() }
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, view: &View) {
        let mut all = ui::page(p);
        look::header(p, gl, &mut all, "SpatiKey", "");
        ui::foot(p, gl, &mut all, "[Enter] Try again", view);
        let mut column = ui::column(all, ui::COLUMN);
        column.take_top(20.0);
        ui::prose(
            p,
            gl,
            &mut column,
            "SpatiKey cannot reach its service on this computer. The service is part of Spatiand and runs while Spatiand does: start Spatiand, then try again.",
            theme::ink_row(),
        );
        if !self.why.is_empty() {
            ui::prose(p, gl, &mut column, &self.why, theme::ink_hint());
        }
        column.take_top(8.0);
        let row = column.take_top(Button::HEIGHT);
        self.retry.focused = true;
        let width = Button::icon_width(p, gl, "Try again");
        self.retry.draw_icon(p, gl, Rect { w: width, ..row }, Icon::Refresh, "Try again");
    }

    pub fn input(&mut self, input: &Input) -> Want {
        if self.retry.input(input) {
            return Want::Status;
        }
        Want::Nothing
    }
}

// --- setting up ---

/// A Mac with Touch ID and no vault: what SpatiKey is, and the button that makes the vault.
pub struct SetUp {
    go: Button,
}

impl SetUp {
    pub fn new() -> SetUp {
        SetUp { go: ui::keyed_button() }
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, view: &View) {
        let mut all = ui::page(p);
        let beside = if view.status.name.is_empty() { String::new() } else { format!("on {}", view.status.name) };
        look::header(p, gl, &mut all, "Set up SpatiKey", &beside);
        ui::foot(p, gl, &mut all, "[Enter] Set up SpatiKey", view);
        let side = 150.0;
        let mark = Rect::new(all.right() - side - 20.0, all.y + 30.0, side, side);
        ui::mark(p, gl, mark);
        let mut column = ui::column(all, (all.w - side - 90.0).min(ui::COLUMN));
        column.take_top(20.0);
        ui::prose(
            p,
            gl,
            &mut column,
            "SpatiKey keeps your passwords, passkeys and two-factor codes in one vault, encrypted, on every Spatiand you link.",
            theme::ink(),
        );
        ui::prose(
            p,
            gl,
            &mut column,
            "Only Touch ID on this Mac opens it. The Steam Deck and the Beam Pro ask this Mac each time, and only while they are on the same network as it, never over Tailscale.",
            theme::ink_row(),
        );
        ui::prose(
            p,
            gl,
            &mut column,
            "Setting up makes the vault, asks for your finger, and shows a recovery code once: the way back in on the day Touch ID cannot open it.",
            theme::ink_row(),
        );
        column.take_top(12.0);
        let row = column.take_top(Button::HEIGHT);
        match view.waiting {
            Some((since, words)) => ui::waiting(p, gl, row, since, words),
            None => {
                self.go.focused = true;
                let width = Button::icon_width(p, gl, "Set up SpatiKey");
                self.go.draw_icon(p, gl, Rect { w: width, ..row }, Icon::Lock, "Set up SpatiKey");
            }
        }
    }

    pub fn input(&mut self, input: &Input, view: &View) -> Want {
        if !view.busy() && self.go.input(input) {
            return Want::SetUp;
        }
        Want::Nothing
    }
}

// --- the recovery code, shown once ---

/// The recovery code, large, with what to do with it -- and then, before it goes for good, the
/// question whether that was done. There is no copy button: the room's clipboard is shared
/// with every computer it is linked to.
pub struct ShowCode {
    code: Zeroizing<String>,
    /// Shown after setting up, rather than as a new code from settings: where "Saved it" leads.
    pub first: bool,
    asking: bool,
    saved: Button,
    sure: Button,
    again: Button,
    /// In the question: whether the keys are on "Show it again".
    on_again: bool,
}

impl ShowCode {
    pub fn new(code: Zeroizing<String>, first: bool) -> ShowCode {
        ShowCode { code, first, asking: false, saved: ui::keyed_button(), sure: Button::default(), again: Button::default(), on_again: false }
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, view: &View) {
        let mut all = ui::page(p);
        if self.asking {
            look::header(p, gl, &mut all, "Saved?", "");
            ui::foot(p, gl, &mut all, "[\u{2190}] [\u{2192}] Choose  [Enter] Answer", view);
            let mut column = ui::column(all, ui::COLUMN);
            column.take_top(20.0);
            ui::prose(
                p,
                gl,
                &mut column,
                "SpatiKey will not show this code again. Is it saved in Apple Passwords as \u{201C}SpatiKey recovery\u{201D}?",
                theme::ink(),
            );
            column.take_top(12.0);
            let mut row = column.take_top(Button::HEIGHT);
            let sure_w = Button::width(p, gl, "Yes, it is saved");
            self.sure.focused = !self.on_again;
            self.sure.draw(p, gl, row.take_left(sure_w), "Yes, it is saved");
            row.take_left(14.0);
            let again_w = Button::width(p, gl, "Show it again");
            self.again.focused = self.on_again;
            self.again.draw(p, gl, row.take_left(again_w), "Show it again");
            return;
        }
        look::header(p, gl, &mut all, "Your recovery code", "shown this once");
        ui::foot(p, gl, &mut all, "[Enter] Saved it", view);
        let mut column = ui::column(all, ui::COLUMN.max(1110.0));
        column.take_top(12.0);
        ui::recovery_code(p, gl, &mut column, &self.code);
        ui::prose(
            p,
            gl,
            &mut column,
            "Save it now in Apple Passwords: open Passwords, press +, call it \u{201C}SpatiKey recovery\u{201D}, and put the code in as its password. iCloud Keychain then keeps it behind Face ID or Touch ID on your Apple devices.",
            theme::ink(),
        );
        ui::prose(
            p,
            gl,
            &mut column,
            "It is the only way back into the vault if the fingers enrolled in Touch ID change. There is no copy button on purpose: Spatiand\u{2019}s clipboard is shared with the computers it is linked to.",
            theme::ink_row(),
        );
        column.take_top(8.0);
        let row = column.take_top(Button::HEIGHT);
        self.saved.focused = true;
        let width = Button::width(p, gl, "Saved it");
        self.saved.draw(p, gl, Rect { w: width, ..row }, "Saved it");
    }

    pub fn input(&mut self, input: &Input) -> Want {
        if !self.asking {
            if self.saved.input(input) {
                self.asking = true;
                self.on_again = false;
            }
            return Want::Nothing;
        }
        if let Input::Key { key: Key::Left | Key::Right | Key::Tab | Key::Up | Key::Down, .. } = input {
            self.on_again = !self.on_again;
            return Want::Nothing;
        }
        if let Input::Key { key: Key::Escape, .. } = input {
            self.asking = false;
            return Want::Nothing;
        }
        self.sure.focused = !self.on_again;
        self.again.focused = self.on_again;
        if self.sure.input(input) {
            return Want::CodeSaved { first: self.first };
        }
        if self.again.input(input) {
            self.asking = false;
        }
        Want::Nothing
    }
}

// --- Touch ID set up again from the recovery code ---

/// The recovery code typed in, for Touch ID on this Mac to be given a key to the vault again:
/// after the fingers changed, on a Mac the vault has reached that never had one, or asked for in
/// settings.
pub struct Recover {
    field: Field,
    go: Button,
    back: Button,
    /// Opened from settings, where Back leads; otherwise there is nowhere else to go.
    pub from_settings: bool,
    /// 0: the field; 1: the button.
    focus: usize,
    pointer: (f32, f32),
}

impl Recover {
    pub fn new(from_settings: bool) -> Recover {
        let mut field = Field::default();
        field.hint = "XXXXX-XXXXX-XXXXX-XXXXX-XXXXX".into();
        field.secret = true;
        field.focused = true;
        Recover { field, go: Button::default(), back: Button::default(), from_settings, focus: 0, pointer: (0.0, 0.0) }
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, view: &View) {
        let mut all = ui::page(p);
        // A Mac the vault reached, whose Touch ID never had a key to it, is told so.
        let new_here = !view.status.opens_here && !view.fingers_changed && !self.from_settings;
        let title = if new_here { "Open SpatiKey on this Mac" } else { "Set up Touch ID again" };
        if self.from_settings || view.fingers_changed {
            ui::top(p, gl, &mut all, &mut self.back, "Back", Icon::Back, title);
        } else {
            look::header(p, gl, &mut all, title, "");
        }
        let hint = if self.from_settings || view.fingers_changed { "[Enter] Set up Touch ID  [Esc] Back" } else { "[Enter] Set up Touch ID" };
        ui::foot(p, gl, &mut all, hint, view);
        let mut column = ui::column(all, ui::COLUMN);
        column.take_top(10.0);
        let said = if new_here {
            "The vault has reached this Mac, but Touch ID here has no key to it yet. Enter the recovery code you saved in Apple Passwords as \u{201C}SpatiKey recovery\u{201D} to give it one."
        } else {
            "Enter the recovery code you saved in Apple Passwords as \u{201C}SpatiKey recovery\u{201D}. SpatiKey then sets Touch ID up on this Mac again, with the fingers enrolled now."
        };
        ui::prose(p, gl, &mut column, said, theme::ink());
        column.take_top(6.0);
        let busy = view.busy();
        self.field.focused = self.focus == 0 && !busy;
        self.field.draw(p, gl, Rect { w: column.w.min(760.0), ..column.take_top(Field::HEIGHT) });
        column.take_top(18.0);
        let row = column.take_top(Button::HEIGHT);
        match view.waiting {
            Some((since, words)) => ui::waiting(p, gl, row, since, words),
            None => {
                self.go.focused = self.focus == 1;
                let width = Button::icon_width(p, gl, "Set up Touch ID");
                self.go.draw_icon(p, gl, Rect { w: width, ..row }, Icon::Lock, "Set up Touch ID");
            }
        }
    }

    fn submit(&mut self) -> Want {
        let typed = Zeroizing::new(self.field.text.trim().to_string());
        if typed.is_empty() {
            self.focus = 0;
            return Want::Nothing;
        }
        Want::Recover(typed)
    }

    pub fn input(&mut self, input: &Input, view: &View) -> Want {
        if view.busy() {
            return Want::Nothing;
        }
        let can_go_back = self.from_settings || view.fingers_changed;
        if can_go_back && (self.back.input(input) || matches!(input, Input::Key { key: Key::Escape, .. })) {
            return if self.from_settings { Want::Settings } else { Want::Status };
        }
        if let Input::Pointer { x, y } = input {
            self.pointer = (*x, *y);
        }
        // A press on the field gives it the keys.
        if let Input::Press { pressed: true } = input {
            if self.field.contains(self.pointer.0, self.pointer.1) {
                self.focus = 0;
            }
        }
        self.go.focused = self.focus == 1;
        if self.go.input(input) {
            return self.submit();
        }
        if ui::step(&mut self.focus, 2, input) {
            return Want::Nothing;
        }
        if self.focus == 0 {
            // The code is not to go onto the room's clipboard, shown or not.
            if is_copy(input) {
                return Want::Nothing;
            }
            self.field.focused = true;
            if self.field.input(input) == Some(Typed::Entered) {
                return self.submit();
            }
        }
        Want::Nothing
    }
}

impl Drop for Recover {
    fn drop(&mut self) {
        self.field.text.zeroize();
    }
}

/// Control-C or Control-X (Command on a Mac): kept from fields holding a secret, whose text
/// would otherwise go onto the room's clipboard, which is every linked computer's.
pub fn is_copy(input: &Input) -> bool {
    matches!(input, Input::Key { key: Key::Char('c' | 'x'), modifiers, .. } if modifiers.control || modifiers.logo)
}

// --- pairing a device with a Mac ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// What pairing is, and the button that looks for Macs.
    Intro,
    /// The Macs that answered on this network.
    Found,
    /// The six digits, while the Mac's owner is asked.
    Code,
    /// The Mac said yes: does it show the same digits?
    Compare,
}

/// A device with no Mac to ask -- the Deck, the Beam Pro -- pairing with SpatiKey on a Mac, once,
/// on the same network: both show six digits, and both people say they match.
pub struct Pair {
    pub stage: Stage,
    start: Button,
    again: Button,
    back: Button,
    pub macs: Vec<Found>,
    list: List,
    pub chosen: Option<Found>,
    /// The six digits as this device works them out.
    pub code: String,
    yes: Button,
    no: Button,
    /// Whether the keys are on No: where they start each time the question is asked, since a yes
    /// given without looking would let in whatever Mac answered -- and a paired Mac is sent
    /// requests that carry passwords.
    on_no: bool,
    /// 0: the list; 1: "Look again".
    focus: usize,
}

impl Pair {
    pub fn new() -> Pair {
        Pair {
            stage: Stage::Intro,
            start: ui::keyed_button(),
            again: Button::default(),
            back: Button::default(),
            macs: Vec::new(),
            list: ui::keyed_list(),
            chosen: None,
            code: String::new(),
            yes: Button::default(),
            no: Button::default(),
            on_no: true,
            focus: 0,
        }
    }

    /// The Macs that answered: shown as a list to choose from.
    pub fn found(&mut self, macs: Vec<Found>) {
        self.macs = macs;
        self.stage = Stage::Found;
        self.list.cursor = 0;
        self.focus = if self.macs.is_empty() { 1 } else { 0 };
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, view: &View) {
        let mut all = ui::page(p);
        let me = if view.status.name.is_empty() { "This device".to_string() } else { view.status.name.clone() };
        let mac = self.chosen.as_ref().map_or_else(|| "your Mac".to_string(), |m| m.name.clone());
        match self.stage {
            Stage::Intro => {
                look::header(p, gl, &mut all, &format!("SpatiKey on {me}"), "asks your Mac");
                ui::foot(p, gl, &mut all, "[Enter] Pair with your Mac", view);
                let side = 150.0;
                ui::mark(p, gl, Rect::new(all.right() - side - 20.0, all.y + 30.0, side, side));
                let mut column = ui::column(all, (all.w - side - 90.0).min(ui::COLUMN));
                column.take_top(20.0);
                ui::prose(
                    p,
                    gl,
                    &mut column,
                    &format!(
                        "{me} has no fingerprint reader, so SpatiKey here asks your Mac: each use is approved with Touch ID on the Mac, and only while both are on the same network. Tailscale does not count."
                    ),
                    theme::ink(),
                );
                ui::prose(
                    p,
                    gl,
                    &mut column,
                    "Pair once with SpatiKey on your Mac to begin. Spatiand must be running there, and both must be on the same Wi-Fi.",
                    theme::ink_row(),
                );
                column.take_top(12.0);
                let row = column.take_top(Button::HEIGHT);
                match view.waiting {
                    Some((since, words)) => ui::waiting(p, gl, row, since, words),
                    None => {
                        self.start.focused = true;
                        let width = Button::icon_width(p, gl, "Pair with your Mac");
                        self.start.draw_icon(p, gl, Rect { w: width, ..row }, Icon::Link, "Pair with your Mac");
                    }
                }
            }
            Stage::Found => {
                ui::top(p, gl, &mut all, &mut self.back, "Back", Icon::Back, "Pair with your Mac");
                ui::foot(p, gl, &mut all, "[\u{2191}] [\u{2193}] Choose  [Enter] Pair  [Esc] Back", view);
                let mut column = ui::column(all, ui::COLUMN);
                let mut foot = column.take_bottom(Button::HEIGHT);
                column.take_bottom(18.0);
                match view.waiting {
                    Some((since, words)) => ui::waiting(p, gl, foot, since, words),
                    None => {
                        self.again.focused = self.focus == 1 || self.macs.is_empty();
                        let width = Button::icon_width(p, gl, "Look again");
                        self.again.draw_icon(p, gl, foot.take_left(width), Icon::Refresh, "Look again");
                    }
                }
                if self.macs.is_empty() {
                    ui::prose(
                        p,
                        gl,
                        &mut column,
                        "No Mac answered on this network. SpatiKey answers from inside Spatiand: start Spatiand on the Mac, and put both on the same Wi-Fi. Over Tailscale it does not answer.",
                        theme::ink_row(),
                    );
                    return;
                }
                ui::prose(p, gl, &mut column, "Macs on this network with SpatiKey. Choose yours:", theme::ink_row());
                let rows: Vec<Row> = self
                    .macs
                    .iter()
                    .map(|m| Row::with(m.name.clone(), if m.paired { "paired already".to_string() } else { m.address.clone() }).icon(Icon::Desktop))
                    .collect();
                self.list.focused = self.focus == 0;
                self.list.draw(p, gl, column, &rows);
            }
            Stage::Code | Stage::Compare => {
                look::header(p, gl, &mut all, &format!("Pair with {mac}"), "");
                let hint = if self.stage == Stage::Compare { "[\u{2190}] [\u{2192}] Choose  [Enter] Answer" } else { "" };
                ui::foot(p, gl, &mut all, hint, view);
                let mut column = ui::column(all, ui::COLUMN);
                column.take_top(10.0);
                ui::digits(p, gl, &mut column, &self.code);
                if self.stage == Stage::Code {
                    ui::prose(
                        p,
                        gl,
                        &mut column,
                        &format!("{mac} asks whether to let {me} use SpatiKey, and shows six digits. Allow it there if they are these."),
                        theme::ink(),
                    );
                    column.take_top(8.0);
                    if let Some((since, words)) = view.waiting {
                        ui::waiting(p, gl, column.take_top(Button::HEIGHT), since, words);
                    }
                    return;
                }
                ui::prose(p, gl, &mut column, &format!("{mac} let {me} in. Does it show these same six digits?"), theme::ink());
                column.take_top(8.0);
                let mut row = column.take_top(Button::HEIGHT);
                let yes_w = Button::width(p, gl, "Yes, the same");
                self.yes.focused = !self.on_no;
                self.yes.draw(p, gl, row.take_left(yes_w), "Yes, the same");
                row.take_left(14.0);
                let no_w = Button::width(p, gl, "No");
                self.no.focused = self.on_no;
                self.no.draw(p, gl, row.take_left(no_w), "No");
            }
        }
    }

    pub fn input(&mut self, input: &Input, view: &View) -> Want {
        if view.busy() {
            return Want::Nothing;
        }
        match self.stage {
            Stage::Intro => {
                if self.start.input(input) {
                    return Want::Find;
                }
            }
            Stage::Found => {
                if self.back.input(input) || matches!(input, Input::Key { key: Key::Escape, .. }) {
                    self.stage = Stage::Intro;
                    return Want::Nothing;
                }
                self.again.focused = self.focus == 1 || self.macs.is_empty();
                if self.again.input(input) {
                    return Want::Find;
                }
                if matches!(input, Input::Key { key: Key::Tab, .. }) && !self.macs.is_empty() {
                    self.focus = 1 - self.focus.min(1);
                    return Want::Nothing;
                }
                self.list.focused = self.focus == 0;
                if let Some(chosen) = self.list.input(input) {
                    if let Some(mac) = self.macs.get(chosen).cloned() {
                        self.code = ui::pairing_code(&view.status.fp, &mac.fp).unwrap_or_default();
                        self.chosen = Some(mac.clone());
                        self.stage = Stage::Code;
                        self.on_no = true;
                        return Want::PairWith(mac);
                    }
                }
            }
            Stage::Code => {}
            Stage::Compare => {
                let Some(mac) = self.chosen.clone() else { return Want::Nothing };
                if let Input::Key { key: Key::Left | Key::Right | Key::Tab | Key::Up | Key::Down, .. } = input {
                    self.on_no = !self.on_no;
                    return Want::Nothing;
                }
                self.yes.focused = !self.on_no;
                self.no.focused = self.on_no;
                if self.yes.input(input) {
                    return Want::PairConfirm(mac.fp, true);
                }
                if self.no.input(input) || matches!(input, Input::Key { key: Key::Escape, .. }) {
                    return Want::PairConfirm(mac.fp, false);
                }
            }
        }
        Want::Nothing
    }
}

// --- locked ---

/// The vault is shut: one button to open it, by Touch ID here or by asking the Mac.
pub struct Locked {
    open: Button,
    recover: Button,
    /// 0: open; 1: the recovery code, when it is offered.
    focus: usize,
}

impl Locked {
    pub fn new() -> Locked {
        Locked { open: ui::keyed_button(), recover: Button::default(), focus: 0 }
    }

    fn label(view: &View) -> String {
        if view.status.opens_here || view.on_mac() {
            "Open with Touch ID".into()
        } else {
            format!("Ask {}", view.mac)
        }
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, view: &View) {
        let mut all = ui::page(p);
        look::header(p, gl, &mut all, "SpatiKey", "locked");
        let label = Self::label(view);
        ui::foot(p, gl, &mut all, &format!("[Enter] {label}"), view);
        // The mark, the button and the words under it, a little above the middle.
        let side = 168.0;
        let near_lines = if view.on_mac() { 0.0 } else { view.status.approvers.len() as f32 * 64.0 };
        let tall = side + 48.0 + Button::HEIGHT + 34.0 + theme::DETAIL_EM * 3.2 + near_lines;
        all.take_top(((all.h - tall) * 0.4).max(20.0));
        ui::mark(p, gl, Rect::new(all.x + (all.w - side) * 0.5, all.y, side, side));
        all.take_top(side + 48.0);
        let row = all.take_top(Button::HEIGHT);
        let offer_recovery = view.fingers_changed && view.on_mac();
        match view.waiting {
            Some((since, words)) => ui::waiting_centred(p, gl, row, since, words),
            None => {
                let width = Button::icon_width(p, gl, &label);
                let mut recover_w = 0.0;
                if offer_recovery {
                    recover_w = Button::icon_width(p, gl, "Use the recovery code") + 16.0;
                }
                let x = row.x + (row.w - width - recover_w) * 0.5;
                self.open.focused = self.focus == 0;
                self.open.draw_icon(p, gl, Rect { x, w: width, ..row }, if view.on_mac() { Icon::Lock } else { Icon::Desktop }, &label);
                if offer_recovery {
                    self.recover.focused = self.focus == 1;
                    self.recover.draw_icon(p, gl, Rect { x: x + width + 16.0, w: recover_w - 16.0, ..row }, Icon::Pencil, "Use the recovery code");
                }
            }
        }
        all.take_top(34.0);
        let column = Rect { x: all.x + (all.w - all.w.min(980.0)) * 0.5, w: all.w.min(980.0), ..all };
        let mut column = column;
        if view.on_mac() {
            centred(p, gl, &mut column, "One touch opens SpatiKey in this window. It closes again when the window does, or when it sits unused.", theme::ink_hint());
            return;
        }
        centred(p, gl, &mut column, &format!("Each use is approved with Touch ID on {}.", view.mac), theme::ink_hint());
        column.take_top(6.0);
        for approver in &view.status.approvers {
            let me = if view.status.name.is_empty() { "this device".to_string() } else { view.status.name.clone() };
            let (said, dot) = if approver.near {
                (format!("{} answered on this network just now.", approver.name), look::TEAL)
            } else {
                (
                    format!("{} did not answer on this network. It must be awake, running Spatiand, and on the same Wi-Fi as {me}. Tailscale does not count.", approver.name),
                    look::trouble(),
                )
            };
            let (w, h) = p.measure_left(gl, &said, theme::DETAIL_EM, column.w - 40.0);
            let x = column.x + (column.w - w - 26.0) * 0.5;
            p.fill(gl, Rect::new(x, column.y + theme::DETAIL_EM * 0.45, 12.0, 12.0), 6.0, dot);
            p.text_left(gl, &said, x + 26.0, column.y, theme::DETAIL_EM, column.w - 40.0, theme::ink_detail());
            column.take_top(h + 10.0);
        }
    }

    pub fn input(&mut self, input: &Input, view: &View) -> Want {
        if view.busy() {
            return Want::Nothing;
        }
        let offer_recovery = view.fingers_changed && view.on_mac();
        if offer_recovery && matches!(input, Input::Key { key: Key::Left | Key::Right | Key::Tab, .. }) {
            self.focus = 1 - self.focus.min(1);
            return Want::Nothing;
        }
        self.open.focused = self.focus == 0 || !offer_recovery;
        self.recover.focused = self.focus == 1 && offer_recovery;
        if self.open.input(input) {
            return Want::Open;
        }
        if offer_recovery && self.recover.input(input) {
            return Want::RecoverPage;
        }
        Want::Nothing
    }
}

/// Words centred in `column`, line by line, taken off its top.
fn centred(p: &mut Painter, gl: &Gl, column: &mut Rect, text: &str, color: spatiand_app::ui::Color) {
    let (w, h) = p.measure(gl, text, theme::DETAIL_EM, column.w);
    p.text(gl, text, column.x + (column.w - w) * 0.5, column.y, theme::DETAIL_EM, column.w, color);
    column.take_top(h + 12.0);
}
