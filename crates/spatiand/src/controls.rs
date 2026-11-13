//! Game controls: the focused application's layout, running against every controller there is.
//!
//! Every frame the Deck's controls, any Bluetooth or USB pad and the glasses' gyro and temple
//! buttons are merged into one snapshot, run through the layout the focused application uses,
//! and delivered as one virtual gamepad plus keys, mouse and Spatiand commands. The game is
//! told to ignore every controller but the virtual one — see [`Controls::launch_environment`]
//! and `docs/input-mapper.md` — so it sees exactly one device, as it would in Game Mode.
//!
//! The layout follows focus: a Steam game by its app id, anything else by its app id or window
//! class. Each is stored on its own, in `~/.config/spatiand/apps/layouts` (shared with every
//! paired machine) or, from before that, `~/.config/spatiand/layouts` -- see `Store`.
//!
//! **A trackpad's presses** (a Mac's: [`spatiand_input::PadPress`]) are the pointer's clicks
//! unless the application's layout binds them. Where the press lands decides first: on the
//! room's own things -- a title bar and its buttons, the keyboard, a notice, a menu, a drag going
//! on -- it is the room's, as always. Anywhere else it is the application's, and if the layout
//! binds that press it is the layout's alone: the click it would have made is held back. See
//! [`Controls::route_pad_press`].

use std::time::{Duration, Instant};

use glam::DVec3;
use spatiand_input::gamepad::{GamepadState, Gamepads};
use spatiand_input::virtual_pad::{self, Report, VirtualPad};
use spatiand_input::{Control, ControllerState, PadPress};
use spatiand_mapper::editor::{self, Editor};
use spatiand_mapper::{
    AppKey, Button, Command, Engine, Frame, MouseButton, PadButton, RadialView, Rates, Snapshot,
    Store, Touch,
};

/// The glasses only say a temple button was pressed, never that it was let go, so a press is
/// held down for this long.
const GLASSES_PRESS: Duration = Duration::from_millis(120);
/// How long a pad's guide button is held to open the launcher rather than the settings.
///
/// A PlayStation or Xbox pad has no `⋯`, and every other button it has is a game's. So the one
/// button that is already the session's does both: a press is STEAM, a hold is `⋯`.
const GUIDE_HOLD: Duration = Duration::from_millis(500);
/// The controls of another pad that work the menus, as the Deck's own controls they stand for.
const PAD_MENU_CONTROLS: [(Button, Control); 7] = [
    (Button::DpadUp, Control::Up),
    (Button::DpadDown, Control::Down),
    (Button::DpadLeft, Control::Left),
    (Button::DpadRight, Control::Right),
    (Button::A, Control::A),
    (Button::B, Control::B),
    (Button::Y, Control::Y),
];

/// What the frame's layout asks the compositor to deliver, as changes.
#[derive(Debug, Default)]
pub struct Delivery {
    /// `(evdev code, pressed)`.
    pub keys: Vec<(u32, bool)>,
    pub mouse_buttons: Vec<(u32, bool)>,
    pub motion: (i32, i32),
    pub wheel: (i32, i32),
    pub commands: Vec<Command>,
    /// Which triggers click Spatiand's pointer, `[left, right]`.
    pub pointer_clicks: [bool; 2],
    /// A Bluetooth pad's guide button was pressed and let go: what STEAM does on the Deck.
    pub guide: bool,
    /// ...or held: what `⋯` does. See [`GUIDE_HOLD`].
    pub guide_held: bool,
    /// Menu controls another pad pressed this frame, as the Deck's controls they stand for.
    /// Only the menus read these; in the world the same presses belong to the layout.
    pub menu_presses: Vec<Control>,
    pub radial: Option<RadialView>,
}

pub struct Controls {
    store: Store,
    engine: Engine,
    app: AppKey,
    app_name: String,
    pad: Option<VirtualPad>,
    gamepads: Gamepads,
    glasses_sum: DVec3,
    glasses_samples: u32,
    glasses_pressed: [Option<Instant>; 2],
    keys: Vec<u16>,
    mouse: Vec<MouseButton>,
    guide_now: bool,
    /// When the guide button went down, and whether its hold has already done its thing.
    guide_down: Option<(Instant, bool)>,
    /// Other pads' buttons last frame and this, for the menus' presses.
    pad_buttons_was: spatiand_mapper::Buttons,
    pad_buttons_now: spatiand_mapper::Buttons,
    /// The first touchpad on another pad that has a thumb on it, or failing that the first
    /// there is.
    touchpad: Option<spatiand_input::gamepad::Touchpad>,
    last_step: Option<Instant>,
    suspended: bool,
    editor: Option<Editor>,
    /// The editor was told to go back to the default, so closing it should not save that
    /// default as though it were a layout of the application's own.
    reset_to_default: bool,
    /// Whether the virtual pad has already been complained about, so it is said once.
    pad_complained: bool,
    /// The buttons the pad was last told about, so a change in them is said once.
    pad_buttons: (u16, bool, bool, bool, bool),
    /// The last report built, whether or not there was a device here to take it. A remote
    /// application is played with this same report, sent to its host — see `remote::Remotes`.
    report: Report,
    /// Where the trackpad's press going on went. See [`Controls::route_pad_press`].
    route: PressRoute,
    /// Whether other pads are read at all. See [`Controls::without_devices`].
    devices: bool,
    /// Whether a controller has said it reports a trackpad's presses, for the editor.
    pad_presses_reported: bool,
}

