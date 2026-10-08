//! One remote window's decoder on the Mac: VideoToolbox, asked for BGRA `IOSurface`s.
//!
//! The host sends each picture as one Annex B access unit: NAL units behind start codes, with
//! the parameter sets in front of every keyframe. VideoToolbox wants the other shape: a format
//! description made once from those parameter sets, and samples whose NAL units are each behind
//! a four-byte length. That conversion is the first half of this file.

use std::ffi::c_void;
use std::ptr;
use std::sync::{Arc, Mutex};

use spatiand_stream::Codec;

use super::{Device, Output, Picture, Result, Surface, VideoError};

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

const INVALID: Time = Time { value: 0, timescale: 0, flags: 0, epoch: 0 };

#[repr(C)]
struct Timing {
    duration: Time,
    presentation: Time,
    decode: Time,
}

type OutputCallback = unsafe extern "C" fn(
    user: *mut c_void,
    frame: *mut c_void,
    status: i32,
    flags: u32,
    image: Ref,
    presentation: Time,
    duration: Time,
);

#[repr(C)]
struct CallbackRecord {
    callback: OutputCallback,
    user: *mut c_void,
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    static kCFBooleanTrue: Ref;
    fn CFRelease(object: Ref);
    fn CFDictionaryCreate(
        allocator: Ref,
        keys: *const Ref,
        values: *const Ref,
        count: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> Ref;
    fn CFNumberCreate(allocator: Ref, kind: isize, value: *const c_void) -> Ref;
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
        allocator: Ref,
        count: usize,
        pointers: *const *const u8,
        sizes: *const usize,
        length_bytes: i32,
        out: *mut Ref,
    ) -> i32;
    fn CMVideoFormatDescriptionCreateFromHEVCParameterSets(
        allocator: Ref,
        count: usize,
        pointers: *const *const u8,
        sizes: *const usize,
        length_bytes: i32,
        extensions: Ref,
        out: *mut Ref,
    ) -> i32;
    fn CMBlockBufferCreateWithMemoryBlock(
        allocator: Ref,
        memory: *mut c_void,
        length: usize,
        block_allocator: Ref,
        source: *const c_void,
        offset: usize,
        data_length: usize,
        flags: u32,
        out: *mut Ref,
    ) -> i32;
    fn CMBlockBufferReplaceDataBytes(source: *const c_void, buffer: Ref, offset: usize, length: usize) -> i32;
    fn CMSampleBufferCreateReady(
        allocator: Ref,
        data: Ref,
        format: Ref,
        samples: isize,
        timings: isize,
        timing: *const Timing,
        sizes: isize,
        size: *const usize,
        out: *mut Ref,
    ) -> i32;
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: Ref;
    static kCVPixelBufferIOSurfacePropertiesKey: Ref;
    static kCVPixelBufferMetalCompatibilityKey: Ref;
    fn CVPixelBufferGetIOSurface(buffer: Ref) -> Ref;
    fn CVPixelBufferGetWidth(buffer: Ref) -> usize;
    fn CVPixelBufferGetHeight(buffer: Ref) -> usize;
}

#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    fn VTDecompressionSessionCreate(
        allocator: Ref,
        format: Ref,
        decoder: Ref,
        image_attributes: Ref,
        callback: *const CallbackRecord,
        out: *mut Ref,
    ) -> i32;
    fn VTDecompressionSessionDecodeFrame(session: Ref, sample: Ref, flags: u32, frame: *mut c_void, info: *mut u32) -> i32;
    fn VTDecompressionSessionInvalidate(session: Ref);
    fn VTIsHardwareDecodeSupported(codec: u32) -> u8;
}

const BGRA: i32 = i32::from_be_bytes(*b"BGRA");
const NUMBER_SINT32: isize = 3;

/// Whether this Mac decodes the codec on its GPU. In software a window's worth of pictures
/// costs a core, so a codec it only has in software is one to say no to.
pub fn can_decode(codec: Codec) -> bool {
    let kind = match codec {
        Codec::H264 => return true,
        Codec::H265 => u32::from_be_bytes(*b"hvc1"),
        Codec::Av1 => u32::from_be_bytes(*b"av01"),
    };
    unsafe { VTIsHardwareDecodeSupported(kind) != 0 }
}

