//! Smithay's EGL on the Mac: ANGLE's display on Metal, and a surface on a layer.
//!
//! macOS has no EGL and no OpenGL ES. ANGLE is both, on Metal, and the app carries it (see
//! `mac/fetch-angle.sh`). Smithay knows GBM, Wayland and X11 displays and ANGLE's is none of
//! those, so -- as on Android -- what it needs is two small adapters: the display, opened the
//! way ANGLE opens it, and a surface made from a layer the app handed over. After that its
//! `GlesRenderer` is the Deck's, unchanged.

use std::ffi::c_void;
use std::sync::Arc;

use smithay::backend::egl::display::EGLDisplayHandle;
use smithay::backend::egl::ffi;
use smithay::backend::egl::native::EGLNativeSurface;
use smithay::backend::egl::{EGLContext, EGLDisplay, EGLError};
use smithay::backend::renderer::gles::GlesRenderer;

const PLATFORM_ANGLE: u32 = 0x3202;
const PLATFORM_ANGLE_TYPE: isize = 0x3203;
const PLATFORM_ANGLE_TYPE_METAL: isize = 0x3489;

/// Say where ANGLE is before anything asks for EGL: beside the program, or in its bundle's
/// `Frameworks`, or wherever `SPATIAND_EGL` already says.
fn find_angle() {
    if std::env::var_os("SPATIAND_EGL").is_some() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(dir) = exe.parent() else { return };
    let candidates = [
        dir.join("libEGL.dylib"),
        dir.join("../Frameworks/libEGL.dylib"),
        // A development build: `target/<profile>/` under the crate, and ANGLE in `mac/vendor`.
        dir.join("../../../../mac/vendor/angle/libEGL.dylib"),
        std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../mac/vendor/angle/libEGL.dylib")),
    ];
    if let Some(found) = candidates.iter().find(|p| p.exists()) {
        std::env::set_var("SPATIAND_EGL", found);
    }
}

/// ANGLE's display on Metal, handed to Smithay.
pub fn open_display() -> Result<EGLDisplay, String> {
    use smithay::backend::egl::ffi::egl;
    find_angle();
    ffi::make_sure_egl_is_loaded().map_err(|e| format!("no EGL: {e}"))?;
    unsafe {
        let attributes: [isize; 3] = [PLATFORM_ANGLE_TYPE, PLATFORM_ANGLE_TYPE_METAL, egl::NONE as isize];
        let display = egl::GetPlatformDisplay(PLATFORM_ANGLE, egl::DEFAULT_DISPLAY as *mut c_void, attributes.as_ptr());
        if display == egl::NO_DISPLAY {
            return Err("ANGLE gave no display".into());
        }
        let (mut major, mut minor) = (0, 0);
        if egl::Initialize(display, &mut major, &mut minor) == egl::FALSE {
            return Err(format!("eglInitialize failed: {:#x}", egl::GetError()));
        }
        log::info!("EGL {major}.{minor} (ANGLE on Metal)");
        let attributes = [
            egl::RED_SIZE as i32, 8,
            egl::GREEN_SIZE as i32, 8,
            egl::BLUE_SIZE as i32, 8,
            egl::ALPHA_SIZE as i32, 8,
            egl::RENDERABLE_TYPE as i32, egl::OPENGL_ES3_BIT as i32,
            egl::SURFACE_TYPE as i32, (egl::WINDOW_BIT | egl::PBUFFER_BIT) as i32,
            egl::NONE as i32,
        ];
        let mut config = std::ptr::null();
        let mut found = 0;
        if egl::ChooseConfig(display, attributes.as_ptr(), &mut config, 1, &mut found) == egl::FALSE || found == 0 {
            return Err("no EGL config for GLES 3 windows".into());
        }
        EGLDisplay::from_raw(display, config).map_err(|e| format!("{e}"))
    }
}

/// The renderer the session draws with.
pub fn renderer(display: &EGLDisplay) -> Result<GlesRenderer, Box<dyn std::error::Error>> {
    let context = EGLContext::new_with_config(
        display,
        smithay::backend::egl::context::GlAttributes {
            version: (3, 0),
            profile: None,
            debug: false,
            vsync: true,
        },
        smithay::backend::egl::context::PixelFormatRequirements::_8_bit(),
    )?;
    Ok(unsafe { GlesRenderer::new(context)? })
}

/// A renderer with nothing to draw into but its own textures: the snapshot's.
pub fn offscreen() -> Result<GlesRenderer, Box<dyn std::error::Error>> {
    let display = open_display()?;
    let renderer = renderer(&display)?;
    log::info!("offscreen renderer ready on ANGLE");
    Ok(renderer)
}