/// Where a trackpad's press went, decided as it went down, and the clicks it held back.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct PressRoute {
    /// The press this is about, by its fingers; 0 for none.
    fingers: u8,
    /// It went to the application: the layout reads it (a frame after it went down).
    to_layout: bool,
    /// The layout binds it, so the click it made is not the pointer's.
    claimed: bool,
    /// The pointer's two buttons as last seen, and whether each one's press is held back.
    right_was: bool,
    left_was: bool,
    right_held_back: bool,
    left_held_back: bool,
}

impl PressRoute {
    /// One frame: the press as the controller says it, whether the pointer is on the room's own
    /// things (asked only as a press begins), whether the layout binds a press, and the
    /// pointer's two buttons as the controller made them. Returns the buttons the pointer is to
    /// see: unchanged, except a press of either that began with a claimed press.
    ///
    /// Only a button's going down is held back, and then until it comes up: a button already
    /// held -- the three-finger drag -- goes on being held whatever is pressed meanwhile.
    pub fn frame(
        &mut self,
        press: Option<PadPress>,
        room_pointed: impl FnOnce() -> bool,
        binds: impl Fn(Button) -> bool,
        right: bool,
        left: bool,
    ) -> (bool, bool) {
        let fingers = press.map(|p| p.fingers).unwrap_or(0);
        if fingers == 0 {
            self.fingers = 0;
            self.to_layout = false;
            self.claimed = false;
        } else if self.fingers == 0 {
            self.fingers = fingers;
            self.to_layout = !room_pointed();
            self.claimed = self.to_layout
                && Button::pad_press(fingers, false).is_some_and(&binds);
        }
        if self.claimed {
            self.right_held_back |= right && !self.right_was;
            self.left_held_back |= left && !self.left_was;
        }
        self.right_held_back &= right;
        self.left_held_back &= left;
        self.right_was = right;
        self.left_was = left;
        (right && !self.right_held_back, left && !self.left_held_back)
    }

    /// The press's buttons as the layout is to read them: only one that went to the
    /// application, and only while it is the same press.
    pub fn buttons(&self, press: Option<PadPress>) -> Vec<Button> {
        match press {
            Some(p) if self.to_layout && p.fingers == self.fingers => {
                let mut out: Vec<Button> = Button::pad_press(p.fingers, false).into_iter().collect();
                if p.deep {
                    out.extend(Button::pad_press(p.fingers, true));
                }
                out
            }
            _ => Vec::new(),
        }
    }
}

/// What a layout's Spatiand action asks of the room, for every room loop to do the same way.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Hand the shell this, as a button of the room's would.
    Intent(spatiand_shell::Intent),
    /// Do this, as the shell's own rows do.
    Event(spatiand_shell::ShellEvent),
    /// Show or hide the room's keyboard.
    Keyboard,
    Screenshot,
    /// Open the list of open windows, which is filled first.
    Switcher,
    /// The keyboard's microphone key: dictate into what has the keys, or stop.
    Dictate,
    /// Nothing to do it to: closing or hiding with no window in front.
    Nothing,
}

/// What `command` does, with `focused` the window in front (`WindowLayout::focused_id`).
pub fn command_effect(command: Command, focused: Option<usize>) -> Effect {
    use spatiand_shell::{HudAction, Intent, ShellEvent};
    match command {
        Command::Hud => Effect::Intent(Intent::ToggleHud),
        Command::Launcher => Effect::Intent(Intent::ToggleLauncher),
        Command::Keyboard => Effect::Keyboard,
        Command::Screenshot => Effect::Screenshot,
        Command::Recentre => Effect::Event(ShellEvent::Hud(HudAction::Recentre)),
        // The window in front, as the bar's cross and hide button do it.
        Command::CloseWindow => focused.map_or(Effect::Nothing, |id| Effect::Event(ShellEvent::CloseWindow(id))),
        Command::HideWindow => {
            focused.map_or(Effect::Nothing, |id| Effect::Event(ShellEvent::HideWindow { id, hidden: true }))
        }
        Command::Switcher => Effect::Switcher,
        Command::Dictate => Effect::Dictate,
        Command::Record => Effect::Event(ShellEvent::Hud(HudAction::Record)),
    }
}

