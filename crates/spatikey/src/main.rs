//! SpatiKey: Spatiand's passwords, passkeys and two-factor codes (docs/spatikey.md).
//!
//! This is its window. The vault is never read here: SpatiKey's service holds it, and this asks
//! the service for what the wearer wants to see or have typed, one entry at a time, over the
//! service's local socket and inside a *session* -- which one touch begins, and which ends when
//! this window closes or sits unused. On a Mac the touch is its own Touch ID; on the Deck and the
//! Beam Pro it is asked of a paired Mac on the same network.
//!
//! ```text
//! spatikey                 the window
//! spatikey --demo[=...]    the same screens over made-up entries, with no service (see `demo`)
//! ```
//!
//! Nothing of the vault leaves the window. Its title is always "SpatiKey"; when the room asks what
//! the window shows, for dictation or the assistant, the answer is that title and nothing more;
//! no secret goes onto the clipboard, which is the room's and every linked computer's; and the
//! log says what was asked, never what came back.

mod ask;
mod demo;
mod generate;
mod locked;
mod open;
mod ui;

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde_json::Value;
use spatiand_app::ui::{look, theme, Input, Painter};
use spatiand_app::{App, Description, Event, WindowId};
use spatikey_core::entry::Summary;
use spatikey_core::wire::{
    Code, DevicesAnswer, FindAnswer, GetAnswer, IdAnswer, ListAnswer, OpenAnswer, PairAnswer, Refusal, Request, Settings, SettingsAnswer, SetupAnswer,
    StatusAnswer, TargetAnswer, TotpAnswer,
};
use zeroize::Zeroizing;

use ask::{Answer, Asked, Asker, Trouble};
use locked::{Away, Locked, Pair, Recover, SetUp, ShowCode, Stage};
use open::{Current, Edit, EntryPage, ListPage, SettingsPage};
use ui::{Screen, View, Want};

const APP_ID: &str = "id.prasetya.spatiand.spatikey";
const WINDOW: (u32, u32) = (1280, 860);
/// The window's one title, whatever it shows: a title is read by the room's dictation and its
/// assistant, and listed to any program that asks what windows there are.
const TITLE: &str = "SpatiKey";
/// How long something done is said at the foot.
const SAID_FOR: Duration = Duration::from_secs(6);
/// How often a device looks again whether its Mac is on this network, while it is locked.
const LOOK_FOR_MAC: Duration = Duration::from_secs(15);
/// How often, at most, the service is told that an open SpatiKey is being used: well inside the
/// shortest time it may be set to stay open.
const KEEP_OPEN: Duration = Duration::from_secs(5);

/// Something to wake the window's loop with from the asking thread: a byte down a pipe it watches.
fn pipe() -> (std::fs::File, std::fs::File) {
    use std::os::fd::FromRawFd;
    let mut ends = [0i32; 2];
    // SAFETY: pipe fills both descriptors, which are then this process's alone to own.
    unsafe {
        libc::pipe(ends.as_mut_ptr());
        for fd in ends {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
        }
        (std::fs::File::from_raw_fd(ends[0]), std::fs::File::from_raw_fd(ends[1]))
    }
}

struct SpatiKey {
    app: App,
    window: WindowId,
    painter: Option<Painter>,
    ready: bool,
    dirty: bool,
    asker: Asker,
    /// The asking thread is gone (a fault in it): "Try again" starts another.
    asker_gone: bool,
    /// What asks: `--demo`'s scenario, or None for the service.
    demo: Option<String>,
    woken: std::fs::File,
    wake: Arc<std::fs::File>,
    status: StatusAnswer,
    /// The Mac that opens the vault, by name, as the status has it.
    mac: String,
    /// The open session, while SpatiKey is open.
    session: Option<String>,
    settings: Settings,
    entries: Vec<Summary>,
    target: Option<TargetAnswer>,
    devices: Option<DevicesAnswer>,
    screen: Screen,
    /// The list as the wearer left it for an entry or settings -- searched, on a tab, a row
    /// chosen -- to come back to as it was.
    kept_list: Option<ListPage>,
    /// What is being waited on: the request, since when, and the words for it.
    waiting: Option<(Asked, Instant, String)>,
    trouble: Option<String>,
    /// What was just done, and until when it is said.
    said: Option<(String, Instant)>,
    fingers_changed: bool,
    /// When the wearer last did something to this window. An open SpatiKey that sits unused
    /// shuts itself, rather than leave its list on show until the service's own clock runs out
    /// -- which a code being kept fresh on an entry's page would never let it do.
    used: Instant,
    /// When the service was last told that this window is in use (`keep_open`).
    kept: Instant,
    /// When the paired Macs were last looked for, for the locked screen's line on a device.
    looked: Instant,
}

