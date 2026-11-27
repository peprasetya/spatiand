//! The engine: FFmpeg, with the platform's own decoder under it.
//!
//! Where the Vibrefy port had libmpv, which decided everything itself, this is FFmpeg's
//! libraries and the deciding done here -- because each platform's decoder has to be asked
//! for by name (VideoToolbox, VAAPI, MediaCodec), and because what comes out has to stay on
//! the GPU all the way to the room, which mpv's own renderer does not let it.
//!
//! Three threads, as in every player built on these libraries. One reads the file and sorts
//! its packets into two queues; one decodes pictures; one decodes sound. The sound's clock
//! is the film's clock -- a picture is shown when the sound that goes with it is heard -- and
//! a film with no sound keeps time by the wall.
//!
//! A seek does not stop anything. It moves the reader, and bumps a number (`serial`) that
//! every packet, picture and block of sound carries: whatever still has the old number is
//! from before the seek, and is thrown away wherever it is found.

use std::collections::VecDeque;
use std::ffi::{c_int, c_void, CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rsmpeg::ffi;

use crate::hints::{ContainerHints, ContainerStereo};
use crate::sound::{Block, Sound};
use crate::subs::{self, Cue, Words};
use crate::timing::{self, Landing};

/// How many decoded pictures wait to be shown. Each is a frame of GPU or main memory, and a
/// 4K one is not small: enough to ride out an uneven decode, no more.
const PICTURES_AHEAD: usize = 4;
/// How many pictures are held outside a decoder or a deinterlacer on the GPU, which its pool
/// must keep room for: those waiting to be shown, the one on show and the one the screen lets
/// go of.
pub(crate) const PICTURES_HELD: c_int = PICTURES_AHEAD as c_int + 3;
/// How much of the film the reader keeps ahead of the decoders, in the film's seconds: enough
/// to ride out a network's stumble, no more. Every second of it is a second of the film held in
/// memory and, from a server, fetched at whatever pace the network allows; it was "512 packets
/// or 24 MB", which from a server was the line flat out for many seconds after every pause and
/// seek, and a second film in the room as much again.
const SECONDS_AHEAD: f64 = 4.0;
/// Paused, nothing more is fetched: what is in hand is plenty to start again from.
const SECONDS_AHEAD_PAUSED: f64 = 0.5;
/// Bounds whatever the seconds say, for a file whose packets carry no times.
const PACKETS_AHEAD_BYTES: usize = 24 * 1024 * 1024;
const PACKETS_AHEAD: usize = 512;
/// The most a network stream's connection holds that the reader has not asked for. The
/// system's own grows to many megabytes, all of it fetched whether wanted or not.
const NETWORK_BUFFER: &str = "1048576";

fn error_text(code: c_int) -> String {
    let mut buffer = [0 as std::ffi::c_char; 128];
    unsafe {
        ffi::av_strerror(code, buffer.as_mut_ptr(), buffer.len());
        CStr::from_ptr(buffer.as_ptr()).to_string_lossy().into_owned()
    }
}

/// How a picture's memory is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pixels {
    /// Three planes, eight bits: Y, then U and V at half size each way.
    Planar420,
    /// Three planes like [`Pixels::Planar420`], ten bits in sixteen: what an HDR film is.
    Planar420Ten,
    /// Two planes, eight bits: Y, then U and V interleaved at half size.
    Nv12,
    /// A `CVPixelBuffer` from VideoToolbox, two planes like [`Pixels::Nv12`], on the GPU.
    VideoToolbox,
    /// A ten-bit `CVPixelBuffer` from VideoToolbox: two planes like [`Pixels::VideoToolbox`],
    /// sixteen bits a sample, the ten in the top of them (P010), as [`Pixels::Dmabuf10`] is.
    VideoToolbox10,
    /// A VAAPI surface lent as dmabufs, two planes like [`Pixels::Nv12`], on the GPU.
    Dmabuf,
    /// A ten-bit VAAPI surface lent as dmabufs: two planes like [`Pixels::Dmabuf`], sixteen
    /// bits a sample, the ten in the top of them (P010).
    Dmabuf10,
    /// An Android hardware buffer MediaCodec drew into, YUV in the GPU's own layout, sampled
    /// through the GPU's own conversion (`crate::mediacodec`).
    HardwareBuffer,
}

/// One plane of a [`Pixels::Dmabuf`] picture: where it is in which descriptor.
#[derive(Debug, Clone, Copy)]
pub struct DmabufPlane {
    pub fd: std::os::fd::RawFd,
    pub offset: u32,
    pub pitch: u32,
    pub modifier: u64,
}

// `libavutil/hwcontext_drm.h`, which the bindings do not carry.
#[repr(C)]
struct DrmObject {
    fd: c_int,
    size: usize,
    format_modifier: u64,
}
#[repr(C)]
struct DrmPlane {
    object_index: c_int,
    offset: isize,
    pitch: isize,
}
#[repr(C)]
struct DrmLayer {
    format: u32,
    nb_planes: c_int,
    planes: [DrmPlane; 4],
}
#[repr(C)]
struct DrmFrame {
    nb_objects: c_int,
    objects: [DrmObject; 4],
    nb_layers: c_int,
    layers: [DrmLayer; 4],
}

/// How a picture's numbers become colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colour {
    /// BT.709's matrix rather than BT.601's.
    pub hd: bool,
    /// BT.2020's, which outranks both.
    pub wide: bool,
    /// The whole range of a byte, rather than broadcast's 16 to 235.
    pub full_range: bool,
    /// How its numbers stand for light: an ordinary film's gamma, or one of HDR's two curves.
    pub light: Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Light {
    #[default]
    Ordinary,
    /// SMPTE 2084, "PQ": HDR10, and the layer of Dolby Vision that HDR10 players can show.
    Pq,
    /// Hybrid log-gamma: broadcast HDR.
    Hlg,
}

/// One decoded picture, held until it is dropped.
pub struct Picture {
    frame: *mut ffi::AVFrame,
    pub pixels: Pixels,
    pub width: u32,
    pub height: u32,
    /// When it is to be shown, in seconds from the start of the film.
    pub time: f64,
    pub colour: Colour,
    /// Two fields woven, which nothing has made whole: to be drawn with them blended
    /// (`crate::deinterlace`).
    pub fields: bool,
    serial: u64,
    /// A [`Pixels::HardwareBuffer`] picture's place in its reader, given back when dropped.
    #[cfg(target_os = "android")]
    image: Option<crate::mediacodec::Image>,
}

// SAFETY: an AVFrame's buffers are reference counted with atomics, and nothing here writes
// to one after it is made.
unsafe impl Send for Picture {}
unsafe impl Sync for Picture {}

impl Picture {
    /// One plane's bytes and how many of them make a row.
    pub fn plane(&self, index: usize) -> (&[u8], usize) {
        unsafe {
            let frame = &*self.frame;
            let stride = frame.linesize[index] as usize;
            let rows = if index == 0 { self.height as usize } else { (self.height as usize).div_ceil(2) };
            (std::slice::from_raw_parts(frame.data[index], stride * rows), stride)
        }
    }

    /// The planes of a [`Pixels::Dmabuf`] picture, brightness first: however the driver
    /// grouped them into layers, they come out in the order they are in the picture.
    pub fn dmabuf_planes(&self) -> Vec<DmabufPlane> {
        if !matches!(self.pixels, Pixels::Dmabuf | Pixels::Dmabuf10) {
            return Vec::new();
        }
        unsafe {
            let described = &*((*self.frame).data[0] as *const DrmFrame);
            let mut planes = Vec::new();
            for layer in described.layers.iter().take(described.nb_layers.clamp(0, 4) as usize) {
                for plane in layer.planes.iter().take(layer.nb_planes.clamp(0, 4) as usize) {
                    let Some(object) = described.objects.get(plane.object_index.max(0) as usize) else { continue };
                    planes.push(DmabufPlane { fd: object.fd, offset: plane.offset as u32, pitch: plane.pitch as u32, modifier: object.format_modifier });
                }
            }
            planes
        }
    }

    /// The `CVPixelBufferRef`, for a [`Pixels::VideoToolbox`] or [`Pixels::VideoToolbox10`]
    /// picture.
    pub fn pixel_buffer(&self) -> *const c_void {
        unsafe { (*self.frame).data[3] as *const c_void }
    }

    /// The `AHardwareBuffer` of a [`Pixels::HardwareBuffer`] picture, and the part of it that is
    /// picture (left, top, right, bottom, each 0 to 1).
    #[cfg(target_os = "android")]
    pub fn hardware_buffer(&self) -> Option<(*mut c_void, [f32; 4])> {
        self.image.as_ref().map(|image| (image.buffer as *mut c_void, image.crop))
    }
}

impl Drop for Picture {
    fn drop(&mut self) {
        unsafe { ffi::av_frame_free(&mut self.frame) };
    }
}

/// A sound track a file offers.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    /// The stream's number in the file, which choosing it names.
    pub stream: usize,
    pub codec: String,
    pub profile: Option<String>,
    pub channels: u16,
    pub language: Option<String>,
    pub title: Option<String>,
}

/// What a file turned out to be.
#[derive(Debug, Clone, Default)]
pub struct Info {
    pub duration: f64,
    /// `duration` is judged from the file's size and what has been read of it, and changes as
    /// more is read: a file that keeps no times (`crate::timing`).
    pub duration_guessed: bool,
    pub width: u32,
    pub height: u32,
    /// How wide each of the frame's pixels is, over how tall: 1 for square ones, and 0 where
    /// not known (taken as 1). A DVD's 720 x 576 frame shown at 16:9 is 64/45.
    pub sample_aspect: f64,
    /// How the frame is to be turned and mirrored to be shown the right way up: the stream's
    /// display matrix (a phone's film taken upright is stored on its side). `width` and
    /// `height` are the frame's as stored.
    pub upright: crate::still::Upright,
    pub frame_rate: f64,
    pub video_codec: String,
    /// The platform's decoder is doing the work, and its name.
    pub hardware: Option<&'static str>,
    pub hints: ContainerHints,
    /// The container says the right eye is first. A starting position for a control, not a
    /// fact: see [`ContainerStereo::from_ffmpeg`].
    pub eyes_swapped: bool,
    pub audio: Vec<Track>,
    /// Which of `audio` is playing.
    pub audio_playing: Option<usize>,
    /// The subtitles the file carries. `channels` means nothing for these.
    pub subtitles: Vec<Track>,
    /// Which of `subtitles` is on show, if any.
    pub subtitle_showing: Option<usize>,
}

struct Packet(*mut ffi::AVPacket);
unsafe impl Send for Packet {}
impl Drop for Packet {
    fn drop(&mut self) {
        unsafe { ffi::av_packet_free(&mut self.0) };
    }
}

/// Packets waiting for a decoder. `None` is the end of the file: drain what is in hand.
#[derive(Default)]
struct Queue {
    packets: Mutex<(VecDeque<(Option<Packet>, u64)>, usize)>,
    /// The film's time of each packet in `packets`, in the same order: NaN where it has none.
    times: Mutex<VecDeque<f64>>,
    arrived: Condvar,
}

impl Queue {
    fn push(&self, packet: Option<Packet>, serial: u64) {
        self.push_at(packet, serial, f64::NAN);
    }

    fn push_at(&self, packet: Option<Packet>, serial: u64, time: f64) {
        let mut held = self.packets.lock().unwrap();
        held.1 += packet.as_ref().map_or(0, |p| unsafe { (*p.0).size.max(0) as usize });
        held.0.push_back((packet, serial));
        self.times.lock().unwrap().push_back(time);
        self.arrived.notify_one();
    }

    /// How many seconds of the film are waiting: from the first packet with a time to the last.
    fn seconds(&self) -> f64 {
        let times = self.times.lock().unwrap();
        let first = times.iter().copied().find(|t| t.is_finite());
        let last = times.iter().rev().copied().find(|t| t.is_finite());
        match (first, last) {
            (Some(first), Some(last)) => (last - first).max(0.0),
            _ => 0.0,
        }
    }

