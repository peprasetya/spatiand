//! The Mac's end of the fed engine: one Core Audio output unit, and the microphone in.
//!
//! Both are audio units talked to through their C interface, which is all of four calls each;
//! nothing here is worth a binding crate.

use std::ffi::c_void;
use std::sync::atomic::Ordering;

use super::Sink;

pub const NAME: &str = "Core Audio";

type Unit = *mut c_void;

#[repr(C)]
struct ComponentDescription {
    kind: u32,
    sub_kind: u32,
    manufacturer: u32,
    flags: u32,
    flags_mask: u32,
}

/// `AudioStreamBasicDescription`.
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Format {
    rate: f64,
    format: u32,
    flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels: u32,
    bits: u32,
    reserved: u32,
}

#[repr(C)]
struct Buffer {
    channels: u32,
    bytes: u32,
    data: *mut c_void,
}

/// `AudioBufferList` with its one buffer: everything here is interleaved.
#[repr(C)]
struct BufferList {
    count: u32,
    buffers: [Buffer; 1],
}

type RenderCallback = unsafe extern "C" fn(
    user: *mut c_void,
    flags: *mut u32,
    time: *const c_void,
    bus: u32,
    frames: u32,
    data: *mut BufferList,
) -> i32;

#[repr(C)]
struct CallbackStruct {
    callback: RenderCallback,
    user: *mut c_void,
}

#[repr(C)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    fn AudioComponentFindNext(after: *mut c_void, description: *const ComponentDescription) -> *mut c_void;
    fn AudioComponentInstanceNew(component: *mut c_void, out: *mut Unit) -> i32;
    fn AudioComponentInstanceDispose(unit: Unit) -> i32;
    fn AudioUnitSetProperty(unit: Unit, id: u32, scope: u32, element: u32, data: *const c_void, size: u32) -> i32;
    fn AudioUnitGetProperty(unit: Unit, id: u32, scope: u32, element: u32, data: *mut c_void, size: *mut u32) -> i32;
    fn AudioUnitInitialize(unit: Unit) -> i32;
    fn AudioUnitUninitialize(unit: Unit) -> i32;
    fn AudioOutputUnitStart(unit: Unit) -> i32;
    fn AudioOutputUnitStop(unit: Unit) -> i32;
    fn AudioUnitRender(unit: Unit, flags: *mut u32, time: *const c_void, bus: u32, frames: u32, data: *mut BufferList) -> i32;
}

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
}