/// What the screen is lent, from the fields themselves, so that the screen itself can be
/// borrowed beside it.
macro_rules! view {
    ($key:expr) => {
        View {
            status: &$key.status,
            mac: &$key.mac,
            entries: &$key.entries,
            settings: $key.settings,
            waiting: $key.waiting.as_ref().map(|(_, since, words)| (*since, words.as_str())),
            trouble: $key.trouble.as_deref(),
            said: $key.said.as_ref().filter(|(_, until)| Instant::now() < *until).map(|(said, _)| said.as_str()),
            target: $key.target.as_ref(),
            devices: $key.devices.as_ref(),
            fingers_changed: $key.fingers_changed,
        }
    };
}

/// The thread that asks: the service, or `--demo`'s stand-in for it. `wake` is nudged after each
/// answer.
fn start_asking(demo: &Option<String>, wake: Arc<std::fs::File>) -> Asker {
    let service: Box<dyn ask::Service> = match demo {
        Some(scenario) => {
            log::info!("a demonstration: nothing in this SpatiKey is real");
            Box::new(demo::Fake::new(scenario))
        }
        None => Box::new(ask::Socket::new()),
    };
    Asker::start(service, move || {
        let _ = (&*wake).write(&[1]);
    })
}

/// The Mac that opens the vault, by name: this one on a Mac, else the paired Mac on this network
/// (or the first paired).
fn mac_of(status: &StatusAnswer) -> String {
    if status.opens_here || status.can_set_up {
        return if status.name.is_empty() { "this Mac".into() } else { status.name.clone() };
    }
    status.approvers.iter().find(|a| a.near).or(status.approvers.first()).map_or_else(|| "your Mac".into(), |a| a.name.clone())
}

/// The screen a closed SpatiKey shows, from how things stand.
fn door(status: &StatusAnswer) -> Screen {
    if status.opens_here {
        Screen::Locked(Locked::new())
    } else if status.can_set_up && !status.vault {
        Screen::SetUp(SetUp::new())
    } else if status.can_set_up {
        // A Mac the vault has reached whose Touch ID has no key to it.
        Screen::Recover(Recover::new(false))
    } else if !status.approvers.is_empty() {
        Screen::Locked(Locked::new())
    } else {
        Screen::Pair(Pair::new())
    }
}

/// An answer as what it should be; None, said in the log by its type only, if it is not.
fn read<T: DeserializeOwned>(value: Value) -> Option<T> {
    match serde_json::from_value(value) {
        Ok(read) => Some(read),
        Err(_) => {
            log::warn!("an answer that is not a {}", std::any::type_name::<T>());
            None
        }
    }
}

impl SpatiKey {
    fn ask(&mut self, asked: Asked, request: Request) {
        self.asker.ask(asked, request);
    }

    /// Ask, with the three dots and the words for what is waited on until the answer comes.
    fn ask_waiting(&mut self, asked: Asked, request: Request, words: impl Into<String>) {
        self.waiting = Some((asked.clone(), Instant::now(), words.into()));
        self.ask(asked, request);
    }

    fn on_mac(&self) -> bool {
        self.status.opens_here || self.status.can_set_up
    }

    fn say(&mut self, said: impl Into<String>) {
        self.said = Some((said.into(), Instant::now() + SAID_FOR));
    }

    /// Ask how things stand, and show what follows from it.
    fn route(&mut self) {
        self.ask(Asked::Status { route: true }, Request::Status);
    }

