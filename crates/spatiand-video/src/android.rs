//! Decoding on Android: the same interface as the VAAPI path, on `MediaCodec`.
//!
//! The Deck's pictures are dmabufs, handed to the compositor through `linux-dmabuf`. Android's
//! decoder writes `AHardwareBuffer`s instead, which no Wayland protocol carries, so the handover
//! is different while everything around it -- the session, its rules about keyframes and gaps,
//! the in-process client that makes each remote window a real window -- is the Deck's own:
//!
//! ```text
//!   session:     frame ─▶ Decoder (MediaCodec) ─▶ its AImageReader
//!   client:      Converted ─▶ a placeholder shm buffer the picture's size, carrying a token
//!   compositor:  the surface's token ─▶ the reader's newest picture ─▶ the window's texture
//! ```
//!
//! The placeholder gives the window its size, so it is placed, framed, resized and pointed at
//! like any other; its pixels are never looked at except for the token in the first four bytes.
//! The picture itself stays on the GPU the whole way.

use std::collections::{HashMap, VecDeque};
use std::ffi::CString;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use ndk_sys as ndk;
use spatiand_stream::Codec;

#[derive(Debug)]
pub struct VideoError(String);

impl VideoError {
    fn new(what: impl Into<String>) -> VideoError {
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

/// Pictures out of the codec at once: the newest waiting, the ones the compositor still draws
/// from (it keeps each a few frames, until the GPU is surely done), and room for the newest to
/// be taken. The codec draws into the rest.
const MAX_IMAGES: i32 = 8;

/// Where one decoder's pictures come out, read by the compositor's thread.
pub struct Output {
    reader: *mut ndk::AImageReader,
    /// The newest picture, taken off the reader as soon as it arrives -- see [`on_image`].
    latest: Mutex<usize>,
}

/// A picture arrived: take it now, and let go of the one it replaces.
///
/// **A picture left on the reader is a buffer the codec cannot decode into.** They used to be
/// taken only when the glasses drew their window. A window drawn less often than its frames
/// came filled the reader, the codec stalled with nowhere to put its output and stopped taking
/// frames, and the session threw everything away until a keyframe: SpatiWorld froze for up to
/// a second at a time.
unsafe extern "C" fn on_image(context: *mut std::ffi::c_void, reader: *mut ndk::AImageReader) {
    let output = &*(context as *const Output);
    let mut image = ptr::null_mut();
    if ndk::AImageReader_acquireLatestImage(reader, &mut image) != ndk::media_status_t::AMEDIA_OK
        || image.is_null()
    {
        return;
    }
    match output.latest.lock() {
        Ok(mut latest) => {
            let before = std::mem::replace(&mut *latest, image as usize);
            if before != 0 {
                ndk::AImage_delete(before as *mut ndk::AImage);
            }
        }
        Err(_) => ndk::AImage_delete(image),
    }
}

// The reader is read from the compositor's thread only, and deleted when the last owner lets go.
unsafe impl Send for Output {}
unsafe impl Sync for Output {}

impl Drop for Output {
    fn drop(&mut self) {
        let latest = self.latest.get_mut().map(std::mem::take).unwrap_or(0);
        if latest != 0 {
            unsafe { ndk::AImage_delete(latest as *mut ndk::AImage) };
        }
        unsafe { ndk::AImageReader_delete(self.reader) };
    }
}

/// One decoded picture, taken from its reader, and returned to it when dropped.
pub struct Frame {
    pub image: *mut ndk::AImage,
    pub buffer: *mut ndk::AHardwareBuffer,
    /// The part of the buffer that is picture: (u0, v0, u1, v1).
    pub crop: [f32; 4],
    /// The picture's own size in pixels, inside the crop.
    pub size: (u32, u32),
    /// Keeps the reader alive for as long as its image is.
    _output: Arc<Output>,
}

impl Drop for Frame {
    fn drop(&mut self) {
        unsafe { ndk::AImage_delete(self.image) };
    }
}

impl Output {
    /// The newest picture, if one has arrived since the last was taken.
    pub fn take_latest(self: &Arc<Self>) -> Option<Frame> {
        let image = self.latest.lock().map(|mut l| std::mem::take(&mut *l)).unwrap_or(0)
            as *mut ndk::AImage;
        if image.is_null() {
            return None;
        }
        let mut buffer = ptr::null_mut();
        if unsafe { ndk::AImage_getHardwareBuffer(image, &mut buffer) }
            != ndk::media_status_t::AMEDIA_OK
            || buffer.is_null()
        {
            unsafe { ndk::AImage_delete(image) };
            return None;
        }
        let mut desc = ndk::AHardwareBuffer_Desc {
            width: 0,
            height: 0,
            layers: 0,
            format: 0,
            usage: 0,
            stride: 0,
            rfu0: 0,
            rfu1: 0,
        };
        unsafe { ndk::AHardwareBuffer_describe(buffer, &mut desc) };
        let mut rect = ndk::AImageCropRect { left: 0, top: 0, right: 0, bottom: 0 };
        let crop = if unsafe { ndk::AImage_getCropRect(image, &mut rect) }
            == ndk::media_status_t::AMEDIA_OK
            && desc.width > 0
            && desc.height > 0
            && rect.right > rect.left
            && rect.bottom > rect.top
        {
            let (w, h) = (desc.width as f32, desc.height as f32);
            [
                rect.left as f32 / w,
                rect.top as f32 / h,
                rect.right as f32 / w,
                rect.bottom as f32 / h,
            ]
        } else {
            [0.0, 0.0, 1.0, 1.0]
        };
        let size = (
            ((crop[2] - crop[0]) * desc.width as f32).round().max(1.0) as u32,
            ((crop[3] - crop[1]) * desc.height as f32).round().max(1.0) as u32,
        );
        Some(Frame {
            image,
            buffer,
            crop,
            size,
            _output: self.clone(),
        })
    }
}

// --- the registry the compositor finds a window's pictures through ---

fn registry() -> &'static Mutex<HashMap<u32, Arc<Output>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<u32, Arc<Output>>>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

/// Make a decoder's output findable by the token its window's placeholder carries.
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

/// The window is gone, or has a new decoder.
pub fn forget(token: u32) {
    registry().lock().unwrap().remove(&token);
}

/// The first four bytes of a placeholder: this, then the token, little-endian.
pub const PLACEHOLDER_MAGIC: u32 = 0x5350_4144; // "DAPS"

// --- the interface the session drives, as on the Deck ---

/// What `Decoder::device` hands `Converter::new`. Nothing is needed on Android.
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

/// One remote window's decoder.
pub struct Decoder {
    codec: *mut ndk::AMediaCodec,
    output: Arc<Output>,
    /// The picture's size inside the codec's buffers, once the codec has said.
    size: Option<(u32, u32)>,
    /// Capture times of frames queued and not yet out, oldest first, so a picture carries the
    /// time the host gave its frame.
    queued: VecDeque<i64>,
}

unsafe impl Send for Decoder {}

fn mime(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "video/avc",
        Codec::H265 => "video/hevc",
        Codec::Av1 => "video/av01",
    }
}

/// Whether the phone has a decoder for this codec at all.
pub fn can_decode(codec: Codec) -> bool {
    let Ok(name) = CString::new(mime(codec)) else { return false };
    let decoder = unsafe { ndk::AMediaCodec_createDecoderByType(name.as_ptr()) };
    if decoder.is_null() {
        return false;
    }
    unsafe { ndk::AMediaCodec_delete(decoder) };
    true
}

/// How big a picture the reader is made for, until the stream says. The codec draws its own
/// size into it: a reader's size is only the default for buffers nobody sized.
const DEFAULT_SIZE: (i32, i32) = (1920, 1080);

impl Decoder {
    /// Open the phone's decoder for `codec`. The node is the Deck's render node, meaningless here.
    pub fn new(_node: &str, codec: Codec) -> Result<Decoder> {
        let mut reader = ptr::null_mut();
        let status = unsafe {
            ndk::AImageReader_newWithUsage(
                DEFAULT_SIZE.0,
                DEFAULT_SIZE.1,
                ndk::AIMAGE_FORMATS::AIMAGE_FORMAT_PRIVATE.0 as i32,
                ndk::AHardwareBuffer_UsageFlags::AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE.0,
                MAX_IMAGES,
                &mut reader,
            )
        };
        if status != ndk::media_status_t::AMEDIA_OK || reader.is_null() {
            return Err(VideoError::new(format!("no image reader: {status:?}")));
        }
        let output = Arc::new(Output {
            reader,
            latest: Mutex::new(0),
        });
        // The context is the `Output` itself, which owns the reader and so outlives every call.
        let mut listener = ndk::AImageReader_ImageListener {
            context: Arc::as_ptr(&output) as *mut std::ffi::c_void,
            onImageAvailable: Some(on_image),
        };
        unsafe { ndk::AImageReader_setImageListener(reader, &mut listener) };
        let mut window = ptr::null_mut();
        if unsafe { ndk::AImageReader_getWindow(reader, &mut window) }
            != ndk::media_status_t::AMEDIA_OK
        {
            return Err(VideoError::new("the image reader has no surface"));
        }

        let name = CString::new(mime(codec)).unwrap();
        let decoder = unsafe { ndk::AMediaCodec_createDecoderByType(name.as_ptr()) };
        if decoder.is_null() {
            return Err(VideoError::new(format!("this phone cannot decode {}", codec.label())));
        }
        let format = unsafe { ndk::AMediaFormat_new() };
        let set_int = |key: &str, value: i32| {
            let key = CString::new(key).unwrap();
            unsafe { ndk::AMediaFormat_setInt32(format, key.as_ptr(), value) };
        };
        unsafe {
            let key = CString::new("mime").unwrap();
            ndk::AMediaFormat_setString(format, key.as_ptr(), name.as_ptr());
        }
        set_int("width", DEFAULT_SIZE.0);
        set_int("height", DEFAULT_SIZE.1);
        // Room for anything a host sends, SpatiWorld's two eyes included.
        set_int("max-width", 3840);
        set_int("max-height", 2160);
        // A picture as soon as it is decoded: the host sends no B-frames, and every frame the
        // codec holds back to reorder is a frame of latency.
        set_int("low-latency", 1);
        set_int("priority", 0);
        let status = unsafe { ndk::AMediaCodec_configure(decoder, format, window, ptr::null_mut(), 0) };
        unsafe { ndk::AMediaFormat_delete(format) };
        if status != ndk::media_status_t::AMEDIA_OK {
            unsafe { ndk::AMediaCodec_delete(decoder) };
            return Err(VideoError::new(format!("the {} decoder would not configure: {status:?}", codec.label())));
        }
        if unsafe { ndk::AMediaCodec_start(decoder) } != ndk::media_status_t::AMEDIA_OK {
            unsafe { ndk::AMediaCodec_delete(decoder) };
            return Err(VideoError::new(format!("the {} decoder would not start", codec.label())));
        }
        Ok(Decoder {
            codec: decoder,
            output,
            size: None,
            queued: VecDeque::new(),
        })
    }

