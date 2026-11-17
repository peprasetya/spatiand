//! Bluetooth devices, for the shell's Bluetooth page.
//!
//! Through `bluetoothctl`, BlueZ's own command-line client, rather than a D-Bus binding: it is
//! on every system that has BlueZ at all, the handful of questions asked here are one command
//! each, and it keeps a D-Bus stack out of the compositor for the sake of one menu.
//!
//! Everything runs on a thread of its own. `connect` in particular can take the whole of its
//! timeout when a device is asleep, and the compositor thread draws the glasses.
//!
//! The list is only refreshed while the page is open — every two seconds, which is how a
//! keyboard that wakes up shows up as connected without anyone pressing anything.
//!
//! **The page is the same on every platform; only what is behind it differs** ([`Adapter`]):
//! BlueZ here ([`BlueZ`]), IOBluetooth through the app on the Mac
//! (`crates/spatiand-mac/src/mac/bluetooth.rs`), Android's Bluetooth on the Beam Pro
//! (`spatiand-android`'s `bluetooth`), each answering the same questions -- is it on, which
//! devices are paired and connected -- and doing the same things, judged the same way by what
//! the system says afterwards ([`worked`], [`words`]). Adding a device is the system's own
//! pairing page: the desktop's wizard here, System Settings' Bluetooth pane on the Mac,
//! Android's Bluetooth page on the Beam Pro, each a window in the room.

use std::process::Command;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use spatiand_shell::{BluetoothAction, BluetoothView, DeviceRow, Shell};

const TOOL: &str = "bluetoothctl";
const REFRESH: Duration = Duration::from_secs(2);
/// How long a connect may try. A sleeping device is found within a few seconds of waking;
/// longer than this and the wearer is better told to wake it.
pub const CONNECT_TIMEOUT_S: u32 = 15;

/// Is there anything here to ask?
pub fn available() -> bool {
    Command::new(TOOL)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A platform's Bluetooth, as the page asks it. Asked on the page's own thread, never the
/// compositor's: any of these may take seconds.
pub trait Adapter: Send + 'static {
    /// Whether it is on (`None`: no adapter, or it cannot be asked), and the paired devices, in
    /// any order. A word for the wearer when something stands in the way of asking.
    fn read_state(&mut self) -> (Option<bool>, Vec<DeviceRow>, Option<String>);
    /// Connect, disconnect, forget or switch, and say how it went, judged by what the system
    /// says afterwards. `label` is the device's name. Not asked for [`BluetoothAction::Add`].
    fn perform(&mut self, action: &BluetoothAction, label: &str) -> String;
}

/// What adding a device came to.
pub enum Added {
    /// A pairing wizard to launch as a window: the desktop's.
    Wizard(String),
    /// The system's own pairing page is open in the room already (the Mac's System Settings
    /// pane, Android's Bluetooth page); what to tell the wearer, if anything.
    #[allow(dead_code)] // the Mac's and the Beam Pro's
    Opened(String),
    /// None to open: why, for the wearer.
    Not(String),
}

/// How a device is added on this platform.
pub type AddDevice = fn() -> Added;

/// BlueZ, through `bluetoothctl`.
pub struct BlueZ;

impl Adapter for BlueZ {
    fn read_state(&mut self) -> (Option<bool>, Vec<DeviceRow>, Option<String>) {
        let (powered, devices) = read_state();
        (powered, devices, None)
    }

    fn perform(&mut self, action: &BluetoothAction, label: &str) -> String {
        perform(action, label)
    }
}

/// The desktop's pairing wizard, when it has one.
fn desktop_wizard() -> Added {
    match spatiand_platform::settings_command("bluetooth") {
        Some(command) => Added::Wizard(command),
        None => Added::Not("There is no pairing wizard on this system.".into()),
    }
}

enum Ask {
    Refresh,
    Do(BluetoothAction, String),
}

enum Heard {
    State { powered: Option<bool>, devices: Vec<DeviceRow>, note: Option<String> },
    Began(String),
    /// A command finished: the address it was about (if any) and what to tell the wearer.
    Done(Option<String>, String),
}

