//! Pictures on the Mac: the same interface as the VAAPI path, on `IOSurface`s.
//!
//! As on Android (see `android.rs`), a window's picture is not something a Wayland protocol can
//! carry, so the handover is the placeholder's: the window is given a buffer the picture's size
//! that holds a token, and the compositor draws the newest `IOSurface` behind that token in its
//! place.
//!
//! ```text
//!   a host's window:   frame ─▶ Decoder (VideoToolbox) ─▶ IOSurface ─┐
//!   this Mac's window: ScreenCaptureKit ─────────────────▶ IOSurface ─┴▶ Output ─▶ the texture
//! ```
//!
//! Both kinds of window end in an [`Output`], which is all the compositor knows of either.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use spatiand_stream::Codec;

#[path = "mac_decode.rs"]
mod decode;
pub use decode::{can_decode, Decoder};

#[derive(Debug)]
pub struct VideoError(String);

impl VideoError {
    pub(crate) fn new(what: impl Into<String>) -> VideoError {
        VideoError(what.into())
    }
}

impl std::fmt::Display for VideoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for VideoError {}

pub type Result<T> = std::result::Result<T, VideoError>;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRetain(object: *const c_void) -> *const c_void;
    fn CFRelease(object: *const c_void);
}

#[link(name = "IOSurface", kind = "framework")]
extern "C" {
    fn IOSurfaceIncrementUseCount(surface: *const c_void);
    fn IOSurfaceDecrementUseCount(surface: *const c_void);
    fn IOSurfaceGetWidth(surface: *const c_void) -> usize;
    fn IOSurfaceGetHeight(surface: *const c_void) -> usize;
    fn IOSurfaceGetID(surface: *const c_void) -> u32;
    fn IOSurfaceGetPixelFormat(surface: *const c_void) -> u32;
}

/// An `IOSurface`, held: kept alive, and marked as in use so that whoever made it -- a screen
/// capture or a decoder, both of which draw into a small pool of them -- does not draw the next
/// picture over this one while it is on screen.
pub struct Surface(*const c_void);

unsafe impl Send for Surface {}
unsafe impl Sync for Surface {}

impl Surface {
    /// Hold `surface`, an `IOSurfaceRef`. The caller's own reference is untouched.
    ///
    /// # Safety
    /// `surface` must be a live `IOSurfaceRef`.
    pub unsafe fn hold(surface: *const c_void) -> Surface {
        CFRetain(surface);
        IOSurfaceIncrementUseCount(surface);
        Surface(surface)
    }

    pub fn raw(&self) -> *const c_void {
        self.0
    }

    pub fn id(&self) -> u32 {
        unsafe { IOSurfaceGetID(self.0) }
    }

    pub fn size(&self) -> (u32, u32) {
        unsafe { (IOSurfaceGetWidth(self.0) as u32, IOSurfaceGetHeight(self.0) as u32) }
    }

    /// The four-character pixel format: `BGRA` for everything made here.
    pub fn format(&self) -> u32 {
        unsafe { IOSurfaceGetPixelFormat(self.0) }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            IOSurfaceDecrementUseCount(self.0);
            CFRelease(self.0);
        }
    }
}

/// One picture, taken from its output.
pub struct Frame {
    pub surface: Surface,
    /// The part of the surface that is picture: (u0, v0, u1, v1).
    pub crop: [f32; 4],
    /// The picture's own size in pixels, inside the crop.
    pub size: (u32, u32),
}

/// Where one window's pictures come out, read by the compositor's thread.
#[derive(Default)]
pub struct Output {
    latest: Mutex<Option<Frame>>,
}

impl Output {
    pub fn new() -> Arc<Output> {
        Arc::new(Output::default())
    }

    /// A new picture: the one it replaces, if nobody took it, is let go of.
    pub fn put(&self, surface: Surface, crop: [f32; 4]) {
        let (w, h) = surface.size();
        let size = (
            ((crop[2] - crop[0]) * w as f32).round().max(1.0) as u32,
            ((crop[3] - crop[1]) * h as f32).round().max(1.0) as u32,
        );
        if let Ok(mut latest) = self.latest.lock() {
            *latest = Some(Frame { surface, crop, size });
        }
    }

