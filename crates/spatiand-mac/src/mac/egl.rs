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

/// A `CALayer` the app handed over, as something EGL can draw into. ANGLE puts a Metal layer of
/// its own inside it.
pub struct Layer(pub *mut c_void);

unsafe impl Send for Layer {}

unsafe impl EGLNativeSurface for Layer {
    unsafe fn create(
        &self,
        display: &Arc<EGLDisplayHandle>,
        config_id: ffi::egl::types::EGLConfig,
    ) -> Result<*const c_void, EGLError> {
        let surface = unsafe {
            ffi::egl::CreateWindowSurface(
                display.handle,
                config_id,
                self.0 as ffi::NativeWindowType,
                [ffi::egl::NONE as ffi::EGLint].as_ptr(),
            )
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