pub struct Devices {
    asks: Sender<Ask>,
    heard: Receiver<Heard>,
    view: BluetoothView,
    last: Option<Instant>,
    changed: bool,
    /// The wizard's command, waiting for `tick` to hand it out.
    wizard: Option<String>,
    add: AddDevice,
}

impl Devices {
    /// BlueZ's devices, and the desktop's wizard to add one.
    pub fn start() -> Devices {
        Self::start_with(BlueZ, desktop_wizard)
    }

    /// A platform's devices, and its way to add one.
    pub fn start_with(adapter: impl Adapter, add: AddDevice) -> Devices {
        let (asks, asked) = channel();
        let (tell, heard) = channel();
        let spawned = std::thread::Builder::new()
            .name("bluetooth".into())
            .spawn(move || worker(adapter, asked, tell));
        if let Err(e) = spawned {
            log::warn!("bluetooth: could not start its thread: {e}");
        }
        Devices {
            asks,
            heard,
            view: BluetoothView::default(),
            last: None,
            changed: false,
            wizard: None,
            add,
        }
    }

    /// Ask now rather than at the next tick; the page has just opened.
    pub fn refresh(&mut self) {
        self.last = Some(Instant::now());
        let _ = self.asks.send(Ask::Refresh);
    }

    /// Do what the page asked for.
    pub fn act(&mut self, action: BluetoothAction) {
        let name = |address: &str| {
            self.view
                .devices
                .iter()
                .find(|d| d.address == address)
                .map(|d| d.label.clone())
                .unwrap_or_else(|| address.to_string())
        };
        let label = match &action {
            BluetoothAction::Connect(a)
            | BluetoothAction::Disconnect(a)
            | BluetoothAction::Forget(a) => name(a),
            _ => String::new(),
        };
        if action == BluetoothAction::Add {
            // The wizard, which is still the right tool for discovery and passkeys.
            return self.open_wizard();
        }
        if let BluetoothAction::Connect(a)
        | BluetoothAction::Disconnect(a)
        | BluetoothAction::Forget(a) = &action
        {
            // At once, so a second press while this one waits in line does nothing.
            if !self.view.busy.contains(a) {
                self.view.busy.push(a.clone());
                self.changed = true;
            }
        }
        let _ = self.asks.send(Ask::Do(action, label));
    }

    fn open_wizard(&mut self) {
        match (self.add)() {
            Added::Wizard(command) => {
                log::info!("bluetooth: {command}");
                self.wizard = Some(command);
            }
            Added::Opened(note) | Added::Not(note) => {
                log::info!("bluetooth: adding a device: {note}");
                self.view.note = note;
                self.changed = true;
            }
        }
    }

