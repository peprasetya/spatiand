//! Pictures made of two fields, a broadcast's or a DVD's, made whole.
//!
//! An interlaced picture is two half-pictures taken a fiftieth (or sixtieth) of a second apart,
//! woven line by line: shown as it is, anything that moves has combs. Each picture is put
//! through FFmpeg's deinterlacer for where it is, so that it stays there:
//!
//! | the picture | where it is | the deinterlacer |
//! |---|---|---|
//! | decoded in software | main memory | `bwdif` |
//! | VA-API's (Linux, the Deck) | the GPU | `deinterlace_vaapi`, the GPU's own |
//! | VideoToolbox's (the Mac) | the GPU | `yadif_videotoolbox` where FFmpeg was built with Metal |
//! | MediaCodec's (Android) | the GPU, in the codec's buffer | none in FFmpeg |
//!
//! Where there is none, the picture is drawn with its two fields blended on the GPU as it is
//! sampled (`crate::screen`, `Picture::fields`): no combs, a little of the height's sharpness
//! lost. Each makes one picture of each, at the film's own rate.

use std::ffi::{c_int, CStr};
use std::ptr;

use rsmpeg::ffi;

/// How many pictures a deinterlacer on the GPU must have room for besides its own: those held
/// outside it, as the decoder's own pool keeps room for (`player::open_decoder`).
const HELD: c_int = crate::player::PICTURES_HELD;

pub(crate) struct Deinterlacer {
    graph: *mut ffi::AVFilterGraph,
    source: *mut ffi::AVFilterContext,
    sink: *mut ffi::AVFilterContext,
    /// What it was made for: format, size, and the GPU's frames, which a new shape changes.
    shape: (c_int, c_int, c_int, *mut u8),
    /// The pictures' unit of time, which what it makes is brought back to: yadif and bwdif give
    /// theirs in halves of it, ready to make two of each.
    time_base: ffi::AVRational,
}

// SAFETY: made, used and dropped on the picture decoder's thread only; moved there once.
unsafe impl Send for Deinterlacer {}

unsafe fn shape(frame: *const ffi::AVFrame) -> (c_int, c_int, c_int, *mut u8) {
    let frames = (*frame).hw_frames_ctx;
    ((*frame).format, (*frame).width, (*frame).height, if frames.is_null() { ptr::null_mut() } else { (*frames).data })
}

unsafe fn fail(what: &str, code: c_int) -> String {
    let mut buffer = [0 as std::ffi::c_char; 128];
    ffi::av_strerror(code, buffer.as_mut_ptr(), buffer.len());
    format!("{what}: {}", CStr::from_ptr(buffer.as_ptr()).to_string_lossy())
}

/// The deinterlacer for pictures in this format, and how it is told to make one picture of each
/// whole one: none where FFmpeg has none that keeps them where they are.
fn filter_for(format: c_int) -> Option<(&'static CStr, &'static CStr)> {
    let named = |name: &CStr| unsafe { !ffi::avfilter_get_by_name(name.as_ptr()).is_null() };
    match format {
        f if f == ffi::AV_PIX_FMT_VAAPI => Some((c"deinterlace_vaapi", c"rate=frame:auto=0")),
        f if f == ffi::AV_PIX_FMT_VIDEOTOOLBOX && named(c"yadif_videotoolbox") => Some((c"yadif_videotoolbox", c"mode=send_frame:deint=all")),
        f if f == ffi::AV_PIX_FMT_VIDEOTOOLBOX || f == ffi::AV_PIX_FMT_MEDIACODEC => None,
        _ => Some((c"bwdif", c"mode=send_frame:parity=auto:deint=all")),
    }
}

