//! The JNI surface: what `id.prasetya.spatiand.Native` calls.
//!
//! Deliberately small, and all of it on the Java side's terms -- plain ints, a `Surface`, a
//! `String` back. Every call is short: the work happens on the two threads these start, the
//! glasses' ([`glasses`]) and the drawing ([`render`]).

mod glasses;
mod logging;
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
}

struct App {
    shared: Arc<Shared>,
    glasses: Option<glasses::Glasses>,
    render: Option<render::Renderer>,
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
            }),
            glasses: None,
            render: None,
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