    /// Everything an open SpatiKey knew, gone: the session ended.
    fn forget_open(&mut self) {
        self.session = None;
        self.entries.clear();
        self.devices = None;
        self.target = None;
        self.kept_list = None;
    }

    /// Show `screen`, keeping the list if it is the list being left.
    fn set_screen(&mut self, screen: Screen) {
        if let Screen::List(list) = std::mem::replace(&mut self.screen, screen) {
            self.kept_list = Some(list);
        }
    }

    /// The list, as it was left.
    fn show_list(&mut self) {
        let list = self.kept_list.take().unwrap_or_else(ListPage::new);
        self.screen = Screen::List(list);
    }

    /// Close the session and show the locked screen.
    fn lock(&mut self) {
        if let Some(session) = self.session.clone() {
            self.ask(Asked::Close, Request::Close { session });
        }
        self.forget_open();
        self.waiting = None;
        // What was just done was done in the session, and is not the locked screen's to say.
        self.said = None;
        self.screen = Screen::Locked(Locked::new());
        self.ask(Asked::Status { route: false }, Request::Status);
    }

    fn show_entry(&mut self, id: String) {
        let Some(session) = self.session.clone() else { return self.route() };
        self.set_screen(Screen::Entry(EntryPage::new(id.clone())));
        self.ask(Asked::Get(id.clone()), Request::Get { session, id });
        self.ask_target();
    }

    /// The window to type into, asked again each time a page that types opens: it is whichever
    /// the wearer was in before SpatiKey, which changes.
    fn ask_target(&mut self) {
        self.target = None;
        self.ask(Asked::Target, Request::Target);
    }

    fn show_settings(&mut self) {
        let Some(session) = self.session.clone() else { return self.route() };
        self.set_screen(Screen::Settings(SettingsPage::new()));
        self.ask(Asked::Devices, Request::Devices { session: session.clone() });
        self.ask(Asked::Settings, Request::Settings { session, set: None });
    }

    fn want(&mut self, want: Want) {
        if matches!(want, Want::Nothing) {
            return;
        }
        self.dirty = true;
        // Whatever went wrong, or was just done, was about what came before.
        self.trouble = None;
        self.said = None;
        match want {
            Want::Nothing => {}
            Want::Status => {
                if std::mem::take(&mut self.asker_gone) {
                    log::info!("asking again, on a new thread");
                    self.asker = start_asking(&self.demo, self.wake.clone());
                }
                self.screen = Screen::Waiting(Instant::now());
                self.route();
            }
            Want::SetUp => {
                let name = self.status.name.clone();
                self.ask_waiting(Asked::Setup, Request::Setup { name }, "Touch ID on this Mac");
            }
            Want::CodeSaved { first } => {
                if first {
                    self.route();
                } else {
                    self.show_settings();
                }
            }
            Want::Open => {
                self.fingers_changed = false;
                let words = if self.on_mac() { "Touch ID on this Mac".to_string() } else { format!("Touch ID on {}", self.mac) };
                self.ask_waiting(Asked::Open, Request::Open, words);
            }
            Want::Lock => self.lock(),
            Want::RecoverPage => self.set_screen(Screen::Recover(Recover::new(self.session.is_some()))),
            Want::Recover(code) => {
                self.ask_waiting(Asked::Recover, Request::Recover { code: code.to_string() }, "Touch ID on this Mac");
            }
            Want::Find => self.ask_waiting(Asked::Find, Request::Find, "Looking for Macs on this network"),
            Want::PairWith(mac) => {
                let (fp, words) = (mac.fp.clone(), format!("Waiting for {}", mac.name));
                self.ask_waiting(Asked::PairWith(mac), Request::PairWith { fp }, words);
            }
            Want::PairConfirm(fp, ok) => {
                if !ok {
                    if let Screen::Pair(pair) = &mut self.screen {
                        pair.stage = Stage::Found;
                    }
                    self.trouble = Some("Not paired: the digits were not the same. Choose the Mac again to start over.".into());
                }
                self.ask(Asked::PairConfirm(ok), Request::PairConfirm { fp, ok });
            }
            Want::Show(id) => self.show_entry(id),
            Want::List => {
                if self.session.is_some() {
                    self.show_list();
                }
            }
            Want::Add => {
                self.set_screen(Screen::Edit(Edit::new()));
                self.ask_target();
            }
            Want::Edit(entry) => {
                self.set_screen(Screen::Edit(Edit::of(*entry)));
                self.ask_target();
            }
            Want::Save(entry) => {
                if let Some(session) = self.session.clone() {
                    self.ask_waiting(Asked::Put, Request::Put { session, entry: *entry }, "Saving");
                }
            }
            Want::Delete(id) => {
                if let Some(session) = self.session.clone() {
                    self.ask(Asked::Delete, Request::Delete { session, id });
                }
            }
            Want::Type { id, fields } => {
                let (Some(session), Some(target)) = (self.session.clone(), self.target.clone()) else { return };
                let name = ui::target_name(&target);
                let words = format!("Typing into {name}");
                self.ask_waiting(Asked::Type(name), Request::Type { session: Some(session), id, fields, window: Some(target.window) }, words);
            }
            Want::Settings => self.show_settings(),
            Want::SetSettings(settings) => {
                if let Some(session) = self.session.clone() {
                    self.ask(Asked::Settings, Request::Settings { session, set: Some(settings) });
                }
            }
            Want::Forget(fp) => {
                if let Some(session) = self.session.clone() {
                    self.ask(Asked::Forget, Request::ForgetDevice { session, fp });
                }
            }
            Want::NewCode => {
                if let Some(session) = self.session.clone() {
                    self.ask_waiting(Asked::RecoveryNew, Request::RecoveryNew { session }, "Making a new code");
                }
            }
        }
    }

