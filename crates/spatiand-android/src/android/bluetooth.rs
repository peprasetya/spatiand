//! The room's Bluetooth page with Android's Bluetooth behind it: the same page as the Deck's
//! (`crate::bluetooth`), asked of the phone through the app (`Radios.java`).
//!
//! What Android lets an ordinary app do, the app does: say whether Bluetooth is on and which
//! devices are paired and connected (once the wearer has let Spatiand use Bluetooth: the
//! question comes up on the phone's screen the first time the page opens). What it keeps for the
//! system -- connecting and disconnecting a device, forgetting one, switching Bluetooth on and
//! off -- Spatiand's shell helper does, as the shell may. Adding a device is Android's own
//! Bluetooth page, opened as a window in the room, as the Deck opens its desktop's wizard.

use std::time::{Duration, Instant};

use jni_sys::jvalue;
use spatiand_shell::{BluetoothAction, DeviceRow};

use super::jni;

/// Android's Bluetooth, through the app.
pub struct Phone;

#[derive(serde::Deserialize)]
struct State {
    powered: Option<bool>,
    #[serde(default)]
    devices: Vec<Device>,
    #[serde(default)]
    note: String,
}

#[derive(serde::Deserialize)]
struct Device {
    address: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    connected: bool,
    #[serde(default = "no_battery")]
    battery: i32,
}

fn no_battery() -> i32 {
    -1
}

impl Phone {
    fn state(&self) -> Option<State> {
        let json = java_text(c"bluetoothState", None)?;
        serde_json::from_str(&json).map_err(|e| log::warn!("bluetooth: {e}")).ok()
    }

    /// Wait up to `limit` for what Android says to come true.
    fn until(&self, limit: Duration, done: impl Fn(&Self) -> bool) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            if done(self) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

impl Phone {
    /// Whether Bluetooth is on and the paired devices, as the page's rows.
    fn rows(&self) -> (Option<bool>, Vec<DeviceRow>) {
        let (powered, devices, _) = self.read();
        (powered, devices)
    }

    fn read(&self) -> (Option<bool>, Vec<DeviceRow>, Option<String>) {
        let Some(state) = self.state() else {
            return (None, Vec::new(), Some("Android's Bluetooth could not be asked.".into()));
        };
        let devices = state
            .devices
            .into_iter()
            .filter(|d| crate::bluetooth::is_address(&d.address))
            .map(|d| DeviceRow {
                label: if d.name.trim().is_empty() { d.address.clone() } else { d.name },
                address: d.address,
                kind: d.kind,
                connected: d.connected,
                battery: u8::try_from(d.battery).ok().filter(|b| *b <= 100),
            })
            .collect();
        (state.powered, devices, (!state.note.is_empty()).then_some(state.note))
    }
}

impl crate::bluetooth::Adapter for Phone {
    fn read_state(&mut self) -> (Option<bool>, Vec<DeviceRow>, Option<String>) {
        self.read()
    }

    fn perform(&mut self, action: &BluetoothAction, label: &str) -> String {
        let (op, address) = match action {
            BluetoothAction::Connect(a) => ("connect", a.as_str()),
            BluetoothAction::Disconnect(a) => ("disconnect", a.as_str()),
            BluetoothAction::Forget(a) => ("forget", a.as_str()),
            BluetoothAction::Power(true) => ("on", ""),
            BluetoothAction::Power(false) => ("off", ""),
            BluetoothAction::Add | BluetoothAction::None => return String::new(),
        };
        match java_do(op, address) {
            // Done, or under way: judged below by what Android says afterwards.
            0 => {}
            HELPER_MISSING => {
                super::apps::start_helper();
                return "Spatiand's helper does this, and it is starting: try again in a moment.".into();
            }
            NOT_ALLOWED => return "Allow Spatiand to use Bluetooth: answer on the phone's screen.".into(),
            code => log::info!("bluetooth: {op} {address}: Android said {code}"),
        }
        // Android's calls return before the device has answered: judged by what Android says
        // afterwards, as the Deck's is by what BlueZ says, waiting as long as a connect may take.
        let limit = match action {
            BluetoothAction::Connect(_) => Duration::from_secs(crate::bluetooth::CONNECT_TIMEOUT_S as u64),
            _ => Duration::from_secs(5),
        };
        let worked = self.until(limit, |p| {
            let (powered, devices) = p.rows();
            crate::bluetooth::worked(action, powered, &devices)
        });
        let (did, failed) = crate::bluetooth::words(action, label);
        if worked {
            did
        } else {
            failed
        }
    }
}

/// Adding a device: Android's own Bluetooth page, as a window in the room.
pub fn add_device() -> crate::bluetooth::Added {
    if super::settings_window("bluetooth") {
        crate::bluetooth::Added::Opened("Android's Bluetooth page opens in front of you: add the device there.".into())
    } else {
        crate::bluetooth::Added::Not("Android's Bluetooth page could not be opened here.".into())
    }
}

/// What `Shell.bluetoothDo` answers when the shell helper is not there.
const HELPER_MISSING: i32 = -2;
/// ... and when the wearer has not let Spatiand use Bluetooth.
const NOT_ALLOWED: i32 = -3;

fn java_do(op: &str, address: &str) -> i32 {
    jni::with_shell(|env, class| unsafe {
        let method = jni::shell_method(env, class, c"bluetoothDo", c"(Ljava/lang/String;Ljava/lang/String;)I")?;
        let args = [jvalue { l: jni::new_string(env, op) }, jvalue { l: jni::new_string(env, address) }];
        Some(((**env).v1_1.CallStaticIntMethodA)(env, class, method, args.as_ptr()))
    })
    .flatten()
    .unwrap_or(-1)
}

/// A static `String` method of `Shell`, with one `String` argument or none.
pub(super) fn java_text(name: &std::ffi::CStr, arg: Option<&str>) -> Option<String> {
    jni::with_shell(|env, class| unsafe {
        let signature = if arg.is_some() { c"(Ljava/lang/String;)Ljava/lang/String;" } else { c"()Ljava/lang/String;" };
        let method = jni::shell_method(env, class, name, signature)?;
        let args: Vec<jvalue> = arg.map(|a| jvalue { l: jni::new_string(env, a) }).into_iter().collect();
        let said = ((**env).v1_1.CallStaticObjectMethodA)(env, class, method, if args.is_empty() { std::ptr::null() } else { args.as_ptr() });
        if said.is_null() {
            return None;
        }
        jni::java_string(env, said.cast())
    })
    .flatten()
}