/// What the keys held went from and to, as presses and releases in the order a keyboard makes
/// them for a shortcut: modifiers down first, then the rest; the rest up first, then the
/// modifiers. So a layout's Ctrl+Shift+T reaches the window as Control, Shift, T, and is let go
/// as T, Shift, Control -- by code it would have been T first, a bare T.
pub fn key_changes(was: &[u16], now: &[u16]) -> Vec<(u32, bool)> {
    use spatiand_mapper::keys::is_modifier;
    let up = |modifier: bool| {
        was.iter()
            .filter(move |c| !now.contains(c) && is_modifier(**c) == modifier)
            .map(|c| (*c as u32, false))
    };
    let down = |modifier: bool| {
        now.iter()
            .filter(move |c| !was.contains(c) && is_modifier(**c) == modifier)
            .map(|c| (*c as u32, true))
    };
    up(false).chain(up(true)).chain(down(true)).chain(down(false)).collect()
}

impl Controls {
    pub fn new() -> Self {
        let pad = match VirtualPad::create() {
            Ok(pad) => Some(pad),
            Err(e) => {
                log::warn!("no virtual gamepad ({e}); games will not see a controller");
                None
            }
        };
        Self::with_pad(pad, true)
    }

    /// With no virtual gamepad made and no other pad read: the layouts and their keys, mouse and
    /// commands alone. For the snapshot room's scripted presses, which must not make or take a
    /// device on the machine they run on.
    #[allow(dead_code)] // Only the snapshot room's; the Mac and the phone compile this file too.
    pub fn without_devices() -> Self {
        Self::with_pad(None, false)
    }

    fn with_pad(pad: Option<VirtualPad>, devices: bool) -> Self {
        let app = AppKey::App(String::new());
        Self {
            devices,
            store: Store::new(spatiand_track::config::config_dir().join("layouts"))
                .with_synced(spatiand_sync::default_folders().0.join("layouts")),
            engine: Engine::new(app.default_layout()),
            app,
            app_name: "the desktop".into(),
            pad,
            gamepads: Gamepads::new(),
            glasses_sum: DVec3::ZERO,
            glasses_samples: 0,
            glasses_pressed: [None, None],
            keys: Vec::new(),
            mouse: Vec::new(),
            guide_now: false,
            guide_down: None,
            pad_buttons_was: Default::default(),
            pad_buttons_now: Default::default(),
            touchpad: None,
            last_step: None,
            suspended: false,
            editor: None,
            reset_to_default: false,
            pad_complained: false,
            pad_buttons: (0, false, false, false, false),
            report: Report::default(),
            route: PressRoute::default(),
            pad_presses_reported: false,
        }
    }

    /// What every launched application's environment gets, so that a game — and Steam, whose
    /// environment every game it starts inherits — sees only the virtual pad.
    pub fn launch_environment() -> Vec<(String, String)> {
        virtual_pad::hide_other_controllers()
    }

    /// The application in front of the wearer changed.
    pub fn focus(&mut self, app: AppKey, name: &str) {
        if app == self.app {
            self.app_name = name.into();
            return;
        }
        if self.editor.is_some() {
            self.close_editor();
        }
        let (layout, own) = self.store.layout_for(&app);
        log::info!(
            "controls for {name}: {} ({})",
            layout.name,
            if own { "its own layout" } else { "the default" }
        );
        self.engine.set_layout(layout);
        self.app = app;
        self.app_name = name.into();
    }

    /// One glasses IMU sample, bias removed and in the head frame (+X forward, +Y left, +Z up),
    /// degrees per second.
    pub fn glasses_gyro(&mut self, head: DVec3) {
        self.glasses_sum += head;
        self.glasses_samples += 1;
    }

    pub fn glasses_button(&mut self, up: bool) {
        self.glasses_pressed[if up { 0 } else { 1 }] = Some(Instant::now());
    }

    /// Everything physical, merged.
    pub fn gather(&mut self, deck: Option<&ControllerState>) -> Snapshot {
        let mut snapshot = deck.map(deck_snapshot).unwrap_or_default();
        // A trackpad's press, where it went to the application (`route_pad_press`, last frame).
        let press = deck.and_then(|d| d.pad_press);
        self.pad_presses_reported |= deck.is_some_and(|d| d.reports_pad_presses);
        if press.is_none() {
            self.route.fingers = 0;
            self.route.to_layout = false;
            self.route.claimed = false;
        }
        for button in self.route.buttons(press) {
            snapshot.buttons.set(button, true);
        }
        self.guide_now = false;
        self.pad_buttons_was = self.pad_buttons_now;
        self.pad_buttons_now = Default::default();
        self.touchpad = None;
        let pads = if self.devices { self.gamepads.poll() } else { Vec::new() };
        for pad in pads {
            self.guide_now |= pad.guide;
            let theirs = gamepad_snapshot(&pad);
            self.pad_buttons_now = self.pad_buttons_now.union(theirs.buttons);
            if let Some(t) = pad.touchpad {
                if self.touchpad.is_none_or(|held| !held.touched && t.touched) {
                    self.touchpad = Some(t);
                }
            }
            snapshot.merge(&theirs);
        }
        if self.glasses_samples > 0 {
            let r = self.glasses_sum / self.glasses_samples as f64;
            snapshot.glasses_gyro = Some(head_rates(r));
            self.glasses_sum = DVec3::ZERO;
            self.glasses_samples = 0;
        }
        let now = Instant::now();
        for (slot, button) in [(0, Button::GlassesUp), (1, Button::GlassesDown)] {
            let held = self.glasses_pressed[slot].is_some_and(|t| now.duration_since(t) < GLASSES_PRESS);
            snapshot.buttons.set(button, held);
        }
        snapshot
    }