    /// Feed the shell whatever has been learned, and keep the list fresh while it is showing.
    /// Returns a command to launch as a window, when the wizard was asked for.
    pub fn tick(&mut self, shell: &mut Shell) -> Option<String> {
        let open = shell.mode() == spatiand_shell::Mode::Bluetooth;
        if open && self.last.map_or(true, |t| t.elapsed() >= REFRESH) {
            self.refresh();
        }
        if !open {
            self.last = None;
        }
        loop {
            match self.heard.try_recv() {
                Ok(Heard::State { powered, devices, note }) => {
                    self.view.powered = powered;
                    self.view.devices = devices;
                    if let Some(note) = note {
                        self.view.note = note;
                    }
                }
                Ok(Heard::Began(address)) => {
                    if !self.view.busy.contains(&address) {
                        self.view.busy.push(address);
                    }
                }
                Ok(Heard::Done(address, note)) => {
                    if let Some(address) = address {
                        self.view.busy.retain(|b| *b != address);
                    }
                    self.view.note = note;
                    // Say what changed at once rather than two seconds later.
                    let _ = self.asks.send(Ask::Refresh);
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
            self.changed = true;
        }
        if std::mem::take(&mut self.changed) {
            shell.set_bluetooth(self.view.clone());
        }
        self.wizard.take()
    }
}

fn worker(mut adapter: impl Adapter, asked: Receiver<Ask>, tell: Sender<Heard>) {
    while let Ok(first) = asked.recv() {
        // Everything queued behind a slow connect, taken at once: the actions in order, and
        // however many refreshes piled up as one, afterwards.
        let mut batch = vec![first];
        while let Ok(more) = asked.try_recv() {
            batch.push(more);
        }
        let mut refresh = false;
        for ask in batch {
            let (action, label) = match ask {
                Ask::Refresh => {
                    refresh = true;
                    continue;
                }
                Ask::Do(action, label) => (action, label),
            };
            let address = match &action {
                BluetoothAction::Connect(a)
                | BluetoothAction::Disconnect(a)
                | BluetoothAction::Forget(a) => Some(a.clone()),
                _ => None,
            };
            if let Some(a) = &address {
                let _ = tell.send(Heard::Began(a.clone()));
            }
            let note = adapter.perform(&action, &label);
            log::info!("bluetooth: {note}");
            if tell.send(Heard::Done(address, note)).is_err() {
                return;
            }
        }
        if refresh {
            let (powered, mut devices, note) = adapter.read_state();
            sort(&mut devices);
            if tell.send(Heard::State { powered, devices, note }).is_err() {
                return;
            }
        }
    }
}

/// Connected first, then by name, so the answer to "is it connected?" is at the top.
fn sort(devices: &mut [DeviceRow]) {
    devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
            .then_with(|| a.address.cmp(&b.address))
    });
}

/// What to tell the wearer when an action worked, and when it did not: the same words on every
/// platform.
pub fn words(action: &BluetoothAction, label: &str) -> (String, String) {
    match action {
        BluetoothAction::Connect(_) => (
            format!("Connected {label}."),
            format!(
                "{label} did not connect. Wake it with a key or its button and try again; if it \
                 still will not, it may have paired again under another address."
            ),
        ),
        BluetoothAction::Disconnect(_) => (format!("Disconnected {label}."), format!("{label} would not disconnect.")),
        BluetoothAction::Forget(_) => (format!("Forgot {label}."), format!("Could not forget {label}.")),
        BluetoothAction::Power(on) => (
            format!("Bluetooth is {}.", if *on { "on" } else { "off" }),
            "Bluetooth did not switch. Something may be blocking it.".into(),
        ),
        BluetoothAction::Add | BluetoothAction::None => (String::new(), String::new()),
    }
}

/// Whether an action worked, judged by what the system says afterwards rather than by its
/// answer: a connect that runs out of time can say nothing about failing.
#[allow(dead_code)] // the Mac's and the Beam Pro's adapters
pub fn worked(action: &BluetoothAction, powered: Option<bool>, devices: &[DeviceRow]) -> bool {
    let device = |a: &str| devices.iter().find(|d| d.address == *a);
    match action {
        BluetoothAction::Connect(a) => device(a).is_some_and(|d| d.connected),
        BluetoothAction::Disconnect(a) => !device(a).is_some_and(|d| d.connected),
        BluetoothAction::Forget(a) => device(a).is_none(),
        BluetoothAction::Power(on) => powered == Some(*on),
        BluetoothAction::Add | BluetoothAction::None => true,
    }
}

fn perform(action: &BluetoothAction, label: &str) -> String {
    let timeout = CONNECT_TIMEOUT_S.to_string();
    let args: Vec<&str> = match action {
        BluetoothAction::Connect(a) => vec!["--timeout", &timeout, "connect", a],
        BluetoothAction::Disconnect(a) => vec!["disconnect", a],
        BluetoothAction::Forget(a) => vec!["remove", a],
        BluetoothAction::Power(on) => vec!["power", if *on { "on" } else { "off" }],
        BluetoothAction::Add | BluetoothAction::None => return String::new(),
    };
    let (did, failed) = words(action, label);
    if let Err(e) = Command::new(TOOL).args(&args).output() {
        return format!("Could not run {TOOL}: {e}");
    }
    // Judged by what BlueZ says afterwards, not by the command: a connect that runs out of
    // time exits 0 and prints nothing about failing.
    let worked = match action {
        BluetoothAction::Connect(a) => connected(a) == Some(true),
        BluetoothAction::Disconnect(a) => connected(a) != Some(true),
        BluetoothAction::Forget(a) => run(&["info", a]).map_or(true, |i| field(&i, "Paired") != Some("yes")),
        BluetoothAction::Power(on) => {
            run(&["show"]).and_then(|s| field(&s, "Powered").map(|v| v == "yes")) == Some(*on)
        }
        _ => true,
    };
    if worked {
        did
    } else {
        failed
    }
}

