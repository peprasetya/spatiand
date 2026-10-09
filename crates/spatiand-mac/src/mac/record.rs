//! A video of what the glasses show. Not on the Mac yet: the HUD's row says so in the log.
//!
//! The interface is Android's `record.rs`, which the frame loop drives.

use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::renderer::gles::{ffi, GlesRenderer};

pub struct Recorder;

impl Recorder {
    pub fn start(_size: (i32, i32), _display: &EGLDisplay, _context: &EGLContext) -> Result<Recorder, String> {
        Err("recording is not built for the Mac yet".into())
    }

    pub fn due(&mut self) -> bool {
        false
    }

    /// # Safety
    /// Called with the frame's context current.
    pub unsafe fn copy(&mut self, _gl: &ffi::Gles2) {}

    pub fn present(&mut self, _renderer: &mut GlesRenderer) {}

    pub fn stop(self, _renderer: &mut GlesRenderer) {}
}

/// A screenshot or a finished video, for the app to show in the Finder.
pub fn publish(path: String) {
    super::ffi::tell(super::ffi::Asked::Saved, &path);
}

pub fn publish_leftovers() {}