    fn answered(&mut self, answer: Answer) {
        let Answer { asked, result } = answer;
        if self.waiting.as_ref().is_some_and(|(waited, _, _)| *waited == asked) {
            self.waiting = None;
        }
        self.dirty = true;
        let value = match result {
            Ok(value) => value,
            Err(Trouble::Away(why)) => {
                self.forget_open();
                self.waiting = None;
                self.screen = Screen::Away(Away::new(why));
                return;
            }
            Err(Trouble::Refused(refusal)) => return self.refused(asked, refusal),
        };
        // What an answer leads to asking next, asked once the screen is let go of.
        let mut then: Vec<(Asked, Request)> = Vec::new();
        match asked {
            Asked::Status { route } => {
                let Some(status) = read::<StatusAnswer>(value) else { return };
                self.status = status;
                self.mac = mac_of(&self.status);
                let unsettled = matches!(self.screen, Screen::Waiting(_) | Screen::Away(_));
                if self.session.is_none() && (route || unsettled) {
                    self.screen = door(&self.status);
                }
            }
            Asked::Setup => {
                let Some(answer) = read::<SetupAnswer>(value) else { return };
                self.screen = Screen::Code(ShowCode::new(Zeroizing::new(answer.recovery_code), true));
                then.push((Asked::Status { route: false }, Request::Status));
            }
            Asked::Open => {
                let Some(answer) = read::<OpenAnswer>(value) else { return };
                log::info!("SpatiKey is open: {} entries", answer.entries.len());
                self.said = None;
                self.entries = answer.entries;
                self.used = Instant::now();
                self.kept_list = None;
                self.screen = Screen::List(ListPage::new());
                then.push((Asked::Settings, Request::Settings { session: answer.session.clone(), set: None }));
                self.session = Some(answer.session);
            }
            Asked::Close | Asked::PairConfirm(false) => {}
            Asked::List => {
                if let Some(answer) = read::<ListAnswer>(value) {
                    self.entries = answer.entries;
                }
            }
            Asked::Get(id) => {
                let Some(answer) = read::<GetAnswer>(value) else { return };
                let mut entry = answer.entry;
                match &mut self.screen {
                    Screen::Entry(page) if page.id == id => page.entry = Some(entry),
                    // Left before it came: not kept.
                    _ => entry.wipe(),
                }
            }
            Asked::Put => {
                let Some(answer) = read::<IdAnswer>(value) else { return };
                self.say("Saved.");
                if let Some(session) = self.session.clone() {
                    then.push((Asked::List, Request::List { session }));
                }
                self.show_entry(answer.id);
            }
            Asked::Delete => {
                self.say("Deleted.");
                if let Some(session) = self.session.clone() {
                    then.push((Asked::List, Request::List { session }));
                }
                self.show_list();
            }
            Asked::Totp(id) => {
                let Some(answer) = read::<TotpAnswer>(value) else { return };
                if let Screen::Entry(page) = &mut self.screen {
                    if page.id == id {
                        page.totp_asked = false;
                        page.totp = Some(Current { code: Zeroizing::new(answer.code), until: Instant::now() + Duration::from_secs(answer.remaining.max(1)), period: answer.period });
                    }
                }
            }
            Asked::Type(title) => self.say(format!("Typed into {title}.")),
            Asked::Target => self.target = read::<TargetAnswer>(value),
            Asked::RecoveryNew => {
                let Some(answer) = read::<SetupAnswer>(value) else { return };
                self.set_screen(Screen::Code(ShowCode::new(Zeroizing::new(answer.recovery_code), false)));
            }
            Asked::Recover => {
                self.fingers_changed = false;
                self.say("Touch ID is set up again on this Mac.");
                if self.session.is_some() {
                    self.show_settings();
                } else {
                    self.route();
                }
            }
            Asked::Devices => self.devices = read::<DevicesAnswer>(value),
            Asked::Forget => {
                self.say("Forgotten.");
                if let Some(session) = self.session.clone() {
                    then.push((Asked::Devices, Request::Devices { session }));
                }
            }
            Asked::Settings => {
                if let Some(answer) = read::<SettingsAnswer>(value) {
                    self.settings = answer.settings;
                }
            }
            Asked::Find => {
                let Some(answer) = read::<FindAnswer>(value) else { return };
                if let Screen::Pair(pair) = &mut self.screen {
                    pair.found(answer.macs);
                }
            }
            Asked::PairWith(mac) => {
                let Some(answer) = read::<PairAnswer>(value) else { return };
                if let Screen::Pair(pair) = &mut self.screen {
                    if !answer.paired {
                        pair.stage = Stage::Found;
                        self.trouble = Some(format!("{} said no. Nothing was paired.", mac.name));
                    } else if !ui::same_digits(&answer.code, &pair.code) {
                        // The Mac showed digits other than the ones worked out here: not the
                        // Mac this device meant, whatever it calls itself.
                        pair.stage = Stage::Found;
                        self.trouble = Some(format!("{} showed other digits than these. Nothing was paired.", mac.name));
                        then.push((Asked::PairConfirm(false), Request::PairConfirm { fp: mac.fp, ok: false }));
                    } else {
                        pair.stage = Stage::Compare;
                    }
                }
            }
            Asked::PairConfirm(true) => {
                self.say(format!("Paired with {}.", self.mac_named()));
                self.route();
            }
        }
        for (asked, request) in then {
            self.ask(asked, request);
        }
    }