fn connected(address: &str) -> Option<bool> {
    run(&["info", address]).and_then(|i| field(&i, "Connected").map(|v| v == "yes"))
}

fn run(args: &[&str]) -> Option<String> {
    let out = Command::new(TOOL).args(args).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn read_state() -> (Option<bool>, Vec<DeviceRow>) {
    let powered = run(&["show"]).and_then(|s| field(&s, "Powered").map(|v| v == "yes"));
    // `devices Paired` is BlueZ 5.65 and later; `paired-devices` is what came before it.
    let listing = run(&["devices", "Paired"])
        .filter(|s| s.lines().any(|l| l.starts_with("Device ")))
        .or_else(|| run(&["paired-devices"]))
        .unwrap_or_default();
    let devices: Vec<DeviceRow> = listing
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("Device ")?;
            let (address, _) = rest.split_once(' ').unwrap_or((rest, ""));
            if !is_address(address) {
                return None;
            }
            let info = run(&["info", address]).unwrap_or_default();
            Some(device_from_info(address, &info))
        })
        .collect();
    (powered, devices)
}

pub fn is_address(s: &str) -> bool {
    s.len() == 17 && s.split(':').count() == 6
}

fn field<'a>(info: &'a str, name: &str) -> Option<&'a str> {
    info.lines().find_map(|l| {
        let (k, v) = l.trim().split_once(':')?;
        (k == name).then(|| v.trim())
    })
}