    /// Give the codec one whole frame, and say which pictures it has finished since.
    pub fn decode(&mut self, timestamp: i64, frame: &[u8]) -> Result<Vec<Picture>> {
        let mut pictures = self.drain();
        // Full, it may only be waiting for its output to be taken: take it and try again.
        let mut index = unsafe { ndk::AMediaCodec_dequeueInputBuffer(self.codec, 10_000) };
        for _ in 0..3 {
            if index >= 0 {
                break;
            }
            pictures.extend(self.drain());
            index = unsafe { ndk::AMediaCodec_dequeueInputBuffer(self.codec, 10_000) };
        }
        if index < 0 {
            return Err(VideoError::new("the decoder has no room for another frame"));
        }
        let mut size = 0usize;
        let buffer = unsafe { ndk::AMediaCodec_getInputBuffer(self.codec, index as usize, &mut size) };
        if buffer.is_null() || size < frame.len() {
            unsafe { ndk::AMediaCodec_queueInputBuffer(self.codec, index as usize, 0, 0, 0, 0) };
            return Err(VideoError::new(format!(
                "a {} byte frame does not fit the decoder's {size}",
                frame.len()
            )));
        }
        unsafe { ptr::copy_nonoverlapping(frame.as_ptr(), buffer, frame.len()) };
        let status = unsafe {
            ndk::AMediaCodec_queueInputBuffer(
                self.codec,
                index as usize,
                0,
                frame.len(),
                timestamp.max(0) as u64,
                0,
            )
        };
        if status != ndk::media_status_t::AMEDIA_OK {
            return Err(VideoError::new(format!("the decoder refused a frame: {status:?}")));
        }
        self.queued.push_back(timestamp);
        while self.queued.len() > 64 {
            self.queued.pop_front();
        }
        pictures.extend(self.drain());
        Ok(pictures)
    }