/// What the app handed over, as something EGL can draw into: a `CALayer`, in which ANGLE puts
/// a Metal layer of its own -- or, with no layer, a picture of that size that goes nowhere.
pub struct Layer {
    layer: *mut c_void,
    size: (i32, i32),
}

unsafe impl Send for Layer {}

/// The surface to draw into for a window the app handed over.
pub fn surface_of(window: &super::NativeWindow) -> Layer {
    Layer { layer: window.layer, size: window.size }
}

unsafe impl EGLNativeSurface for Layer {
    unsafe fn create(
        &self,
        display: &Arc<EGLDisplayHandle>,
        config_id: ffi::egl::types::EGLConfig,
    ) -> Result<*const c_void, EGLError> {
        let surface = unsafe {
            if self.layer.is_null() {
                let attributes = [
                    ffi::egl::WIDTH as ffi::EGLint,
                    self.size.0,
                    ffi::egl::HEIGHT as ffi::EGLint,
                    self.size.1,
                    ffi::egl::NONE as ffi::EGLint,
                ];
                ffi::egl::CreatePbufferSurface(display.handle, config_id, attributes.as_ptr())
            } else {
                ffi::egl::CreateWindowSurface(
                    display.handle,
                    config_id,
                    self.layer as ffi::NativeWindowType,
                    [ffi::egl::NONE as ffi::EGLint].as_ptr(),
                )
            }
        };
        if surface == ffi::egl::NO_SURFACE {
            Err(EGLError::BadSurface)
        } else {
            Ok(surface)
        }
    }

    fn identifier(&self) -> Option<String> {
        Some("Mac/CALayer".into())
    }
}

// --- the frame clock ---
//
// On the Deck a frame is drawn for each of the display's refreshes because the page flip is what
// the loop waits on, and on Android the window's swap waits the same way. ANGLE's swap on a
// Metal layer does not wait: asked to, it still returned at once and the loop ran at twice the
// display's rate, every other frame thrown away and the head predicted for the wrong moment. So
// the Mac's clock is the display's own: a display link ticks at each refresh, and the loop waits
// for the tick before it draws.

type DisplayLinkCallback = unsafe extern "C" fn(
    link: *mut c_void,
    now: *const c_void,
    output: *const c_void,
    flags_in: u64,
    flags_out: *mut u64,
    user: *mut c_void,
) -> i32;

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVDisplayLinkCreateWithCGDisplay(display: u32, link: *mut *mut c_void) -> i32;
    fn CVDisplayLinkCreateWithActiveCGDisplays(link: *mut *mut c_void) -> i32;
    fn CVDisplayLinkSetOutputCallback(link: *mut c_void, callback: DisplayLinkCallback, user: *mut c_void) -> i32;
    fn CVDisplayLinkStart(link: *mut c_void) -> i32;
    fn CVDisplayLinkStop(link: *mut c_void) -> i32;
    fn CVDisplayLinkRelease(link: *mut c_void);
}

struct Clock {
    /// How many refreshes there have been.
    ticks: std::sync::Mutex<u64>,
    ticked: std::sync::Condvar,
    /// The link that is running, and the display it is for.
    link: std::sync::Mutex<Option<(usize, u32)>>,
    /// The refresh the loop last drew for.
    drawn: std::sync::Mutex<u64>,
    /// Whether the window now being drawn into has been set up.
    set: std::sync::atomic::AtomicBool,
}

fn clock() -> &'static Clock {
    static CLOCK: std::sync::OnceLock<Clock> = std::sync::OnceLock::new();
    CLOCK.get_or_init(|| Clock {
        ticks: std::sync::Mutex::new(0),
        ticked: std::sync::Condvar::new(),
        link: std::sync::Mutex::new(None),
        drawn: std::sync::Mutex::new(0),
        set: std::sync::atomic::AtomicBool::new(false),
    })
}

unsafe extern "C" fn refreshed(
    _link: *mut c_void,
    _now: *const c_void,
    _output: *const c_void,
    _flags_in: u64,
    _flags_out: *mut u64,
    _user: *mut c_void,
) -> i32 {
    let clock = clock();
    *clock.ticks.lock().unwrap() += 1;
    clock.ticked.notify_all();
    0
}

