//! A recording's encoder on the Mac: VideoToolbox's hardware H.264, fed `IOSurface`s.
//!
//! The Mac's part of the room's recorder (the Beam Pro's and the Mac's `record.rs`), in place of
//! the Beam Pro's MediaCodec and the Deck's VA-API: the same picture -- both eyes side by side,
//! H.264 at the recording's rate and bit rate -- written to the same Matroska file.
//!
//! The pictures are drawn into a few buffers the encoder hands out at the start, each an
//! `IOSurface` the compositor's GL (ANGLE, on Metal) can draw into, so a frame reaches the
//! encoder with one copy on the GPU and none on the CPU. A buffer is the encoder's from
//! [`Encoder::encode`] until its picture comes out ([`Encoded`]), and only then drawn into
//! again.
//!
//! What comes out is Annex B, as the Beam Pro's encoder writes it: NAL units behind start codes,
//! with the stream's parameter sets said on their own first (`config`).

use std::ffi::c_void;
use std::ptr;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;

use super::{Result, Surface, VideoError};

type Ref = *const c_void;

/// `CMTime`, which Core Media packs to four bytes.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct Time {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

const TIME_VALID: u32 = 1;
const INVALID: Time = Time { value: 0, timescale: 0, flags: 0, epoch: 0 };

type OutputCallback = unsafe extern "C" fn(user: *mut c_void, frame: *mut c_void, status: i32, flags: u32, sample: Ref);

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    static kCFBooleanTrue: Ref;
    static kCFBooleanFalse: Ref;
    fn CFRelease(object: Ref);
    fn CFDictionaryCreate(
        allocator: Ref,
        keys: *const Ref,
        values: *const Ref,
        count: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> Ref;
    fn CFDictionaryGetValue(dictionary: Ref, key: Ref) -> Ref;
    fn CFArrayGetCount(array: Ref) -> isize;
    fn CFArrayGetValueAtIndex(array: Ref, index: isize) -> Ref;
    fn CFBooleanGetValue(boolean: Ref) -> u8;
    fn CFNumberCreate(allocator: Ref, kind: isize, value: *const c_void) -> Ref;
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    static kCMSampleAttachmentKey_NotSync: Ref;
    fn CMSampleBufferGetDataBuffer(sample: Ref) -> Ref;
    fn CMSampleBufferGetFormatDescription(sample: Ref) -> Ref;
    fn CMSampleBufferGetPresentationTimeStamp(sample: Ref) -> Time;
    fn CMSampleBufferGetSampleAttachmentsArray(sample: Ref, create: u8) -> Ref;
    fn CMBlockBufferGetDataLength(buffer: Ref) -> usize;
    fn CMBlockBufferCopyDataBytes(buffer: Ref, offset: usize, length: usize, out: *mut c_void) -> i32;
    fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
        format: Ref,
        index: usize,
        pointer: *mut *const u8,
        size: *mut usize,
        count: *mut usize,
        length_bytes: *mut i32,
    ) -> i32;
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: Ref;
    static kCVPixelBufferWidthKey: Ref;
    static kCVPixelBufferHeightKey: Ref;
    static kCVPixelBufferIOSurfacePropertiesKey: Ref;
    static kCVPixelBufferMetalCompatibilityKey: Ref;
    fn CVPixelBufferPoolCreatePixelBuffer(allocator: Ref, pool: Ref, out: *mut Ref) -> i32;
    fn CVPixelBufferGetIOSurface(buffer: Ref) -> Ref;
}

#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    static kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: Ref;
    static kVTCompressionPropertyKey_RealTime: Ref;
    static kVTCompressionPropertyKey_ProfileLevel: Ref;
    static kVTProfileLevel_H264_High_AutoLevel: Ref;
    static kVTCompressionPropertyKey_AverageBitRate: Ref;
    static kVTCompressionPropertyKey_ExpectedFrameRate: Ref;
    static kVTCompressionPropertyKey_MaxKeyFrameInterval: Ref;
    static kVTCompressionPropertyKey_AllowFrameReordering: Ref;
    fn VTCompressionSessionCreate(
        allocator: Ref,
        width: i32,
        height: i32,
        codec: u32,
        specification: Ref,
        image_attributes: Ref,
        compressed_allocator: Ref,
        callback: OutputCallback,
        user: *mut c_void,
        out: *mut Ref,
    ) -> i32;
    fn VTSessionSetProperty(session: Ref, key: Ref, value: Ref) -> i32;
    fn VTCompressionSessionPrepareToEncodeFrames(session: Ref) -> i32;
    fn VTCompressionSessionGetPixelBufferPool(session: Ref) -> Ref;
    fn VTCompressionSessionEncodeFrame(
        session: Ref,
        image: Ref,
        presentation: Time,
        duration: Time,
        properties: Ref,
        frame: *mut c_void,
        info: *mut u32,
    ) -> i32;
    fn VTCompressionSessionCompleteFrames(session: Ref, until: Time) -> i32;
    fn VTCompressionSessionInvalidate(session: Ref);
}