    fn bytes(&self) -> usize {
        self.packets.lock().unwrap().1
    }

    fn clear(&self) {
        let mut held = self.packets.lock().unwrap();
        held.0.clear();
        held.1 = 0;
        self.times.lock().unwrap().clear();
    }

    fn crowded(&self) -> bool {
        self.packets.lock().unwrap().0.len() >= PACKETS_AHEAD
    }

    fn len(&self) -> usize {
        self.packets.lock().unwrap().0.len()
    }

    /// The next packet, waiting a little for one: `Err` is "nothing yet".
    fn pop(&self) -> Result<(Option<Packet>, u64), ()> {
        let mut held = self.packets.lock().unwrap();
        if held.0.is_empty() {
            held = self.arrived.wait_timeout(held, Duration::from_millis(20)).unwrap().0;
        }
        let (packet, serial) = held.0.pop_front().ok_or(())?;
        held.1 = held.1.saturating_sub(packet.as_ref().map_or(0, |p| unsafe { (*p.0).size.max(0) as usize }));
        self.times.lock().unwrap().pop_front();
        Ok((packet, serial))
    }
}

/// The film's time, when there is no sound to keep it.
struct Wall {
    /// The film's time at `since`.
    time: f64,
    since: Instant,
}

struct Shared {
    stop: AtomicBool,
    paused: AtomicBool,
    serial: AtomicU64,
    video: Queue,
    audio: Queue,
    pictures: Mutex<VecDeque<Picture>>,
    /// A picture was taken, or everything was thrown away: the decoder may make another.
    room: Condvar,
    sound: Option<Arc<Sound>>,
    wall: Mutex<Wall>,
    /// Where the reader has been asked to go.
    seek: Mutex<Option<f64>>,
    /// Where the last seek went, and whether a picture from after it has been shown yet.
    target: Mutex<(f64, bool)>,
    /// Which sound stream the reader passes on; changed by choosing a track.
    audio_stream: AtomicU64,
    /// How fast the film is played: 1 is as it was made.
    speed: std::sync::atomic::AtomicU32,
    /// Which subtitle stream the reader passes on, if any.
    subtitle_stream: AtomicU64,
    subtitle: Queue,
    /// The subtitles decoded so far since the last seek, in order of their starts.
    cues: Mutex<Vec<Arc<Cue>>>,
    /// The reader has reached the end of the file.
    read_out: AtomicBool,
    /// And each decoder has emptied what it was given since.
    video_out: AtomicBool,
    audio_out: AtomicBool,
    /// Which picture decoder is the one: one that finds it is not any more stops.
    video_generation: AtomicU64,
    /// When the picture decoder last got anywhere: see `Player::watch_decoder`.
    video_alive: Mutex<Instant>,
    /// The file keeps no times: the reader numbers its packets and the pictures' times are
    /// counted (`crate::timing`). Set by the reader before its first picture packet is given.
    counted: AtomicBool,
    /// Such a film's length as the reader has learnt it so far (an `f64`'s bits; 0 while it is
    /// not known), and whether that is still a guess.
    length: AtomicU64,
    length_guessed: AtomicBool,
    /// The film cannot be moved in: no times, and nowhere to jump to by bytes.
    fixed: AtomicBool,
    /// How many seeks have been asked for, and how many of them the reader has made (or given
    /// up on). Between the two, what is in hand is from before the move and is not shown.
    asked: AtomicU64,
    moved: AtomicU64,
    /// The picture stream's rate as the container has it, for where the codec states none.
    container_rate: (i32, i32),
    /// Since when, after a seek, no sound has been heard while every picture there is room for
    /// waits and nothing more of the sound is coming; and the seek's number + 1 once the film
    /// has gone over to the wall's time for it (see `Player::position`).
    unheard: Mutex<Option<Instant>>,
    walled: AtomicU64,
    /// Why the reader stopped short of the end, if it did: the file could not be read further.
    failed: Mutex<Option<String>>,
}

/// How long the pictures may wait for a sound that is not coming before the wall keeps time.
const UNHEARD: Duration = Duration::from_millis(500);

const NO_STREAM: u64 = u64::MAX;

/// When FFmpeg's own reading -- its HTTP, HLS, anything not fetched by `fetch` -- is to give
/// up: the film closed, or opening it taking too long. Without it a server that never answers
/// kept "Opening..." on screen for ever, and the reading thread waiting on it.
#[derive(Default)]
struct Interrupt {
    stop: AtomicBool,
    deadline: Mutex<Option<Instant>>,
}

/// How long opening a film may take, its index fetched and its streams looked at.
const OPENING: Duration = Duration::from_secs(30);
/// How long a read of FFmpeg's own network protocols may wait for a byte, in microseconds.
const NETWORK_WAIT: &str = "20000000";

unsafe extern "C" fn interrupted(opaque: *mut c_void) -> c_int {
    let interrupt = &*(opaque as *const Interrupt);
    let late = interrupt.deadline.lock().unwrap().is_some_and(|deadline| Instant::now() >= deadline);
    (interrupt.stop.load(Ordering::Relaxed) || late) as c_int
}

/// A film being played.
pub struct Player {
    shared: Arc<Shared>,
    info: Info,
    threads: Vec<JoinHandle<()>>,
    /// The picture on show, kept so it can be drawn again.
    showing: Option<Arc<Picture>>,
    /// How much later than its sound a picture is shown, in seconds: for a film whose two
    /// were put together out of step.
    lip: f64,
    /// While the platform's decoder makes the pictures: its stream and start, and its thread
    /// (an index into `threads`), for going over to software if it hangs.
    hardware: Option<(SendPointer<ffi::AVStream>, f64, usize)>,
    /// A film on a web server, being fetched.
    remote: Option<crate::fetch::Remote>,
    /// What FFmpeg's own reading is told to give up by.
    interrupt: Arc<Interrupt>,
}

/// What is opened, owned by the reader thread once it starts.
struct Source {
    format: *mut ffi::AVFormatContext,
    video_stream: Option<usize>,
    start: f64,
    /// What `format` reads through, where it is not FFmpeg's own: freed after it.
    io: Option<crate::fetch::Io>,
    /// What `format`'s interrupt callback reads, kept for as long as `format` is.
    _interrupt: Arc<Interrupt>,
}
unsafe impl Send for Source {}

struct Decoder {
    context: *mut ffi::AVCodecContext,
    time_base: f64,
    /// Where Android's decoder draws its pictures (`crate::mediacodec`), freed after the codec.
    #[cfg(target_os = "android")]
    reader: Option<Arc<crate::mediacodec::Reader>>,
}
unsafe impl Send for Decoder {}
impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { ffi::avcodec_free_context(&mut self.context) };
    }
}

/// The picture format the platform's decoder hands back, which `get_format` looks for.
#[cfg(target_os = "macos")]
const HARDWARE: Option<(ffi::AVHWDeviceType, ffi::AVPixelFormat, &str)> =
    Some((ffi::AV_HWDEVICE_TYPE_VIDEOTOOLBOX, ffi::AV_PIX_FMT_VIDEOTOOLBOX, "VideoToolbox"));
#[cfg(target_os = "linux")]
const HARDWARE: Option<(ffi::AVHWDeviceType, ffi::AVPixelFormat, &str)> = Some((ffi::AV_HWDEVICE_TYPE_VAAPI, ffi::AV_PIX_FMT_VAAPI, "VAAPI"));
// MediaCodec, through FFmpeg's decoders of that name, drawing into an AImageReader's hardware
// buffers (`crate::mediacodec`).
#[cfg(target_os = "android")]
const HARDWARE: Option<(ffi::AVHWDeviceType, ffi::AVPixelFormat, &str)> =
    Some((ffi::AV_HWDEVICE_TYPE_MEDIACODEC, ffi::AV_PIX_FMT_MEDIACODEC, "MediaCodec"));
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
const HARDWARE: Option<(ffi::AVHWDeviceType, ffi::AVPixelFormat, &str)> = None;

unsafe extern "C" fn choose_format(_context: *mut ffi::AVCodecContext, offered: *const ffi::AVPixelFormat) -> ffi::AVPixelFormat {
    let mut at = offered;
    if let Some((_, wanted, _)) = HARDWARE {
        while *at != ffi::AV_PIX_FMT_NONE {
            if *at == wanted {
                return wanted;
            }
            at = at.add(1);
        }
    }
    // The decoder's own first choice, which for one with no hardware behind it is software.
    *offered
}

/// Android's decoder for the stream, where the phone has one in hardware: FFmpeg's MediaCodec
/// decoder of the codec's name, and the reader it is to draw into.
#[cfg(target_os = "android")]
unsafe fn android_decoder(stream: *mut ffi::AVStream) -> Option<(*const ffi::AVCodec, Arc<crate::mediacodec::Reader>)> {
    let parameters = (*stream).codecpar;
    let name = text(ffi::avcodec_get_name((*parameters).codec_id))?;
    let (decoder, mime) = crate::mediacodec::decoder_for(&name)?;
    if !crate::mediacodec::hardware_decodes(mime) {
        log::info!("this phone has no hardware decoder for {name}");
        return None;
    }
    let named = CString::new(decoder).ok()?;
    let codec = ffi::avcodec_find_decoder_by_name(named.as_ptr());
    if codec.is_null() {
        log::warn!("FFmpeg was built without {decoder}");
        return None;
    }
    let size = ((*parameters).width.max(0) as u32, (*parameters).height.max(0) as u32);
    match crate::mediacodec::Reader::new(size) {
        Ok(reader) => Some((codec, reader)),
        Err(e) => {
            log::warn!("{e}");
            None
        }
    }
}

unsafe fn open_decoder(stream: *mut ffi::AVStream, hardware: bool) -> Result<(Decoder, bool), String> {
    let parameters = (*stream).codecpar;
    // On Android the hardware is a decoder of its own, not a mode of FFmpeg's.
    #[cfg(target_os = "android")]
    let android = if hardware && (*parameters).codec_type == ffi::AVMEDIA_TYPE_VIDEO { android_decoder(stream) } else { None };
    #[cfg(target_os = "android")]
    let codec = android.as_ref().map_or_else(|| ffi::avcodec_find_decoder((*parameters).codec_id), |(codec, _)| *codec);
    #[cfg(not(target_os = "android"))]
    let codec = ffi::avcodec_find_decoder((*parameters).codec_id);
    if codec.is_null() {
        return Err("FFmpeg has no decoder for this".into());
    }
    let mut context = ffi::avcodec_alloc_context3(codec);
    ffi::avcodec_parameters_to_context(context, parameters);
    (*context).pkt_timebase = (*stream).time_base;
    let mut on_gpu = false;
    #[cfg(target_os = "android")]
    let mut reader = None;
    #[cfg(target_os = "android")]
    if let Some((_, drawn_into)) = android {
        // A device whose window is the reader's: FFmpeg's own would have none, and the pictures
        // would be copied out of the codec.
        match crate::mediacodec::device(&drawn_into) {
            Ok(device) => {
                (*context).hw_device_ctx = device;
                (*context).get_format = Some(choose_format);
                on_gpu = true;
                reader = Some(drawn_into);
            }
            Err(e) => {
                ffi::avcodec_free_context(&mut context);
                return Err(e);
            }
        }
    }
    // Every codec the GPU decodes, on Linux too (the owner: video is decoded by the hardware,
    // whichever way). From 2026-10-13 to 11-17 Linux gave only H.264 to VA-API: an HEVC stream
    // (a Mac's VideoToolbox's, raw) hung the decoder of deepmagpie's GPU (Radeon 8060S, VCN 5)
    // and the kernel reset the GPU. That was never seen on the Deck, whose room decodes hosts'
    // HEVC on VA-API every day; on 11-17 the Deck decoded that same stream, HEVC 8 and 10 bit
    // (4K too), VP9 and AV1 through this player, with its kernel's log clean
    // (docs/spatiand-apps.md). `SPATIAND_MEDIA_HARDWARE=0` still decodes everything in software.
    if let (true, Some((kind, _, _))) = (hardware && !cfg!(target_os = "android") && (*parameters).codec_type == ffi::AVMEDIA_TYPE_VIDEO, HARDWARE) {
        let mut device = ptr::null_mut();
        if ffi::av_hwdevice_ctx_create(&mut device, kind, ptr::null(), ptr::null_mut(), 0) >= 0 {
            (*context).hw_device_ctx = device;
            (*context).get_format = Some(choose_format);
            // Room in the decoder's pool for the pictures held outside it: those waiting to be
            // shown, the one on show and the one the screen lets go of, as the room's own decoder
            // keeps room (spatiand-video). Without it VA-API's pool is only the codec's
            // references, and the held pictures take surfaces it needs.
            (*context).extra_hw_frames = PICTURES_HELD;
            on_gpu = true;
        }
    }
    if !on_gpu {
        // As many threads as there are cores: a software 4K decode needs all of them.
        (*context).thread_count = 0;
    }
    let opened = ffi::avcodec_open2(context, codec, ptr::null_mut());
    if opened < 0 {
        ffi::avcodec_free_context(&mut context);
        return Err(format!("the decoder would not open: {}", error_text(opened)));
    }
    let time_base = (*stream).time_base;
    Ok((
        Decoder {
            context,
            time_base: time_base.num as f64 / time_base.den.max(1) as f64,
            #[cfg(target_os = "android")]
            reader,
        },
        on_gpu,
    ))
}

