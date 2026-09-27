//! Recording what the glasses show and what the wearer hears, to a file.
//!
//! Two pieces, both on the recorder's own thread:
//!
//! * [`VideoEncoder`] — the picture. The compositor copies each recorded frame, on the GPU,
//!   into one of a few VA-API surfaces handed out as dmabufs ([`VideoEncoder::canvases`]); the
//!   video postprocessor turns it into NV12 and the hardware encoder into HEVC. Nothing crosses
//!   the CPU. It is the host's encoder (`spatiand-host/src/encode.rs`) with more than one
//!   canvas, so a frame can be drawn while the last is encoding, and with keyframes on a timer,
//!   because a file is seeked in and a stream is not.
//! * [`Writer`] — the file. Matroska, because a recording cut short by a crash is still a file
//!   that plays up to the moment it stopped, which an MP4 without its index is not. One video
//!   track and any number of audio tracks, each uncompressed 24-bit PCM: lossless for editing,
//!   and not FLAC because FLAC stops at eight channels and the surround track has twelve.
//!
//! Built on the Deck's own ffmpeg 7.1, like the decoder beside it. The host encodes with its
//! own copy of this and a different ffmpeg; the two only have to agree on a codec.

use std::ffi::{CStr, CString};
use std::os::fd::{FromRawFd, OwnedFd};
use std::ptr;

use rsmpeg::ffi;

/// What went wrong, naming the step.
#[derive(Debug)]
pub struct RecordError(pub String);

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for RecordError {}

type Result<T> = std::result::Result<T, RecordError>;

fn fail(what: &str, code: i32) -> RecordError {
    let mut buf = [0i8; 256];
    let text = unsafe {
        ffi::av_strerror(code, buf.as_mut_ptr().cast(), buf.len());
        CStr::from_ptr(buf.as_ptr().cast()).to_string_lossy().into_owned()
    };
    RecordError(format!("{what}: {text} ({code})"))
}

