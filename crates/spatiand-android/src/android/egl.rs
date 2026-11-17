//! Smithay's EGL on Android: the default display, and window surfaces on `ANativeWindow`s.
//!
//! Smithay knows GBM, Wayland and X11 displays; Android's is none of those. What it needs is two
//! small adapters -- Android's default display, and a surface made from a window the app handed
//! over -- after which its `GlesRenderer` is the Deck's, unchanged.

use std::ffi::c_void;
use std::sync::Arc;

use smithay::backend::egl::display::EGLDisplayHandle;
use smithay::backend::egl::ffi;
use smithay::backend::egl::native::EGLNativeSurface;
use smithay::backend::egl::EGLError;

/// Android's default display, opened the way Android opens it, and handed to Smithay.
///
/// Smithay would open one through `eglGetPlatformDisplayEXT` on `EGL_PLATFORM_ANDROID_KHR`;
/// Android answers that with a handle `eglInitialize` then calls invalid. The core `eglGetDisplay` is
/// what Android implements, so the display is made with that and adopted with `from_raw`.
pub fn open_display() -> Result<smithay::backend::egl::EGLDisplay, String> {
    use smithay::backend::egl::ffi::egl;
    smithay::backend::egl::ffi::make_sure_egl_is_loaded().map_err(|e| format!("no EGL: {e}"))?;
    unsafe {
        let display = egl::GetDisplay(egl::DEFAULT_DISPLAY as _);
        if display == egl::NO_DISPLAY {
            return Err("no default EGL display".into());
        }
        let (mut major, mut minor) = (0, 0);
        if egl::Initialize(display, &mut major, &mut minor) == egl::FALSE {
            return Err(format!("eglInitialize failed: {:#x}", egl::GetError()));
        }
        log::info!("EGL {major}.{minor}");
        // Recordable as well: a video encoder's input surface is a window only a config that
        // says so may draw into, and the one context draws the glasses, the phone and that.
        const RECORDABLE_ANDROID: i32 = 0x3142;
        let mut attributes = vec![
            egl::RED_SIZE as i32, 8,
            egl::GREEN_SIZE as i32, 8,
            egl::BLUE_SIZE as i32, 8,
            egl::ALPHA_SIZE as i32, 8,
            egl::RENDERABLE_TYPE as i32, egl::OPENGL_ES3_BIT as i32,
            egl::SURFACE_TYPE as i32, (egl::WINDOW_BIT | egl::PBUFFER_BIT) as i32,
            RECORDABLE_ANDROID, 1,
            egl::NONE as i32,
        ];
        let mut config = std::ptr::null();
        let mut found = 0;
        if egl::ChooseConfig(display, attributes.as_ptr(), &mut config, 1, &mut found) == egl::FALSE || found == 0 {
            log::warn!("no recordable EGL config; recording will not work");
            attributes.truncate(attributes.len() - 3);
            attributes.push(egl::NONE as i32);
            found = 0;
            if egl::ChooseConfig(display, attributes.as_ptr(), &mut config, 1, &mut found) == egl::FALSE || found == 0 {
                return Err("no EGL config for GLES 3 windows".into());
            }
        }
        smithay::backend::egl::EGLDisplay::from_raw(display, config).map_err(|e| format!("{e}"))
    }
}

/// A window the app handed over, as something EGL can draw into. Holds its own reference, so
/// the window outlives the surface made from it whatever the app does meanwhile.
pub struct AndroidWindow(*mut ndk_sys::ANativeWindow);

unsafe impl Send for AndroidWindow {}

impl AndroidWindow {
    pub fn new(window: *mut ndk_sys::ANativeWindow) -> AndroidWindow {
        unsafe { ndk_sys::ANativeWindow_acquire(window) };
        AndroidWindow(window)
    }
}

impl Drop for AndroidWindow {
    fn drop(&mut self) {
        unsafe { ndk_sys::ANativeWindow_release(self.0) };
    }
}

unsafe impl EGLNativeSurface for AndroidWindow {
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
        Some("Android/ANativeWindow".into())
    }
}

/// The surface to draw into for a window the app handed over.
pub fn surface_of(window: &super::NativeWindow) -> AndroidWindow {
    AndroidWindow::new(window.0)
}

/// **Wait until the glasses' window can take the next frame**: the frame clock, called before the
/// room reads what its clients have committed (see the frame loop).
///
/// Android has no call that only waits. The wait is the window handing out its next buffer, which
/// it does when that buffer is first drawn into -- blocking while the display still holds every
/// other one, until a refresh frees one. So the window is cleared here, which is drawing into it,
/// and the clear sent to the GPU: the buffer is the window's from then until the swap, and the
/// frame drawn after this clears it again. Measured on the Beam Pro (Adreno, 2026-11-17): this
/// takes the 12-14 ms of each frame that the frame's own first draw used to take.
pub fn wait_for_refresh(
    renderer: &mut smithay::backend::renderer::gles::GlesRenderer,
    surface: &mut smithay::backend::egl::EGLSurface,
    size: (i32, i32),
) {
    use smithay::backend::renderer::gles::ffi;
    use smithay::backend::renderer::{Bind, Frame, Renderer};
    let Ok(mut target) = renderer.bind(surface) else { return };
    let Ok(mut frame) = renderer.render(&mut target, size.into(), smithay::utils::Transform::Normal) else {
        return;
    };
    let _ = frame.with_context(|gl| unsafe {
        gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
        gl.Disable(ffi::SCISSOR_TEST);
        gl.Viewport(0, 0, size.0, size.1);
        gl.ClearColor(0.0, 0.0, 0.0, 1.0);
        gl.Clear(ffi::COLOR_BUFFER_BIT);
        gl.Flush();
    });
    // Finished rather than dropped: a dropped frame waits for its GPU work to be done.
    let _ = frame.finish();
}

/// Nothing: Android's display takes the frame as it is.
///
/// # Safety
/// Called with the frame's context current.
pub unsafe fn finish_frame(_gl: &smithay::backend::renderer::gles::ffi::Gles2, _size: (i32, i32)) {}