    /// Run the layout for a frame. `suspended` while a menu covers the world: everything is let
    /// go and the virtual pad rests, so a game never sees a button held through a menu.
    pub fn step(&mut self, snapshot: &Snapshot, suspended: bool) -> Delivery {
        let now = Instant::now();
        let dt = self
            .last_step
            .map(|t| now.duration_since(t).as_secs_f64())
            .unwrap_or(0.0);
        self.last_step = Some(now);

        let frame = if suspended {
            if !self.suspended {
                self.engine.reset();
            }
            Frame::default()
        } else {
            self.engine.step(snapshot, dt)
        };
        self.suspended = suspended;

        self.report = report_of(&frame);
        {
            let report = self.report;
            // Say, every time the buttons change, what the pad was told. This is the line that
            // settles "the game does not see my controller": if it is in the log while the
            // game does nothing, everything on this side worked and the question is what the
            // game does with a device it has been given; if it is absent, the layout never
            // produced anything and the game is innocent.
            //
            // The buttons rather than the whole report, because a thumb resting on a stick
            // reads as a hundredth off centre and never comes back — which was enough to make
            // this say "driven" once and then stay quiet through every press afterwards.
            let buttons = (report.buttons, report.dpad_up, report.dpad_down, report.dpad_left, report.dpad_right);
            if buttons != self.pad_buttons {
                self.pad_buttons = buttons;
                if buttons != (0, false, false, false, false) {
                    log::info!(
                        "virtual gamepad: buttons {:#06x}, sticks ({:.2},{:.2}) ({:.2},{:.2}), \
                         triggers {:.2}/{:.2}",
                        report.buttons,
                        report.left.0,
                        report.left.1,
                        report.right.0,
                        report.right.1,
                        report.left_trigger,
                        report.right_trigger
                    );
                }
            }
            match self.pad.as_mut().map(|pad| pad.send(&report)) {
                None => {}
                Some(Ok(())) => self.pad_complained = false,
                // Once, not every frame: this runs at the frame rate, and a pad that has
                // stopped taking reports will fail on every one of them. It was `debug` and so
                // said nothing at all in a session's log -- which is the wrong way round for
                // the one thing standing between a layout and the game it is mapped for.
                Some(Err(e)) if !self.pad_complained => {
                    log::warn!("the virtual gamepad stopped taking reports: {e}");
                    self.pad_complained = true;
                }
                Some(Err(_)) => {}
            }
        }

        let mut delivery = Delivery::default();
        delivery.keys = key_changes(&self.keys, &frame.keys);
        self.keys = frame.keys.clone();
        for button in &self.mouse {
            if !frame.mouse_buttons.contains(button) {
                delivery.mouse_buttons.push((button.code(), false));
            }
        }
        for button in &frame.mouse_buttons {
            if !self.mouse.contains(button) {
                delivery.mouse_buttons.push((button.code(), true));
            }
        }
        self.mouse = frame.mouse_buttons.clone();

        delivery.motion = frame.mouse_motion;
        delivery.wheel = frame.wheel;
        delivery.commands = frame.commands;
        delivery.pointer_clicks = frame.pointer_clicks;
        delivery.radial = frame.radial;
        let now = Instant::now();
        match (self.guide_now, self.guide_down) {
            (true, None) => self.guide_down = Some((now, false)),
            (true, Some((since, false))) if now.duration_since(since) >= GUIDE_HOLD => {
                delivery.guide_held = true;
                self.guide_down = Some((since, true));
            }
            (false, Some((_, held))) => {
                delivery.guide = !held;
                self.guide_down = None;
            }
            _ => {}
        }
        delivery.menu_presses = PAD_MENU_CONTROLS
            .iter()
            .filter(|(b, _)| self.pad_buttons_now.is_down(*b) && !self.pad_buttons_was.is_down(*b))
            .map(|(_, c)| *c)
            .collect();
        delivery
    }

    /// The D-pad direction another pad is holding, as the Deck's control, for the menus'
    /// held repeat. As of the last [`Self::gather`].
    pub fn pad_held_direction(&self) -> Option<Control> {
        PAD_MENU_CONTROLS[..4]
            .iter()
            .find(|(b, _)| self.pad_buttons_now.is_down(*b))
            .map(|(_, c)| *c)
    }