impl Deinterlacer {
    /// One for pictures like `frame`, whose times are in `time_base`; none where there is no
    /// deinterlacer that keeps them where they are.
    pub(crate) unsafe fn new(frame: *const ffi::AVFrame, time_base: ffi::AVRational) -> Result<Option<Deinterlacer>, String> {
        let Some((name, options)) = filter_for((*frame).format) else { return Ok(None) };
        let filter = ffi::avfilter_get_by_name(name.as_ptr());
        if filter.is_null() {
            return Ok(None);
        }
        let time_base = if time_base.num > 0 && time_base.den > 0 { time_base } else { ffi::AVRational { num: 1, den: 90_000 } };
        let mut graph = ffi::avfilter_graph_alloc();
        let made = (|| {
            let source = ffi::avfilter_graph_alloc_filter(graph, ffi::avfilter_get_by_name(c"buffer".as_ptr()), c"in".as_ptr());
            if source.is_null() {
                return Err("no filter graph".to_string());
            }
            let params = ffi::av_buffersrc_parameters_alloc();
            (*params).format = (*frame).format;
            (*params).width = (*frame).width;
            (*params).height = (*frame).height;
            (*params).time_base = time_base;
            (*params).sample_aspect_ratio = (*frame).sample_aspect_ratio;
            (*params).hw_frames_ctx = (*frame).hw_frames_ctx;
            let r = ffi::av_buffersrc_parameters_set(source, params);
            ffi::av_free(params.cast());
            if r < 0 {
                return Err(fail("the deinterlacer would not take the pictures' shape", r));
            }
            let r = ffi::avfilter_init_str(source, ptr::null());
            if r < 0 {
                return Err(fail("the deinterlacer's input would not start", r));
            }
            let deinterlace = ffi::avfilter_graph_alloc_filter(graph, filter, c"deinterlace".as_ptr());
            if deinterlace.is_null() {
                return Err("no deinterlacer".to_string());
            }
            (*deinterlace).extra_hw_frames = HELD;
            let r = ffi::avfilter_init_str(deinterlace, options.as_ptr());
            if r < 0 {
                return Err(fail(&format!("{} would not start", name.to_string_lossy()), r));
            }
            let sink = ffi::avfilter_graph_alloc_filter(graph, ffi::avfilter_get_by_name(c"buffersink".as_ptr()), c"out".as_ptr());
            if sink.is_null() || ffi::avfilter_init_str(sink, ptr::null()) < 0 {
                return Err("the deinterlacer's output would not start".to_string());
            }
            for (a, b) in [(source, deinterlace), (deinterlace, sink)] {
                let r = ffi::avfilter_link(a, 0, b, 0);
                if r < 0 {
                    return Err(fail("the deinterlacer would not link", r));
                }
            }
            let r = ffi::avfilter_graph_config(graph, ptr::null_mut());
            if r < 0 {
                return Err(fail(&format!("{} would not take these pictures", name.to_string_lossy()), r));
            }
            Ok((source, sink))
        })();
        match made {
            Ok((source, sink)) => {
                log::info!("the film is interlaced: its pictures are made whole by {}", name.to_string_lossy());
                Ok(Some(Deinterlacer { graph, source, sink, shape: shape(frame), time_base }))
            }
            Err(e) => {
                ffi::avfilter_graph_free(&mut graph);
                Err(e)
            }
        }
    }

    /// Whether it was made for pictures like this one.
    pub(crate) unsafe fn fits(&self, frame: *const ffi::AVFrame) -> bool {
        shape(frame) == self.shape
    }

    /// Give it a picture -- or, null, say there are no more -- and take what it has made of what
    /// it has been given: none for the first (it looks one ahead), one for each after.
    pub(crate) unsafe fn run(&mut self, frame: *mut ffi::AVFrame) -> Result<Vec<*mut ffi::AVFrame>, String> {
        let r = ffi::av_buffersrc_add_frame_flags(self.source, frame, ffi::AV_BUFFERSRC_FLAG_KEEP_REF as c_int);
        if r < 0 {
            return Err(fail("the deinterlacer would not take a picture", r));
        }
        let mut made = Vec::new();
        loop {
            let mut out = ffi::av_frame_alloc();
            if ffi::av_buffersink_get_frame(self.sink, out) < 0 {
                ffi::av_frame_free(&mut out);
                return Ok(made);
            }
            // Its time is the picture's own, in the picture's unit, as a decoder would give it.
            if (*out).pts != ffi::AV_NOPTS_VALUE {
                (*out).pts = ffi::av_rescale_q((*out).pts, ffi::av_buffersink_get_time_base(self.sink), self.time_base);
            }
            (*out).best_effort_timestamp = (*out).pts;
            made.push(out);
        }
    }
}

impl Drop for Deinterlacer {
    fn drop(&mut self) {
        unsafe { ffi::avfilter_graph_free(&mut self.graph) };
    }
}