unsafe fn text(value: *const std::ffi::c_char) -> Option<String> {
    (!value.is_null()).then(|| CStr::from_ptr(value).to_string_lossy().into_owned())
}

unsafe fn tag(dictionary: *mut ffi::AVDictionary, key: &CStr) -> Option<String> {
    let entry = ffi::av_dict_get(dictionary, key.as_ptr(), ptr::null(), 0);
    if entry.is_null() {
        None
    } else {
        text((*entry).value)
    }
}

/// What the container says about eyes and spheres.
unsafe fn read_hints(stream: *mut ffi::AVStream) -> (ContainerHints, bool) {
    let parameters = (*stream).codecpar;
    let mut hints = ContainerHints {
        frame_width: (*parameters).width.max(0) as u32,
        frame_height: (*parameters).height.max(0) as u32,
        ..ContainerHints::default()
    };
    let mut swapped = false;
    let side = |kind| ffi::av_packet_side_data_get((*parameters).coded_side_data, (*parameters).nb_coded_side_data, kind);
    let stereo = side(ffi::AV_PKT_DATA_STEREO3D);
    if !stereo.is_null() && !(*stereo).data.is_null() {
        let stereo = (*stereo).data as *const ffi::AVStereo3D;
        hints.stereo = ContainerStereo::from_ffmpeg((*stereo).type_ as u32);
        swapped = (*stereo).flags & ffi::AV_STEREO3D_FLAG_INVERT as c_int != 0;
    }
    let sphere = side(ffi::AV_PKT_DATA_SPHERICAL);
    if !sphere.is_null() && !(*sphere).data.is_null() {
        let sphere = (*sphere).data as *const ffi::AVSphericalMapping;
        hints.has_projection_data = true;
        // A whole sphere cut down by its bounds to the front half is how a VR180 file says
        // what it is: a tile whose left and right bounds each take a quarter of the turn.
        let projection = (*sphere).projection as u32;
        let quarter = u32::MAX / 4;
        hints.projection_is_half = projection == ffi::AV_SPHERICAL_HALF_EQUIRECTANGULAR as u32
            || (projection == ffi::AV_SPHERICAL_EQUIRECTANGULAR_TILE as u32
                && (*sphere).bound_left >= quarter / 2
                && (*sphere).bound_right >= quarter / 2);
    }
    (hints, swapped)
}

impl Player {
    /// Open a file or anything else FFmpeg can read by name, and start it playing.
    pub fn open(url: &str) -> Result<Player, String> {
        Player::open_with(url, &[])
    }

    /// As [`Player::open`], with what FFmpeg is to be told about how to fetch it: `headers`
    /// for a server that wants a cookie with every request, say.
    pub fn open_with(url: &str, options: &[(String, String)]) -> Result<Player, String> {
        let name = CString::new(url).map_err(|_| "a name with a nul in it".to_string())?;
        Player::open_named(name, url, options)
    }

    /// A file on this machine by the system's own name for it: one whose name is not UTF-8 (an
    /// old camera card's, a copied disc's) is opened as it is, where its name made text with
    /// replacement marks would be the name of nothing.
    pub fn open_file(path: &std::path::Path) -> Result<Player, String> {
        #[cfg(unix)]
        let name = {
            use std::os::unix::ffi::OsStrExt;
            CString::new(path.as_os_str().as_bytes())
        };
        #[cfg(not(unix))]
        let name = CString::new(path.to_string_lossy().into_owned());
        let name = name.map_err(|_| "a name with a nul in it".to_string())?;
        Player::open_named(name, &path.display().to_string(), &[])
    }

    /// Open `name`, called `url` where it is shown or looked at for what it is.
    fn open_named(name: CString, url: &str, options: &[(String, String)]) -> Result<Player, String> {
        crate::trusted_certificates();
        // The film's sound is an output of its own.
        let group = &crate::SoundGroup::new();
        unsafe {
            let mut told: *mut ffi::AVDictionary = ptr::null_mut();
            let network = url.starts_with("http://") || url.starts_with("https://");
            // A film on a web server is fetched as a browser fetches one (`fetch`), where it can
            // be; what cannot is left to FFmpeg's own protocols.
            let remote = if network { crate::fetch::Remote::open(url, options) } else { None };
            let io = remote.as_ref().map(|remote| remote.io());
            let interrupt = Arc::new(Interrupt::default());
            *interrupt.deadline.lock().unwrap() = Some(Instant::now() + OPENING);
            let mut format = ffi::avformat_alloc_context();
            (*format).interrupt_callback = ffi::AVIOInterruptCB { callback: Some(interrupted), opaque: Arc::as_ptr(&interrupt) as *mut c_void };
            if let Some(io) = &io {
                (*format).pb = io.context;
                (*format).flags |= ffi::AVFMT_FLAG_CUSTOM_IO as c_int;
            } else {
                let own: &[(&str, &str)] = if network { &[("recv_buffer_size", NETWORK_BUFFER), ("rw_timeout", NETWORK_WAIT)] } else { &[] };
                for (key, value) in own.iter().map(|(k, v)| (k.to_string(), v.to_string())).chain(options.iter().cloned()) {
                    if let (Ok(key), Ok(value)) = (CString::new(key.as_str()), CString::new(value.as_str())) {
                        ffi::av_dict_set(&mut told, key.as_ptr(), value.as_ptr(), 0);
                    }
                }
            }
            let opened = ffi::avformat_open_input(&mut format, name.as_ptr(), ptr::null(), &mut told);
            ffi::av_dict_free(&mut told);
            let why = |code: c_int| if code == ffi::AVERROR_EXIT { format!("no answer in {} s", OPENING.as_secs()) } else { error_text(code) };
            if opened < 0 {
                return Err(format!("could not open {url}: {}", why(opened)));
            }
            let found = ffi::avformat_find_stream_info(format, ptr::null_mut());
            if found < 0 {
                ffi::avformat_close_input(&mut format);
                return Err(format!("could not read {url}: {}", why(found)));
            }
            // Open: from here a stalled read gives up by `rw_timeout`, or when the film is closed.
            *interrupt.deadline.lock().unwrap() = None;
            let streams = std::slice::from_raw_parts((*format).streams, (*format).nb_streams as usize);
            let best = |kind, related: c_int| {
                let index = ffi::av_find_best_stream(format, kind, -1, related, ptr::null_mut(), 0);
                (index >= 0).then_some(index as usize)
            };
            let mut video_stream = best(ffi::AVMEDIA_TYPE_VIDEO, -1);
            // A file whose only pictures are those it carries of itself -- a song -- plays with
            // the cover a library shows it by (`cover`): its front cover, not whichever of a
            // front and a back FFmpeg would take, nor Matroska's landscape or small one.
            if let Some(index) = video_stream.filter(|&index| (*streams[index]).disposition & ffi::AV_DISPOSITION_ATTACHED_PIC as c_int != 0) {
                video_stream = crate::cover::chosen(format).or(Some(index));
            }
            let audio_stream = best(ffi::AVMEDIA_TYPE_AUDIO, video_stream.map_or(-1, |v| v as c_int));
            // The pictures' decoder: the platform's, or software's. Where neither will decode
            // them, the sound is still played if there is any; and if not, the file is closed.
            let mut pictures_decoder = None;
            if let Some(index) = video_stream {
                let stream = streams[index];
                // A song's cover is one picture, not a film, and is decoded in software as its
                // tile is (`cover`): AMD's VCN refused a 4:4:4 JPEG cover ("Unsupported sampling
                // factor"), and the song showed none while it played.
                let cover = (*stream).disposition & ffi::AV_DISPOSITION_ATTACHED_PIC as c_int != 0;
                let wanted = std::env::var("SPATIAND_MEDIA_HARDWARE").as_deref() != Ok("0") && !cover;
                let opened = open_decoder(stream, wanted).or_else(|e| {
                    log::warn!("{e}; trying without the platform's decoder");
                    open_decoder(stream, false)
                });
                match opened {
                    Ok(decoder) => pictures_decoder = Some(decoder),
                    Err(e) if audio_stream.is_none() => {
                        ffi::avformat_close_input(&mut format);
                        return Err(format!("{url}: its pictures cannot be decoded: {e}"));
                    }
                    Err(e) => {
                        log::warn!("{url}: its pictures cannot be decoded ({e}); its sound is played alone");
                        video_stream = None;
                    }
                }
            }

            let mut info = Info::default();
            let start = if (*format).start_time != ffi::AV_NOPTS_VALUE { (*format).start_time as f64 / ffi::AV_TIME_BASE as f64 } else { 0.0 };
            if (*format).duration != ffi::AV_NOPTS_VALUE {
                info.duration = (*format).duration as f64 / ffi::AV_TIME_BASE as f64;
            }
            if let Some(remote) = &remote {
                // How far ahead to fetch is in the film's seconds, so its pace is wanted.
                let rate = if (*format).bit_rate > 0 { (*format).bit_rate as f64 / 8.0 } else { remote.size() as f64 / info.duration };
                remote.set_rate(rate);
            }
            for (index, stream) in streams.iter().enumerate() {
                let parameters = (**stream).codecpar;
                if (*parameters).codec_type == ffi::AVMEDIA_TYPE_SUBTITLE {
                    info.subtitles.push(Track {
                        stream: index,
                        codec: text(ffi::avcodec_get_name((*parameters).codec_id)).unwrap_or_default(),
                        profile: None,
                        channels: 0,
                        language: tag((**stream).metadata, c"language"),
                        title: tag((**stream).metadata, c"title"),
                    });
                }
                if (*parameters).codec_type != ffi::AVMEDIA_TYPE_AUDIO {
                    continue;
                }
                let profile = ffi::avcodec_profile_name((*parameters).codec_id, (*parameters).profile);
                if Some(index) == audio_stream {
                    info.audio_playing = Some(info.audio.len());
                }
                info.audio.push(Track {
                    stream: index,
                    codec: text(ffi::avcodec_get_name((*parameters).codec_id)).unwrap_or_default(),
                    profile: text(profile),
                    channels: (*parameters).ch_layout.nb_channels.max(0) as u16,
                    language: tag((**stream).metadata, c"language"),
                    title: tag((**stream).metadata, c"title"),
                });
            }

            let wanted = info.audio_playing.and_then(|index| info.audio.get(index)).map_or(2, |track| track.channels);
            let sound = audio_stream.and_then(|_| Sound::open_in(group, wanted).map_err(|e| log::warn!("no sound: {e}")).ok()).map(Arc::new);
            let shared = Arc::new(Shared {
                stop: AtomicBool::new(false),
                paused: AtomicBool::new(false),
                serial: AtomicU64::new(0),
                video: Queue::default(),
                audio: Queue::default(),
                pictures: Mutex::new(VecDeque::new()),
                room: Condvar::new(),
                sound: sound.filter(|_| audio_stream.is_some()),
                wall: Mutex::new(Wall { time: 0.0, since: Instant::now() }),
                seek: Mutex::new(None),
                target: Mutex::new((0.0, false)),
                audio_stream: AtomicU64::new(audio_stream.map_or(NO_STREAM, |s| s as u64)),
                speed: std::sync::atomic::AtomicU32::new(1f32.to_bits()),
                subtitle_stream: AtomicU64::new(NO_STREAM),
                subtitle: Queue::default(),
                cues: Mutex::new(Vec::new()),
                read_out: AtomicBool::new(false),
                video_out: AtomicBool::new(video_stream.is_none()),
                audio_out: AtomicBool::new(audio_stream.is_none()),
                video_generation: AtomicU64::new(0),
                video_alive: Mutex::new(Instant::now()),
                counted: AtomicBool::new(false),
                length: AtomicU64::new(0),
                length_guessed: AtomicBool::new(false),
                fixed: AtomicBool::new(false),
                asked: AtomicU64::new(0),
                moved: AtomicU64::new(0),
                container_rate: video_stream.map_or((0, 0), |index| {
                    let rate = (*streams[index]).avg_frame_rate;
                    (rate.num, rate.den)
                }),
                unheard: Mutex::new(None),
                walled: AtomicU64::new(0),
                failed: Mutex::new(None),
            });

            let mut threads = Vec::new();
            let mut hardware = None;
            if let (Some(index), Some((decoder, on_gpu))) = (video_stream, pictures_decoder) {
                let stream = streams[index];
                let parameters = (*stream).codecpar;
                info.width = (*parameters).width.max(0) as u32;
                info.height = (*parameters).height.max(0) as u32;
                info.video_codec = text(ffi::avcodec_get_name((*parameters).codec_id)).unwrap_or_default();
                let sample = ffi::av_guess_sample_aspect_ratio(format, stream, ptr::null_mut());
                info.sample_aspect = if sample.num > 0 && sample.den > 0 { sample.num as f64 / sample.den as f64 } else { 1.0 };
                let rate = ffi::av_guess_frame_rate(format, stream, ptr::null_mut());
                info.frame_rate = if rate.den > 0 { rate.num as f64 / rate.den as f64 } else { 0.0 };
                // A container that keeps no times has no rate of its own to guess from: the
                // codec's statement, or what such a stream is taken to be (`timing`).
                if !(*format).iformat.is_null() && (*(*format).iformat).flags & ffi::AVFMT_NOTIMESTAMPS as c_int != 0 {
                    let stated = (*parameters).framerate;
                    info.frame_rate = timing::frame_rate(&[(stated.num, stated.den), shared.container_rate]);
                }
                (info.hints, info.eyes_swapped) = read_hints(stream);
                info.upright = crate::still::Upright::of(crate::still::side_matrix((*parameters).coded_side_data, (*parameters).nb_coded_side_data));
                info.hardware = HARDWARE.filter(|_| on_gpu).map(|(_, _, name)| name);
                if on_gpu {
                    hardware = Some((SendPointer(stream), start, threads.len()));
                }
                let stream = SendPointer(stream);
                let shared = shared.clone();
                threads.push(spawn("pictures", move || decode_pictures(decoder, stream, on_gpu, start, shared, 0)));
            }
            // Every sound stream gets a decoder when it is chosen, not before: see `decode_sound`.
            if audio_stream.is_some() && shared.sound.is_some() {
                let streams: Vec<SendPointer<ffi::AVStream>> = streams.iter().map(|s| SendPointer(*s)).collect();
                let shared = shared.clone();
                threads.push(spawn("sound", move || decode_sound(streams, start, shared)));
            }
            if !info.subtitles.is_empty() {
                let streams: Vec<SendPointer<ffi::AVStream>> = streams.iter().map(|s| SendPointer(*s)).collect();
                let shared = shared.clone();
                let frame = (info.width, info.height);
                threads.push(spawn("words", move || decode_words(streams, start, frame, shared)));
            }
            {
                let source = Source { format, video_stream, start, io, _interrupt: interrupt.clone() };
                let shared = shared.clone();
                threads.push(spawn("reader", move || read(source, shared)));
            }
            log::info!(
                "playing {url}: {}x{} {} at {:.2} a second, {:.0} s, {}",
                info.width,
                info.height,
                info.video_codec,
                info.frame_rate,
                info.duration,
                info.hardware.unwrap_or("decoded in software"),
            );
            Ok(Player { shared, info, threads, showing: None, lip: 0.0, hardware, remote, interrupt })
        }
    }