/// libavutil's dmabuf description; see the same declaration in the host's encoder.
mod drm {
    pub const MAX_PLANES: usize = 4;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Object {
        pub fd: std::os::raw::c_int,
        pub size: usize,
        pub format_modifier: u64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Plane {
        pub object_index: std::os::raw::c_int,
        pub offset: isize,
        pub pitch: isize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Layer {
        pub format: u32,
        pub nb_planes: std::os::raw::c_int,
        pub planes: [Plane; MAX_PLANES],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct FrameDescriptor {
        pub nb_objects: std::os::raw::c_int,
        pub objects: [Object; MAX_PLANES],
        pub nb_layers: std::os::raw::c_int,
        pub layers: [Layer; MAX_PLANES],
    }
}

/// One buffer the compositor draws a recorded frame into: a DRM format, a modifier, and its
/// planes (descriptor, offset, stride). The descriptors are the caller's to keep or close.
pub struct Canvas {
    pub fourcc: u32,
    pub modifier: u64,
    pub width: u32,
    pub height: u32,
    pub planes: Vec<(OwnedFd, u32, u32)>,
}

/// How the picture is encoded.
#[derive(Debug, Clone, Copy)]
pub struct VideoSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Average bit rate, in kbit/s. The peak is allowed half as much again.
    pub kbit: u32,
    /// HEVC if true, H.264 if not.
    pub hevc: bool,
}

/// The hardware encoder, with `canvases` surfaces to draw into.
pub struct VideoEncoder {
    codec_ctx: *mut ffi::AVCodecContext,
    device: *mut ffi::AVBufferRef,
    frames: *mut ffi::AVBufferRef,
    source_frames: *mut ffi::AVBufferRef,
    canvases: Vec<*mut ffi::AVFrame>,
    filter: Filter,
    packet: *mut ffi::AVPacket,
    last_pts: i64,
    pub settings: VideoSettings,
}

// SAFETY: every pointer is owned here and freed in `Drop`, and the encoder lives on one thread.
unsafe impl Send for VideoEncoder {}

/// One encoded picture: bytes, whether it is a keyframe, and its time in milliseconds.
pub struct Coded {
    pub bytes: Vec<u8>,
    pub keyframe: bool,
    pub pts_ms: i64,
}

impl VideoEncoder {
    /// Open the encoder on `node` (a render node), with `canvases` surfaces to draw into.
    pub fn new(node: &str, settings: VideoSettings, canvases: usize) -> Result<VideoEncoder> {
        unsafe {
            let mut device = ptr::null_mut();
            let path = CString::new(node).unwrap();
            let r = ffi::av_hwdevice_ctx_create(
                &mut device,
                ffi::AV_HWDEVICE_TYPE_VAAPI,
                path.as_ptr(),
                ptr::null_mut(),
                0,
            );
            if r < 0 {
                return Err(fail(&format!("could not open {node} for VA-API"), r));
            }
            let (w, h) = (settings.width, settings.height);
            let frames = hw_frames_of(device, ffi::AV_PIX_FMT_NV12, w, h, 8)?;
            let source_frames = hw_frames_of(device, ffi::AV_PIX_FMT_BGRA, w, h, canvases as i32)?;
            let mut canvas_frames = Vec::new();
            for _ in 0..canvases {
                let canvas = ffi::av_frame_alloc();
                let r = ffi::av_hwframe_get_buffer(source_frames, canvas, 0);
                if r < 0 {
                    return Err(fail("could not allocate a surface to draw into", r));
                }
                canvas_frames.push(canvas);
            }

            let name = if settings.hevc { "hevc_vaapi" } else { "h264_vaapi" };
            let cname = CString::new(name).unwrap();
            let encoder = ffi::avcodec_find_encoder_by_name(cname.as_ptr());
            if encoder.is_null() {
                return Err(RecordError(format!("this libavcodec has no {name} encoder")));
            }
            let codec_ctx = ffi::avcodec_alloc_context3(encoder);
            (*codec_ctx).width = w as i32;
            (*codec_ctx).height = h as i32;
            (*codec_ctx).pix_fmt = ffi::AV_PIX_FMT_VAAPI;
            (*codec_ctx).time_base = ffi::AVRational { num: 1, den: 1000 };
            // What the rate controller divides the bit rate by; see the host's encoder for
            // what leaving it out does.
            (*codec_ctx).framerate = ffi::AVRational { num: settings.fps as i32, den: 1 };
            (*codec_ctx).bit_rate = settings.kbit as i64 * 1000;
            (*codec_ctx).rc_max_rate = settings.kbit as i64 * 1500;
            (*codec_ctx).rc_buffer_size = settings.kbit as i32 * 2000;
            (*codec_ctx).max_b_frames = 0;
            // A keyframe every two seconds, so an editor can cut and seek anywhere.
            (*codec_ctx).gop_size = (settings.fps * 2) as i32;
            // The file keeps the parameter sets once, in its header, which Matroska needs.
            (*codec_ctx).flags |= ffi::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
            (*codec_ctx).hw_frames_ctx = ffi::av_buffer_ref(frames);
            let mut options: *mut ffi::AVDictionary = ptr::null_mut();
            set(&mut options, "rc_mode", "VBR");
            let r = ffi::avcodec_open2(codec_ctx, encoder, &mut options);
            ffi::av_dict_free(&mut options);
            if r < 0 {
                return Err(fail("could not open the VA-API encoder", r));
            }
            let filter = Filter::new(device, source_frames, w, h)?;
            log::info!(
                "recording: {name} at {w}x{h}, {} fps, {} kbit/s",
                settings.fps,
                settings.kbit
            );
            Ok(VideoEncoder {
                codec_ctx,
                device,
                frames,
                source_frames,
                canvases: canvas_frames,
                filter,
                packet: ffi::av_packet_alloc(),
                last_pts: -1,
                settings,
            })
        }
    }

    /// The surfaces to draw into, as dmabufs, in the order [`VideoEncoder::encode`] numbers
    /// them. Asked once; each call hands out fresh descriptors.
    pub fn canvases(&self) -> Result<Vec<Canvas>> {
        self.canvases
            .iter()
            .map(|&frame| unsafe { export(frame, self.settings.width, self.settings.height) })
            .collect()
    }

    /// Encode what was drawn into canvas `index`, at `pts_ms` from the start of the recording.
    pub fn encode(&mut self, index: usize, pts_ms: i64) -> Result<Vec<Coded>> {
        unsafe {
            let converted = self.filter.run(self.canvases[index])?;
            // Strictly increasing, which a muxer insists on.
            let pts = pts_ms.max(self.last_pts + 1);
            self.last_pts = pts;
            (*converted).pts = pts;
            let r = ffi::avcodec_send_frame(self.codec_ctx, converted);
            ffi::av_frame_free(&mut { converted });
            if r < 0 {
                return Err(fail("the encoder would not take a frame", r));
            }
            self.drain()
        }
    }

    /// Everything still inside the encoder, at the end of a recording.
    pub fn finish(&mut self) -> Result<Vec<Coded>> {
        unsafe {
            ffi::avcodec_send_frame(self.codec_ctx, ptr::null());
            self.drain()
        }
    }

    unsafe fn drain(&mut self) -> Result<Vec<Coded>> {
        let mut out = Vec::new();
        loop {
            let r = ffi::avcodec_receive_packet(self.codec_ctx, self.packet);
            if r == ffi::AVERROR(ffi::EAGAIN) || r == ffi::AVERROR_EOF {
                break;
            }
            if r < 0 {
                return Err(fail("the encoder produced an error instead of a packet", r));
            }
            let packet = &*self.packet;
            out.push(Coded {
                bytes: std::slice::from_raw_parts(packet.data, packet.size as usize).to_vec(),
                keyframe: packet.flags & ffi::AV_PKT_FLAG_KEY as i32 != 0,
                pts_ms: packet.pts,
            });
            ffi::av_packet_unref(self.packet);
        }
        Ok(out)
    }
}

impl Drop for VideoEncoder {
    fn drop(&mut self) {
        unsafe {
            ffi::av_packet_free(&mut self.packet);
            ffi::avcodec_free_context(&mut self.codec_ctx);
            for frame in &mut self.canvases {
                ffi::av_frame_free(frame);
            }
            ffi::av_buffer_unref(&mut self.frames);
            ffi::av_buffer_unref(&mut self.source_frames);
            ffi::av_buffer_unref(&mut self.device);
        }
    }
}

unsafe fn hw_frames_of(
    device: *mut ffi::AVBufferRef,
    sw_format: ffi::AVPixelFormat,
    width: u32,
    height: u32,
    pool: i32,
) -> Result<*mut ffi::AVBufferRef> {
    let frames = ffi::av_hwframe_ctx_alloc(device);
    if frames.is_null() {
        return Err(RecordError("out of memory for a frame pool".into()));
    }
    let ctx = (*frames).data as *mut ffi::AVHWFramesContext;
    (*ctx).format = ffi::AV_PIX_FMT_VAAPI;
    (*ctx).sw_format = sw_format;
    (*ctx).width = width as i32;
    (*ctx).height = height as i32;
    (*ctx).initial_pool_size = pool;
    let r = ffi::av_hwframe_ctx_init(frames);
    if r < 0 {
        return Err(fail("could not describe a pool of surfaces", r));
    }
    Ok(frames)
}

/// BGRA in, NV12 out, on the GPU's postprocessor. As in the host's encoder.
struct Filter {
    graph: *mut ffi::AVFilterGraph,
    source: *mut ffi::AVFilterContext,
    sink: *mut ffi::AVFilterContext,
}

impl Filter {
    unsafe fn new(
        device: *mut ffi::AVBufferRef,
        source_frames: *mut ffi::AVBufferRef,
        width: u32,
        height: u32,
    ) -> Result<Filter> {
        let graph = ffi::avfilter_graph_alloc();
        let buffer = ffi::avfilter_get_by_name(c"buffer".as_ptr());
        let source = ffi::avfilter_graph_alloc_filter(graph, buffer, c"in".as_ptr());
        if source.is_null() {
            return Err(RecordError("could not create the filter graph's input".into()));
        }
        let params = ffi::av_buffersrc_parameters_alloc();
        (*params).format = ffi::AV_PIX_FMT_VAAPI;
        (*params).width = width as i32;
        (*params).height = height as i32;
        (*params).time_base = ffi::AVRational { num: 1, den: 1000 };
        (*params).hw_frames_ctx = source_frames;
        let r = ffi::av_buffersrc_parameters_set(source, params);
        ffi::av_free(params.cast());
        if r < 0 {
            return Err(fail("the filter graph would not take its input's shape", r));
        }
        let r = ffi::avfilter_init_str(source, ptr::null());
        if r < 0 {
            return Err(fail("could not start the filter graph's input", r));
        }
        let buffersink = ffi::avfilter_get_by_name(c"buffersink".as_ptr());
        let sink = ffi::avfilter_graph_alloc_filter(graph, buffersink, c"out".as_ptr());
        let r = ffi::avfilter_init_str(sink, ptr::null());
        if r < 0 {
            return Err(fail("could not start the filter graph's output", r));
        }
        let scale_filter = ffi::avfilter_get_by_name(c"scale_vaapi".as_ptr());
        if scale_filter.is_null() {
            return Err(RecordError("this libavfilter has no scale_vaapi".into()));
        }
        let scale = ffi::avfilter_graph_alloc_filter(graph, scale_filter, c"nv12".as_ptr());
        (*scale).hw_device_ctx = ffi::av_buffer_ref(device);
        let r = ffi::avfilter_init_str(scale, c"format=nv12".as_ptr());
        if r < 0 {
            return Err(fail("could not start the colour conversion", r));
        }
        for (a, b) in [(source, scale), (scale, sink)] {
            let r = ffi::avfilter_link(a, 0, b, 0);
            if r < 0 {
                return Err(fail("could not link the colour conversion", r));
            }
        }
        let r = ffi::avfilter_graph_config(graph, ptr::null_mut());
        if r < 0 {
            return Err(fail("the filter graph would not configure", r));
        }
        Ok(Filter { graph, source, sink })
    }

    unsafe fn run(&mut self, frame: *mut ffi::AVFrame) -> Result<*mut ffi::AVFrame> {
        let r = ffi::av_buffersrc_add_frame_flags(
            self.source,
            frame,
            ffi::AV_BUFFERSRC_FLAG_KEEP_REF as i32,
        );
        if r < 0 {
            return Err(fail("the conversion would not take a frame", r));
        }
        let out = ffi::av_frame_alloc();
        let r = ffi::av_buffersink_get_frame(self.sink, out);
        if r < 0 {
            ffi::av_frame_free(&mut { out });
            return Err(fail("the conversion produced nothing", r));
        }
        Ok(out)
    }
}

impl Drop for Filter {
    fn drop(&mut self) {
        unsafe { ffi::avfilter_graph_free(&mut self.graph) }
    }
}

/// A VA-API surface, as dmabuf planes the compositor can draw into.
unsafe fn export(frame: *mut ffi::AVFrame, width: u32, height: u32) -> Result<Canvas> {
    let drm_frame = ffi::av_frame_alloc();
    (*drm_frame).format = ffi::AV_PIX_FMT_DRM_PRIME;
    let r = ffi::av_hwframe_map(
        drm_frame,
        frame,
        (ffi::AV_HWFRAME_MAP_DIRECT | ffi::AV_HWFRAME_MAP_WRITE | ffi::AV_HWFRAME_MAP_READ) as i32,
    );
    if r < 0 {
        ffi::av_frame_free(&mut { drm_frame });
        return Err(fail("could not lend a recording surface to GL", r));
    }
    let descriptor = &*((*drm_frame).data[0] as *const drm::FrameDescriptor);
    let result = (|| {
        if descriptor.nb_layers < 1 {
            return Err(RecordError("a recording surface has no layers".into()));
        }
        let layer = &descriptor.layers[0];
        let mut planes = Vec::new();
        for plane in &layer.planes[..layer.nb_planes as usize] {
            let object = &descriptor.objects[plane.object_index as usize];
            // Duplicated: the mapping closes its own when the frame goes.
            let fd = libc::dup(object.fd);
            if fd < 0 {
                return Err(RecordError("could not duplicate a buffer descriptor".into()));
            }
            planes.push((OwnedFd::from_raw_fd(fd), plane.offset as u32, plane.pitch as u32));
        }
        Ok(Canvas {
            fourcc: layer.format,
            modifier: descriptor.objects[0].format_modifier,
            width,
            height,
            planes,
        })
    })();
    ffi::av_frame_free(&mut { drm_frame });
    result
}

unsafe fn set(options: *mut *mut ffi::AVDictionary, key: &str, value: &str) {
    let key = CString::new(key).unwrap();
    let value = CString::new(value).unwrap();
    ffi::av_dict_set(options, key.as_ptr(), value.as_ptr(), 0);
}

/// An audio track in the file: a name the editor shows, a channel layout ffmpeg knows by name
/// (`"stereo"`, `"mono"`, `"7.1.4"`), and the sample rate.
pub struct AudioTrack {
    pub title: String,
    pub layout: String,
    pub channels: usize,
    pub rate: u32,
}

/// A Matroska file with one video track and some audio tracks.
pub struct Writer {
    ctx: *mut ffi::AVFormatContext,
    video: *mut ffi::AVStream,
    /// Each audio track: its stream, channels, frames written, and sample rate.
    audio: Vec<(*mut ffi::AVStream, usize, i64, i32)>,
    /// Scratch for one audio packet, as 24-bit little-endian samples.
    bytes: Vec<u8>,
    closed: bool,
}

// SAFETY: owned here and used from one thread.
unsafe impl Send for Writer {}

impl Writer {
    /// Create the file and write its header. `title` goes in the file's own metadata.
    pub fn create(
        path: &std::path::Path,
        title: &str,
        video: &VideoEncoder,
        tracks: &[AudioTrack],
    ) -> Result<Writer> {
        unsafe {
            let cpath = CString::new(path.to_string_lossy().as_bytes())
                .map_err(|_| RecordError("a path with a nul in it".into()))?;
            let mut ctx = ptr::null_mut();
            let r = ffi::avformat_alloc_output_context2(
                &mut ctx,
                ptr::null(),
                c"matroska".as_ptr(),
                cpath.as_ptr(),
            );
            if r < 0 {
                return Err(fail("could not start a Matroska file", r));
            }
            dict(&mut (*ctx).metadata, "title", title);

            let vs = ffi::avformat_new_stream(ctx, ptr::null());
            let r = ffi::avcodec_parameters_from_context((*vs).codecpar, video.codec_ctx);
            if r < 0 {
                return Err(fail("could not describe the video track", r));
            }
            (*vs).time_base = ffi::AVRational { num: 1, den: 1000 };
            (*vs).avg_frame_rate = ffi::AVRational { num: video.settings.fps as i32, den: 1 };
            dict(&mut (*vs).metadata, "title", "What the glasses showed (left eye | right eye)");
            // Tells a player that knows about it that this is side-by-side stereo.
            dict(&mut (*vs).metadata, "stereo_mode", "left_right");
            (*vs).disposition = ffi::AV_DISPOSITION_DEFAULT as i32;

            let mut audio = Vec::new();
            for track in tracks {
                let st = ffi::avformat_new_stream(ctx, ptr::null());
                let par = (*st).codecpar;
                (*par).codec_type = ffi::AVMEDIA_TYPE_AUDIO;
                (*par).codec_id = ffi::AV_CODEC_ID_PCM_S24LE;
                (*par).sample_rate = track.rate as i32;
                (*par).bits_per_coded_sample = 24;
                (*par).block_align = (track.channels * 3) as i32;
                let name = CString::new(track.layout.as_str()).unwrap();
                if ffi::av_channel_layout_from_string(&mut (*par).ch_layout, name.as_ptr()) < 0
                    || (*par).ch_layout.nb_channels as usize != track.channels
                {
                    ffi::av_channel_layout_default(&mut (*par).ch_layout, track.channels as i32);
                }
                (*st).time_base = ffi::AVRational { num: 1, den: track.rate as i32 };
                dict(&mut (*st).metadata, "title", &track.title);
                // The first is the one a player plays, and the only one that says so.
                (*st).disposition = if audio.is_empty() { ffi::AV_DISPOSITION_DEFAULT as i32 } else { 0 };
                audio.push((st, track.channels, 0i64, track.rate as i32));
            }

            let r = ffi::avio_open(&mut (*ctx).pb, cpath.as_ptr(), ffi::AVIO_FLAG_WRITE as i32);
            if r < 0 {
                ffi::avformat_free_context(ctx);
                return Err(fail(&format!("could not create {}", path.display()), r));
            }
            let r = ffi::avformat_write_header(ctx, ptr::null_mut());
            if r < 0 {
                ffi::avio_closep(&mut (*ctx).pb);
                ffi::avformat_free_context(ctx);
                return Err(fail("could not write the file's header", r));
            }
            Ok(Writer {
                ctx,
                video: vs,
                audio,
                bytes: Vec::new(),
                closed: false,
            })
        }
    }

    /// One encoded picture.
    pub fn video(&mut self, coded: &Coded) -> Result<()> {
        unsafe {
            let packet = ffi::av_packet_alloc();
            let r = ffi::av_new_packet(packet, coded.bytes.len() as i32);
            if r < 0 {
                ffi::av_packet_free(&mut { packet });
                return Err(fail("out of memory for a packet", r));
            }
            ptr::copy_nonoverlapping(coded.bytes.as_ptr(), (*packet).data, coded.bytes.len());
            (*packet).pts = coded.pts_ms;
            (*packet).dts = coded.pts_ms;
            if coded.keyframe {
                (*packet).flags |= ffi::AV_PKT_FLAG_KEY as i32;
            }
            (*packet).stream_index = (*self.video).index;
            ffi::av_packet_rescale_ts(
                packet,
                ffi::AVRational { num: 1, den: 1000 },
                (*self.video).time_base,
            );
            self.write(packet)
        }
    }

    /// Interleaved float samples for audio track `track`, in its own channel count. Written
    /// back to back: each track's time is simply how many samples it has had.
    pub fn audio(&mut self, track: usize, samples: &[f32]) -> Result<()> {
        let (stream, channels, written, rate) = self.audio[track];
        let frames = samples.len() / channels;
        if frames == 0 {
            return Ok(());
        }
        self.bytes.clear();
        for &x in &samples[..frames * channels] {
            let x = if x.is_finite() { x.clamp(-1.0, 1.0) } else { 0.0 };
            let v = (x * 8_388_607.0) as i32;
            self.bytes.extend_from_slice(&v.to_le_bytes()[..3]);
        }
        unsafe {
            let packet = ffi::av_packet_alloc();
            let r = ffi::av_new_packet(packet, self.bytes.len() as i32);
            if r < 0 {
                ffi::av_packet_free(&mut { packet });
                return Err(fail("out of memory for a packet", r));
            }
            ptr::copy_nonoverlapping(self.bytes.as_ptr(), (*packet).data, self.bytes.len());
            (*packet).pts = written;
            (*packet).dts = written;
            (*packet).duration = frames as i64;
            (*packet).flags |= ffi::AV_PKT_FLAG_KEY as i32;
            (*packet).stream_index = (*stream).index;
            // Counted in samples, and written in the stream's own time base -- which the muxer
            // is free to have changed when it wrote the header, and Matroska does: to
            // milliseconds. Left in samples, two seconds of sound claimed to be ninety-six.
            ffi::av_packet_rescale_ts(packet, ffi::AVRational { num: 1, den: rate }, (*stream).time_base);
            self.audio[track].2 += frames as i64;
            self.write(packet)
        }
    }

    /// How many frames audio track `track` has had.
    pub fn audio_frames(&self, track: usize) -> i64 {
        self.audio[track].2
    }

    unsafe fn write(&mut self, packet: *mut ffi::AVPacket) -> Result<()> {
        let r = ffi::av_interleaved_write_frame(self.ctx, packet);
        ffi::av_packet_free(&mut { packet });
        if r < 0 {
            return Err(fail("could not write to the file", r));
        }
        Ok(())
    }

    /// Write the file's index and close it.
    pub fn finish(mut self) -> Result<()> {
        self.close()
    }

    fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        unsafe {
            let r = ffi::av_write_trailer(self.ctx);
            ffi::avio_closep(&mut (*self.ctx).pb);
            ffi::avformat_free_context(self.ctx);
            if r < 0 {
                return Err(fail("could not finish the file", r));
            }
        }
        Ok(())
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

unsafe fn dict(d: *mut *mut ffi::AVDictionary, key: &str, value: &str) {
    let key = CString::new(key).unwrap();
    let value = CString::new(value.replace('\0', "")).unwrap();
    ffi::av_dict_set(d, key.as_ptr(), value.as_ptr(), 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two seconds at the glasses' size through the real encoder into a real file, with the
    /// three tracks a session records. Needs the Deck's GPU, so it is asked for by name:
    /// `--ignored a_recording_is_a_file`. The picture is whatever the surfaces held.
    #[test]
    #[ignore]
    fn a_recording_is_a_file() {
        let node = std::env::var("SPATIAND_RENDER_NODE").unwrap_or("/dev/dri/renderD128".into());
        let settings = VideoSettings { width: 3840, height: 1080, fps: 36, kbit: 40_000, hevc: false };
        let mut encoder = VideoEncoder::new(&node, settings, 3).expect("an encoder");
        let canvases = encoder.canvases().expect("canvases");
        assert_eq!(canvases.len(), 3);
        let path = std::env::temp_dir().join("spatiand-record-test.mkv");
        let tracks = [
            AudioTrack { title: "What you heard".into(), layout: "stereo".into(), channels: 2, rate: 48_000 },
            AudioTrack { title: "Surround".into(), layout: "7.1.4".into(), channels: 12, rate: 48_000 },
            AudioTrack { title: "Microphone".into(), layout: "mono".into(), channels: 1, rate: 48_000 },
        ];
        let mut writer = Writer::create(&path, "test", &encoder, &tracks).expect("a file");
        let per_frame = 48_000 / 36;
        let mut slowest = std::time::Duration::ZERO;
        let mut total = std::time::Duration::ZERO;
        for i in 0..72 {
            let started = std::time::Instant::now();
            for coded in encoder.encode(i % 3, i as i64 * 1000 / 36).expect("a frame") {
                writer.video(&coded).expect("written");
            }
            let took = started.elapsed();
            slowest = slowest.max(took);
            total += took;
            for (t, track) in tracks.iter().enumerate() {
                let tone: Vec<f32> = (0..per_frame * track.channels)
                    .map(|n| ((n / track.channels) as f32 * 0.05).sin() * 0.25)
                    .collect();
                writer.audio(t, &tone).expect("sound written");
            }
        }
        for coded in encoder.finish().expect("the rest") {
            writer.video(&coded).expect("written");
        }
        writer.finish().expect("finished");
        let size = std::fs::metadata(&path).expect("the file").len();
        println!(
            "{}: {size} bytes; encoding took {:.1} ms a frame on average, {:.1} ms at most",
            path.display(),
            total.as_secs_f64() * 1000.0 / 72.0,
            slowest.as_secs_f64() * 1000.0
        );
        assert!(size > 100_000);
    }
}
