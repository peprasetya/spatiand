//! The JNI surface: what `id.prasetya.spatiand.Native` calls.
//!
//! Deliberately small, and all of it on the Java side's terms -- plain ints, a `Surface`, a
//! `String` back. Every call is short: the work happens on the two threads these start, the
//! glasses' ([`glasses`]) and the drawing ([`render`]).

mod decode;
mod glasses;
mod logging;
mod remote;
mod render;

use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use jni_sys::{jboolean, jclass, jint, jobject, jstring, JNIEnv, JNI_FALSE, JNI_TRUE};
use spatiand_track::{AxisMap, HeadTracker, TrackerConfig};

/// What both threads and the status line share.
pub struct Shared {
    pub tracker: Mutex<HeadTracker>,
    /// The glasses' horizontal field of view per eye, degrees, once they have said.
    pub h_fov_deg: Mutex<f64>,
    pub imu_hz: AtomicU32,
    pub fps: AtomicU32,
    /// A line for the phone's screen about the glasses themselves.
    pub glasses_state: Mutex<String>,
    /// The host being shown, for the drawing thread: its windows, and where to send the head.
    pub remote: Mutex<Option<remote::Handle>>,
}

struct App {
    shared: Arc<Shared>,
    glasses: Option<glasses::Glasses>,
    render: Option<render::Renderer>,
    remote: Option<remote::Remote>,
    pairing: Option<remote::Pairing>,
}

impl App {
    /// Show this host, in place of any other.
    fn show_host(&mut self, entry: remote::HostEntry) {
        *self.shared.remote.lock().unwrap() = None;
        self.remote = None;
        match remote::Remote::start(entry) {
            Ok(remote) => {
                *self.shared.remote.lock().unwrap() = Some(remote.handle.clone());
                self.remote = Some(remote);
            }
            Err(e) => log::error!("remote: {e}"),
        }
    }
}

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

fn app() -> &'static Mutex<App> {
    static APP: OnceLock<Mutex<App>> = OnceLock::new();
    APP.get_or_init(|| {
        logging::init();
        Mutex::new(App {
            shared: Arc::new(Shared {
                tracker: Mutex::new(HeadTracker::new(AxisMap::XREAL_AIR, TrackerConfig::default())),
                // The Air's, until the glasses report their own.
                h_fov_deg: Mutex::new(45.0),
                imu_hz: AtomicU32::new(0),
                fps: AtomicU32::new(0),
                glasses_state: Mutex::new("no glasses yet".into()),
                remote: Mutex::new(None),
            }),
            glasses: None,
            render: None,
            remote: None,
            pairing: None,
        })
    })
}

/// `Native.start(fd)`: the glasses, through the usbfs descriptor of a `UsbDeviceConnection`.
/// The descriptor is duplicated; Java keeps the connection open and owns its own.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_start(
    _env: *mut JNIEnv,
    _class: jclass,
    fd: jint,
) -> jboolean {
    let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        log::error!("cannot duplicate the glasses' descriptor: {}", std::io::Error::last_os_error());
        return JNI_FALSE;
    }
    let fd = unsafe { OwnedFd::from_raw_fd(duplicate) };
    let mut app = app().lock().unwrap();
    // One handle at a time: see `XrealGlasses::open_any` on what a second one does.
    app.glasses = None;
    let shared = app.shared.clone();
    app.glasses = Some(glasses::Glasses::start(fd, shared));
    JNI_TRUE
}

/// `Native.configDir(path)`: where to keep what is learned between runs -- the app's own files
/// directory. `spatiand_track::config` finds it the way it finds `~/.config` on the Deck.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_configDir(
    env: *mut JNIEnv,
    _class: jclass,
    path: jni_sys::jstring,
) {
    // Logging starts with the app's state; this is the first call, so start it here.
    let _ = app();
    let Some(dir) = (unsafe { java_string(env, path) }) else { return };
    let mut app = app().lock().unwrap();
    if std::env::var_os("XDG_CONFIG_HOME").is_some() {
        // Called again by an activity made again; everything below is already running.
        return;
    }
    // Set before anything reads it, from the one thread that starts everything else.
    std::env::set_var("XDG_CONFIG_HOME", &dir);
    log::info!("remembering in {dir}/spatiand");
    // The host paired most recently, shown from the start: it is what the glasses are for.
    match remote::load_hosts().into_iter().next() {
        Some(entry) => app.show_host(entry),
        None => log::info!("no host paired yet"),
    }
}

/// `Native.stop()`: let go of the glasses, which puts them back in 2D on the way.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_stop(_env: *mut JNIEnv, _class: jclass) {
    app().lock().unwrap().glasses = None;
}

