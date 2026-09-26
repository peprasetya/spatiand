//! The JNI surface: what `id.prasetya.spatiand.Native` calls.
//!
//! Every call is short and on the app's terms -- ints, floats, a `Surface`, a `String`. The
//! work happens on the compositor's thread, which [`Java_id_prasetya_spatiand_Native_begin`]
//! starts once and which runs for the life of the process, as the session does on the Deck: the
//! glasses and the phone's screen come and go under it without closing a window.

use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::atomic::Ordering;

use jni_sys::{jboolean, jclass, jfloat, jint, jobject, jstring, JNIEnv, JNI_FALSE, JNI_TRUE};
use spatiand_input::phone::Phase;
use spatiand_input::Control;

use super::phone::{self, Typed};
use super::{logging, shared, NativeWindow};

/// A Java string as a Rust one.
unsafe fn java_string(env: *mut JNIEnv, text: jstring) -> Option<String> {
    let chars = ((**env).v1_1.GetStringUTFChars)(env, text, std::ptr::null_mut());
    if chars.is_null() {
        return None;
    }
    let owned = std::ffi::CStr::from_ptr(chars).to_string_lossy().into_owned();
    ((**env).v1_1.ReleaseStringUTFChars)(env, text, chars);
    Some(owned)
}

/// A Rust string as a Java one.
unsafe fn new_string(env: *mut JNIEnv, text: &str) -> jstring {
    let c = std::ffi::CString::new(text.replace('\0', "")).unwrap_or_default();
    ((**env).v1_1.NewStringUTF)(env, c.as_ptr())
}

/// `Native.configDir(files, cache)`: where the session keeps things, as the Deck's XDG
/// directories. The first call; logging starts here.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_configDir(
    env: *mut JNIEnv,
    _class: jclass,
    files: jstring,
    cache: jstring,
) {
    logging::init();
    let (Some(files), Some(cache)) = (unsafe { java_string(env, files) }, unsafe { java_string(env, cache) }) else {
        return;
    };
    if std::env::var_os("XDG_CONFIG_HOME").is_some() {
        return;
    }
    let run = format!("{files}/run");
    let _ = std::fs::create_dir_all(&run);
    // Set before the compositor's thread exists, from the one thread that starts it.
    for (name, value) in [
        ("HOME", files.as_str()),
        ("XDG_CONFIG_HOME", files.as_str()),
        ("XDG_DATA_HOME", files.as_str()),
        ("XDG_CACHE_HOME", cache.as_str()),
        ("XDG_RUNTIME_DIR", run.as_str()),
        ("TMPDIR", cache.as_str()),
    ] {
        std::env::set_var(name, value);
    }
    log::info!("remembering in {files}/spatiand");
}

/// `Native.xkbDir(path)`: where the keyboard layouts were unpacked. See `android/xkbcommon`.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_xkbDir(env: *mut JNIEnv, _class: jclass, path: jstring) {
    if let Some(path) = unsafe { java_string(env, path) } {
        std::env::set_var("XKB_CONFIG_ROOT", path);
    }
}

/// `Native.begin()`: start the session, once. Returns at once; the session runs on its thread.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_begin(_env: *mut JNIEnv, _class: jclass) {
    let shared = shared().clone();
    if shared.running.swap(true, Ordering::SeqCst) {
        return;
    }
    let started = std::thread::Builder::new().name("spatiand".into()).spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            use smithay::reexports::calloop::EventLoop;
            use smithay::reexports::wayland_server::Display;
            let mut event_loop: EventLoop<crate::Runtime> = match EventLoop::try_new() {
                Ok(l) => l,
                Err(e) => return log::error!("no event loop: {e}"),
            };
            let mut display: Display<crate::Spatiand> = match Display::new() {
                Ok(d) => d,
                Err(e) => return log::error!("no wayland display: {e}"),
            };
            let display_handle = display.handle();
            let state = crate::Spatiand::new(&mut display, &event_loop.handle());
            let mut runtime = crate::Runtime { state, display_handle };
            log::info!("session started");
            if let Err(e) = super::backend::run(&mut event_loop, &mut display, &mut runtime, &shared) {
                log::error!("the session stopped: {e}");
            }
        }));
        if result.is_err() {
            log::error!("the session died; see the panic above");
        }
        shared.running.store(false, Ordering::SeqCst);
    });
    if let Err(e) = started {
        log::error!("could not start the session: {e}");
        super::shared().running.store(false, Ordering::SeqCst);
    }
}

/// `Native.start(fd)`: the glasses, through the usbfs descriptor of a `UsbDeviceConnection`.
/// The descriptor is duplicated; Java keeps the connection open and owns its own.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_start(_env: *mut JNIEnv, _class: jclass, fd: jint) -> jboolean {
    let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        log::error!("cannot duplicate the glasses' descriptor: {}", std::io::Error::last_os_error());
        return JNI_FALSE;
    }
    *shared().usb.lock().unwrap() = Some(unsafe { OwnedFd::from_raw_fd(duplicate) });
    JNI_TRUE
}

/// `Native.stop()`: let go of the glasses, which puts them back in 2D on the way.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_stop(_env: *mut JNIEnv, _class: jclass) {
    shared().usb_gone.store(true, Ordering::SeqCst);
}