    pub fn info(&self) -> &Info {
        &self.info
    }

    /// What the reader has learnt of the length of a film whose file keeps no times, taken up.
    fn learn_length(&mut self) {
        let length = f64::from_bits(self.shared.length.load(Ordering::Relaxed));
        if length > 0.0 {
            self.info.duration = length;
            self.info.duration_guessed = self.shared.length_guessed.load(Ordering::Relaxed);
        }
    }

    pub fn paused(&self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    pub fn set_paused(&mut self, paused: bool) {
        let now = self.position();
        if self.shared.paused.swap(paused, Ordering::Relaxed) == paused {
            return;
        }
        *self.shared.wall.lock().unwrap() = Wall { time: now, since: Instant::now() };
        if let Some(sound) = self.shared.sound.as_ref() {
            sound.set_paused(paused);
        }
    }

    /// Where the film is, in seconds.
    pub fn position(&self) -> f64 {
        let shared = &self.shared;
        let (target, shown) = *shared.target.lock().unwrap();
        // Just after a seek the clocks still tell the old time: until something from after it
        // has been seen or heard, the film is where it was asked to go.
        // Sound that has nowhere to go any more keeps no time: the wall does.
        let sound = shared.sound.as_ref().filter(|s| s.alive());
        let serial = shared.serial.load(Ordering::Relaxed);
        let position = match sound.and_then(|sound| sound.time(serial)) {
            Some(heard) => heard,
            None if sound.is_some() && !shared.audio_out.load(Ordering::Relaxed) && !self.unheard(serial, target) => target,
            None => {
                let wall = shared.wall.lock().unwrap();
                if shared.paused.load(Ordering::Relaxed) || !shown {
                    wall.time
                } else {
                    wall.time + wall.since.elapsed().as_secs_f64() * f32::from_bits(shared.speed.load(Ordering::Relaxed)) as f64
                }
            }
        };
        // Never past the end -- where the end is known: one only guessed at may be short of it.
        position.clamp(0.0, if self.info.duration > 0.0 && !self.info.duration_guessed { self.info.duration } else { f64::MAX })
    }

    /// Whether the sound being waited for since the seek numbered `serial` is not coming: none
    /// heard, none waiting to be, none read, and the pictures waiting for it with no room for
    /// more -- so the reader, its queue of pictures full, reads nothing more either. A track
    /// that starts late, or has a long gap, or ended before the pictures (a seek into the
    /// tail), stood the film at its first picture for good. Once so, the wall keeps the time
    /// until something is heard.
    fn unheard(&self, serial: u64, target: f64) -> bool {
        let shared = &self.shared;
        if shared.walled.load(Ordering::Relaxed) == serial + 1 {
            return true;
        }
        let stalled = shared.sound.as_ref().is_some_and(|s| s.empty())
            && shared.audio.len() == 0
            && (shared.pictures.lock().unwrap().len() >= PICTURES_AHEAD || shared.read_out.load(Ordering::Relaxed));
        let mut unheard = shared.unheard.lock().unwrap();
        if !stalled {
            *unheard = None;
            return false;
        }
        if unheard.get_or_insert_with(Instant::now).elapsed() < UNHEARD {
            return false;
        }
        log::info!("no sound has come since the move to {target:.1} s: the film keeps time by the wall until some does");
        *unheard = None;
        shared.walled.store(serial + 1, Ordering::Relaxed);
        *shared.wall.lock().unwrap() = Wall { time: target, since: Instant::now() };
        true
    }

    pub fn seek(&mut self, seconds: f64) {
        self.learn_length();
        // A stream with no times and nowhere to jump to plays on as it is.
        if self.shared.fixed.load(Ordering::Relaxed) || !seconds.is_finite() {
            return;
        }
        // No further than the end, where the end is known. Where it is not (a stream still
        // being made, a file with no times not yet read far enough to judge) the reader is
        // asked for what was asked for: held to nought, as this was, a track chosen or ten
        // seconds skipped started such a film again from its beginning.
        let most = if self.info.duration > 0.0 { (self.info.duration - 0.25).max(0.0) } else { f64::MAX };
        let seconds = seconds.clamp(0.0, most);
        *self.shared.target.lock().unwrap() = (seconds, false);
        *self.shared.wall.lock().unwrap() = Wall { time: seconds, since: Instant::now() };
        let mut seek = self.shared.seek.lock().unwrap();
        self.shared.asked.fetch_add(1, Ordering::Relaxed);
        *seek = Some(seconds);
    }

    /// Play another of the file's sound tracks, by its place in [`Info::audio`].
    pub fn set_audio(&mut self, index: usize) {
        let Some(track) = self.info.audio.get(index) else { return };
        self.info.audio_playing = Some(index);
        self.shared.audio_stream.store(track.stream as u64, Ordering::Relaxed);
        // From where the film is: the new track's packets from here on are what is wanted.
        let now = self.position();
        self.seek(now);
    }

    /// Show one of the file's subtitle tracks, by its place in [`Info::subtitles`], or none.
    pub fn set_subtitle(&mut self, index: Option<usize>) {
        let stream = index.and_then(|i| self.info.subtitles.get(i)).map_or(NO_STREAM, |track| track.stream as u64);
        self.info.subtitle_showing = index.filter(|_| stream != NO_STREAM);
        self.shared.subtitle_stream.store(stream, Ordering::Relaxed);
        self.shared.cues.lock().unwrap().clear();
        if stream != NO_STREAM {
            // From where the film is, so that what is being said now is on show now and not
            // from the next line on.
            let now = self.position();
            self.seek((now - 4.0).max(0.0));
        }
    }

    /// The subtitles to show now, from the file's own track, in the order they began: every one
    /// begun and not ended -- a Blu-ray's subtitle in two pictures is two cues of the same start,
    /// and only one of them was shown.
    pub fn subtitles(&self) -> Vec<Arc<Cue>> {
        let now = self.position();
        let cues = self.shared.cues.lock().unwrap();
        let before = cues.partition_point(|cue| cue.start <= now);
        let mut on: Vec<Arc<Cue>> = cues[..before].iter().rev().take(16).filter(|cue| cue.end > now).cloned().collect();
        on.reverse();
        on
    }

    /// Play faster or slower than the film was made, its sound's pitch staying where it was.
    pub fn set_speed(&mut self, speed: f32) {
        let speed = speed.clamp(0.5, 3.0);
        let now = self.position();
        *self.shared.wall.lock().unwrap() = Wall { time: now, since: Instant::now() };
        self.shared.speed.store(speed.to_bits(), Ordering::Relaxed);
        if let Some(sound) = self.shared.sound.as_ref() {
            sound.set_speed(speed);
        }
    }

    pub fn speed(&self) -> f32 {
        f32::from_bits(self.shared.speed.load(Ordering::Relaxed))
    }

    /// Show pictures this much later than their sound (earlier, if negative).
    pub fn set_lip(&mut self, seconds: f64) {
        self.lip = seconds.clamp(-2.0, 2.0);
    }

    pub fn lip(&self) -> f64 {
        self.lip
    }

    pub fn set_volume(&self, volume: f32) {
        if let Some(sound) = self.shared.sound.as_ref() {
            sound.set_volume(volume);
        }
    }

    /// Why the film stopped before its end, if it did: what could not be read. [`Player::ended`]
    /// is true of it too, once what was read has been played -- but it has not been watched to
    /// the end, and whoever keeps the wearer's place should not be told it has.
    pub fn failed(&self) -> Option<String> {
        let failed = self.shared.failed.lock().unwrap().clone()?;
        // The server's own words, where it was a server that stopped giving the film.
        Some(self.remote.as_ref().and_then(|remote| remote.failure()).unwrap_or(failed))
    }

    /// The film has been played to its end.
    pub fn ended(&self) -> bool {
        let shared = &self.shared;
        shared.read_out.load(Ordering::Relaxed)
            && shared.video_out.load(Ordering::Relaxed)
            && shared.pictures.lock().unwrap().is_empty()
            && shared.seek.lock().unwrap().is_none()
            && shared.moved.load(Ordering::Relaxed) == shared.asked.load(Ordering::Relaxed)
            && (shared.sound.as_ref().is_none_or(|s| !s.alive()) || shared.audio_out.load(Ordering::Relaxed) && shared.sound.as_ref().is_some_and(|s| s.empty()))
    }

    /// The picture that should be on show now, and whether it is a new one since last asked.
    /// The platform's decoder has been seen to stop for good, deep inside the system, on some
    /// streams (YouTube's fragments, on an Intel Mac's VideoToolbox, 2026-10-18) --
    /// the clock and the sound going on and the picture standing still at the first frame. A
    /// decoder that has had pictures to make and made nothing for a second and a half is left
    /// where it is, and software decodes from where the film is instead.
    fn watch_decoder(&mut self) {
        let Some((stream, start, thread)) = self.hardware.as_ref().map(|(s, st, t)| (s.0, *st, *t)) else { return };
        let stuck = self.shared.video_alive.lock().unwrap().elapsed() > Duration::from_millis(1500);
        if !stuck || self.shared.video.len() == 0 {
            return;
        }
        log::warn!("the platform's decoder has stopped making pictures; decoding in software from here");
        self.hardware = None;
        let generation = self.shared.video_generation.fetch_add(1, Ordering::Relaxed) + 1;
        // Its thread is not waited for: it may never come back. Told it is not the one, it
        // ends if it does.
        if thread < self.threads.len() {
            drop(self.threads.remove(thread));
        }
        let decoder = match unsafe { open_decoder(stream, false) } {
            Ok((decoder, _)) => decoder,
            Err(e) => {
                log::warn!("{e}");
                return;
            }
        };
        self.info.hardware = None;
        *self.shared.video_alive.lock().unwrap() = Instant::now();
        let shared = self.shared.clone();
        let stream = SendPointer(stream);
        self.threads.push(spawn("pictures", move || decode_pictures(decoder, stream, false, start, shared, generation)));
        // From a key frame: the new decoder knows nothing of what the old one was given.
        let at = self.position();
        self.seek(at);
    }

    pub fn picture(&mut self) -> (Option<Arc<Picture>>, bool) {
        self.watch_decoder();
        self.learn_length();
        let shared = &self.shared;
        let now = self.position() - self.lip;
        let mut fresh = None;
        // Until the reader has made a seek asked for, every picture in hand is from before
        // it. One of them at or after where the seek was going (a move backwards) was taken
        // for the first picture after it: the clock was set by it, and the film then ran
        // through everything from where it had gone back to, as fast as it decoded.
        let moving = shared.moved.load(Ordering::Relaxed) != shared.asked.load(Ordering::Relaxed);
        // Read after that: a seek just made has its number by now.
        let serial = shared.serial.load(Ordering::Relaxed);
        if !moving {
            let mut target = shared.target.lock().unwrap();
            let mut pictures = shared.pictures.lock().unwrap();
            while let Some(next) = pictures.front() {
                if next.serial != serial {
                    pictures.pop_front();
                    continue;
                }
                // The first picture after a seek is shown whatever the clock says, so that a
                // paused film shows where it has been moved to.
                let first = !target.1 && next.time >= target.0 - 0.001;
                if next.time <= now + 0.004 || first {
                    if first {
                        target.1 = true;
                        // The wall keeps time from here where no sound does (yet).
                        if shared.sound.as_ref().is_none_or(|sound| sound.time(serial).is_none()) {
                            *shared.wall.lock().unwrap() = Wall { time: next.time, since: Instant::now() };
                        }
                    }
                    fresh = pictures.pop_front();
                    if first {
                        break;
                    }
                } else {
                    break;
                }
            }
            shared.room.notify_all();
        }
        let new = fresh.is_some();
        if let Some(picture) = fresh {
            self.showing = Some(Arc::new(picture));
        }
        (self.showing.clone(), new)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.interrupt.stop.store(true, Ordering::Relaxed);
        // A read waiting on the network gives up at once.
        if let Some(remote) = self.remote.as_ref() {
            remote.close();
        }
        self.shared.room.notify_all();
        self.shared.video.arrived.notify_all();
        self.shared.audio.arrived.notify_all();
        self.shared.subtitle.arrived.notify_all();
        if let Some(sound) = self.shared.sound.as_ref() {
            sound.stop();
        }
        // Each is waited for, but not for ever: one inside the platform's decoder may never
        // come back (`watch_decoder`), and a film closing or a program ending must not hang on
        // it. One still out after a second is left to end by itself, told to stop.
        let until = Instant::now() + Duration::from_secs(1);
        for thread in self.threads.drain(..) {
            while !thread.is_finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
            if thread.is_finished() {
                let _ = thread.join();
            } else {
                log::warn!("a film's {} thread did not stop; it is left behind", thread.thread().name().unwrap_or("?"));
            }
        }
    }
}

struct SendPointer<T>(*mut T);
unsafe impl<T> Send for SendPointer<T> {}

fn spawn(name: &str, run: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
    std::thread::Builder::new().name(format!("film-{name}")).spawn(run).expect("a thread")
}

/// Whether the reader has enough in hand to stop for now: as ffplay has it, when every
/// stream being decoded has its seconds ahead (so neither starves the other), or when what is
/// held is too much whatever it is.
fn enough(shared: &Shared, video: bool) -> bool {
    let (video_q, audio_q) = (&shared.video, &shared.audio);
    if video_q.bytes() + audio_q.bytes() >= PACKETS_AHEAD_BYTES || video_q.crowded() || audio_q.crowded() {
        return true;
    }
    let ahead = if shared.paused.load(Ordering::Relaxed) { SECONDS_AHEAD_PAUSED } else { SECONDS_AHEAD };
    let has_sound = shared.sound.is_some() && shared.audio_stream.load(Ordering::Relaxed) != NO_STREAM;
    // A stream that has finished needs nothing more.
    let video_done = !video || shared.video_out.load(Ordering::Relaxed) || video_q.seconds() >= ahead;
    let audio_done = !has_sound || shared.audio_out.load(Ordering::Relaxed) || audio_q.seconds() >= ahead;
    video_done && audio_done
}

/// How many packets are passed over after a jump by bytes, looking for a key frame to start
/// from, before whatever comes is taken: a stream that marks none must still play.
const LOOKED_FOR: u32 = 300;

/// The reader's part of a film whose file keeps no times (`crate::timing`).
struct Counted {
    index: timing::Index,
    /// The film's time of the next packet.
    clock: f64,
    /// Packets passed over since a jump, looking for a key frame; none when not looking.
    looking: Option<u32>,
    /// Nothing has been read yet: the first packet starts the film, key frame or not.
    beginning: bool,
    /// The picture stream's unit of time, in seconds.
    unit: f64,
    /// How long a packet lasts that does not say.
    step: f64,
    /// The whole second of the film the length was last told at.
    told: i64,
}

impl Counted {
    /// What is known of the film's length, told to whoever is watching.
    fn tell(&self, shared: &Shared) {
        if let Some((length, guessed)) = self.index.length() {
            shared.length_guessed.store(guessed, Ordering::Relaxed);
            shared.length.store(length.to_bits(), Ordering::Relaxed);
        }
    }
}

/// Everything in hand is from before a seek, wherever it has got to.
fn start_afresh(shared: &Shared, video: bool) {
    shared.serial.fetch_add(1, Ordering::Relaxed);
    shared.video.clear();
    shared.audio.clear();
    shared.subtitle.clear();
    shared.cues.lock().unwrap().clear();
    shared.pictures.lock().unwrap().clear();
    shared.room.notify_all();
    if let Some(sound) = shared.sound.as_ref() {
        sound.clear();
    }
    shared.read_out.store(false, Ordering::Relaxed);
    *shared.failed.lock().unwrap() = None;
    shared.video_out.store(!video, Ordering::Relaxed);
    shared.audio_out.store(shared.audio_stream.load(Ordering::Relaxed) == NO_STREAM, Ordering::Relaxed);
}

/// A seek that could not be made: the film plays on from where it is, the next picture to
/// come being where it is.
fn stay(shared: &Shared) {
    *shared.target.lock().unwrap() = (0.0, false);
}

/// The reader: the file's packets, sorted into the two queues, and seeks.
fn read(source: Source, shared: Arc<Shared>) {
    let Source { mut format, video_stream, start, io, _interrupt } = source;
    unsafe {
        let mut packet = ffi::av_packet_alloc();
        // A file that keeps no times: found out at its first picture packet.
        let mut counted: Option<Counted> = None;
        let mut first_picture = true;
        // A seek in such a file that waits until enough has been read to judge where to go.
        let mut waiting: Option<f64> = None;
        let flags = if (*format).iformat.is_null() { 0 } else { (*(*format).iformat).flags };
        while !shared.stop.load(Ordering::Relaxed) {
            let (asked, number) = {
                let mut seek = shared.seek.lock().unwrap();
                (seek.take(), shared.asked.load(Ordering::Relaxed))
            };
            if let Some(seconds) = asked.or(waiting.take()) {
                match counted.as_mut() {
                    // Before the first picture packet it is not known whether the file keeps
                    // times (a film taken up where it was left is moved in at once): the seek
                    // waits for that packet. Asked of FFmpeg, a seek by time in a file with none
                    // reads the whole file looking for the time, finds none, and leaves the
                    // reader at its end.
                    None if first_picture && video_stream.is_some() && !shared.read_out.load(Ordering::Relaxed) => waiting = Some(seconds),
                    None => {
                        let to = ((seconds + start) * ffi::AV_TIME_BASE as f64) as i64;
                        let pb = (*format).pb;
                        let before = if pb.is_null() { -1 } else { ffi::avio_seek(pb, 0, libc::SEEK_CUR) };
                        let sought = ffi::av_seek_frame(format, -1, to, ffi::AVSEEK_FLAG_BACKWARD as c_int);
                        if sought < 0 {
                            log::warn!("could not seek to {seconds:.1}: {}", error_text(sought));
                            let after = if pb.is_null() { -1 } else { ffi::avio_seek(pb, 0, libc::SEEK_CUR) };
                            if before >= 0 && after != before {
                                // A seek that reads on looking for its time (a file with no index)
                                // and fails leaves the reader wherever it gave up -- at the end,
                                // often, and the film then ended there. Back to where it was, by
                                // bytes, and everything from before thrown away, being from there.
                                let back = ffi::av_seek_frame(format, -1, before, ffi::AVSEEK_FLAG_BYTE as c_int);
                                if back < 0 {
                                    log::warn!("could not go back to byte {before} either: {}", error_text(back));
                                }
                                start_afresh(&shared, video_stream.is_some());
                            }
                            // Otherwise nothing has moved: what is in hand is still what comes
                            // next. Thrown away, as it was, the decoders lost their place for
                            // nothing.
                            stay(&shared);
                        } else {
                            start_afresh(&shared, video_stream.is_some());
                        }
                    }
                    Some(film) => match film.index.seek(seconds) {
                        Some(landing) => {
                            let pos = match landing {
                                Landing::Key { pos, .. } | Landing::Bytes { pos } => pos,
                            };
                            let sought = ffi::av_seek_frame(format, -1, pos as i64, ffi::AVSEEK_FLAG_BYTE as c_int);
                            if sought < 0 {
                                log::warn!("could not go to byte {pos} for {seconds:.1} s: {}; the film cannot be moved in", error_text(sought));
                                shared.fixed.store(true, Ordering::Relaxed);
                                stay(&shared);
                            } else {
                                match landing {
                                    Landing::Key { time, .. } => {
                                        film.clock = time;
                                        film.looking = None;
                                        film.index.went_to(pos);
                                    }
                                    Landing::Bytes { .. } => {
                                        film.clock = seconds;
                                        film.looking = Some(0);
                                        film.index.jumped();
                                    }
                                }
                                film.beginning = false;
                                start_afresh(&shared, video_stream.is_some());
                            }
                        }
                        None if film.index.size() == 0 => {
                            log::warn!("a stream with no times and no end cannot be moved in");
                            shared.fixed.store(true, Ordering::Relaxed);
                            stay(&shared);
                        }
                        // Not enough read yet to judge where that is: read on, and ask again.
                        // What is decoded meanwhile is from before it, and is not shown.
                        None => waiting = Some(seconds),
                    },
                }
                // Made, or not to be made: what comes now is where the film is.
                if waiting.is_none() {
                    shared.moved.store(number, Ordering::Relaxed);
                }
            }
            // Waiting to judge a seek, more is wanted whatever is in hand.
            let held = waiting.is_none() && enough(&shared, video_stream.is_some());
            if shared.read_out.load(Ordering::Relaxed) || held {
                std::thread::sleep(Duration::from_millis(8));
                continue;
            }
            let serial = shared.serial.load(Ordering::Relaxed);
            let got = ffi::av_read_frame(format, packet);
            if got == ffi::AVERROR(libc::EAGAIN as u32) {
                // Nothing yet, from a stream being made.
                std::thread::sleep(Duration::from_millis(8));
                continue;
            }
            if got < 0 {
                // The end, or the file could not be read further: either way the decoders are let
                // run dry. A failure is kept to be told apart from the end: taken for it, the film
                // closed with nothing said, and its server was told it had been watched to the end.
                // Some demuxers say "end of file" where reading failed; the reader's own error says.
                let error = if got != ffi::AVERROR_EOF {
                    got
                } else if !(*format).pb.is_null() && (*(*format).pb).error < 0 && (*(*format).pb).error != ffi::AVERROR_EOF {
                    (*(*format).pb).error
                } else {
                    0
                };
                if error != 0 && !shared.stop.load(Ordering::Relaxed) {
                    log::warn!("reading stopped: {}", error_text(error));
                    *shared.failed.lock().unwrap() = Some(error_text(error));
                }
                if let Some(film) = counted.as_mut() {
                    film.index.ended();
                    film.tell(&shared);
                }
                shared.video.push(None, serial);
                shared.audio.push(None, serial);
                shared.read_out.store(true, Ordering::Relaxed);
                continue;
            }
            let stream = (*packet).stream_index as usize;
            let base = (**(*format).streams.add(stream)).time_base;
            let unit = base.num as f64 / base.den.max(1) as f64;
            if Some(stream) == video_stream {
                let unstamped = (*packet).pts == ffi::AV_NOPTS_VALUE && (*packet).dts == ffi::AV_NOPTS_VALUE;
                if std::mem::take(&mut first_picture) && unstamped && (flags & ffi::AVFMT_NOTIMESTAMPS as c_int != 0 || (*format).nb_streams == 1) {
                    let parameters = (**(*format).streams.add(stream)).codecpar;
                    let stated = (*parameters).framerate;
                    let rate = timing::frame_rate(&[(stated.num, stated.den), shared.container_rate]);
                    let size = if (*format).pb.is_null() { 0 } else { ffi::avio_size((*format).pb).max(0) as u64 };
                    log::info!("the file keeps no times: its pictures are counted at {rate:.3} a second (the stream's own rate, or 25 where it states none)");
                    if size == 0 || flags & ffi::AVFMT_NO_BYTE_SEEK as c_int != 0 {
                        shared.fixed.store(true, Ordering::Relaxed);
                    }
                    shared.counted.store(true, Ordering::Relaxed);
                    counted = Some(Counted { index: timing::Index::new(size), clock: 0.0, looking: None, beginning: true, unit, step: 1.0 / rate, told: -1 });
                }
                if let Some(film) = counted.as_mut() {
                    let mut key = (*packet).flags & ffi::AV_PKT_FLAG_KEY as c_int != 0 || std::mem::take(&mut film.beginning);
                    let pos = if (*packet).pos >= 0 { (*packet).pos as u64 } else { ffi::avio_seek((*format).pb, 0, 1).max(0) as u64 };
                    if let Some(passed) = film.looking {
                        if !key && passed < LOOKED_FOR {
                            // Between key frames after a jump: nothing a decoder can start from.
                            film.looking = Some(passed + 1);
                            ffi::av_packet_unref(packet);
                            continue;
                        }
                        film.looking = None;
                        key = true;
                        // A key frame passed before keeps the time it had.
                        if let Some(known) = film.index.key_time(pos) {
                            film.clock = known;
                            film.index.went_to(pos);
                        }
                    }
                    // How long it lasts is the packet's own word where that is a length a picture
                    // can have. FFmpeg, having found no rate, gives the packets of such a stream
                    // one tick each after the first two seconds (HEVC with no timing in it: a
                    // 1 200 000th of a second), which is not one.
                    let said = (*packet).duration as f64 * film.unit;
                    let lasts = if said >= 0.001 { said } else { film.step };
                    film.index.passed(pos, (*packet).size.max(0) as u64, film.clock, lasts, key);
                    // Numbered in the order read: told apart by every decoder, and how much is
                    // in hand known. Not when its picture is shown: see `timing`.
                    let stamp = (film.clock / film.unit).round() as i64;
                    (*packet).pts = stamp;
                    (*packet).dts = stamp;
                    film.clock += lasts;
                    if film.clock as i64 != film.told {
                        film.told = film.clock as i64;
                        film.tell(&shared);
                    }
                }
            }
            let queue = if Some(stream) == video_stream {
                Some(&shared.video)
            } else if stream as u64 == shared.audio_stream.load(Ordering::Relaxed) && shared.sound.is_some() {
                Some(&shared.audio)
            } else if stream as u64 == shared.subtitle_stream.load(Ordering::Relaxed) {
                Some(&shared.subtitle)
            } else {
                None
            };
            match queue {
                Some(queue) => {
                    let stamp = if (*packet).pts != ffi::AV_NOPTS_VALUE { (*packet).pts } else { (*packet).dts };
                    let time = if stamp == ffi::AV_NOPTS_VALUE { f64::NAN } else { stamp as f64 * unit };
                    queue.push_at(Some(Packet(packet)), serial, time);
                    packet = ffi::av_packet_alloc();
                }
                None => ffi::av_packet_unref(packet),
            }
        }
        ffi::av_packet_free(&mut packet);
        ffi::avformat_close_input(&mut format);
        drop(io);
    }
}

unsafe fn colour_of(frame: *const ffi::AVFrame) -> Colour {
    let space = (*frame).colorspace;
    Colour {
        // A file that does not say is taken by its size, as every player takes it.
        hd: space == ffi::AVCOL_SPC_BT709 || (space == ffi::AVCOL_SPC_UNSPECIFIED && (*frame).height >= 720),
        wide: space == ffi::AVCOL_SPC_BT2020_NCL || space == ffi::AVCOL_SPC_BT2020_CL,
        full_range: (*frame).color_range == ffi::AVCOL_RANGE_JPEG,
        light: match (*frame).color_trc {
            trc if trc == ffi::AVCOL_TRC_SMPTE2084 => Light::Pq,
            trc if trc == ffi::AVCOL_TRC_ARIB_STD_B67 => Light::Hlg,
            _ => Light::Ordinary,
        },
    }
}

/// Make a decoder's frame into something [`crate::screen`] can draw: as it is where it can
/// be, and otherwise brought to main memory and to three eight-bit planes.
unsafe fn settle(frame: *mut ffi::AVFrame, scaler: &mut *mut ffi::SwsContext) -> Option<(*mut ffi::AVFrame, Pixels)> {
    let format = (*frame).format;
    let _ = format;
    #[cfg(target_os = "macos")]
    if format == ffi::AV_PIX_FMT_VIDEOTOOLBOX {
        // Eight- and ten-bit pictures stay where they are, as VA-API's do on Linux. Ten-bit ones
        // (P010) came down to main memory and were converted there every frame until 11-18.
        let frames = (*frame).hw_frames_ctx;
        let software = if frames.is_null() { ffi::AV_PIX_FMT_NONE } else { (*((*frames).data as *const ffi::AVHWFramesContext)).sw_format };
        let kind = match software {
            f if f == ffi::AV_PIX_FMT_NV12 => Some(Pixels::VideoToolbox),
            f if f == ffi::AV_PIX_FMT_P010LE => Some(Pixels::VideoToolbox10),
            _ => None,
        };
        if let (Some(kind), true) = (kind, std::env::var("SPATIAND_MEDIA_COPY").is_err()) {
            let kept = ffi::av_frame_alloc();
            ffi::av_frame_move_ref(kept, frame);
            return Some((kept, kind));
        }
    }
    #[cfg(target_os = "linux")]
    if format == ffi::AV_PIX_FMT_VAAPI {
        // An eight- or ten-bit picture is lent as it is: mapped, not copied, and the mapping
        // keeps the surface until the picture is dropped. Anything else comes down to main
        // memory. Ten bits came down too until 2026-11-17, and were converted there: a 4K HDR
        // film decoded by the Deck's GPU then played slower than in software (26 a second).
        let frames = (*frame).hw_frames_ctx;
        let software = if frames.is_null() { ffi::AV_PIX_FMT_NONE } else { (*((*frames).data as *const ffi::AVHWFramesContext)).sw_format };
        let kind = match software {
            f if f == ffi::AV_PIX_FMT_NV12 => Some(Pixels::Dmabuf),
            f if f == ffi::AV_PIX_FMT_P010LE => Some(Pixels::Dmabuf10),
            _ => None,
        };
        if let (Some(kind), true) = (kind, std::env::var("SPATIAND_MEDIA_COPY").is_err()) {
            let mut lent = ffi::av_frame_alloc();
            (*lent).format = ffi::AV_PIX_FMT_DRM_PRIME;
            if ffi::av_hwframe_map(lent, frame, (ffi::AV_HWFRAME_MAP_READ | ffi::AV_HWFRAME_MAP_DIRECT) as c_int) >= 0 {
                ffi::av_frame_copy_props(lent, frame);
                ffi::av_frame_unref(frame);
                return Some((lent, kind));
            }
            ffi::av_frame_free(&mut lent);
        }
    }
    let mut held = ffi::av_frame_alloc();
    if !(*frame).hw_frames_ctx.is_null() {
        if ffi::av_hwframe_transfer_data(held, frame, 0) < 0 {
            ffi::av_frame_free(&mut held);
            return None;
        }
        ffi::av_frame_copy_props(held, frame);
        ffi::av_frame_unref(frame);
    } else {
        ffi::av_frame_move_ref(held, frame);
    }
    let format = (*held).format;
    let _ = format;
    let pixels = match (*held).format {
        f if f == ffi::AV_PIX_FMT_YUV420P || f == ffi::AV_PIX_FMT_YUVJ420P => Some(Pixels::Planar420),
        f if f == ffi::AV_PIX_FMT_YUV420P10LE => Some(Pixels::Planar420Ten),
        f if f == ffi::AV_PIX_FMT_NV12 => Some(Pixels::Nv12),
        _ => None,
    };
    if let Some(pixels) = pixels {
        return Some((held, pixels));
    }
    // Anything else -- 4:2:2, a palette, ten bits packed another way -- is converted: to ten
    // bits if it had more than eight, so that an HDR film keeps the steps its curve needs.
    let deep = {
        let described = ffi::av_pix_fmt_desc_get((*held).format);
        !described.is_null() && (*described).comp[0].depth > 8
    };
    let (to, kind) = if deep { (ffi::AV_PIX_FMT_YUV420P10LE, Pixels::Planar420Ten) } else { (ffi::AV_PIX_FMT_YUV420P, Pixels::Planar420) };
    // Colours are left as they are: only the layout changes, so the same matrix still
    // applies afterwards.
    let (width, height) = ((*held).width, (*held).height);
    *scaler = ffi::sws_getCachedContext(
        *scaler, width, height, (*held).format, width, height, to, ffi::SWS_BILINEAR as c_int,
        ptr::null_mut(), ptr::null_mut(), ptr::null(),
    );
    if scaler.is_null() {
        ffi::av_frame_free(&mut held);
        return None;
    }
    let mut plain = ffi::av_frame_alloc();
    (*plain).format = to;
    (*plain).width = width;
    (*plain).height = height;
    if ffi::av_frame_get_buffer(plain, 0) < 0 {
        ffi::av_frame_free(&mut plain);
        ffi::av_frame_free(&mut held);
        return None;
    }
    ffi::sws_scale(*scaler, (*held).data.as_ptr() as *const *const u8, (*held).linesize.as_ptr(), 0, height, (*plain).data.as_mut_ptr(), (*plain).linesize.as_mut_ptr());
    ffi::av_frame_copy_props(plain, held);
    ffi::av_frame_free(&mut held);
    Some((plain, kind))
}

/// A MediaCodec picture drawn into its decoder's reader, and taken from it: the codec's buffer
/// rendered (which gives it back to the codec), then the reader's picture of the same time waited
/// for. Only then is the next drawn (see `crate::mediacodec`). None if it never arrives.
#[cfg(target_os = "android")]
unsafe fn drawn(frame: *mut ffi::AVFrame, decoder: &Decoder, complained: &mut bool) -> Option<crate::mediacodec::Image> {
    let reader = decoder.reader.as_ref()?;
    // The time the codec was given for this picture, in microseconds, as FFmpeg gave it.
    let base = (*decoder.context).pkt_timebase;
    let pts = (*frame).pts;
    let expected = (pts != ffi::AV_NOPTS_VALUE).then(|| {
        if base.num > 0 && base.den > 0 {
            ffi::av_rescale_q(pts, base, ffi::AVRational { num: 1, den: 1_000_000 })
        } else {
            pts
        }
    });
    if ffi::av_mediacodec_release_buffer((*frame).data[3].cast(), 1) < 0 {
        return None;
    }
    let image = reader.take(expected, Duration::from_millis(100));
    if image.is_none() && !std::mem::replace(complained, true) {
        log::warn!("a picture MediaCodec drew never reached its reader; it is skipped");
    }
    image
}

fn decode_pictures(decoder: Decoder, stream: SendPointer<ffi::AVStream>, _on_gpu: bool, start: f64, shared: Arc<Shared>, generation: u64) {
    unsafe {
        let cover = (*stream.0).disposition & ffi::AV_DISPOSITION_ATTACHED_PIC as c_int != 0;
        // A film the container says is interlaced: its pictures are, whatever the decoder says
        // of each (MediaCodec's say nothing). `SPATIAND_MEDIA_DEINTERLACE=0` shows them woven,
        // `=blend` has their fields blended as they are drawn, as where there is no deinterlacer.
        let order = (*(*stream.0).codecpar).field_order;
        let woven = order != ffi::AV_FIELD_UNKNOWN && order != ffi::AV_FIELD_PROGRESSIVE;
        let asked = std::env::var("SPATIAND_MEDIA_DEINTERLACE");
        let deinterlacing = asked.as_deref() != Ok("0");
        let blend_asked = asked.as_deref() == Ok("blend");
        let mut deinterlacer: Option<crate::deinterlace::Deinterlacer> = None;
        // Pictures of this shape have no deinterlacer to go through: their fields are blended
        // as they are drawn.
        let mut blend_only = blend_asked;
        // What the deinterlacer has made, waiting to be taken as a decoder's picture would be.
        let mut made: VecDeque<*mut ffi::AVFrame> = VecDeque::new();
        let mut frame = ffi::av_frame_alloc();
        let mut scaler: *mut ffi::SwsContext = ptr::null_mut();
        let mut serial = shared.serial.load(Ordering::Relaxed);
        let mut complained = false;
        // The pictures' times, where the file keeps none or a picture comes without its own.
        let mut count = timing::Count::default();
        #[cfg(target_os = "android")]
        let mut lost = false;
        let mine = || shared.video_generation.load(Ordering::Relaxed) == generation;
        while !shared.stop.load(Ordering::Relaxed) && mine() {
            let Ok((packet, packet_serial)) = shared.video.pop() else { continue };
            *shared.video_alive.lock().unwrap() = Instant::now();
            if packet_serial != shared.serial.load(Ordering::Relaxed) {
                continue;
            }
            if packet_serial != serial {
                ffi::avcodec_flush_buffers(decoder.context);
                serial = packet_serial;
                count.restart();
                // What the deinterlacer holds is from before the move.
                deinterlacer = None;
                for mut old in made.drain(..) {
                    ffi::av_frame_free(&mut old);
                }
            }
            let ending = packet.is_none();
            if let Some(packet) = packet.as_ref() {
                // Where the reader is: the first packet since it moved is where counting starts.
                let stamp = if (*packet.0).dts != ffi::AV_NOPTS_VALUE { (*packet.0).dts } else { (*packet.0).pts };
                if stamp != ffi::AV_NOPTS_VALUE {
                    count.begin(stamp as f64 * decoder.time_base - start);
                }
            }
            let sent = ffi::avcodec_send_packet(decoder.context, packet.as_ref().map_or(ptr::null(), |p| p.0));
            if sent < 0 && !ending && !std::mem::replace(&mut complained, true) {
                log::warn!("a picture would not decode: {}", error_text(sent));
            }
            let mut flushed = false;
            loop {
                // The next picture: what the deinterlacer has made, or the decoder's next -- put
                // through the deinterlacer first if it is two fields woven -- or, at the end, what
                // the deinterlacer was still holding.
                let mut fields = false;
                if let Some(mut done) = made.pop_front() {
                    ffi::av_frame_move_ref(frame, done);
                    ffi::av_frame_free(&mut done);
                } else if ffi::avcodec_receive_frame(decoder.context, frame) >= 0 {
                    let interlaced = deinterlacing && ((*frame).flags & ffi::AV_FRAME_FLAG_INTERLACED as c_int != 0 || woven);
                    if interlaced {
                        if deinterlacer.as_ref().is_some_and(|d| !d.fits(frame)) {
                            deinterlacer = None;
                            blend_only = blend_asked;
                        }
                        if deinterlacer.is_none() && !blend_only {
                            match crate::deinterlace::Deinterlacer::new(frame, (*decoder.context).pkt_timebase) {
                                Ok(Some(made)) => deinterlacer = Some(made),
                                Ok(None) => {
                                    log::info!("the film is interlaced: its fields are blended as they are drawn (no deinterlacer for pictures where these are)");
                                    blend_only = true;
                                }
                                Err(e) => {
                                    log::warn!("{e}; the film's fields are blended as they are drawn");
                                    blend_only = true;
                                }
                            }
                        }
                        if let Some(d) = deinterlacer.as_mut() {
                            match d.run(frame) {
                                Ok(out) => {
                                    made.extend(out);
                                    ffi::av_frame_unref(frame);
                                    continue;
                                }
                                Err(e) => {
                                    log::warn!("{e}; the film's fields are blended as they are drawn");
                                    deinterlacer = None;
                                    blend_only = true;
                                }
                            }
                        }
                        fields = true;
                    }
                } else if ending && !std::mem::replace(&mut flushed, true) {
                    if let Some(d) = deinterlacer.as_mut() {
                        made.extend(d.run(ptr::null_mut()).unwrap_or_default());
                        deinterlacer = None;
                    }
                    continue;
                } else {
                    break;
                }
                // Stopped, nothing more is made: the room for pictures is not waited for then,
                // and every picture the decoder still held went on into the queue -- on Android
                // into the reader, until every one of its pictures was held.
                if shared.stop.load(Ordering::Relaxed) || !mine() {
                    ffi::av_frame_unref(frame);
                    break;
                }
                // Its own time, where the file keeps times. Where it keeps none the packets were
                // only numbered in the order they were read, and the pictures are counted, at
                // the rate the stream states (`timing`). A picture with no time in a film that
                // has them is a frame after the one before: it was "at nought", and shown at once.
                let stamp = (*frame).best_effort_timestamp;
                let own = (stamp != ffi::AV_NOPTS_VALUE && !shared.counted.load(Ordering::Relaxed)).then(|| stamp as f64 * decoder.time_base - start);
                let stated = (*decoder.context).framerate;
                let rate = timing::frame_rate(&[(stated.num, stated.den), shared.container_rate]);
                let time = count.time(own, rate, (*frame).repeat_pict);
                // Pictures from before where a seek was aimed are decoded only to get to the
                // ones after: a seek lands on the key frame before its target.
                // A song's cover is its one picture, at nought, wherever the song is taken up:
                // thrown away as from before the target, a song taken up past 5 s had none.
                let target = shared.target.lock().unwrap().0;
                if time < target - 0.001 && serial == shared.serial.load(Ordering::Relaxed) && !cover {
                    ffi::av_frame_unref(frame);
                    continue;
                }
                let colour = colour_of(frame);
                let (width, height) = ((*frame).width.max(0) as u32, (*frame).height.max(0) as u32);
                // Android's decoder's picture: in its reader, on the GPU, nothing of it in the frame.
                #[cfg(target_os = "android")]
                let image = if (*frame).format == ffi::AV_PIX_FMT_MEDIACODEC {
                    let image = drawn(frame, &decoder, &mut lost);
                    ffi::av_frame_unref(frame);
                    if image.is_none() {
                        continue;
                    }
                    image
                } else {
                    None
                };
                #[cfg(target_os = "android")]
                let settled = if image.is_some() { Some((ptr::null_mut(), Pixels::HardwareBuffer)) } else { settle(frame, &mut scaler) };
                #[cfg(not(target_os = "android"))]
                let settled = settle(frame, &mut scaler);
                let Some((held, pixels)) = settled else {
                    ffi::av_frame_unref(frame);
                    continue;
                };
                let picture = Picture {
                    frame: held,
                    pixels,
                    width,
                    height,
                    time,
                    colour,
                    fields,
                    serial,
                    #[cfg(target_os = "android")]
                    image,
                };
                let mut pictures = shared.pictures.lock().unwrap();
                while pictures.len() >= PICTURES_AHEAD && !shared.stop.load(Ordering::Relaxed) && serial == shared.serial.load(Ordering::Relaxed) {
                    // Waiting for room is not being stuck.
                    *shared.video_alive.lock().unwrap() = Instant::now();
                    pictures = shared.room.wait_timeout(pictures, Duration::from_millis(20)).unwrap().0;
                }
                if serial == shared.serial.load(Ordering::Relaxed) && mine() {
                    pictures.push_back(picture);
                }
                *shared.video_alive.lock().unwrap() = Instant::now();
            }
            if ending {
                ffi::avcodec_flush_buffers(decoder.context);
                if serial == shared.serial.load(Ordering::Relaxed) {
                    shared.video_out.store(true, Ordering::Relaxed);
                }
            }
        }
        for mut old in made.drain(..) {
            ffi::av_frame_free(&mut old);
        }
        ffi::av_frame_free(&mut frame);
        ffi::sws_freeContext(scaler);
    }
}

fn decode_sound(streams: Vec<SendPointer<ffi::AVStream>>, start: f64, shared: Arc<Shared>) {
    let Some(sound) = shared.sound.clone() else { return };
    unsafe {
        let mut frame = ffi::av_frame_alloc();
        let mut decoder: Option<(u64, Decoder)> = None;
        // The track whose sound cannot be made, if one has been found so.
        let mut broken: Option<u64> = None;
        let mut resampler: *mut ffi::SwrContext = ptr::null_mut();
        // What the resampler was made for: a track may change its mind part way through.
        let mut made_for = (0, 0, 0);
        let mut serial = shared.serial.load(Ordering::Relaxed);
        let (rate, channels) = (sound.rate() as c_int, sound.channels() as c_int);
        let mut stretch = crate::stretch::Stretch::new(rate as u32, channels as u16);
        // The film's time of the next sound the stretcher gives, while it is in use.
        let mut stretched_to: Option<f64> = None;
        let mut to: ffi::AVChannelLayout = std::mem::zeroed();
        // The height layouts by name: for ten and twelve channels FFmpeg's own guess is not
        // the order the room's sink names them in.
        let named = match channels {
            10 => Some(c"5.1.4"),
            12 => Some(c"7.1.4"),
            _ => None,
        };
        if named.map_or(true, |name| ffi::av_channel_layout_from_string(&mut to, name.as_ptr()) < 0) {
            ffi::av_channel_layout_default(&mut to, channels);
        }
        while !shared.stop.load(Ordering::Relaxed) {
            let Ok((packet, packet_serial)) = shared.audio.pop() else { continue };
            if packet_serial != shared.serial.load(Ordering::Relaxed) {
                continue;
            }
            // The decoder is for whichever stream is chosen, made when its first packet comes.
            let wanted = shared.audio_stream.load(Ordering::Relaxed);
            if broken != Some(wanted) && decoder.as_ref().map(|(stream, _)| *stream) != Some(wanted) {
                decoder = streams.get(wanted as usize).and_then(|stream| match open_decoder(stream.0, false) {
                    Ok((decoder, _)) => Some((wanted, decoder)),
                    Err(e) => {
                        log::warn!("no sound from this track: {e}; the film keeps time without it");
                        None
                    }
                });
                if decoder.is_none() {
                    broken = Some(wanted);
                }
            }
            // A track that cannot be heard is asked again of nobody, and is done with at once:
            // waited for, its end never came and the film stood still at its first picture.
            let decoder = match decoder.as_ref() {
                Some((_, decoder)) if broken != Some(wanted) => decoder,
                _ => {
                    if packet_serial == shared.serial.load(Ordering::Relaxed) {
                        shared.audio_out.store(true, Ordering::Relaxed);
                    }
                    continue;
                }
            };
            if packet_serial != serial {
                ffi::avcodec_flush_buffers(decoder.context);
                serial = packet_serial;
                stretch.reset();
                stretched_to = None;
            }
            let ending = packet.is_none();
            if let Some(packet) = packet.as_ref() {
                if (*packet.0).stream_index as u64 != wanted {
                    continue;
                }
            }
            ffi::avcodec_send_packet(decoder.context, packet.as_ref().map_or(ptr::null(), |p| p.0));
            while ffi::avcodec_receive_frame(decoder.context, frame) >= 0 {
                let shape = ((*frame).format, (*frame).sample_rate, (*frame).ch_layout.nb_channels);
                if resampler.is_null() || shape != made_for {
                    ffi::swr_free(&mut resampler);
                    let made = ffi::swr_alloc_set_opts2(
                        &mut resampler, &to, ffi::AV_SAMPLE_FMT_FLT, rate, &(*frame).ch_layout, (*frame).format, (*frame).sample_rate, 0, ptr::null_mut(),
                    );
                    if made < 0 || ffi::swr_init(resampler) < 0 {
                        // Sound of a shape nothing can be made of (no rate, no channels): as a
                        // track with no decoder.
                        log::warn!("no sound from this track: it cannot be resampled ({} Hz, {} channels); the film keeps time without it", shape.1, shape.2);
                        ffi::swr_free(&mut resampler);
                        ffi::av_frame_unref(frame);
                        broken = Some(wanted);
                        if serial == shared.serial.load(Ordering::Relaxed) {
                            shared.audio_out.store(true, Ordering::Relaxed);
                        }
                        break;
                    }
                    made_for = shape;
                }
                let stamp = (*frame).best_effort_timestamp;
                let time = if stamp == ffi::AV_NOPTS_VALUE { f64::NAN } else { stamp as f64 * decoder.time_base - start };
                let room = ffi::swr_get_out_samples(resampler, (*frame).nb_samples).max(0) as usize;
                let mut samples = vec![0f32; room * channels as usize];
                let mut out = samples.as_mut_ptr() as *mut u8;
                let made = ffi::swr_convert(resampler, &mut out, room as c_int, (*frame).extended_data as *mut *const u8, (*frame).nb_samples);
                ffi::av_frame_unref(frame);
                if made <= 0 {
                    continue;
                }
                samples.truncate(made as usize * channels as usize);
                // Sound from before where a seek was aimed is not heard, as its pictures are
                // not seen.
                let target = shared.target.lock().unwrap().0;
                let length = made as f64 / rate as f64;
                if time.is_finite() && time + length < target {
                    continue;
                }
                // At any pace but the film's own the sound goes through the stretcher, which
                // gives back a different amount of it and knows what moment of the film each
                // of its samples is.
                let speed = f32::from_bits(shared.speed.load(Ordering::Relaxed)) as f64;
                let (samples, time) = if (speed - 1.0).abs() < 0.001 {
                    if stretched_to.take().is_some() {
                        stretch.reset();
                    }
                    (samples, time)
                } else {
                    let at = *stretched_to.get_or_insert(if time.is_finite() { time } else { 0.0 });
                    let out = stretch.feed(&samples, speed);
                    stretched_to = Some(at + (out.len() / channels as usize) as f64 / rate as f64 * speed);
                    if out.is_empty() {
                        continue;
                    }
                    (out, at)
                };
                let block = Block { samples, time, serial };
                while !sound.offer(&block) {
                    if shared.stop.load(Ordering::Relaxed) || serial != shared.serial.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(8));
                }
            }
            if ending {
                ffi::avcodec_flush_buffers(decoder.context);
                if serial == shared.serial.load(Ordering::Relaxed) {
                    shared.audio_out.store(true, Ordering::Relaxed);
                }
            }
        }
        ffi::av_frame_free(&mut frame);
        ffi::swr_free(&mut resampler);
        ffi::av_channel_layout_uninit(&mut to);
    }
}