    /// A trackpad's press, routed: called where the pointer's clicks are decided, once the
    /// aim is known, with the press as the controller says it (`ControllerState::pad_press`),
    /// whether the pointer is on the room's own things (a title bar or its buttons, the
    /// keyboard, a notice, a menu, a drag going on), and the pointer's two buttons. Returns the
    /// buttons the pointer is to act on.
    ///
    /// A press on the room's things is the room's, as it always was: the layout never hears
    /// of it. Anywhere else it is the application's: the layout reads it from the next frame,
    /// and if it binds that press, the click the press made is held back from the pointer. An
    /// unbound press -- every press, in a layout that names none -- changes nothing at all,
    /// and is not one frame late: the pointer acts on it in this frame, as before.
    pub fn route_pad_press(
        &mut self,
        press: Option<PadPress>,
        room_pointed: impl FnOnce() -> bool,
        right: bool,
        left: bool,
    ) -> (bool, bool) {
        let engine = &self.engine;
        self.route.frame(press, room_pointed, |b| engine.binds(b), right, left)
    }

    /// Another pad's touchpad, as of the last [`Self::gather`]: what the right trackpad is on
    /// a Deck, for a pad that has one.
    pub fn touchpad(&self) -> Option<spatiand_input::gamepad::Touchpad> {
        self.touchpad
    }

    /// The pad as the layout last made it.
    ///
    /// The same report the local device was given, so an application on another machine is
    /// played exactly as one here is: its own layout, the head on the spare axes, everything.
    pub fn report(&self) -> Report {
        self.report
    }

    /// Motor strengths a game asked for, when they changed.
    pub fn rumble(&mut self) -> Option<(u16, u16)> {
        self.pad.as_mut()?.poll_rumble()
    }

    // --- the editor ---

    /// Does the layout in force give this button a meaning of its own?
    ///
    /// Asked about A, B and X: while a thumb is on a trackpad those are the pointer's mouse
    /// buttons, but only where the application has not claimed them — in a game, A is jump.
    pub fn binds(&self, button: Button) -> bool {
        self.engine.binds(button)
    }

    /// Is the application in front played with the virtual gamepad? See
    /// [`spatiand_mapper::engine::Engine::drives_pad`].
    #[allow(dead_code)] // Same: the shape of a per-layout hover switch, when one is wanted.
    pub fn drives_pad(&self) -> bool {
        self.engine.drives_pad()
    }

    pub fn editor_open(&self) -> bool {
        self.editor.is_some()
    }

    pub fn open_editor(&mut self) {
        let layout = self.engine.layout().clone();
        // Every other application's saved layout, so a new one can start from controls the
        // wearer already built rather than from a template. Read now rather than at startup:
        // a layout saved for another application a minute ago has to be on the list.
        let others = self.store.saved_except(&self.app);
        self.editor = Some(
            Editor::new(self.app.clone(), self.app_name.clone(), layout)
                .with_others(others)
                .with_pad_presses(self.pad_presses_reported),
        );
        self.reset_to_default = false;
    }

    pub fn editor_view(&self) -> Option<editor::View> {
        self.editor.as_ref().map(|e| e.view())
    }

    /// Feed the editor. `false` once it has closed.
    pub fn editor_input(&mut self, input: editor::Input) -> bool {
        let Some(editor) = self.editor.as_mut() else {
            return false;
        };
        match editor.handle(input) {
            editor::Event::None => true,
            editor::Event::Changed => {
                self.reset_to_default = false;
                self.engine.set_layout(editor.layout().clone());
                true
            }
            editor::Event::Reset => {
                if let Err(e) = self.store.forget(&self.app) {
                    log::warn!("could not remove the saved layout: {e}");
                }
                let default = self.app.default_layout();
                editor.replace(default.clone());
                self.engine.set_layout(default);
                self.reset_to_default = true;
                true
            }
            editor::Event::Close => {
                self.close_editor();
                false
            }
        }
    }

    /// Save what the editor changed, and put it away.
    pub fn close_editor(&mut self) {
        let Some(editor) = self.editor.take() else {
            return;
        };
        let untouched_default =
            self.reset_to_default && *editor.layout() == editor.app().default_layout();
        if editor.is_dirty() && !untouched_default {
            match self.store.save(editor.app(), editor.layout()) {
                Ok(()) => log::info!(
                    "saved the controller layout to {}",
                    self.store.path(editor.app()).display()
                ),
                Err(e) => log::warn!("could not save the controller layout: {e}"),
            }
        }
    }
}

/// Which application a window belongs to, for its layout.
///
/// A Steam game is recognised by the `SteamAppId` Steam puts in the environment of everything it
/// starts, read from the window's process — which, for a Proton game, is a Wine process that
/// inherited it. Anything else goes by its app id or X11 class.
pub fn app_key(pid: Option<u32>, app_id: Option<&str>) -> AppKey {
    if let Some(pid) = pid {
        if let Ok(environment) = std::fs::read(format!("/proc/{pid}/environ")) {
            for variable in environment.split(|b| *b == 0) {
                if let Some(value) = variable.strip_prefix(b"SteamAppId=") {
                    if let Some(key) = AppKey::from_steam_app_id(&String::from_utf8_lossy(value)) {
                        return key;
                    }
                }
            }
        }
    }
    AppKey::App(app_id.unwrap_or_default().to_string())
}