const fn four(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

const OUTPUT: u32 = four(b"auou");
const DEFAULT_OUTPUT: u32 = four(b"def ");
const HAL_OUTPUT: u32 = four(b"ahal");
const APPLE: u32 = four(b"appl");
const LINEAR_PCM: u32 = four(b"lpcm");
const FLOAT_PACKED: u32 = 1 | 8;
const SIGNED_PACKED: u32 = 4 | 8;

const SCOPE_GLOBAL: u32 = 0;
const SCOPE_INPUT: u32 = 1;
const SCOPE_OUTPUT: u32 = 2;

const STREAM_FORMAT: u32 = 8;
const SET_RENDER_CALLBACK: u32 = 23;
const CURRENT_DEVICE: u32 = 2000;
const ENABLE_IO: u32 = 2003;
const SET_INPUT_CALLBACK: u32 = 2005;

const SYSTEM_OBJECT: u32 = 1;
const DEFAULT_INPUT_DEVICE: u32 = four(b"dIn ");
const GLOBAL: u32 = four(b"glob");

unsafe fn set<T>(unit: Unit, id: u32, scope: u32, element: u32, value: &T) -> i32 {
    AudioUnitSetProperty(unit, id, scope, element, value as *const T as *const c_void, std::mem::size_of::<T>() as u32)
}

unsafe fn new_unit(sub_kind: u32) -> Option<Unit> {
    let description = ComponentDescription { kind: OUTPUT, sub_kind, manufacturer: APPLE, flags: 0, flags_mask: 0 };
    let component = AudioComponentFindNext(std::ptr::null_mut(), &description);
    if component.is_null() {
        return None;
    }
    let mut unit: Unit = std::ptr::null_mut();
    (AudioComponentInstanceNew(component, &mut unit) == 0 && !unit.is_null()).then_some(unit)
}

/// One output unit, stopped and disposed of when dropped.
pub struct Output(Unit);

unsafe impl Send for Output {}

impl Output {
    /// Core Audio does not count them where this can read; the queues' own counts say enough.
    pub fn xruns(&self) -> i32 {
        0
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        unsafe {
            AudioOutputUnitStop(self.0);
            AudioUnitUninitialize(self.0);
            AudioComponentInstanceDispose(self.0);
        }
    }
}

unsafe extern "C" fn play(
    _user: *mut c_void,
    _flags: *mut u32,
    _time: *const c_void,
    _bus: u32,
    frames: u32,
    data: *mut BufferList,
) -> i32 {
    let buffer = &mut (*data).buffers[0];
    if buffer.data.is_null() {
        return 0;
    }
    let samples = (frames as usize * 2).min(buffer.bytes as usize / 4);
    super::mix(std::slice::from_raw_parts_mut(buffer.data as *mut f32, samples));
    0
}

/// Open the output: `device` is a Core Audio device id, or 0 for the system's default output --
/// which the unit then follows by itself when the default changes.
pub fn open_output(rate: u32, device: i32) -> Option<Output> {
    unsafe {
        let unit = new_unit(if device == 0 { DEFAULT_OUTPUT } else { HAL_OUTPUT })?;
        let output = Output(unit);
        if device != 0 {
            let id = device as u32;
            let status = set(unit, CURRENT_DEVICE, SCOPE_GLOBAL, 0, &id);
            if status != 0 {
                log::warn!("spatial audio: device {device} would not be chosen ({status})");
                return None;
            }
        }
        let format = Format {
            rate: rate as f64,
            format: LINEAR_PCM,
            flags: FLOAT_PACKED,
            bytes_per_packet: 8,
            frames_per_packet: 1,
            bytes_per_frame: 8,
            channels: 2,
            bits: 32,
            reserved: 0,
        };
        let callback = CallbackStruct { callback: play, user: std::ptr::null_mut() };
        let status = [
            set(unit, STREAM_FORMAT, SCOPE_INPUT, 0, &format),
            set(unit, SET_RENDER_CALLBACK, SCOPE_INPUT, 0, &callback),
            AudioUnitInitialize(unit),
            AudioOutputUnitStart(unit),
        ];
        if let Some(bad) = status.iter().find(|s| **s != 0) {
            log::warn!("spatial audio: could not open an output ({bad})");
            return None;
        }
        log::info!(
            "spatial audio: playing at {rate} Hz to {}",
            if device == 0 { "the default output".to_string() } else { format!("device {device}") }
        );
        Some(output)
    }
}

// --- the microphone ---

struct Recording {
    unit: Unit,
    sink: Sink,
    channels: usize,
    /// The device's own rate over the one asked for: how far to step through what it gives.
    step: f64,
    /// Where in the device's samples the next one handed on falls, carried between callbacks.
    phase: f64,
    scratch: Vec<i16>,
    out: Vec<i16>,
}

/// The microphone, being recorded: 16-bit samples handed to a sink as they arrive, on Core
/// Audio's own thread. Stops when dropped.
pub struct Capture(*mut Recording);

unsafe impl Send for Capture {}

unsafe extern "C" fn captured(
    user: *mut c_void,
    flags: *mut u32,
    time: *const c_void,
    bus: u32,
    frames: u32,
    _data: *mut BufferList,
) -> i32 {
    let recording = &mut *(user as *mut Recording);
    let channels = recording.channels;
    recording.scratch.resize(frames as usize * channels, 0);
    let mut list = BufferList {
        count: 1,
        buffers: [Buffer {
            channels: channels as u32,
            bytes: (recording.scratch.len() * 2) as u32,
            data: recording.scratch.as_mut_ptr() as *mut c_void,
        }],
    };
    if AudioUnitRender(recording.unit, flags, time, bus, frames, &mut list) != 0 {
        return 0;
    }
    if (recording.step - 1.0).abs() < 1e-6 {
        (recording.sink)(&recording.scratch);
        return 0;
    }
    // The unit does not change the rate of what it records, so that is done here: the nearest
    // sample is plenty for a voice, and is what stays in step without a filter's delay.
    recording.out.clear();
    let frames = frames as usize;
    while (recording.phase as usize) < frames {
        let at = recording.phase as usize * channels;
        recording.out.extend_from_slice(&recording.scratch[at..at + channels]);
        recording.phase += recording.step;
    }
    recording.phase -= frames as f64;
    (recording.sink)(&recording.out);
    0
}

impl Capture {
    /// Record `channels` at `rate` from the chosen input, or the system's default one.
    pub fn open(rate: u32, channels: u16, sink: Sink) -> Result<Capture, String> {
        unsafe {
            let unit = new_unit(HAL_OUTPUT).ok_or("no audio unit to record with")?;
            let fail = |unit: Unit, what: &str, status: i32| -> String {
                AudioComponentInstanceDispose(unit);
                format!("the microphone would not open ({what}: {status}); has Spatiand been allowed to use it?")
            };
            let (on, off) = (1u32, 0u32);
            let status = set(unit, ENABLE_IO, SCOPE_INPUT, 1, &on);
            if status != 0 {
                return Err(fail(unit, "input", status));
            }
            set(unit, ENABLE_IO, SCOPE_OUTPUT, 0, &off);

            let mut device = super::INPUT_DEVICE.load(Ordering::SeqCst) as u32;
            if device == 0 {
                let address = PropertyAddress { selector: DEFAULT_INPUT_DEVICE, scope: GLOBAL, element: 0 };
                let mut size = 4u32;
                AudioObjectGetPropertyData(SYSTEM_OBJECT, &address, 0, std::ptr::null(), &mut size, &mut device as *mut u32 as *mut c_void);
            }
            let status = set(unit, CURRENT_DEVICE, SCOPE_GLOBAL, 0, &device);
            if status != 0 {
                return Err(fail(unit, "device", status));
            }

            // What the device records at, which is the only rate the unit will hand over.
            let mut native = Format::default();
            let mut size = std::mem::size_of::<Format>() as u32;
            AudioUnitGetProperty(unit, STREAM_FORMAT, SCOPE_INPUT, 1, &mut native as *mut Format as *mut c_void, &mut size);
            let native_rate = if native.rate > 0.0 { native.rate } else { rate as f64 };
            let format = Format {
                rate: native_rate,
                format: LINEAR_PCM,
                flags: SIGNED_PACKED,
                bytes_per_packet: 2 * channels as u32,
                frames_per_packet: 1,
                bytes_per_frame: 2 * channels as u32,
                channels: channels as u32,
                bits: 16,
                reserved: 0,
            };
            let status = set(unit, STREAM_FORMAT, SCOPE_OUTPUT, 1, &format);
            if status != 0 {
                return Err(fail(unit, "format", status));
            }
            let recording = Box::into_raw(Box::new(Recording {
                unit,
                sink,
                channels: channels as usize,
                step: native_rate / rate as f64,
                phase: 0.0,
                scratch: Vec::new(),
                out: Vec::new(),
            }));
            let callback = CallbackStruct { callback: captured, user: recording as *mut c_void };
            let status = [
                set(unit, SET_INPUT_CALLBACK, SCOPE_GLOBAL, 0, &callback),
                AudioUnitInitialize(unit),
                AudioOutputUnitStart(unit),
            ];
            if let Some(bad) = status.iter().find(|s| **s != 0) {
                drop(Box::from_raw(recording));
                return Err(fail(unit, "start", *bad));
            }
            log::info!("microphone: recording {channels} channel(s), {native_rate} Hz as {rate} Hz");
            Ok(Capture(recording))
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        unsafe {
            let recording = Box::from_raw(self.0);
            AudioOutputUnitStop(recording.unit);
            AudioUnitUninitialize(recording.unit);
            AudioComponentInstanceDispose(recording.unit);
        }
    }
}