/// Split an Annex B access unit into its NAL units, start codes removed.
pub(crate) fn nal_units(data: &[u8]) -> Vec<&[u8]> {
    let mut units = Vec::new();
    let mut start: Option<usize> = None;
    let mut i = 0;
    while i + 2 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            if let Some(s) = start {
                // A four-byte start code leaves a zero on the end of the unit before it.
                let mut end = i;
                while end > s && data[end - 1] == 0 {
                    end -= 1;
                }
                units.push(&data[s..end]);
            }
            start = Some(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    if let Some(s) = start {
        if s < data.len() {
            units.push(&data[s..]);
        }
    }
    units
}

/// Whether a NAL unit is a parameter set: what the format is made from, and not a picture.
pub(crate) fn is_parameter_set(nal: &[u8], codec: Codec) -> bool {
    let Some(first) = nal.first() else { return false };
    match codec {
        Codec::H265 => (32..=34).contains(&((first >> 1) & 0x3f)),
        _ => matches!(first & 0x1f, 7 | 8),
    }
}

/// What the decoder's callback writes into: shared with it by pointer for the session's life.
struct Sink {
    output: Arc<Output>,
    /// Pictures finished and not yet handed to the session: their sizes and capture times.
    finished: Mutex<Vec<(u32, u32, i64)>>,
    failed: Mutex<Option<i32>>,
}

unsafe extern "C" fn decoded(
    user: *mut c_void,
    frame: *mut c_void,
    status: i32,
    _flags: u32,
    image: Ref,
    _presentation: Time,
    _duration: Time,
) {
    let sink = &*(user as *const Sink);
    if status != 0 || image.is_null() {
        *sink.failed.lock().unwrap() = Some(status);
        return;
    }
    let surface = CVPixelBufferGetIOSurface(image);
    if surface.is_null() {
        *sink.failed.lock().unwrap() = Some(-1);
        return;
    }
    let (width, height) = (CVPixelBufferGetWidth(image) as u32, CVPixelBufferGetHeight(image) as u32);
    sink.output.put(Surface::hold(surface), [0.0, 0.0, 1.0, 1.0]);
    // The capture time came through as the frame's own word, which needs no clock to agree.
    sink.finished.lock().unwrap().push((width, height, frame as usize as i64));
}

/// One remote window's decoder.
pub struct Decoder {
    codec: Codec,
    session: Ref,
    format: Ref,
    parameter_sets: Vec<Vec<u8>>,
    sink: Box<Sink>,
}

unsafe impl Send for Decoder {}

impl Decoder {
    /// Open a decoder for `codec`. The node is the Deck's render node, meaningless here; the
    /// session itself is made when the first keyframe says what the pictures are.
    pub fn new(_node: &str, codec: Codec) -> Result<Decoder> {
        if !can_decode(codec) {
            return Err(VideoError::new(format!("this Mac cannot decode {}", codec.label())));
        }
        Ok(Decoder {
            codec,
            session: ptr::null(),
            format: ptr::null(),
            parameter_sets: Vec::new(),
            sink: Box::new(Sink {
                output: Output::new(),
                finished: Mutex::new(Vec::new()),
                failed: Mutex::new(None),
            }),
        })
    }

    fn close(&mut self) {
        unsafe {
            if !self.session.is_null() {
                VTDecompressionSessionInvalidate(self.session);
                CFRelease(self.session);
            }
            if !self.format.is_null() {
                CFRelease(self.format);
            }
        }
        self.session = ptr::null();
        self.format = ptr::null();
    }

    /// Make the format and the session for a stream with these parameter sets.
    fn open(&mut self, sets: &[&[u8]]) -> Result<()> {
        self.close();
        let pointers: Vec<*const u8> = sets.iter().map(|s| s.as_ptr()).collect();
        let sizes: Vec<usize> = sets.iter().map(|s| s.len()).collect();
        let mut format: Ref = ptr::null();
        let status = unsafe {
            match self.codec {
                Codec::H265 => CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                    ptr::null(),
                    sets.len(),
                    pointers.as_ptr(),
                    sizes.as_ptr(),
                    4,
                    ptr::null(),
                    &mut format,
                ),
                _ => CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    ptr::null(),
                    sets.len(),
                    pointers.as_ptr(),
                    sizes.as_ptr(),
                    4,
                    &mut format,
                ),
            }
        };
        if status != 0 || format.is_null() {
            return Err(VideoError::new(format!("the stream's parameter sets made no format ({status})")));
        }
        self.format = format;

        let session = unsafe {
            let number = CFNumberCreate(ptr::null(), NUMBER_SINT32, &BGRA as *const i32 as *const c_void);
            let empty = CFDictionaryCreate(
                ptr::null(),
                ptr::null(),
                ptr::null(),
                0,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            let keys = [
                kCVPixelBufferPixelFormatTypeKey,
                kCVPixelBufferIOSurfacePropertiesKey,
                kCVPixelBufferMetalCompatibilityKey,
            ];
            let values = [number, empty, kCFBooleanTrue];
            let attributes = CFDictionaryCreate(
                ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                3,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            let record = CallbackRecord {
                callback: decoded,
                user: &*self.sink as *const Sink as *mut c_void,
            };
            let mut session: Ref = ptr::null();
            let status = VTDecompressionSessionCreate(ptr::null(), format, ptr::null(), attributes, &record, &mut session);
            CFRelease(attributes);
            CFRelease(empty);
            CFRelease(number);
            if status != 0 || session.is_null() {
                return Err(VideoError::new(format!("the {} decoder would not open ({status})", self.codec.label())));
            }
            session
        };
        self.session = session;
        self.parameter_sets = sets.iter().map(|s| s.to_vec()).collect();
        log::info!("decoder: {} on VideoToolbox", self.codec.label());
        Ok(())
    }

    /// Give the decoder one whole frame, and say which pictures it has finished since.
    pub fn decode(&mut self, timestamp: i64, frame: &[u8]) -> Result<Vec<Picture>> {
        let nals = nal_units(frame);
        let sets: Vec<&[u8]> = nals.iter().copied().filter(|n| is_parameter_set(n, self.codec)).collect();
        if !sets.is_empty() && (self.session.is_null() || !sets.iter().copied().eq(self.parameter_sets.iter().map(|s| &s[..]))) {
            self.open(&sets)?;
        }
        if self.session.is_null() {
            // Before the first keyframe there is nothing to decode with.
            return Ok(Vec::new());
        }

        // Length-prefixed, parameter sets left out: they live in the format.
        let mut sample = Vec::with_capacity(frame.len() + 16);
        for nal in nals.iter().filter(|n| !is_parameter_set(n, self.codec)) {
            sample.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            sample.extend_from_slice(nal);
        }
        if sample.is_empty() {
            return Ok(Vec::new());
        }
        let status = unsafe {
            let mut block: Ref = ptr::null();
            let made = CMBlockBufferCreateWithMemoryBlock(
                ptr::null(),
                ptr::null_mut(),
                sample.len(),
                ptr::null(),
                ptr::null(),
                0,
                sample.len(),
                0,
                &mut block,
            );
            if made != 0 || block.is_null() {
                return Err(VideoError::new(format!("no memory for a frame ({made})")));
            }
            let copied = CMBlockBufferReplaceDataBytes(sample.as_ptr() as *const c_void, block, 0, sample.len());
            let timing = Timing {
                duration: INVALID,
                presentation: Time { value: timestamp.max(0), timescale: 1_000_000, flags: 1, epoch: 0 },
                decode: INVALID,
            };
            let size = sample.len();
            let mut buffer: Ref = ptr::null();
            let ready = if copied == 0 {
                CMSampleBufferCreateReady(ptr::null(), block, self.format, 1, 1, &timing, 1, &size, &mut buffer)
            } else {
                copied
            };
            CFRelease(block);
            if ready != 0 || buffer.is_null() {
                return Err(VideoError::new(format!("a frame could not be wrapped ({ready})")));
            }
            // No flags: decoded before this returns, so the picture is out as soon as it can be
            // and the callback never outlives the decoder.
            let status = VTDecompressionSessionDecodeFrame(
                self.session,
                buffer,
                0,
                timestamp as usize as *mut c_void,
                ptr::null_mut(),
            );
            CFRelease(buffer);
            status
        };
        let failed = self.sink.failed.lock().unwrap().take();
        if status != 0 || failed.is_some() {
            // A session that has lost its place is no use until it is made again from a keyframe.
            self.close();
            return Err(VideoError::new(format!(
                "the decoder refused a frame ({})",
                failed.unwrap_or(status)
            )));
        }
        Ok(self.finished())
    }

    /// Whether a frame was given and its picture has not come out yet. Never: see `decode`.
    pub fn waiting(&self) -> bool {
        false
    }

    /// Pictures finished since the last look.
    pub fn finished(&mut self) -> Vec<Picture> {
        let finished = std::mem::take(&mut *self.sink.finished.lock().unwrap());
        finished
            .into_iter()
            .map(|(width, height, timestamp)| Picture {
                width,
                height,
                fourcc: 0,
                modifier: 0,
                planes: Vec::new(),
                timestamp,
                output: self.sink.output.clone(),
            })
            .collect()
    }

    pub fn device(&self) -> (Device, Device) {
        (Device, Device)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_access_unit_splits_at_three_and_four_byte_start_codes() {
        let unit = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 4, 5];
        let nals = nal_units(&unit);
        assert_eq!(nals, vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 4, 5][..]]);
    }

    #[test]
    fn parameter_sets_are_told_from_pictures() {
        assert!(is_parameter_set(&[0x67], Codec::H264));
        assert!(is_parameter_set(&[0x68], Codec::H264));
        assert!(!is_parameter_set(&[0x65], Codec::H264));
        // HEVC: VPS 32, SPS 33, PPS 34 in the six bits after the first.
        assert!(is_parameter_set(&[32 << 1, 1], Codec::H265));
        assert!(is_parameter_set(&[34 << 1, 1], Codec::H265));
        assert!(!is_parameter_set(&[19 << 1, 1], Codec::H265));
    }
}