/// A launch command, with Steam told to leave controllers alone.
///
/// Steam started without `-nojoy` takes the Deck's controller for its own Steam Input and
/// zeroes every report Spatiand reads from it — the pointer, the menus and the layout all go
/// dead. With it, Steam holds no controller at all and still launches games, which then find
/// only the virtual pad. Measured on the Deck: see `docs/input-mapper.md`.
pub fn without_steam_input(exec: &str) -> String {
    let parts = spatiand_platform::launch::split_command(exec);
    let Some(program) = parts.first() else {
        return exec.to_string();
    };
    let is_steam = std::path::Path::new(program)
        .file_name()
        .is_some_and(|n| n == "steam");
    if !is_steam || parts.iter().any(|p| p == "-nojoy") {
        return exec.to_string();
    }
    match exec.split_once(program.as_str()) {
        Some((before, after)) => format!("{before}{program} -nojoy{after}"),
        None => exec.to_string(),
    }
}

const DECK_BUTTONS: [(Control, Button); 24] = [
    (Control::A, Button::A),
    (Control::B, Button::B),
    (Control::X, Button::X),
    (Control::Y, Button::Y),
    (Control::Up, Button::DpadUp),
    (Control::Down, Button::DpadDown),
    (Control::Left, Button::DpadLeft),
    (Control::Right, Button::DpadRight),
    (Control::L1, Button::L1),
    (Control::R1, Button::R1),
    (Control::L2, Button::L2),
    (Control::R2, Button::R2),
    (Control::L4, Button::L4),
    (Control::R4, Button::R4),
    (Control::L5, Button::L5),
    (Control::R5, Button::R5),
    (Control::Menu, Button::Menu),
    (Control::View, Button::View),
    (Control::LStickClick, Button::LStick),
    (Control::RStickClick, Button::RStick),
    (Control::LPadTouch, Button::LPadTouch),
    (Control::RPadTouch, Button::RPadTouch),
    (Control::LStickTouch, Button::LStickTouch),
    (Control::RStickTouch, Button::RStickTouch),
];

fn deck_snapshot(state: &ControllerState) -> Snapshot {
    let mut s = Snapshot::default();
    for (control, button) in DECK_BUTTONS {
        s.buttons.set(button, state.buttons.is_down(control));
    }
    s.left_stick = state.left_stick;
    s.right_stick = state.right_stick;
    let touch = |p: &spatiand_input::Pad| Touch {
        x: p.x,
        y: p.y,
        touched: p.touched,
    };
    s.left_pad = touch(&state.left_pad);
    s.right_pad = touch(&state.right_pad);
    s.left_trigger = state.left_trigger.clamp(0.0, 1.0);
    s.right_trigger = state.right_trigger.clamp(0.0, 1.0);
    s.gyro = Some(deck_rates(state.gyro));
    s
}

/// The Deck's gyro in the holder's terms.
///
/// The IMU's Z points out of the screen (a Deck lying face up reads +1 g on it), X to the right
/// and Y towards the top edge. SDL's Steam Deck driver maps it to pitch X, yaw Z and roll Y on
/// that basis, and so does this. The signs are that driver's; they have not been checked here
/// by turning a Deck, which is why every gyro mode has invert switches.
fn deck_rates(g: [f32; 3]) -> Rates {
    Rates {
        pitch: g[0],
        yaw: g[2],
        roll: g[1],
    }
}

/// The glasses' gyro in the holder's terms, from the head frame the tracker uses: +X forward,
/// +Y left, +Z up. Turning left is a positive rotation about up; looking up is a negative
/// rotation about left; tipping the right side down is a positive rotation about forward.
fn head_rates(r: DVec3) -> Rates {
    Rates {
        pitch: -r.y as f32,
        yaw: r.z as f32,
        roll: r.x as f32,
    }
}

fn gamepad_snapshot(pad: &GamepadState) -> Snapshot {
    let mut s = Snapshot::default();
    for (down, button) in [
        (pad.a, Button::A),
        (pad.b, Button::B),
        (pad.x, Button::X),
        (pad.y, Button::Y),
        (pad.up, Button::DpadUp),
        (pad.down, Button::DpadDown),
        (pad.left, Button::DpadLeft),
        (pad.right, Button::DpadRight),
        (pad.l1, Button::L1),
        (pad.r1, Button::R1),
        (pad.l2, Button::L2),
        (pad.r2, Button::R2),
        (pad.start, Button::Menu),
        (pad.select, Button::View),
        (pad.left_stick_click, Button::LStick),
        (pad.right_stick_click, Button::RStick),
    ] {
        s.buttons.set(button, down);
    }
    s.left_stick = pad.left_stick;
    s.right_stick = pad.right_stick;
    s.left_trigger = pad.left_trigger;
    s.right_trigger = pad.right_trigger;
    s.gyro = pad.gyro.map(pad_rates);
    s
}