/// Tick with this display from now on: the one the glasses' window is on, or 0 for the Mac's own.
pub fn follow_display(display: u32) {
    let clock = clock();
    let mut link = clock.link.lock().unwrap();
    if let Some((old, _)) = link.take() {
        unsafe {
            CVDisplayLinkStop(old as *mut c_void);
            CVDisplayLinkRelease(old as *mut c_void);
        }
    }
    clock.set.store(false, std::sync::atomic::Ordering::SeqCst);
    unsafe {
        let mut made: *mut c_void = std::ptr::null_mut();
        let status = if display == 0 {
            CVDisplayLinkCreateWithActiveCGDisplays(&mut made)
        } else {
            CVDisplayLinkCreateWithCGDisplay(display, &mut made)
        };
        if status != 0 || made.is_null() {
            log::warn!("frame clock: no display link for display {display} ({status}); frames are not paced");
            return;
        }
        CVDisplayLinkSetOutputCallback(made, refreshed, std::ptr::null_mut());
        CVDisplayLinkStart(made);
        *link = Some((made as usize, display));
    }
}

/// Wait for the display's next refresh. Called with the glasses' window current, before a frame
/// is drawn into it.
pub fn pace() {
    use smithay::backend::egl::ffi::egl;
    let clock = clock();
    if !clock.set.swap(true, std::sync::atomic::Ordering::SeqCst) {
        // The swap itself never waits: one clock, not two that could each cost a refresh.
        unsafe { egl::SwapInterval(egl::GetCurrentDisplay(), 0) };
        // **Without this the glasses are black.** ANGLE puts its Metal layer into the layer it
        // was handed, from this thread, which has no run loop to commit that change for it; the
        // frames were all drawn and presented into a layer the window server had never heard
        // of. Flushed here, once, with the new window current.
        flush_layers();
    }
    if clock.link.lock().unwrap().is_none() {
        return;
    }
    let mut drawn = clock.drawn.lock().unwrap();
    let ticks = clock.ticks.lock().unwrap();
    // A refresh that has already happened since the last frame is drawn for at once; otherwise
    // the next one is waited for -- but never for long, so a display that has stopped ticking
    // (asleep, unplugged) slows the session rather than stopping it.
    let (ticks, _) = clock
        .ticked
        .wait_timeout_while(ticks, std::time::Duration::from_millis(50), |now| *now <= *drawn)
        .unwrap();
    *drawn = *ticks;
    // Said every few hundred frames while the clock is being looked at.
    if std::env::var_os("SPATIAND_CLOCK_DEBUG").is_some() {
        static STATE: std::sync::Mutex<Option<(std::time::Instant, u64, u32)>> = std::sync::Mutex::new(None);
        let mut state = STATE.lock().unwrap();
        let (since, at, frames) = state.get_or_insert((std::time::Instant::now(), *ticks, 0));
        *frames += 1;
        if since.elapsed().as_secs() >= 3 {
            log::info!("frame clock: {} refreshes and {} frames in {:.1}s", *ticks - *at, *frames, since.elapsed().as_secs_f32());
            *state = Some((std::time::Instant::now(), *ticks, 0));
        }
    }
}

/// Commit this thread's pending Core Animation changes: `[CATransaction flush]`.
fn flush_layers() {
    #[link(name = "QuartzCore", kind = "framework")]
    extern "C" {}
    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const std::ffi::c_char) -> *mut c_void;
        fn sel_registerName(name: *const std::ffi::c_char) -> *mut c_void;
        fn objc_msgSend();
    }
    unsafe {
        let class = objc_getClass(c"CATransaction".as_ptr());
        if class.is_null() {
            return;
        }
        let send: unsafe extern "C" fn(*mut c_void, *mut c_void) =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        send(class, sel_registerName(c"flush".as_ptr()));
    }
}

/// **Make the frame opaque.** The scene blends as it draws and leaves whatever alpha that comes
/// to, which a display never looks at -- but macOS composites this layer over what is behind it,
/// and a frame whose alpha is nought is a frame nobody sees: the glasses were black with every
/// frame drawn correctly. So the alpha of the whole frame is set to one before it is shown.
///
/// # Safety
/// Called with the frame's context current.
pub unsafe fn finish_frame(gl: &smithay::backend::renderer::gles::ffi::Gles2, size: (i32, i32)) {
    use smithay::backend::renderer::gles::ffi;
    gl.Viewport(0, 0, size.0, size.1);
    gl.Disable(ffi::SCISSOR_TEST);
    gl.ColorMask(ffi::FALSE, ffi::FALSE, ffi::FALSE, ffi::TRUE);
    gl.ClearColor(0.0, 0.0, 0.0, 1.0);
    gl.Clear(ffi::COLOR_BUFFER_BIT);
    gl.ColorMask(ffi::TRUE, ffi::TRUE, ffi::TRUE, ffi::TRUE);
}