    /// The Mac just paired with, by name, before the status has caught up.
    fn mac_named(&self) -> String {
        match &self.screen {
            Screen::Pair(pair) => pair.chosen.as_ref().map_or_else(|| self.mac.clone(), |m| m.name.clone()),
            _ => self.mac.clone(),
        }
    }

    fn refused(&mut self, asked: Asked, refusal: Refusal) {
        // A session that has ended -- unused too long, or the service started again -- ends
        // here too.
        if refusal.code == Code::BadSession && self.session.is_some() {
            self.lock();
            self.trouble = Some(ui::meaning(&refusal, &self.mac, self.on_mac()));
            return;
        }
        match &asked {
            // No window to type into is said where the buttons would be, not as trouble.
            Asked::Target => {
                self.target = None;
                return;
            }
            Asked::Totp(_) => {
                if let Screen::Entry(page) = &mut self.screen {
                    page.totp_asked = false;
                    page.totp_refused = true;
                }
                return;
            }
            Asked::Status { route: false } | Asked::Close => return,
            Asked::Status { route: true } if matches!(self.screen, Screen::Waiting(_)) => {
                self.screen = Screen::Away(Away::new(ui::meaning(&refusal, &self.mac, self.on_mac())));
                return;
            }
            Asked::PairWith(_) => {
                if let Screen::Pair(pair) = &mut self.screen {
                    pair.stage = Stage::Found;
                }
            }
            Asked::Open if refusal.code == Code::FingerprintsChanged => self.fingers_changed = true,
            // A code that does not open the vault is a code mistyped far more often than a fault.
            Asked::Recover if matches!(refusal.code, Code::Failed | Code::BadRequest | Code::NoMatch) => {
                let why = if refusal.message.trim().is_empty() { String::new() } else { format!(": {}", refusal.message.trim()) };
                self.said = None;
                self.trouble = Some(format!("That code does not open the vault{why}. Check it against \u{201C}SpatiKey recovery\u{201D} in Apple Passwords."));
                return;
            }
            _ => {}
        }
        self.said = None;
        self.trouble = Some(ui::meaning(&refusal, &self.mac, self.on_mac()));
    }

