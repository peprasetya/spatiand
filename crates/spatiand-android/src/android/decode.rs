//! A remote window's pictures, decoded by the phone's own video hardware.
//!
//! The Deck decodes through VAAPI and hands the compositor dmabufs. The Android equivalent is
//! `MediaCodec` decoding into an `AImageReader`'s surface: every picture stays a GPU buffer (an
//! `AHardwareBuffer`), and the drawing thread samples it as an external texture. No pixel is
//! ever copied or converted on the CPU; the GPU's sampler does the YUV to RGB on the way.
//!
//! ```text
//!   session thread:  bytes ─▶ AMediaCodec ─▶ (released with render=true)
//!                                                  │
//!   drawing thread:           AImageReader ◀───────┘ ─▶ AHardwareBuffer ─▶ EGLImage ─▶ texture
//! ```
//!
//! The two halves live on two threads: the codec is fed where the network is, and the reader
//! is read where the GL context is. [`Output`] is the reader, shared between them, and it
//! outlives the codec: a picture the drawing thread is still showing keeps it alive.

use std::collections::VecDeque;
use std::ffi::CString;
use std::ptr;
use std::sync::{Arc, Mutex};

use ndk_sys as ndk;
use spatiand_stream::Codec;

/// Pictures the drawing thread may hold at once: the one on screen, the one before it until
/// the GPU is surely done with it, and room for the newest to be taken. The codec draws into
/// the rest of the queue.
const MAX_IMAGES: i32 = 5;

/// The reader a decoder draws into, and what the drawing thread needs to know about each
/// picture that comes out of it.
pub struct Output {
    reader: *mut ndk::AImageReader,
    /// Which viewport each frame was drawn for, by its capture time: the picture carries the
    /// time through the codec, and the viewport is looked up when it is shown.
    viewports: Mutex<VecDeque<(u64, u32)>>,
}

// The reader is used from the drawing thread only, and deleted when the last owner lets go.
unsafe impl Send for Output {}
unsafe impl Sync for Output {}

impl Drop for Output {
    fn drop(&mut self) {
        unsafe { ndk::AImageReader_delete(self.reader) };
    }
}

/// One decoded picture, taken from the reader. Returned to it when dropped.
pub struct Picture {
    pub image: *mut ndk::AImage,
    pub buffer: *mut ndk::AHardwareBuffer,
    /// The part of the buffer that is picture, as (u0, v0, u1, v1).
    pub crop: [f32; 4],
    /// The picture's own size in pixels, inside the crop.
    pub size: (u32, u32),
    /// The viewport the host drew it for, if it said.
    pub viewport: Option<u32>,
    /// Keeps the reader alive for as long as its image is.
    _output: Arc<Output>,
}

impl Drop for Picture {
    fn drop(&mut self) {
        unsafe { ndk::AImage_delete(self.image) };
    }
}

impl Output {
    /// The newest picture, if one has arrived since the last was taken.
    pub fn take_latest(self: &Arc<Self>) -> Option<Picture> {
        let mut image = ptr::null_mut();
        let status = unsafe { ndk::AImageReader_acquireLatestImage(self.reader, &mut image) };
        if status != ndk::media_status_t::AMEDIA_OK || image.is_null() {
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
        let mut timestamp_ns = 0i64;
        unsafe { ndk::AImage_getTimestamp(image, &mut timestamp_ns) };
        let captured_us = (timestamp_ns / 1000).max(0) as u64;
        let viewport = {
            let mut viewports = self.viewports.lock().unwrap();
            // Everything older than this picture has been shown or overtaken.
            while viewports.front().is_some_and(|(at, _)| *at < captured_us) {
                viewports.pop_front();
            }
            viewports
                .front()
                .filter(|(at, _)| *at == captured_us)
                .map(|(_, seq)| *seq)
        };
        Some(Picture {
            image,
            buffer,
            crop,
            size,
            viewport,
            _output: self.clone(),
        })
    }
}

/// The codec half, on the session thread.
pub struct Decoder {
    codec: *mut ndk::AMediaCodec,
    output: Arc<Output>,
}

unsafe impl Send for Decoder {}

fn mime(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "video/avc",
        Codec::H265 => "video/hevc",
        Codec::Av1 => "video/av01",
    }
}

/// Whether the phone has a decoder for this codec at all. Asked before saying hello, so a host
/// is only offered what can really be shown.
pub fn can_decode(codec: Codec) -> bool {
    let Ok(name) = CString::new(mime(codec)) else { return false };
    let decoder = unsafe { ndk::AMediaCodec_createDecoderByType(name.as_ptr()) };
    if decoder.is_null() {
        return false;
    }
    unsafe { ndk::AMediaCodec_delete(decoder) };
    true
}