/// `Native.surface(surface)`: draw into this, the glasses' `Presentation`, until told otherwise.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_surface(
    env: *mut JNIEnv,
    _class: jclass,
    surface: jobject,
) {
    let window = unsafe { ndk_sys::ANativeWindow_fromSurface(env.cast(), surface.cast()) };
    if window.is_null() {
        log::error!("the glasses' surface has no native window");
        return;
    }
    let mut app = app().lock().unwrap();
    // The old one first: two threads drawing into windows at once would share one EGL display
    // for no reason.
    app.render = None;
    let shared = app.shared.clone();
    app.render = Some(render::Renderer::start(window, shared));
}

/// `Native.surfaceGone()`: the surface is being destroyed. Returns only once nothing is drawing
/// into it, which is what `SurfaceHolder.Callback.surfaceDestroyed` requires.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_surfaceGone(
    _env: *mut JNIEnv,
    _class: jclass,
) {
    app().lock().unwrap().render = None;
}

/// `Native.recenter()`: wherever the head points now is straight ahead.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_recenter(_env: *mut JNIEnv, _class: jclass) {
    let shared = app().lock().unwrap().shared.clone();
    shared.tracker.lock().unwrap().recenter();
    log::info!("recentred");
}

/// `Native.status()`: one line for the phone's screen.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_status(
    env: *mut JNIEnv,
    _class: jclass,
) -> jstring {
    let shared = app().lock().unwrap().shared.clone();
    let line = {
        let e = shared.tracker.lock().unwrap().euler_degrees();
        format!(
            "{}\nIMU {} Hz, drawing {} fps\nyaw {:.0}°  pitch {:.0}°  roll {:.0}°",
            shared.glasses_state.lock().unwrap(),
            shared.imu_hz.load(Ordering::Relaxed),
            shared.fps.load(Ordering::Relaxed),
            e.yaw,
            e.pitch,
            e.roll,
        )
    };
    let c = std::ffi::CString::new(line).unwrap_or_default();
    unsafe { ((**env).v1_1.NewStringUTF)(env, c.as_ptr()) }
}

/// `Native.remoteStatus()`: the host, for the phone's screen -- its link, a pairing under way,
/// and the windows it has open.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_remoteStatus(
    env: *mut JNIEnv,
    _class: jclass,
) -> jstring {
    let mut app = app().lock().unwrap();
    // A pairing both ends agreed to is written down and shown here, from this call, because
    // it is the one the phone's screen makes twice a second anyway.
    if let Some(entry) = app.pairing.as_ref().and_then(|p| p.settle()) {
        app.show_host(entry);
    }
    let mut lines = Vec::new();
    if let Some(pairing) = &app.pairing {
        lines.push(pairing.describe());
    }
    match &app.remote {
        Some(remote) => {
            let view = remote.handle.view.lock().unwrap();
            lines.push(view.link.clone());
            for window in view.windows.values() {
                lines.push(format!("  {}{}", window.title, match window.layer {
                    spatiand_stream::Layer::Projection => " (the room)",
                    spatiand_stream::Layer::Window => "",
                }));
            }
        }
        None if app.pairing.is_none() => lines.push("no host paired: pair one below".into()),
        None => {}
    }
    unsafe { new_string(env, &lines.join("\n")) }
}

/// `Native.apps()`: what the host offers, one `id\tname` per line.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_apps(env: *mut JNIEnv, _class: jclass) -> jstring {
    let app = app().lock().unwrap();
    let text = app
        .remote
        .as_ref()
        .map(|r| {
            r.handle
                .view
                .lock()
                .unwrap()
                .apps
                .iter()
                .map(|(id, name)| format!("{id}\t{name}"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    unsafe { new_string(env, &text) }
}

/// `Native.launch(id)`: ask the host to start one of its applications.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_launch(env: *mut JNIEnv, _class: jclass, id: jstring) {
    let Some(id) = (unsafe { java_string(env, id) }) else { return };
    if let Some(remote) = &app().lock().unwrap().remote {
        remote.handle.launch(&id);
    }
}

/// `Native.closeAll()`: close every window the host has open here.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_closeAll(_env: *mut JNIEnv, _class: jclass) {
    if let Some(remote) = &app().lock().unwrap().remote {
        let ids: Vec<u32> = remote.handle.view.lock().unwrap().windows.keys().copied().collect();
        for id in ids {
            remote.handle.close(id);
        }
    }
}

/// `Native.pair(address)`: start pairing with a host whose owner has run
/// `spatiand-host --pair` on it.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_pair(env: *mut JNIEnv, _class: jclass, address: jstring) {
    let Some(address) = (unsafe { java_string(env, address) }) else { return };
    let address = address.trim().to_string();
    if address.is_empty() {
        return;
    }
    // With the default port, as the host listens on.
    let address = if address.contains(':') { address } else { format!("{address}:47600") };
    log::info!("pairing with {address}");
    app().lock().unwrap().pairing = Some(remote::Pairing::start(address));
}

/// `Native.confirmPair()`: the person holding the phone says the two codes match.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_Native_confirmPair(_env: *mut JNIEnv, _class: jclass) {
    if let Some(pairing) = &app().lock().unwrap().pairing {
        pairing.state.lock().unwrap().confirmed = true;
    }
}
