//! The Mac's Bluetooth, for the room's Bluetooth page: the same page as the Deck's
//! (`crate::bluetooth::Devices`, whose worker thread calls in here), with IOBluetooth in place of
//! BlueZ. The app asks IOBluetooth (`RoomBluetooth.swift`) and answers through `sp_bluetooth_*`;
//! the worker waits for each answer, as on the Deck it waits for `bluetoothctl`.
//!
//! What it can say and do: the paired devices, whether each is connected and what kind of thing
//! it is; connecting, disconnecting and forgetting one; turning Bluetooth on and off. **Adding a
//! device** is System Settings' Bluetooth pane brought into the room, as on the Deck it is the
//! desktop's pairing wizard: it finds what is near, Bluetooth LE keyboards and mice among them
//! (which IOBluetooth's own search does not see), and asks for a keyboard's passkey.
//! **Battery levels are not shown**: IOBluetooth has no public way to read them.

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use spatiand_shell::{BluetoothAction, DeviceRow};

use super::ffi;

/// How long the app has to say what there is. It answers from its main thread, which is busy
/// for a moment at worst.
const LIST_WAIT: Duration = Duration::from_secs(4);
/// How long a connect may try: as long as the Deck's, and a little for the answer.
const ACT_WAIT: Duration = Duration::from_secs(20);

/// What the app last said, and how many answers there have been: the worker waits for the count
/// to move past the one it asked after.
#[derive(Default)]
struct Answers {
    /// `None` until asked, or with no controller at all.
    powered: Option<bool>,
    devices: Vec<DeviceRow>,
    lists: u64,
    /// Being said now, between `sp_bluetooth_begin` and `sp_bluetooth_end`.
    saying: Vec<DeviceRow>,
    saying_powered: Option<bool>,
    /// The answer to the action waited for: an error, or none for done.
    done: Option<Option<String>>,
    /// The number of the last action asked for, and of the last one answered: an answer that
    /// comes after its action was given up on is not taken for the next one's.
    acts: u32,
    answered: u32,
}

static ANSWERS: Mutex<Answers> = Mutex::new(Answers {
    powered: None,
    devices: Vec::new(),
    lists: 0,
    saying: Vec::new(),
    saying_powered: None,
    done: None,
    acts: 0,
    answered: 0,
});
static ANSWERED: Condvar = Condvar::new();

/// A Mac has Bluetooth: whether its controller is there and on is what the page then says.
pub fn available() -> bool {
    true
}

/// Ask the app what there is, and wait for its answer.
pub fn read_state() -> (Option<bool>, Vec<DeviceRow>) {
    let before = ANSWERS.lock().unwrap().lists;
    ffi::tell(ffi::Asked::BluetoothList, "");
    let deadline = Instant::now() + LIST_WAIT;
    let mut answers = ANSWERS.lock().unwrap();
    while answers.lists == before {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            log::warn!("bluetooth: the app did not say what there is");
            break;
        }
        answers = ANSWERED.wait_timeout(answers, left).unwrap().0;
    }
    let mut devices = answers.devices.clone();
    // Connected first, then by name, as the Deck's list is.
    devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
            .then_with(|| a.address.cmp(&b.address))
    });
    (answers.powered, devices)
}

/// Ask the app to do something, and wait until it has: `Err` with why not.
pub fn act(action: &BluetoothAction) -> Result<(), String> {
    let text = match action {
        BluetoothAction::Connect(a) => format!("connect {a}"),
        BluetoothAction::Disconnect(a) => format!("disconnect {a}"),
        BluetoothAction::Forget(a) => format!("forget {a}"),
        BluetoothAction::Power(on) => format!("power {}", if *on { "on" } else { "off" }),
        BluetoothAction::Add | BluetoothAction::None => return Ok(()),
    };
    let number = {
        let mut answers = ANSWERS.lock().unwrap();
        answers.acts = answers.acts.wrapping_add(1).max(1);
        answers.done = None;
        answers.acts
    };
    ffi::tell_numbered(ffi::Asked::BluetoothDo, number, &text);
    let deadline = Instant::now() + ACT_WAIT;
    let mut answers = ANSWERS.lock().unwrap();
    while answers.answered != number {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("The Mac did not answer.".into());
        }
        answers = ANSWERED.wait_timeout(answers, left).unwrap().0;
    }
    match answers.done.take().flatten() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Adding a device: System Settings' Bluetooth pane, brought into the room.
pub fn add() -> String {
    super::system_settings("bluetooth");
    "System Settings' Bluetooth is open in front of you: choose the device to add there.".into()
}

/// The Mac's Bluetooth behind the room's page (`crate::bluetooth::Adapter`), as BlueZ is the
/// Deck's and Android's the Beam Pro's.
pub struct Mac;

