//! The sound devices' list in the room's settings, as the Mac has it ("Sound devices"): where
//! the room's sound comes out and which microphone listens, chosen in the glasses. The same
//! choice as the two pickers at the top of the phone's screen, which stay in step with it: the
//! app says what Android offers and which is in use (`Native.audioDevices`, whenever either
//! changes), and a row chosen here is chosen there (`Shell.chooseAudio`).

use std::sync::Mutex;

use jni_sys::jvalue;

use super::jni;

/// One of the phone's sound devices as the app lists it. Id 0 is "Automatic": wherever Android
/// routes it.
#[derive(serde::Deserialize, Clone)]
struct Device {
    id: i32,
    name: String,
    #[serde(default)]
    input: bool,
    #[serde(default)]
    current: bool,
}

/// What the app last said there is, and whether the settings have been told since.
static DEVICES: Mutex<(Vec<Device>, bool)> = Mutex::new((Vec::new(), false));

/// The app's list (`Native.audioDevices`): outputs, then microphones.
pub fn set(json: &str) {
    match serde_json::from_str::<Vec<Device>>(json) {
        Ok(list) => *DEVICES.lock().unwrap() = (list, true),
        Err(e) => log::warn!("sound devices: {e}"),
    }
}

/// The settings' list has been opened: the app is asked to say what there is now.
pub fn wanted() {
    jni::with_shell(|env, class| unsafe {
        let Some(method) = jni::shell_method(env, class, c"audioDevicesWanted", c"()V") else { return };
        ((**env).v1_1.CallStaticVoidMethodA)(env, class, method, std::ptr::null());
    });
}

/// The device on this row is to be used.
pub fn chosen(index: usize) {
    let Some(device) = DEVICES.lock().unwrap().0.get(index).cloned() else { return };
    log::info!("sound: {} {} chosen in the room", if device.input { "microphone" } else { "output" }, device.name);
    jni::with_shell(|env, class| unsafe {
        let Some(method) = jni::shell_method(env, class, c"chooseAudio", c"(IZ)V") else { return };
        let args = [jvalue { i: device.id }, jvalue { z: device.input }];
        ((**env).v1_1.CallStaticVoidMethodA)(env, class, method, args.as_ptr());
    });
}

/// The list as the settings show it -- each device's name, whether it is a microphone, whether it
/// is in use -- if it has changed since this was last asked.
pub fn changed() -> Option<Vec<(String, bool, bool)>> {
    let mut devices = DEVICES.lock().unwrap();
    if !std::mem::take(&mut devices.1) {
        return None;
    }
    Some(devices.0.iter().map(|d| (d.name.clone(), d.input, d.current)).collect())
}