/// The words: a subtitle track's packets made into cues as they come. There are few of them
/// and each is small, so nothing here waits for room.
/// `frame` is the film's picture's size: what a disc's subtitle pictures are placed in where
/// their decoder does not say the size of the screen they were made for.
fn decode_words(streams: Vec<SendPointer<ffi::AVStream>>, start: f64, frame: (u32, u32), shared: Arc<Shared>) {
    unsafe {
        let mut decoder: Option<(u64, Decoder)> = None;
        while !shared.stop.load(Ordering::Relaxed) {
            let Ok((packet, serial)) = shared.subtitle.pop() else { continue };
            let Some(packet) = packet else { continue };
            if serial != shared.serial.load(Ordering::Relaxed) {
                continue;
            }
            let wanted = shared.subtitle_stream.load(Ordering::Relaxed);
            if (*packet.0).stream_index as u64 != wanted {
                continue;
            }
            if decoder.as_ref().map(|(stream, _)| *stream) != Some(wanted) {
                decoder = streams.get(wanted as usize).and_then(|stream| open_decoder(stream.0, false).ok()).map(|(decoder, _)| (wanted, decoder));
            }
            let Some((_, decoder)) = decoder.as_ref() else { continue };
            let mut said: ffi::AVSubtitle = std::mem::zeroed();
            let mut got: c_int = 0;
            if ffi::avcodec_decode_subtitle2(decoder.context, &mut said, &mut got, packet.0) < 0 || got == 0 {
                continue;
            }
            let stamp = (*packet.0).pts;
            let begins = if said.pts != ffi::AV_NOPTS_VALUE {
                said.pts as f64 / ffi::AV_TIME_BASE as f64 - start
            } else if stamp != ffi::AV_NOPTS_VALUE {
                stamp as f64 * decoder.time_base - start
            } else {
                ffi::avsubtitle_free(&mut said);
                continue;
            } + said.start_display_time as f64 / 1000.0;
            // How long: what the subtitle says, or its packet, or a few seconds -- a disc's
            // picture says nothing and is ended by the next one.
            let shown = said.end_display_time.saturating_sub(said.start_display_time) as f64 / 1000.0;
            let lasts = if shown > 0.0 && said.end_display_time != u32::MAX {
                shown
            } else if (*packet.0).duration > 0 {
                (*packet.0).duration as f64 * decoder.time_base
            } else {
                6.0
            };
            // The screen the pictures were placed on: the decoder's, or else the film's (a DVD's
            // and a DVB broadcast's often say none, and placed by 1 x 1 they were off the screen),
            // or else a DVD's.
            let (frame_w, frame_h) = match ((*decoder.context).width, (*decoder.context).height) {
                (w, h) if w > 0 && h > 0 => (w as f32, h as f32),
                _ if frame.0 > 0 && frame.1 > 0 => (frame.0 as f32, frame.1 as f32),
                _ => (720.0, 576.0),
            };
            let rects = if said.rects.is_null() { &[][..] } else { std::slice::from_raw_parts(said.rects, said.num_rects as usize) };
            let mut lines: Vec<String> = Vec::new();
            let mut pictures: Vec<Words> = Vec::new();
            for rect in rects {
                let rect = &**rect;
                if !rect.ass.is_null() {
                    lines.push(subs::plain(&CStr::from_ptr(rect.ass).to_string_lossy()));
                } else if !rect.text.is_null() {
                    lines.push(CStr::from_ptr(rect.text).to_string_lossy().trim().to_string());
                } else if !rect.data[0].is_null() && !rect.data[1].is_null() && rect.w > 0 && rect.h > 0 {
                    // Each byte of the picture is a place in a list of 256 colours.
                    let (w, h, stride) = (rect.w as usize, rect.h as usize, rect.linesize[0] as usize);
                    let colours = std::slice::from_raw_parts(rect.data[1] as *const u32, 256);
                    let mut rgba = Vec::with_capacity(w * h * 4);
                    for row in 0..h {
                        for index in std::slice::from_raw_parts(rect.data[0].add(row * stride), w) {
                            let argb = colours[*index as usize];
                            rgba.extend_from_slice(&[(argb >> 16) as u8, (argb >> 8) as u8, argb as u8, (argb >> 24) as u8]);
                        }
                    }
                    pictures.push(Words::Picture {
                        rgba,
                        width: rect.w as u32,
                        height: rect.h as u32,
                        place: [rect.x as f32 / frame_w, rect.y as f32 / frame_h, rect.w as f32 / frame_w, rect.h as f32 / frame_h],
                    });
                }
            }
            ffi::avsubtitle_free(&mut said);
            let mut cues = shared.cues.lock().unwrap();
            if serial != shared.serial.load(Ordering::Relaxed) {
                continue;
            }
            // A picture with nothing in it is how a disc says "take the last one down" -- every
            // part of it: a Blu-ray's subtitle may be two pictures (two objects), each a cue.
            for cue in cues.iter_mut().rev().take(16) {
                if cue.end > begins && cue.start < begins && matches!(cue.words, Words::Picture { .. }) {
                    *cue = Arc::new(Cue { end: begins, ..(**cue).clone() });
                }
            }
            let text = lines.into_iter().filter(|line| !line.is_empty()).collect::<Vec<_>>().join("\n");
            let words = (!text.is_empty()).then_some(Words::Text(text)).into_iter().chain(pictures);
            for words in words {
                cues.push(Arc::new(Cue { start: begins, end: begins + lasts, words }));
            }
            // What is long past is not kept: a film has thousands.
            let now = begins;
            cues.retain(|cue| cue.end > now - 30.0);
        }
    }
}

/// How many packets are waiting, for a line in a log: (pictures', sound's).
impl Player {
    pub fn backlog(&self) -> (usize, usize) {
        (self.shared.video.len(), self.shared.audio.len())
    }
}