const BGRA: i32 = i32::from_be_bytes(*b"BGRA");
const AVC: u32 = u32::from_be_bytes(*b"avc1");
const NUMBER_SINT32: isize = 3;

/// One picture out of the encoder, or one it could not make.
pub enum Encoded {
    Picture {
        /// The buffer it was drawn in, free to be drawn into again.
        buffer: usize,
        /// Its time, as it was given.
        ms: i64,
        key: bool,
        /// The stream's parameter sets (SPS, PPS) as Annex B, with every keyframe.
        config: Option<Vec<u8>>,
        /// The picture, Annex B.
        data: Vec<u8>,
    },
    /// The buffer's picture was dropped or failed; the buffer is free again.
    Lost { buffer: usize, status: i32 },
}

/// What the encoder's callback writes into: shared with it by pointer for the session's life.
struct Sink {
    out: Mutex<Sender<Encoded>>,
}

/// A number, as Core Foundation holds one. The caller releases it.
unsafe fn number(value: i32) -> Ref {
    CFNumberCreate(ptr::null(), NUMBER_SINT32, &value as *const i32 as *const c_void)
}

unsafe fn dictionary(keys: &[Ref], values: &[Ref]) -> Ref {
    CFDictionaryCreate(
        ptr::null(),
        keys.as_ptr(),
        values.as_ptr(),
        keys.len() as isize,
        &kCFTypeDictionaryKeyCallBacks,
        &kCFTypeDictionaryValueCallBacks,
    )
}

unsafe extern "C" fn encoded(user: *mut c_void, frame: *mut c_void, status: i32, _flags: u32, sample: Ref) {
    let sink = &*(user as *const Sink);
    let buffer = frame as usize;
    let said = if status != 0 || sample.is_null() {
        Encoded::Lost { buffer, status }
    } else {
        match picture(sample) {
            Some((ms, key, config, data)) => Encoded::Picture { buffer, ms, key, config, data },
            None => Encoded::Lost { buffer, status: -1 },
        }
    };
    if let Ok(out) = sink.out.lock() {
        let _ = out.send(said);
    }
}

/// A coded sample, taken apart: its time, whether it is a keyframe, the parameter sets if it is
/// one, and its NAL units behind start codes rather than lengths.
unsafe fn picture(sample: Ref) -> Option<(i64, bool, Option<Vec<u8>>, Vec<u8>)> {
    let block = CMSampleBufferGetDataBuffer(sample);
    if block.is_null() {
        return None;
    }
    let length = CMBlockBufferGetDataLength(block);
    let mut bytes = vec![0u8; length];
    if CMBlockBufferCopyDataBytes(block, 0, length, bytes.as_mut_ptr() as *mut c_void) != 0 {
        return None;
    }
    let time = CMSampleBufferGetPresentationTimeStamp(sample);
    let (value, scale) = (time.value, time.timescale);
    let ms = if scale > 0 { value * 1000 / scale as i64 } else { 0 };
    // A sample is a keyframe unless its attachments say "not sync".
    let mut key = true;
    let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, 0);
    if !attachments.is_null() && CFArrayGetCount(attachments) > 0 {
        let first = CFArrayGetValueAtIndex(attachments, 0);
        let not_sync = CFDictionaryGetValue(first, kCMSampleAttachmentKey_NotSync);
        if !not_sync.is_null() && CFBooleanGetValue(not_sync) != 0 {
            key = false;
        }
    }
    let config = if key { parameter_sets(CMSampleBufferGetFormatDescription(sample)) } else { None };
    Some((ms, key, config, annex_b(&bytes)?))
}

/// The format's SPS and PPS, each behind a start code.
unsafe fn parameter_sets(format: Ref) -> Option<Vec<u8>> {
    if format.is_null() {
        return None;
    }
    let mut count = 0usize;
    if CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, 0, ptr::null_mut(), ptr::null_mut(), &mut count, ptr::null_mut()) != 0 {
        return None;
    }
    let mut out = Vec::new();
    for i in 0..count {
        let (mut pointer, mut size) = (ptr::null::<u8>(), 0usize);
        if CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, i, &mut pointer, &mut size, ptr::null_mut(), ptr::null_mut()) != 0
            || pointer.is_null()
        {
            return None;
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(std::slice::from_raw_parts(pointer, size));
    }
    (!out.is_empty()).then_some(out)
}

