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

/// How many decoded pictures wait to be shown. Each is a frame of GPU or main memory, and a
/// 4K one is not small: enough to ride out an uneven decode, no more.
const PICTURES_AHEAD: usize = 4;
/// How much of the file the reader keeps ahead of the decoders.
const PACKETS_AHEAD_BYTES: usize = 24 * 1024 * 1024;
const PACKETS_AHEAD: usize = 512;

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
    /// A VAAPI surface lent as dmabufs, two planes like [`Pixels::Nv12`], on the GPU.
    Dmabuf,
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
    serial: u64,
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
        if self.pixels != Pixels::Dmabuf {
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

    /// The `CVPixelBufferRef`, for a [`Pixels::VideoToolbox`] picture.
    pub fn pixel_buffer(&self) -> *const c_void {
        unsafe { (*self.frame).data[3] as *const c_void }
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
    pub width: u32,
    pub height: u32,
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
    arrived: Condvar,
}

impl Queue {
    fn push(&self, packet: Option<Packet>, serial: u64) {
        let mut held = self.packets.lock().unwrap();
        held.1 += packet.as_ref().map_or(0, |p| unsafe { (*p.0).size.max(0) as usize });
        held.0.push_back((packet, serial));
        self.arrived.notify_one();
    }

    fn clear(&self) {
        let mut held = self.packets.lock().unwrap();
        held.0.clear();
        held.1 = 0;
    }

    fn full(&self) -> bool {
        let held = self.packets.lock().unwrap();
        held.0.len() >= PACKETS_AHEAD || held.1 >= PACKETS_AHEAD_BYTES
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
}

const NO_STREAM: u64 = u64::MAX;

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
}

/// What is opened, owned by the reader thread once it starts.
struct Source {
    format: *mut ffi::AVFormatContext,
    video_stream: Option<usize>,
    start: f64,
}
unsafe impl Send for Source {}

struct Decoder {
    context: *mut ffi::AVCodecContext,
    time_base: f64,
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
// MediaCodec is asked for the same way and is not wired to a way of showing its pictures
// yet: until it is, the Beam Pro decodes in software.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
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

unsafe fn open_decoder(stream: *mut ffi::AVStream, hardware: bool) -> Result<(Decoder, bool), String> {
    let parameters = (*stream).codecpar;
    let codec = ffi::avcodec_find_decoder((*parameters).codec_id);
    if codec.is_null() {
        return Err("FFmpeg has no decoder for this".into());
    }
    let mut context = ffi::avcodec_alloc_context3(codec);
    ffi::avcodec_parameters_to_context(context, parameters);
    (*context).pkt_timebase = (*stream).time_base;
    let mut on_gpu = false;
    // On Linux, H.264 only for now. An HEVC file (one a Mac's encoder made) hung the decoder
    // of an AMD GPU when given to VAAPI from here on 2026-10-13, and the kernel reset the
    // whole GPU to get it back -- which on a Deck is the room going down with the film.
    // Until that is understood, HEVC and the rest are decoded in software there;
    // `SPATIAND_MEDIA_HARDWARE=all` asks for the GPU regardless.
    let trusted = !cfg!(target_os = "linux")
        || (*parameters).codec_id == ffi::AV_CODEC_ID_H264
        || std::env::var("SPATIAND_MEDIA_HARDWARE").as_deref() == Ok("all");
    if let (true, Some((kind, _, _))) = (hardware && trusted && (*parameters).codec_type == ffi::AVMEDIA_TYPE_VIDEO, HARDWARE) {
        let mut device = ptr::null_mut();
        if ffi::av_hwdevice_ctx_create(&mut device, kind, ptr::null(), ptr::null_mut(), 0) >= 0 {
            (*context).hw_device_ctx = device;
            (*context).get_format = Some(choose_format);
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
    Ok((Decoder { context, time_base: time_base.num as f64 / time_base.den.max(1) as f64 }, on_gpu))
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
        unsafe {
            let mut format = ptr::null_mut();
            let mut told: *mut ffi::AVDictionary = ptr::null_mut();
            for (key, value) in options {
                if let (Ok(key), Ok(value)) = (CString::new(key.as_str()), CString::new(value.as_str())) {
                    ffi::av_dict_set(&mut told, key.as_ptr(), value.as_ptr(), 0);
                }
            }
            let opened = ffi::avformat_open_input(&mut format, name.as_ptr(), ptr::null(), &mut told);
            ffi::av_dict_free(&mut told);
            if opened < 0 {
                return Err(format!("could not open {url}: {}", error_text(opened)));
            }
            let found = ffi::avformat_find_stream_info(format, ptr::null_mut());
            if found < 0 {
                ffi::avformat_close_input(&mut format);
                return Err(format!("could not read {url}: {}", error_text(found)));
            }
            let streams = std::slice::from_raw_parts((*format).streams, (*format).nb_streams as usize);
            let best = |kind, related: c_int| {
                let index = ffi::av_find_best_stream(format, kind, -1, related, ptr::null_mut(), 0);
                (index >= 0).then_some(index as usize)
            };
            let video_stream = best(ffi::AVMEDIA_TYPE_VIDEO, -1);
            let audio_stream = best(ffi::AVMEDIA_TYPE_AUDIO, video_stream.map_or(-1, |v| v as c_int));

            let mut info = Info::default();
            let start = if (*format).start_time != ffi::AV_NOPTS_VALUE { (*format).start_time as f64 / ffi::AV_TIME_BASE as f64 } else { 0.0 };
            if (*format).duration != ffi::AV_NOPTS_VALUE {
                info.duration = (*format).duration as f64 / ffi::AV_TIME_BASE as f64;
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
            let sound = audio_stream.and_then(|_| Sound::open(wanted).map_err(|e| log::warn!("no sound: {e}")).ok()).map(Arc::new);
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
            });

            let mut threads = Vec::new();
            if let Some(index) = video_stream {
                let stream = streams[index];
                let parameters = (*stream).codecpar;
                info.width = (*parameters).width.max(0) as u32;
                info.height = (*parameters).height.max(0) as u32;
                info.video_codec = text(ffi::avcodec_get_name((*parameters).codec_id)).unwrap_or_default();
                let rate = ffi::av_guess_frame_rate(format, stream, ptr::null_mut());
                info.frame_rate = if rate.den > 0 { rate.num as f64 / rate.den as f64 } else { 0.0 };
                (info.hints, info.eyes_swapped) = read_hints(stream);
                let wanted = std::env::var("SPATIAND_MEDIA_HARDWARE").as_deref() != Ok("0");
                let (decoder, on_gpu) = open_decoder(stream, wanted).or_else(|e| {
                    log::warn!("{e}; trying without the platform's decoder");
                    open_decoder(stream, false)
                })?;
                info.hardware = HARDWARE.filter(|_| on_gpu).map(|(_, _, name)| name);
                let stream = SendPointer(stream);
                let shared = shared.clone();
                threads.push(spawn("pictures", move || decode_pictures(decoder, stream, on_gpu, start, shared)));
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
                threads.push(spawn("words", move || decode_words(streams, start, shared)));
            }
            {
                let source = Source { format, video_stream, start };
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
            Ok(Player { shared, info, threads, showing: None, lip: 0.0 })
        }
    }

    pub fn info(&self) -> &Info {
        &self.info
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
        let position = match shared.sound.as_ref().and_then(|sound| sound.time(shared.serial.load(Ordering::Relaxed))) {
            Some(heard) => heard,
            None if shared.sound.is_some() && !shared.audio_out.load(Ordering::Relaxed) => target,
            None => {
                let wall = shared.wall.lock().unwrap();
                if shared.paused.load(Ordering::Relaxed) || !shown {
                    wall.time
                } else {
                    wall.time + wall.since.elapsed().as_secs_f64() * f32::from_bits(shared.speed.load(Ordering::Relaxed)) as f64
                }
            }
        };
        position.clamp(0.0, if self.info.duration > 0.0 { self.info.duration } else { f64::MAX })
    }

    pub fn seek(&mut self, seconds: f64) {
        let seconds = seconds.clamp(0.0, (self.info.duration - 0.25).max(0.0));
        *self.shared.target.lock().unwrap() = (seconds, false);
        *self.shared.wall.lock().unwrap() = Wall { time: seconds, since: Instant::now() };
        *self.shared.seek.lock().unwrap() = Some(seconds);
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

    /// The subtitle to show now, from the file's own track.
    pub fn subtitle(&self) -> Option<Arc<Cue>> {
        let now = self.position();
        let cues = self.shared.cues.lock().unwrap();
        let before = cues.partition_point(|cue| cue.start <= now);
        cues[..before].iter().rev().take(4).find(|cue| cue.end > now).cloned()
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

    /// The film has been played to its end.
    pub fn ended(&self) -> bool {
        let shared = &self.shared;
        shared.read_out.load(Ordering::Relaxed)
            && shared.video_out.load(Ordering::Relaxed)
            && shared.pictures.lock().unwrap().is_empty()
            && shared.seek.lock().unwrap().is_none()
            && (shared.sound.is_none() || shared.audio_out.load(Ordering::Relaxed) && shared.sound.as_ref().is_some_and(|s| s.empty()))
    }

    /// The picture that should be on show now, and whether it is a new one since last asked.
    pub fn picture(&mut self) -> (Option<Arc<Picture>>, bool) {
        let shared = &self.shared;
        let serial = shared.serial.load(Ordering::Relaxed);
        let now = self.position() - self.lip;
        let mut fresh = None;
        {
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
                        if shared.sound.is_none() {
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
        self.shared.room.notify_all();
        self.shared.video.arrived.notify_all();
        self.shared.audio.arrived.notify_all();
        self.shared.subtitle.arrived.notify_all();
        if let Some(sound) = self.shared.sound.as_ref() {
            sound.stop();
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

struct SendPointer<T>(*mut T);
unsafe impl<T> Send for SendPointer<T> {}

fn spawn(name: &str, run: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
    std::thread::Builder::new().name(format!("film-{name}")).spawn(run).expect("a thread")
}

/// The reader: the file's packets, sorted into the two queues, and seeks.
fn read(source: Source, shared: Arc<Shared>) {
    let Source { mut format, video_stream, start } = source;
    unsafe {
        let mut packet = ffi::av_packet_alloc();
        while !shared.stop.load(Ordering::Relaxed) {
            if let Some(seconds) = shared.seek.lock().unwrap().take() {
                let to = ((seconds + start) * ffi::AV_TIME_BASE as f64) as i64;
                let sought = ffi::av_seek_frame(format, -1, to, ffi::AVSEEK_FLAG_BACKWARD as c_int);
                if sought < 0 {
                    log::warn!("could not seek to {seconds:.1}: {}", error_text(sought));
                }
                // Everything in hand is from before, wherever it has got to.
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
                shared.video_out.store(video_stream.is_none(), Ordering::Relaxed);
                shared.audio_out.store(shared.audio_stream.load(Ordering::Relaxed) == NO_STREAM, Ordering::Relaxed);
            }
            if shared.read_out.load(Ordering::Relaxed) || shared.video.full() || shared.audio.full() {
                std::thread::sleep(Duration::from_millis(8));
                continue;
            }
            let serial = shared.serial.load(Ordering::Relaxed);
            let got = ffi::av_read_frame(format, packet);
            if got < 0 {
                // The end, or an error that might as well be: let the decoders run dry.
                if got != ffi::AVERROR_EOF {
                    log::warn!("reading stopped: {}", error_text(got));
                }
                shared.video.push(None, serial);
                shared.audio.push(None, serial);
                shared.read_out.store(true, Ordering::Relaxed);
                continue;
            }
            let stream = (*packet).stream_index as usize;
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
                    queue.push(Some(Packet(packet)), serial);
                    packet = ffi::av_packet_alloc();
                }
                None => ffi::av_packet_unref(packet),
            }
        }
        ffi::av_packet_free(&mut packet);
        ffi::avformat_close_input(&mut format);
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
        // Eight-bit pictures stay where they are. Ten-bit ones are two sixteen-bit planes
        // nothing here samples yet: those come down to main memory with the rest.
        let frames = (*frame).hw_frames_ctx;
        let software = if frames.is_null() { ffi::AV_PIX_FMT_NONE } else { (*((*frames).data as *const ffi::AVHWFramesContext)).sw_format };
        if software == ffi::AV_PIX_FMT_NV12 {
            let kept = ffi::av_frame_alloc();
            ffi::av_frame_move_ref(kept, frame);
            return Some((kept, Pixels::VideoToolbox));
        }
    }
    #[cfg(target_os = "linux")]
    if format == ffi::AV_PIX_FMT_VAAPI {
        // An eight-bit picture is lent as it is: mapped, not copied, and the mapping keeps
        // the surface until the picture is dropped. Anything else comes down to main memory.
        let frames = (*frame).hw_frames_ctx;
        let software = if frames.is_null() { ffi::AV_PIX_FMT_NONE } else { (*((*frames).data as *const ffi::AVHWFramesContext)).sw_format };
        if software == ffi::AV_PIX_FMT_NV12 && std::env::var("SPATIAND_MEDIA_COPY").is_err() {
            let mut lent = ffi::av_frame_alloc();
            (*lent).format = ffi::AV_PIX_FMT_DRM_PRIME;
            if ffi::av_hwframe_map(lent, frame, (ffi::AV_HWFRAME_MAP_READ | ffi::AV_HWFRAME_MAP_DIRECT) as c_int) >= 0 {
                ffi::av_frame_copy_props(lent, frame);
                ffi::av_frame_unref(frame);
                return Some((lent, Pixels::Dmabuf));
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

fn decode_pictures(decoder: Decoder, stream: SendPointer<ffi::AVStream>, _on_gpu: bool, start: f64, shared: Arc<Shared>) {
    let _ = stream;
    unsafe {
        let mut frame = ffi::av_frame_alloc();
        let mut scaler: *mut ffi::SwsContext = ptr::null_mut();
        let mut serial = shared.serial.load(Ordering::Relaxed);
        let mut complained = false;
        while !shared.stop.load(Ordering::Relaxed) {
            let Ok((packet, packet_serial)) = shared.video.pop() else { continue };
            if packet_serial != shared.serial.load(Ordering::Relaxed) {
                continue;
            }
            if packet_serial != serial {
                ffi::avcodec_flush_buffers(decoder.context);
                serial = packet_serial;
            }
            let ending = packet.is_none();
            let sent = ffi::avcodec_send_packet(decoder.context, packet.as_ref().map_or(ptr::null(), |p| p.0));
            if sent < 0 && !ending && !std::mem::replace(&mut complained, true) {
                log::warn!("a picture would not decode: {}", error_text(sent));
            }
            while ffi::avcodec_receive_frame(decoder.context, frame) >= 0 {
                let stamp = (*frame).best_effort_timestamp;
                let time = if stamp == ffi::AV_NOPTS_VALUE { 0.0 } else { stamp as f64 * decoder.time_base - start };
                // Pictures from before where a seek was aimed are decoded only to get to the
                // ones after: a seek lands on the key frame before its target.
                let target = shared.target.lock().unwrap().0;
                if time < target - 0.001 && serial == shared.serial.load(Ordering::Relaxed) {
                    ffi::av_frame_unref(frame);
                    continue;
                }
                let colour = colour_of(frame);
                let (width, height) = ((*frame).width.max(0) as u32, (*frame).height.max(0) as u32);
                let Some((held, pixels)) = settle(frame, &mut scaler) else {
                    ffi::av_frame_unref(frame);
                    continue;
                };
                let picture = Picture { frame: held, pixels, width, height, time, colour, serial };
                let mut pictures = shared.pictures.lock().unwrap();
                while pictures.len() >= PICTURES_AHEAD && !shared.stop.load(Ordering::Relaxed) && serial == shared.serial.load(Ordering::Relaxed) {
                    pictures = shared.room.wait_timeout(pictures, Duration::from_millis(20)).unwrap().0;
                }
                if serial == shared.serial.load(Ordering::Relaxed) {
                    pictures.push_back(picture);
                }
            }
            if ending {
                ffi::avcodec_flush_buffers(decoder.context);
                if serial == shared.serial.load(Ordering::Relaxed) {
                    shared.video_out.store(true, Ordering::Relaxed);
                }
            }
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
            if decoder.as_ref().map(|(stream, _)| *stream) != Some(wanted) {
                decoder = streams.get(wanted as usize).and_then(|stream| match open_decoder(stream.0, false) {
                    Ok((decoder, _)) => Some((wanted, decoder)),
                    Err(e) => {
                        log::warn!("no sound from this track: {e}");
                        None
                    }
                });
            }
            let Some((_, decoder)) = decoder.as_ref() else { continue };
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
                        ffi::swr_free(&mut resampler);
                        ffi::av_frame_unref(frame);
                        continue;
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
fn decode_words(streams: Vec<SendPointer<ffi::AVStream>>, start: f64, shared: Arc<Shared>) {
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
            let (frame_w, frame_h) = ((*decoder.context).width.max(1) as f32, (*decoder.context).height.max(1) as f32);
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
            // A picture with nothing in it is how a disc says "take the last one down".
            if let Some(last) = cues.last_mut().filter(|last| last.end > begins && matches!(last.words, Words::Picture { .. })) {
                *last = Arc::new(Cue { end: begins, ..(**last).clone() });
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