    /// Tell the service that this window is in use. The service closes a session by its own
    /// clock, from the last thing asked in it; typing in the editor, reading an entry or
    /// scrolling the list asks it nothing, so without this a long entry written past the idle
    /// time would be refused when saved, and lost with the screen that held it. Asked only when
    /// something was done here since the last asking, and no more often than `KEEP_OPEN`: an
    /// untouched window still runs out, there as here. The settings are what is asked for --
    /// they are small, and nothing of the vault's.
    fn keep_open(&mut self, session: &str) {
        if self.used > self.kept && self.kept.elapsed() >= KEEP_OPEN {
            self.kept = Instant::now();
            self.ask(Asked::Settings, Request::Settings { session: session.to_string(), set: None });
        }
    }

    /// What happens with time alone: an unused SpatiKey shutting, a code running out, a device
    /// looking again for its Mac.
    fn tick(&mut self) {
        if let Some(session) = self.session.clone() {
            // A new recovery code is shown this once, to be typed by eye into another window:
            // it is not taken down for this window's sitting unused meanwhile (the old code
            // stopped working when it was made). The service closes the session by its own
            // clock all the same, and the next thing asked here finds that out.
            let code_shown = matches!(self.screen, Screen::Code(_));
            if !code_shown && self.used.elapsed() >= Duration::from_secs(self.settings.open_secs.max(30)) {
                let unused = ui::span(self.settings.open_secs).to_lowercase();
                log::info!("SpatiKey sat unused for {unused}: shut");
                self.lock();
                // Said for longer than what was just done: it explains the screen.
                self.said = Some((format!("SpatiKey shut after {unused} unused."), Instant::now() + Duration::from_secs(600)));
                return;
            }
            self.keep_open(&session);
            let mut code = None;
            if let Screen::Entry(page) = &mut self.screen {
                if page.wants_code() {
                    page.totp_asked = true;
                    code = Some(page.id.clone());
                }
            }
            if let Some(id) = code {
                self.ask(Asked::Totp(id.clone()), Request::Totp { session: Some(session), id });
            }
        } else if matches!(self.screen, Screen::Locked(_)) && !self.on_mac() && self.asker.out == 0 && self.looked.elapsed() >= LOOK_FOR_MAC {
            self.looked = Instant::now();
            self.ask(Asked::Status { route: false }, Request::Status);
        }
        if self.said.as_ref().is_some_and(|(_, until)| Instant::now() >= *until) {
            self.said = None;
            self.dirty = true;
        }
    }

    fn animating(&self) -> bool {
        self.waiting.is_some() || matches!(self.screen, Screen::Waiting(_)) || matches!(&self.screen, Screen::Entry(page) if page.entry.is_none())
    }