impl crate::bluetooth::Adapter for Mac {
    fn read_state(&mut self) -> (Option<bool>, Vec<DeviceRow>, Option<String>) {
        let (powered, devices) = read_state();
        (powered, devices, None)
    }

    fn perform(&mut self, action: &BluetoothAction, label: &str) -> String {
        if matches!(action, BluetoothAction::Add | BluetoothAction::None) {
            return String::new();
        }
        if let Err(e) = act(action) {
            log::info!("bluetooth: {e}");
        }
        // Judged by what the Mac says afterwards, as the Deck's is by what BlueZ says.
        let (powered, devices) = read_state();
        let (did, failed) = crate::bluetooth::words(action, label);
        if crate::bluetooth::worked(action, powered, &devices) {
            did
        } else {
            failed
        }
    }
}

/// Adding a device: System Settings' Bluetooth pane, in the room ([`add`]).
pub fn add_device() -> crate::bluetooth::Added {
    crate::bluetooth::Added::Opened(add())
}

// --- the app's answers (`ffi`) ---

pub fn list_begin(powered: Option<bool>) {
    let mut answers = ANSWERS.lock().unwrap();
    answers.saying.clear();
    answers.saying_powered = powered;
}

pub fn list_device(row: DeviceRow) {
    ANSWERS.lock().unwrap().saying.push(row);
}

pub fn list_end() {
    let mut answers = ANSWERS.lock().unwrap();
    answers.devices = std::mem::take(&mut answers.saying);
    answers.powered = answers.saying_powered;
    answers.lists += 1;
    ANSWERED.notify_all();
}

pub fn done(action: u32, error: Option<String>) {
    let mut answers = ANSWERS.lock().unwrap();
    if action != answers.acts {
        log::info!("bluetooth: a late answer to an action given up on: {error:?}");
        return;
    }
    answers.done = Some(error);
    answers.answered = action;
    ANSWERED.notify_all();
}

/// IOBluetooth's address, `aa-bb-cc-dd-ee-ff`, as the page keys devices: `AA:BB:CC:DD:EE:FF`.
pub fn address(said: &str) -> String {
    said.trim().replace('-', ":").to_uppercase()
}

/// What kind of thing a device is, from its Bluetooth class of device, as BlueZ's icon names
/// say it (`DeviceRow::kind`): the major class, then the minor where it decides.
pub fn kind(major: u32, minor: u32) -> &'static str {
    match major {
        0x01 => "computer",
        0x02 => "phone",
        0x04 => match minor {
            0x01 | 0x02 | 0x06 => "audio-headset",
            0x05 | 0x07 | 0x0A => "audio-card",
            _ => "audio-headphones",
        },
        0x05 => match (minor & 0x30, minor & 0x0F) {
            (0x10, _) => "input-keyboard",
            (0x20, _) => "input-mouse",
            (0x30, _) => "input-keyboard",
            (_, 0x01 | 0x02) => "input-gaming",
            (_, 0x05) => "input-tablet",
            _ => "",
        },
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_written_as_the_page_keys_it() {
        assert_eq!(address("11-6c-9f-ce-14-2c"), "11:6C:9F:CE:14:2C");
        assert_eq!(address("11:6C:9F:CE:14:2C"), "11:6C:9F:CE:14:2C");
    }

    #[test]
    fn a_class_of_device_says_what_it_is() {
        // Peripheral, keyboard; pointing; combo; gamepad.
        assert_eq!(kind(0x05, 0x10), "input-keyboard");
        assert_eq!(kind(0x05, 0x20), "input-mouse");
        assert_eq!(kind(0x05, 0x30), "input-keyboard");
        assert_eq!(kind(0x05, 0x02), "input-gaming");
        // Audio: headphones, loudspeaker, wearable headset.
        assert_eq!(kind(0x04, 0x06), "audio-headset");
        assert_eq!(kind(0x04, 0x05), "audio-card");
        assert_eq!(kind(0x02, 0x03), "phone");
        assert_eq!(kind(0x1F, 0x00), "");
    }

    #[test]
    fn a_list_waits_for_its_answer() {
        let answering = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(50));
            list_begin(Some(true));
            list_device(DeviceRow {
                label: "Magic Keyboard".into(),
                address: address("aa-bb-cc-dd-ee-ff"),
                kind: kind(0x05, 0x10).into(),
                connected: false,
                battery: None,
            });
            list_device(DeviceRow {
                label: "AirPods".into(),
                address: address("11-22-33-44-55-66"),
                kind: kind(0x04, 0x06).into(),
                connected: true,
                battery: None,
            });
            list_end();
        });
        let (powered, devices) = read_state();
        answering.join().unwrap();
        assert_eq!(powered, Some(true));
        assert_eq!(devices.iter().map(|d| d.label.as_str()).collect::<Vec<_>>(), ["AirPods", "Magic Keyboard"]);
    }
}