fn window_of(env: *mut JNIEnv, surface: jobject) -> Option<NativeWindow> {
    let window = unsafe { ndk_sys::ANativeWindow_fromSurface(env.cast(), surface.cast()) };
    (!window.is_null()).then_some(NativeWindow(window))
}

/// `Native.surface(surface)`: draw the room into this, the glasses' window.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_surface(env: *mut JNIEnv, _class: jclass, surface: jobject) {
    shared().glasses.set(window_of(env, surface));
}

/// `Native.surfaceGone()`: the glasses' window is going. Returns once nothing draws into it,
/// which is what `SurfaceHolder.Callback.surfaceDestroyed` requires.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_surfaceGone(_env: *mut JNIEnv, _class: jclass) {
    let generation = shared().glasses.set(None);
    if shared().running.load(Ordering::SeqCst) {
        shared().glasses.wait_released(generation);
    }
}

/// `Native.phoneSurface(surface)`: the phone's touch area, where the monitors are drawn.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_phoneSurface(env: *mut JNIEnv, _class: jclass, surface: jobject) {
    shared().phone.set(window_of(env, surface));
}

#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_phoneSurfaceGone(_env: *mut JNIEnv, _class: jclass) {
    let generation = shared().phone.set(None);
    if shared().running.load(Ordering::SeqCst) {
        shared().phone.wait_released(generation);
    }
}

/// `Native.touch(action, id, x, y)`: one touch on the touch area, in its own 0..1 coordinates.
/// `action` is 0 down, 1 move, 2 up, 3 cancel.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_touch(
    _env: *mut JNIEnv,
    _class: jclass,
    action: jint,
    id: jint,
    x: jfloat,
    y: jfloat,
) {
    let phase = match action {
        0 => Phase::Down,
        1 => Phase::Move,
        2 => Phase::Up,
        _ => Phase::Cancel,
    };
    phone::touch(phase, id, x, y);
}

/// `Native.rotation(x, y, z, w)`: the phone's orientation, from its game rotation vector.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_rotation(
    _env: *mut JNIEnv,
    _class: jclass,
    x: jfloat,
    y: jfloat,
    z: jfloat,
    w: jfloat,
) {
    let q = glam::DQuat::from_xyzw(x as f64, y as f64, z as f64, w as f64).normalize();
    phone::input().lock().unwrap().rotation = Some(q);
}

/// `Native.button(which, down)`: 0 is the orange key (STEAM), 1 the home button (`⋯`), 2 back
/// (B).
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_button(
    _env: *mut JNIEnv,
    _class: jclass,
    which: jint,
    down: jboolean,
) {
    let control = match which {
        0 => Control::Steam,
        1 => Control::Quick,
        2 => Control::B,
        _ => return,
    };
    phone::input().lock().unwrap().buttons.push((control, down == JNI_TRUE));
}

/// `Native.key(code, down)`: a key, by Android's `KeyEvent` code. Returns whether it was one
/// the session takes.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_key(
    _env: *mut JNIEnv,
    _class: jclass,
    code: jint,
    down: jboolean,
) -> jboolean {
    match super::keys::from_android(code) {
        Some(code) => {
            phone::input().lock().unwrap().keys.push(Typed::Key { code, pressed: down == JNI_TRUE });
            JNI_TRUE
        }
        None => JNI_FALSE,
    }
}

/// `Native.text(text)`: what Android's own keyboard committed.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_text(env: *mut JNIEnv, _class: jclass, text: jstring) {
    let Some(text) = (unsafe { java_string(env, text) }) else { return };
    let mut input = phone::input().lock().unwrap();
    input.keys.extend(text.chars().map(Typed::Char));
}

/// `Native.recenter()`: the HUD's Recentre, from the phone: the room and the laser both.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_recenter(_env: *mut JNIEnv, _class: jclass) {
    // Recentring is the HUD's to do, with everything it moves; the laser is re-aimed with it.
    phone::input().lock().unwrap().reaim = true;
    super::recentre_requested().store(true, Ordering::SeqCst);
}

/// `Native.status()`: a line for the phone's screen.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_status(env: *mut JNIEnv, _class: jclass) -> jstring {
    let line = shared().status.lock().unwrap().clone();
    unsafe { new_string(env, &line) }
}

/// Hosts paired before the session read its preferences, in the `hosts.toml` the first Android
/// build kept, become the preferences' remote hosts: pairing again is not something to ask of
/// anyone for an upgrade.
pub fn adopt_paired_hosts(prefs: &mut crate::prefs::Prefs) {
    #[derive(serde::Deserialize)]
    struct Entry {
        address: String,
        fingerprint: String,
        #[serde(default)]
        launch: Vec<String>,
    }
    #[derive(serde::Deserialize)]
    struct File {
        #[serde(default)]
        host: Vec<Entry>,
    }
    let path = spatiand_track::config::config_dir().join("hosts.toml");
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let Ok(file) = toml::from_str::<File>(&text) else { return };
    let mut changed = false;
    for entry in file.host {
        if prefs.remotes.iter().any(|r| r.host == entry.address) {
            continue;
        }
        log::info!("adopting {} from hosts.toml", entry.address);
        prefs.remotes.push(crate::prefs::RemoteHost {
            host: entry.address,
            fingerprint: entry.fingerprint,
            launch: entry.launch,
        });
        changed = true;
    }
    if changed {
        prefs.save();
        let _ = std::fs::rename(&path, path.with_extension("toml.adopted"));
    }
}