impl Decoder {
    pub fn new(codec: Codec, width: u32, height: u32) -> Result<Decoder, String> {
        let mut reader = ptr::null_mut();
        let status = unsafe {
            ndk::AImageReader_newWithUsage(
                width as i32,
                height as i32,
                ndk::AIMAGE_FORMATS::AIMAGE_FORMAT_PRIVATE.0 as i32,
                ndk::AHardwareBuffer_UsageFlags::AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE.0,
                MAX_IMAGES,
                &mut reader,
            )
        };
        if status != ndk::media_status_t::AMEDIA_OK || reader.is_null() {
            return Err(format!("no image reader for {width}x{height}: {status:?}"));
        }
        let output = Arc::new(Output {
            reader,
            viewports: Mutex::new(VecDeque::new()),
        });
        let mut window = ptr::null_mut();
        if unsafe { ndk::AImageReader_getWindow(reader, &mut window) }
            != ndk::media_status_t::AMEDIA_OK
        {
            return Err("the image reader has no surface".into());
        }

        let name = CString::new(mime(codec)).unwrap();
        let decoder = unsafe { ndk::AMediaCodec_createDecoderByType(name.as_ptr()) };
        if decoder.is_null() {
            return Err(format!("this phone cannot decode {}", codec.label()));
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
        set_int("width", width as i32);
        set_int("height", height as i32);
        // A picture as soon as it is decoded, not when the codec has a few to reorder: the
        // host sends no B-frames, and every frame held back is a frame of latency.
        set_int("low-latency", 1);
        // Realtime, as a video call is: the codec is clocked for it rather than for battery.
        set_int("priority", 0);
        let status = unsafe {
            ndk::AMediaCodec_configure(decoder, format, window, ptr::null_mut(), 0)
        };
        unsafe { ndk::AMediaFormat_delete(format) };
        if status != ndk::media_status_t::AMEDIA_OK {
            unsafe { ndk::AMediaCodec_delete(decoder) };
            return Err(format!("the {} decoder would not take {width}x{height}: {status:?}", codec.label()));
        }
        if unsafe { ndk::AMediaCodec_start(decoder) } != ndk::media_status_t::AMEDIA_OK {
            unsafe { ndk::AMediaCodec_delete(decoder) };
            return Err(format!("the {} decoder would not start", codec.label()));
        }
        Ok(Decoder { codec: decoder, output })
    }

    /// Where its pictures come out.
    pub fn output(&self) -> Arc<Output> {
        self.output.clone()
    }

    /// Give the codec one whole frame. Waits a little for room if the codec is full, which
    /// only happens when the drawing thread has stopped taking pictures.
    pub fn decode(&mut self, captured_us: u64, viewport: u32, bytes: &[u8], keyframe: bool) -> Result<(), String> {
        self.drain();
        let index = unsafe { ndk::AMediaCodec_dequeueInputBuffer(self.codec, 20_000) };
        if index < 0 {
            return Err("the decoder has no room for another frame".into());
        }
        let mut size = 0usize;
        let buffer = unsafe { ndk::AMediaCodec_getInputBuffer(self.codec, index as usize, &mut size) };
        if buffer.is_null() || size < bytes.len() {
            // Handed back empty, so the codec does not lose the slot.
            unsafe { ndk::AMediaCodec_queueInputBuffer(self.codec, index as usize, 0, 0, captured_us, 0) };
            return Err(format!("a {} byte frame does not fit the decoder's {size}", bytes.len()));
        }
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
        {
            let mut viewports = self.output.viewports.lock().unwrap();
            viewports.push_back((captured_us, viewport));
            // Bounded, for a codec that drops frames without saying.
            while viewports.len() > 64 {
                viewports.pop_front();
            }
        }
        let flags = if keyframe { ndk::AMEDIACODEC_BUFFER_FLAG_KEY_FRAME as u32 } else { 0 };
        let status = unsafe {
            ndk::AMediaCodec_queueInputBuffer(self.codec, index as usize, 0, bytes.len(), captured_us, flags)
        };
        if status != ndk::media_status_t::AMEDIA_OK {
            return Err(format!("the decoder refused a frame: {status:?}"));
        }
        self.drain();
        Ok(())
    }

    /// Send every decoded picture on to the reader.
    pub fn drain(&mut self) {
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
            } else if index == ndk::AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED as isize
                || index == ndk::AMEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED as isize
            {
                // Says only that more is coming; keep going.
                continue;
            } else {
                // Nothing yet, or an error the next frame will run into and report.
                return;
            }
        }
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
