//! A video of what the glasses show.
//!
//! The interface is Android's `record.rs`, which the frame loop drives -- but on the Mac the
//! filming is the app's: ScreenCaptureKit takes the glasses' display as the window server shows
//! it, with the Mac's sound, and writes the file. So nothing is copied out of the frame here;
//! starting and stopping are passed on.

use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::renderer::gles::{ffi, GlesRenderer};

pub struct Recorder;

impl Recorder {
    pub fn start(_size: (i32, i32), _display: &EGLDisplay, _context: &EGLContext) -> Result<Recorder, String> {
        super::ffi::tell(super::ffi::Asked::RecordStart, "");
        Ok(Recorder)
    }

    pub fn due(&mut self) -> bool {
        false
    }

    /// # Safety
    /// Called with the frame's context current.
    pub unsafe fn copy(&mut self, _gl: &ffi::Gles2) {}

    pub fn present(&mut self, _renderer: &mut GlesRenderer) {}

    pub fn stop(self, _renderer: &mut GlesRenderer) {
        super::ffi::tell(super::ffi::Asked::RecordStop, "");
    }
}

/// A screenshot or a finished video, for the app to show in the Finder.
pub fn publish(path: String) {
    super::ffi::tell(super::ffi::Asked::Saved, &path);
}

pub fn publish_leftovers() {}
