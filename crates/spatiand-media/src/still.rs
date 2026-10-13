//! One picture out of a film, for a library to show in its place.
//!
//! Decoded in software, a little way in -- a film's first frame is black more often than not
//! -- and made small. Nothing is kept anywhere but in the memory of whoever asked: a library
//! shows these for as long as it is open and makes them again the next time.

use std::ffi::{c_int, CString};
use std::ptr;

use rsmpeg::ffi;

/// A small picture: four bytes a pixel, red first, top row first.
pub struct Still {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A frame of the film at `path`, no wider than `width`. `None` for anything that will not
/// give one, without saying why: a tile with no picture is all that comes of it.
pub fn grab(path: &str, width: u32) -> Option<Still> {
    let name = CString::new(path).ok()?;
    unsafe {
        let mut format = ptr::null_mut();
        if ffi::avformat_open_input(&mut format, name.as_ptr(), ptr::null(), ptr::null_mut()) < 0 {
            return None;
        }
        let still = from_open(format, width);
        ffi::avformat_close_input(&mut format);
        still
    }
}

unsafe fn from_open(format: *mut ffi::AVFormatContext, width: u32) -> Option<Still> {
    if ffi::avformat_find_stream_info(format, ptr::null_mut()) < 0 {
        return None;
    }
    let index = ffi::av_find_best_stream(format, ffi::AVMEDIA_TYPE_VIDEO, -1, -1, ptr::null_mut(), 0);
    if index < 0 {
        return None;
    }
    let stream = *(*format).streams.add(index as usize);
    let codec = ffi::avcodec_find_decoder((*(*stream).codecpar).codec_id);
    if codec.is_null() {
        return None;
    }
    let mut context = ffi::avcodec_alloc_context3(codec);
    ffi::avcodec_parameters_to_context(context, (*stream).codecpar);
    (*context).thread_count = 2;
    let mut still = None;
    if ffi::avcodec_open2(context, codec, ptr::null_mut()) >= 0 {
        // A tenth of the way in, or ten seconds, whichever is sooner: past the studio's cards
        // without being somewhere that gives the film away.
        if (*format).duration > 0 {
            let to = ((*format).duration / 10).min(10 * ffi::AV_TIME_BASE as i64);
            let start = if (*format).start_time != ffi::AV_NOPTS_VALUE { (*format).start_time } else { 0 };
            ffi::av_seek_frame(format, -1, start + to, ffi::AVSEEK_FLAG_BACKWARD as c_int);
        }
        let mut packet = ffi::av_packet_alloc();
        let mut frame = ffi::av_frame_alloc();
        // The first picture the decoder gives after the seek's key frame. Bounded: a file
        // that never yields one is given up on.
        'reading: for _ in 0..600 {
            if ffi::av_read_frame(format, packet) < 0 {
                break;
            }
            if (*packet).stream_index == index {
                ffi::avcodec_send_packet(context, packet);
                if ffi::avcodec_receive_frame(context, frame) >= 0 {
                    still = shrink(frame, width);
                    ffi::av_packet_unref(packet);
                    break 'reading;
                }
            }
            ffi::av_packet_unref(packet);
        }
        ffi::av_frame_free(&mut frame);
        ffi::av_packet_free(&mut packet);
    }
    ffi::avcodec_free_context(&mut context);
    still
}

unsafe fn shrink(frame: *const ffi::AVFrame, width: u32) -> Option<Still> {
    let (from_w, from_h) = ((*frame).width, (*frame).height);
    if from_w <= 0 || from_h <= 0 {
        return None;
    }
    let to_w = (width as c_int).min(from_w).max(2);
    let to_h = ((to_w as i64 * from_h as i64 / from_w as i64) as c_int).max(2);
    let scaler = ffi::sws_getContext(
        from_w, from_h, (*frame).format, to_w, to_h, ffi::AV_PIX_FMT_RGBA, ffi::SWS_BILINEAR as c_int, ptr::null_mut(), ptr::null_mut(), ptr::null(),
    );
    if scaler.is_null() {
        return None;
    }
    let mut rgba = vec![0u8; (to_w * to_h * 4) as usize];
    let planes = [rgba.as_mut_ptr(), ptr::null_mut(), ptr::null_mut(), ptr::null_mut()];
    let strides = [to_w * 4, 0, 0, 0];
    ffi::sws_scale(scaler, (*frame).data.as_ptr() as *const *const u8, (*frame).linesize.as_ptr(), 0, from_h, planes.as_ptr(), strides.as_ptr());
    ffi::sws_freeContext(scaler);
    Some(Still { width: to_w as u32, height: to_h as u32, rgba })
}