    fn draw(&mut self) -> Result<(), String> {
        if !self.ready {
            return Ok(());
        }
        if !std::mem::take(&mut self.dirty) && !self.animating() && !self.screen.ticking() {
            return Ok(());
        }
        let view = view!(self);
        let (screen, painter) = (&mut self.screen, &mut self.painter);
        self.app.draw(self.window, |gl, size| {
            let p = painter.get_or_insert_with(|| Painter::new(gl).expect("the painter"));
            p.begin(gl, size, 1.0, theme::ground());
            look::backdrop(p, gl);
            screen.draw(p, gl, &view);
            p.end(gl);
        })?;
        Ok(())
    }

    fn handle(&mut self, event: Event) -> bool {
        match &event {
            Event::CloseRequested { window } if *window == self.window => {
                // The session ends with the window, said to the service rather than left to the
                // connection's going.
                if let Some(session) = self.session.take() {
                    self.asker.finish(Asked::Close, Request::Close { session }, Duration::from_millis(1500));
                }
                return false;
            }
            Event::Ready { window, .. } if *window == self.window => self.ready = true,
            Event::Resized { window, width, height } if *window == self.window => {
                self.app.resize(self.window, ((*width).max(960), (*height).max(680)));
            }
            Event::Describe { window } => {
                // The title alone: nothing of the vault -- not even what is in it -- is for
                // dictation or the assistant.
                self.app.describe(*window, &Description { title: TITLE.into(), ..Description::default() });
                return true;
            }
            _ => {}
        }
        self.dirty = true;
        if event.window().is_some_and(|window| window != self.window) {
            return true;
        }
        let Some(input) = Input::of(&event, 1.0) else { return true };
        if matches!(input, Input::Key { .. } | Input::Press { .. } | Input::Scroll { .. }) {
            self.used = Instant::now();
        }
        let view = view!(self);
        let want = self.screen.input(&input, &view);
        self.want(want);
        true
    }

    fn run(&mut self) -> Result<(), String> {
        self.app.watch(self.woken.as_raw_fd());
        self.route();
        loop {
            let wait = Duration::from_millis(if self.animating() { 33 } else { 250 });
            for event in self.app.wait(Some(wait))? {
                if !self.handle(event) {
                    return Ok(());
                }
            }
            let mut drained = [0u8; 64];
            while (&self.woken).read(&mut drained).is_ok_and(|n| n > 0) {}
            match self.asker.answers() {
                Some(answers) => {
                    for answer in answers {
                        self.answered(answer);
                    }
                }
                None if !self.asker_gone => {
                    // What it held of the vault went with it: SpatiKey starts again from asking.
                    self.asker_gone = true;
                    self.forget_open();
                    self.waiting = None;
                    self.screen = Screen::Away(Away::new("Something went wrong inside SpatiKey while it was asking.".into()));
                    self.dirty = true;
                }
                None => {}
            }
            self.tick();
            self.draw()?;
        }
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    // `--demo`, or `--demo=deck` and the like: see `demo`.
    let demo = std::env::args().skip(1).find_map(|a| a.strip_prefix("--demo").map(|rest| rest.trim_start_matches('=').to_string()));
    let run = || -> Result<(), String> {
        let mut app = App::connect(APP_ID)?;
        let window = app.open(TITLE, WINDOW);
        let (woken, wake) = pipe();
        let wake = Arc::new(wake);
        let asker = start_asking(&demo, wake.clone());
        let mut key = SpatiKey {
            app,
            window,
            painter: None,
            ready: false,
            dirty: true,
            asker,
            asker_gone: false,
            demo,
            woken,
            wake,
            status: StatusAnswer::default(),
            mac: "your Mac".into(),
            session: None,
            settings: Settings::default(),
            entries: Vec::new(),
            target: None,
            devices: None,
            screen: Screen::Waiting(Instant::now()),
            kept_list: None,
            waiting: None,
            trouble: None,
            said: None,
            fingers_changed: false,
            used: Instant::now(),
            kept: Instant::now(),
            looked: Instant::now(),
        };
        key.run()
    };
    if let Err(e) = run() {
        log::error!("{e}");
        std::process::exit(1);
    }
}