/// NAL units behind four-byte lengths, as VideoToolbox writes them, put behind start codes.
pub fn annex_b(lengths: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(lengths.len() + 16);
    let mut at = 0usize;
    while at < lengths.len() {
        let size = u32::from_be_bytes(lengths.get(at..at + 4)?.try_into().ok()?) as usize;
        at += 4;
        let unit = lengths.get(at..at + size)?;
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(unit);
        at += size;
    }
    Some(out)
}

/// A recording's encoder: hardware H.264 of `buffers` pictures it gave out to be drawn in.
pub struct Encoder {
    session: Ref,
    buffers: Vec<Ref>,
    sink: Box<Sink>,
}

unsafe impl Send for Encoder {}

impl Encoder {
    /// An encoder of `width` x `height` pictures at `fps` and `bits_per_second`, a keyframe each
    /// second, with `buffers` BGRA `IOSurface`s to draw them in. What it makes comes out of the
    /// receiver, from VideoToolbox's own thread.
    pub fn new(width: u32, height: u32, fps: u32, bits_per_second: u32, buffers: usize) -> Result<(Encoder, Receiver<Encoded>)> {
        let (out, coded) = channel();
        let sink = Box::new(Sink { out: Mutex::new(out) });
        let mut session: Ref = ptr::null();
        unsafe {
            let specification =
                dictionary(&[kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder], &[kCFBooleanTrue]);
            let format = number(BGRA);
            let (w, h) = (number(width as i32), number(height as i32));
            let surface_properties = dictionary(&[], &[]);
            let attributes = dictionary(
                &[
                    kCVPixelBufferPixelFormatTypeKey,
                    kCVPixelBufferWidthKey,
                    kCVPixelBufferHeightKey,
                    kCVPixelBufferIOSurfacePropertiesKey,
                    kCVPixelBufferMetalCompatibilityKey,
                ],
                &[format, w, h, surface_properties, kCFBooleanTrue],
            );
            let status = VTCompressionSessionCreate(
                ptr::null(),
                width as i32,
                height as i32,
                AVC,
                specification,
                attributes,
                ptr::null(),
                encoded,
                &*sink as *const Sink as *mut c_void,
                &mut session,
            );
            for object in [specification, format, w, h, surface_properties, attributes] {
                CFRelease(object);
            }
            if status != 0 || session.is_null() {
                return Err(VideoError::new(format!(
                    "this Mac's hardware would not encode H.264 at {width}x{height} ({status})"
                )));
            }
        }
        let mut encoder = Encoder { session, buffers: Vec::new(), sink };
        unsafe {
            let (rate, bits, interval) = (number(fps as i32), number(bits_per_second as i32), number(fps as i32));
            for (key, value) in [
                (kVTCompressionPropertyKey_RealTime, kCFBooleanTrue),
                (kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_High_AutoLevel),
                (kVTCompressionPropertyKey_AverageBitRate, bits),
                (kVTCompressionPropertyKey_ExpectedFrameRate, rate),
                (kVTCompressionPropertyKey_MaxKeyFrameInterval, interval),
                // Pictures out in the order they went in, as the Beam Pro's: no B-frames, so the
                // file's blocks are in time order.
                (kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse),
            ] {
                let status = VTSessionSetProperty(session, key, value);
                if status != 0 {
                    log::warn!("recording: the encoder did not take a setting ({status})");
                }
            }
            for object in [rate, bits, interval] {
                CFRelease(object);
            }
            VTCompressionSessionPrepareToEncodeFrames(session);
            let pool = VTCompressionSessionGetPixelBufferPool(session);
            if pool.is_null() {
                return Err(VideoError::new("the encoder gave no buffers to draw in"));
            }
            for _ in 0..buffers {
                let mut buffer: Ref = ptr::null();
                let status = CVPixelBufferPoolCreatePixelBuffer(ptr::null(), pool, &mut buffer);
                if status != 0 || buffer.is_null() || CVPixelBufferGetIOSurface(buffer).is_null() {
                    return Err(VideoError::new(format!("the encoder's buffer is no IOSurface ({status})")));
                }
                encoder.buffers.push(buffer);
            }
        }
        Ok((encoder, coded))
    }

    /// The `IOSurface` of buffer `index`, held, to be drawn in.
    pub fn surface(&self, index: usize) -> Option<Surface> {
        let buffer = *self.buffers.get(index)?;
        unsafe {
            let surface = CVPixelBufferGetIOSurface(buffer);
            (!surface.is_null()).then(|| Surface::hold(surface))
        }
    }

    pub fn buffers(&self) -> usize {
        self.buffers.len()
    }