/// A PlayStation pad's gyro in the holder's terms: about X is pitch, Y yaw and Z roll, which is
/// how SDL reads these pads. Not yet checked by turning one here; every gyro mode has invert
/// switches for exactly this.
fn pad_rates(g: [f32; 3]) -> Rates {
    Rates {
        pitch: g[0],
        yaw: g[1],
        roll: g[2],
    }
}

/// The buttons in [`virtual_pad::BUTTON_CODES`] order.
const REPORT_ORDER: [PadButton; 11] = [
    PadButton::A,
    PadButton::B,
    PadButton::X,
    PadButton::Y,
    PadButton::LeftBumper,
    PadButton::RightBumper,
    PadButton::LeftStick,
    PadButton::RightStick,
    PadButton::Start,
    PadButton::Back,
    PadButton::Guide,
];

fn report_of(frame: &Frame) -> Report {
    let p = &frame.pad;
    let mut buttons = 0u16;
    for (i, button) in REPORT_ORDER.iter().enumerate() {
        if p.is_down(*button) {
            buttons |= 1 << i;
        }
    }
    Report {
        buttons,
        dpad_up: p.is_down(PadButton::DpadUp),
        dpad_down: p.is_down(PadButton::DpadDown),
        dpad_left: p.is_down(PadButton::DpadLeft),
        dpad_right: p.is_down(PadButton::DpadRight),
        left: p.left,
        right: p.right,
        left_trigger: p.left_trigger,
        right_trigger: p.right_trigger,
        // The first two of the spare axes (`Report::extra`) carry where the gyro aims a VR game's
        // right hand, when a layout has asked for that: right and up, -1..1 for -90..90 degrees.
        // Centred otherwise, which is what a game that reads them sees. The OpenXR runtime reads
        // them, here or beside a game on a host.
        extra: [frame.hand_aim[0].clamp(-1.0, 1.0), frame.hand_aim[1].clamp(-1.0, 1.0), 0.0, 0.0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_pad_button_lands_on_the_evdev_code_with_its_name() {
        // BTN_A, BTN_B, BTN_X, BTN_Y, BTN_TL, BTN_TR, BTN_THUMBL, BTN_THUMBR, BTN_START,
        // BTN_SELECT, BTN_MODE. A transposition here is a game that jumps when you crouch.
        let expected = [0x130, 0x131, 0x133, 0x134, 0x136, 0x137, 0x13D, 0x13E, 0x13B, 0x13A, 0x13C];
        assert_eq!(virtual_pad::BUTTON_CODES, expected);
        for (i, button) in REPORT_ORDER.iter().enumerate() {
            let mut frame = Frame::default();
            frame.pad.buttons = button.bit();
            assert_eq!(report_of(&frame).buttons, 1 << i, "{button:?}");
        }
    }

    fn press(fingers: u8, deep: bool) -> Option<PadPress> {
        Some(PadPress { fingers, deep })
    }

    #[test]
    fn an_unbound_press_leaves_the_clicks_as_they_were_and_in_the_same_frame() {
        let mut route = PressRoute::default();
        let binds_nothing = |_: Button| false;
        // One finger: the primary button, down with the press and up with it.
        assert_eq!(route.frame(press(1, false), || false, binds_nothing, true, false), (true, false));
        assert_eq!(route.frame(press(1, true), || false, binds_nothing, true, false), (true, false));
        assert_eq!(route.frame(None, || false, binds_nothing, false, false), (false, false));
        // Two fingers: the secondary, whichever way macOS was told to make it.
        assert_eq!(route.frame(press(2, false), || false, binds_nothing, false, true), (false, true));
        assert_eq!(route.frame(None, || false, binds_nothing, false, false), (false, false));
        // A mouse's click, no press at all.
        assert_eq!(route.frame(None, || unreachable!(), binds_nothing, true, false), (true, false));
    }

    #[test]
    fn a_bound_press_holds_its_click_back_and_the_layout_reads_it() {
        let mut route = PressRoute::default();
        let binds_two = |b: Button| b == Button::PadClick2;
        assert_eq!(route.frame(press(2, false), || false, binds_two, false, true), (false, false));
        assert_eq!(route.buttons(press(2, false)), vec![Button::PadClick2]);
        assert_eq!(route.buttons(press(2, true)), vec![Button::PadClick2, Button::PadDeepClick2]);
        assert_eq!(route.frame(press(2, false), || false, binds_two, false, true), (false, false));
        assert_eq!(route.frame(None, || false, binds_two, false, false), (false, false));
        assert!(route.buttons(None).is_empty());
        // And a one-finger press in the same layout is the click it always was.
        assert_eq!(route.frame(press(1, false), || false, binds_two, true, false), (true, false));
    }

    #[test]
    fn a_press_on_the_rooms_own_things_stays_the_rooms_even_when_bound() {
        let mut route = PressRoute::default();
        let binds_all = |_: Button| true;
        assert_eq!(route.frame(press(1, false), || true, binds_all, true, false), (true, false));
        assert!(route.buttons(press(1, true)).is_empty(), "the layout never hears of it");
    }

    #[test]
    fn a_drag_already_held_goes_on_through_a_bound_press() {
        let mut route = PressRoute::default();
        let binds_three = |b: Button| b == Button::PadClick3;
        // The three-finger drag holds the primary before any press.
        assert_eq!(route.frame(None, || false, binds_three, true, false), (true, false));
        assert_eq!(route.frame(press(3, false), || false, binds_three, true, false), (true, false));
        assert_eq!(route.buttons(press(3, false)), vec![Button::PadClick3]);
    }

    #[test]
    fn every_room_command_does_its_thing() {
        use spatiand_shell::{HudAction, Intent, ShellEvent};
        let expected = |command| match command {
            Command::Hud => Effect::Intent(Intent::ToggleHud),
            Command::Launcher => Effect::Intent(Intent::ToggleLauncher),
            Command::Keyboard => Effect::Keyboard,
            Command::Screenshot => Effect::Screenshot,
            Command::Recentre => Effect::Event(ShellEvent::Hud(HudAction::Recentre)),
            Command::CloseWindow => Effect::Event(ShellEvent::CloseWindow(7)),
            Command::HideWindow => Effect::Event(ShellEvent::HideWindow { id: 7, hidden: true }),
            Command::Switcher => Effect::Switcher,
            Command::Dictate => Effect::Dictate,
            Command::Record => Effect::Event(ShellEvent::Hud(HudAction::Record)),
        };
        for command in Command::ALL {
            assert_eq!(command_effect(command, Some(7)), expected(command), "{command:?}");
        }
        // With no window in front there is nothing to close or hide.
        assert_eq!(command_effect(Command::CloseWindow, None), Effect::Nothing);
        assert_eq!(command_effect(Command::HideWindow, None), Effect::Nothing);
    }

    #[test]
    fn a_shortcut_is_pressed_modifiers_first_and_let_go_modifiers_last() {
        // Ctrl+Shift+T as the engine holds it: sorted by code, T first.
        let held = [20u16, 29, 42];
        assert_eq!(key_changes(&[], &held), vec![(29, true), (42, true), (20, true)]);
        assert_eq!(key_changes(&held, &[]), vec![(20, false), (29, false), (42, false)]);
        // Keys that stay down are not said again.
        assert_eq!(key_changes(&[29], &[29, 46]), vec![(46, true)]);
    }

    #[test]
    fn looking_up_and_turning_left_are_positive() {
        // Looking up rotates forward (+X) towards up (+Z): about +Y that is a negative rate.
        let up = head_rates(DVec3::new(0.0, -30.0, 0.0));
        assert!(up.pitch > 0.0);
        let left = head_rates(DVec3::new(0.0, 0.0, 30.0));
        assert!(left.yaw > 0.0);
    }

    #[test]
    fn every_deck_control_reaches_the_layout_except_the_ones_spatiand_keeps() {
        // STEAM and the ... button open Spatiand's menus in every layout, and the trackpad
        // clicks are the pointer's mouse buttons in every layout. Everything else is the
        // application's to bind.
        let mapped: Vec<Control> = DECK_BUTTONS.iter().map(|(c, _)| *c).collect();
        for control in Control::ALL {
            let reserved = matches!(
                control,
                Control::Steam | Control::Quick | Control::LPadClick | Control::RPadClick
            );
            assert_eq!(mapped.contains(&control), !reserved, "{control:?}");
        }
    }

    #[test]
    fn a_window_with_no_steam_id_is_known_by_its_app_id() {
        assert_eq!(
            app_key(None, Some("org.kde.dolphin")),
            AppKey::App("org.kde.dolphin".into())
        );
    }

    /// A real process with a real environment, because that is the part that can be wrong.
    ///
    /// A Proton game's window belongs to a Wine process inside Steam's container, and the only
    /// thing tying it to a game is the `SteamAppId` Steam left in its environment. Reading it
    /// works because the container shares this machine's process numbering -- measured: a
    /// process in the Steam runtime reports one `NSpid` and the initial namespace -- so the
    /// process id on a window is a process id here. A test that made up an environment instead
    /// of reading one would prove none of that.
    #[test]
    fn a_process_that_carries_a_steam_id_is_known_by_the_game() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .env("SteamAppId", "1677740")
            .spawn()
            .expect("something to run");
        // A process that has been started but has not finished starting has an empty
        // environment -- the kernel has nothing to show until the new image is in place. In a
        // session the question is asked when a window appears, which is long afterwards; here
        // it has to be waited for.
        let began = std::time::Instant::now();
        while std::fs::read(format!("/proc/{}/environ", child.id())).is_ok_and(|e| e.is_empty())
            && began.elapsed() < std::time::Duration::from_secs(2)
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let key = app_key(Some(child.id()), Some("steam_app_1677740"));
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(key, AppKey::Steam(1677740));
    }
}