    /// Whether a frame was given and its picture has not come out yet.
    pub fn waiting(&self) -> bool {
        !self.queued.is_empty()
    }

    /// Pictures finished since the last look, with no new frame to give.
    ///
    /// **The codec works on its own time.** A frame is queued and decoded a few milliseconds
    /// later, so looking only when the next frame is given finds each picture one frame late --
    /// and a window that has gone still has no next frame. A key typed into a terminal showed
    /// only when the next key was, since each keystroke is the only frame the host sends.
    pub fn finished(&mut self) -> Vec<Picture> {
        self.drain()
    }

    /// Send every picture the codec has finished on to the reader.
    fn drain(&mut self) -> Vec<Picture> {
        let mut pictures = Vec::new();
        loop {
            let mut info = ndk::AMediaCodecBufferInfo {
                offset: 0,
                size: 0,
                presentationTimeUs: 0,
                flags: 0,
            };
            let index = unsafe { ndk::AMediaCodec_dequeueOutputBuffer(self.codec, &mut info, 0) };
            if index >= 0 {
                unsafe { ndk::AMediaCodec_releaseOutputBuffer(self.codec, index as usize, true) };
                let timestamp = info.presentationTimeUs;
                self.queued.retain(|t| *t > timestamp);
                if let Some((width, height)) = self.size {
                    pictures.push(Picture {
                        width,
                        height,
                        fourcc: 0,
                        modifier: 0,
                        planes: Vec::new(),
                        timestamp,
                        output: self.output.clone(),
                    });
                }
            } else if index == ndk::AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED as isize {
                self.size = self.output_size();
                if let Some((w, h)) = self.size {
                    log::info!("decoder: pictures are {w}x{h}");
                }
            } else if index == ndk::AMEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED as isize {
                continue;
            } else {
                return pictures;
            }
        }
    }