    /// Encode what is drawn in buffer `index`, stamped `ms`. The buffer is the encoder's until
    /// its picture comes out.
    pub fn encode(&mut self, index: usize, ms: i64) -> Result<()> {
        let Some(&buffer) = self.buffers.get(index) else {
            return Err(VideoError::new(format!("no buffer {index}")));
        };
        let time = Time { value: ms, timescale: 1000, flags: TIME_VALID, epoch: 0 };
        let status = unsafe {
            VTCompressionSessionEncodeFrame(self.session, buffer, time, INVALID, ptr::null(), index as *mut c_void, ptr::null_mut())
        };
        if status != 0 {
            return Err(VideoError::new(format!("the encoder would not take a picture ({status})")));
        }
        Ok(())
    }

    /// Every picture given is coded and has come out by the time this returns.
    pub fn finish(&mut self) {
        unsafe { VTCompressionSessionCompleteFrames(self.session, INVALID) };
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            VTCompressionSessionInvalidate(self.session);
            CFRelease(self.session);
            for buffer in self.buffers.drain(..) {
                CFRelease(buffer);
            }
        }
        // The callback is not called after the session is invalidated: the sink may go.
        let _ = &self.sink;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[link(name = "IOSurface", kind = "framework")]
    extern "C" {
        fn IOSurfaceLock(surface: Ref, options: u32, seed: *mut u32) -> i32;
        fn IOSurfaceUnlock(surface: Ref, options: u32, seed: *mut u32) -> i32;
        fn IOSurfaceGetBaseAddress(surface: Ref) -> *mut u8;
        fn IOSurfaceGetBytesPerRow(surface: Ref) -> usize;
    }

    #[test]
    fn lengths_become_start_codes() {
        let lengths = [0, 0, 0, 2, 0x65, 0xAA, 0, 0, 0, 1, 0x41];
        assert_eq!(annex_b(&lengths).unwrap(), vec![0, 0, 0, 1, 0x65, 0xAA, 0, 0, 0, 1, 0x41]);
        assert!(annex_b(&[0, 0, 0, 9, 1]).is_none(), "a unit that runs off the end is no unit");
    }

    /// The real hardware: side-by-side sized pictures in, H.264 out, every buffer back.
    #[test]
    fn the_hardware_encodes_what_is_drawn() {
        let (w, h) = (3840u32, 1080u32);
        let (mut encoder, coded) = Encoder::new(w, h, 36, 40_000_000, 3).expect("an encoder");
        let mut free: Vec<usize> = (0..encoder.buffers()).collect();
        let mut pictures = Vec::new();
        for frame in 0..40i64 {
            let index = loop {
                if let Some(i) = free.pop() {
                    break i;
                }
                match coded.recv_timeout(std::time::Duration::from_secs(2)).expect("a picture out") {
                    Encoded::Picture { buffer, ms, key, config, data } => {
                        free.push(buffer);
                        pictures.push((ms, key, config, data));
                    }
                    Encoded::Lost { status, .. } => panic!("lost a picture ({status})"),
                }
            };
            let surface = encoder.surface(index).unwrap();
            unsafe {
                IOSurfaceLock(surface.raw(), 0, ptr::null_mut());
                let base = IOSurfaceGetBaseAddress(surface.raw());
                let row = IOSurfaceGetBytesPerRow(surface.raw());
                for y in 0..h as usize {
                    let line = std::slice::from_raw_parts_mut(base.add(y * row), w as usize * 4);
                    for (x, px) in line.chunks_exact_mut(4).enumerate() {
                        px.copy_from_slice(&[(x as i64 + frame * 8) as u8, y as u8, 128, 255]);
                    }
                }
                IOSurfaceUnlock(surface.raw(), 0, ptr::null_mut());
            }
            encoder.encode(index, frame * 1000 / 36).unwrap();
        }
        encoder.finish();
        while let Ok(e) = coded.try_recv() {
            if let Encoded::Picture { ms, key, config, data, .. } = e {
                pictures.push((ms, key, config, data));
            }
        }
        assert_eq!(pictures.len(), 40, "every picture comes out");
        let (ms, key, config, data) = &pictures[0];
        assert_eq!(*ms, 0);
        assert!(*key, "the first is a keyframe");
        let config = config.as_ref().expect("with the parameter sets");
        assert_eq!(&config[..4], &[0, 0, 0, 1]);
        assert_eq!(config[4] & 0x1F, 7, "an SPS first");
        assert!(config.windows(5).any(|n| n[..4] == [0, 0, 0, 1] && n[4] & 0x1F == 8), "and a PPS");
        assert_eq!(&data[..4], &[0, 0, 0, 1]);
        assert!(pictures.windows(2).all(|p| p[0].0 < p[1].0), "in time order");
        assert!(pictures.iter().filter(|p| p.1).count() >= 2, "a keyframe each second");
    }
}