    /// The newest picture, if one has arrived since the last was taken.
    pub fn take_latest(self: &Arc<Self>) -> Option<Frame> {
        self.latest.lock().ok()?.take()
    }
}

// --- the registry the compositor finds a window's pictures through ---

fn registry() -> &'static Mutex<HashMap<u32, Arc<Output>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<u32, Arc<Output>>>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

/// Make an output findable by the token its window's placeholder carries.
pub fn register(output: Arc<Output>) -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    // Never 0, so a zeroed buffer is never mistaken for a window's.
    let token = NEXT.fetch_add(1, Ordering::Relaxed).max(1);
    registry().lock().unwrap().insert(token, output);
    token
}

/// The output behind a placeholder's token.
pub fn lookup(token: u32) -> Option<Arc<Output>> {
    registry().lock().unwrap().get(&token).cloned()
}

/// The window is gone, or has a new output.
pub fn forget(token: u32) {
    registry().lock().unwrap().remove(&token);
}

/// The first four bytes of a placeholder: this, then the token, little-endian. Android's.
pub const PLACEHOLDER_MAGIC: u32 = 0x5350_4144; // "DAPS"

// --- the interface the session drives, as on the Deck ---

/// What `Decoder::device` hands `Converter::new`. Nothing is needed on the Mac.
#[derive(Debug, Clone, Copy)]
pub struct Device;

/// One decoded picture: which output it is in and how big it is.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// Not a dmabuf: there is no fourcc, modifier or plane to hand over.
    pub fourcc: u32,
    pub modifier: u64,
    pub planes: Vec<(std::os::fd::RawFd, u32, u32)>,
    pub timestamp: i64,
    pub output: Arc<Output>,
}

/// A picture ready for the client to show: on the Mac, the output it is in.
pub struct Converted {
    pub width: u32,
    pub height: u32,
    pub fourcc: u32,
    pub modifier: u64,
    pub planes: Vec<(std::os::fd::RawFd, u32, u32)>,
    pub timestamp: i64,
    pub output: Arc<Output>,
}

impl Converted {
    /// A picture of this Mac's own, or anything else already in an output: what a window is
    /// shown with when nothing decoded it.
    pub fn of(output: Arc<Output>, size: (u32, u32)) -> Converted {
        Converted {
            width: size.0,
            height: size.1,
            fourcc: 0,
            modifier: 0,
            planes: Vec::new(),
            timestamp: 0,
            output,
        }
    }

    /// There are no pixels on this side to read: they never leave the GPU.
    pub fn to_bgra(&self) -> Result<Vec<u8>> {
        Err(VideoError::new("a picture on the Mac is not read back"))
    }
}

/// Nothing to convert on the Mac: the decoder is asked for BGRA, which is what is drawn.
pub struct Converter {
    size: (u32, u32),
}

unsafe impl Send for Converter {}

impl Converter {
    pub fn new(_device: Device, _frames: Device, size: (u32, u32)) -> Result<Converter> {
        Ok(Converter { size })
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn convert(&mut self, picture: &Picture) -> Result<Converted> {
        Ok(Converted {
            width: picture.width,
            height: picture.height,
            fourcc: 0,
            modifier: 0,
            planes: Vec::new(),
            timestamp: picture.timestamp,
            output: picture.output.clone(),
        })
    }
}

/// The microphone's encoder. Not on the Mac yet: a host that asks is told nothing is sent.
pub mod voice {
    pub struct Encoder;

    impl Encoder {
        pub fn new(_rate: u32, _channels: u16) -> Result<Encoder, String> {
            Err("the microphone is not sent from the Mac yet".into())
        }

        pub fn encode(&mut self, _pcm: &[i16]) -> Result<Vec<Vec<u8>>, String> {
            Err("the microphone is not sent from the Mac yet".into())
        }
    }
}

#[allow(unused)]
fn label(codec: Codec) -> &'static str {
    codec.label()
}