fn device_from_info(address: &str, info: &str) -> DeviceRow {
    let label = field(info, "Alias")
        .or_else(|| field(info, "Name"))
        .filter(|s| !s.is_empty())
        .unwrap_or(address)
        .to_string();
    // "Battery Percentage: 0x55 (85)"
    let battery = field(info, "Battery Percentage").and_then(|v| {
        let inner = v.split_once('(')?.1.trim_end_matches(')');
        inner.trim().parse().ok()
    });
    DeviceRow {
        label,
        address: address.to_string(),
        kind: field(info, "Icon").unwrap_or("").to_string(),
        connected: field(info, "Connected") == Some("yes"),
        battery,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: &str = "Device 11:6C:9F:CE:14:2C (public)
\tName: BT5.0 Keyboard
\tAlias: BT5.0 Keyboard
\tIcon: input-keyboard
\tPaired: yes
\tConnected: no
\tUUID: Human Interface Device    (00001812-0000-1000-8000-00805f9b34fb)
\tBattery Percentage: 0x55 (85)
";

    #[test]
    fn a_device_is_read_from_bluetoothctl_info() {
        let d = device_from_info("11:6C:9F:CE:14:2C", INFO);
        assert_eq!(d.label, "BT5.0 Keyboard");
        assert_eq!(d.kind, "input-keyboard");
        assert!(!d.connected);
        assert_eq!(d.battery, Some(85));
    }

    #[test]
    fn a_device_with_nothing_to_say_is_named_by_its_address() {
        let d = device_from_info("AA:BB:CC:DD:EE:FF", "");
        assert_eq!(d.label, "AA:BB:CC:DD:EE:FF");
        assert_eq!(d.battery, None);
    }

    /// A platform's Bluetooth that answers at once, as the Beam Pro's does through its app.
    struct Phone;

    fn row(label: &str, address: &str, connected: bool) -> DeviceRow {
        DeviceRow { label: label.into(), address: address.into(), kind: String::new(), connected, battery: None }
    }

    impl Adapter for Phone {
        fn read_state(&mut self) -> (Option<bool>, Vec<DeviceRow>, Option<String>) {
            let devices = vec![row("zed", "00:00:00:00:00:01", false), row("Keys", "00:00:00:00:00:02", true), row("amp", "00:00:00:00:00:03", false)];
            (Some(true), devices, Some("asked".into()))
        }

        fn perform(&mut self, action: &BluetoothAction, label: &str) -> String {
            format!("did {action:?} to {label}")
        }
    }

    /// Tick until the worker has answered, or give up.
    fn settle(devices: &mut Devices, shell: &mut Shell, until: impl Fn(&BluetoothView) -> bool) -> Option<String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut wizard = None;
        while Instant::now() < deadline {
            wizard = wizard.or(devices.tick(shell));
            if until(&devices.view) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        wizard
    }

    #[test]
    fn a_platforms_bluetooth_fills_the_same_page_connected_first() {
        let mut devices = Devices::start_with(Phone, || Added::Opened(String::new()));
        let mut shell = Shell::new(Vec::new(), spatiand_shell::DesktopPanels::ALL, true);
        devices.refresh();
        settle(&mut devices, &mut shell, |v| !v.devices.is_empty());
        let labels: Vec<&str> = devices.view.devices.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(labels, ["Keys", "amp", "zed"]);
        assert_eq!(devices.view.powered, Some(true));
        assert_eq!(devices.view.note, "asked");
        devices.act(BluetoothAction::Connect("00:00:00:00:00:03".into()));
        settle(&mut devices, &mut shell, |v| v.note.starts_with("did"));
        assert_eq!(devices.view.note, "did Connect(\"00:00:00:00:00:03\") to amp");
    }

    #[test]
    fn adding_a_device_is_the_platforms_own_page_or_its_word_why_not() {
        let mut shell = Shell::new(Vec::new(), spatiand_shell::DesktopPanels::ALL, true);
        let mut opened = Devices::start_with(Phone, || Added::Opened(String::new()));
        opened.act(BluetoothAction::Add);
        assert_eq!(opened.tick(&mut shell), None);
        let mut wizard = Devices::start_with(Phone, || Added::Wizard("pair-wizard".into()));
        wizard.act(BluetoothAction::Add);
        assert_eq!(wizard.tick(&mut shell).as_deref(), Some("pair-wizard"));
        let mut none = Devices::start_with(Phone, || Added::Not("No way to pair here.".into()));
        none.act(BluetoothAction::Add);
        none.tick(&mut shell);
        assert_eq!(none.view.note, "No way to pair here.");
    }

    #[test]
    fn an_action_is_judged_by_what_the_system_says_afterwards() {
        let devices = [row("Keys", "00:00:00:00:00:02", true), row("amp", "00:00:00:00:00:03", false)];
        let a = |s: &str| s.to_string();
        assert!(worked(&BluetoothAction::Connect(a("00:00:00:00:00:02")), Some(true), &devices));
        assert!(!worked(&BluetoothAction::Connect(a("00:00:00:00:00:03")), Some(true), &devices));
        assert!(worked(&BluetoothAction::Disconnect(a("00:00:00:00:00:03")), Some(true), &devices));
        assert!(!worked(&BluetoothAction::Forget(a("00:00:00:00:00:03")), Some(true), &devices));
        assert!(worked(&BluetoothAction::Forget(a("00:00:00:00:00:09")), Some(true), &devices));
        assert!(worked(&BluetoothAction::Power(false), Some(false), &devices));
        assert!(!worked(&BluetoothAction::Power(true), None, &devices));
        assert_eq!(words(&BluetoothAction::Connect(a("x")), "Keys").0, "Connected Keys.");
    }

    #[test]
    fn only_real_addresses_are_devices() {
        assert!(is_address("11:6C:9F:CE:14:2C"));
        assert!(!is_address("Media"));
        assert!(!is_address("0000110a-0000-1000-8000-00805f9b34fb"));
    }
}