    /// The picture's size, from the codec's output format: its crop if it has one.
    fn output_size(&self) -> Option<(u32, u32)> {
        let format = unsafe { ndk::AMediaCodec_getOutputFormat(self.codec) };
        if format.is_null() {
            return None;
        }
        let get = |key: &str| {
            let key = CString::new(key).unwrap();
            let mut value = 0i32;
            unsafe { ndk::AMediaFormat_getInt32(format, key.as_ptr(), &mut value) }.then_some(value)
        };
        let size = match (get("crop-left"), get("crop-right"), get("crop-top"), get("crop-bottom")) {
            (Some(l), Some(r), Some(t), Some(b)) if r > l && b > t => Some(((r - l + 1) as u32, (b - t + 1) as u32)),
            _ => get("width").zip(get("height")).map(|(w, h)| (w as u32, h as u32)),
        };
        unsafe { ndk::AMediaFormat_delete(format) };
        size
    }

    pub fn device(&self) -> (Device, Device) {
        (Device, Device)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            ndk::AMediaCodec_stop(self.codec);
            ndk::AMediaCodec_delete(self.codec);
        }
    }
}

/// A picture ready for the client to show: on Android, the output it is in.
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
    /// There are no pixels on this side to read: they never leave the GPU.
    pub fn to_bgra(&self) -> Result<Vec<u8>> {
        Err(VideoError::new("a picture on Android cannot be read back"))
    }
}

/// Nothing to convert on Android: the GPU samples the decoder's own buffer, YUV and all.
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

/// The microphone's encoder. Not on Android yet: a host that asks is told nothing is sent.
pub mod voice {
    pub struct Encoder;

    impl Encoder {
        pub fn new(_rate: u32, _channels: u16) -> Result<Encoder, String> {
            Err("the microphone is not sent from Android yet".into())
        }

        pub fn encode(&mut self, _pcm: &[i16]) -> Result<Vec<Vec<u8>>, String> {
            Err("the microphone is not sent from Android yet".into())
        }
    }
}
