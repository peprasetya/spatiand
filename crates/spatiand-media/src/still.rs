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
    // A phone's HEIF is a grid of small pictures, each a stream of its own, put together.
    for g in 0..(*format).nb_stream_groups as usize {
        let group = *(*format).stream_groups.add(g);
        if (*group).type_ == ffi::AV_STREAM_GROUP_PARAMS_TILE_GRID {
            if let Some(still) = from_grid(format, group, width) {
                return Some(still);
            }
        }
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
        // A picture is one packet, and a decoder running on threads holds its frame back
        // until it is told there is no more: told, it gives it up.
        if still.is_none() {
            ffi::avcodec_send_packet(context, ptr::null());
            if ffi::avcodec_receive_frame(context, frame) >= 0 {
                still = shrink(frame, width);
            }
        }
        ffi::av_frame_free(&mut frame);
        ffi::av_packet_free(&mut packet);
    }
    ffi::avcodec_free_context(&mut context);
    still
}

/// The tiles of a grid, each decoded and put where the grid says, the whole cropped to the
/// picture it is and made small.
unsafe fn from_grid(format: *mut ffi::AVFormatContext, group: *mut ffi::AVStreamGroup, width: u32) -> Option<Still> {
    let grid = (*group).params.tile_grid;
    if grid.is_null() || (*grid).nb_tiles == 0 {
        return None;
    }
    let (cw, ch) = ((*grid).coded_width as usize, (*grid).coded_height as usize);
    if cw == 0 || ch == 0 || cw * ch > 200_000_000 {
        return None;
    }
    let first = *(*group).streams;
    let codec = ffi::avcodec_find_decoder((*(*first).codecpar).codec_id);
    if codec.is_null() {
        return None;
    }
    let mut context = ffi::avcodec_alloc_context3(codec);
    ffi::avcodec_parameters_to_context(context, (*first).codecpar);
    (*context).thread_count = 1;
    if ffi::avcodec_open2(context, codec, ptr::null_mut()) < 0 {
        ffi::avcodec_free_context(&mut context);
        return None;
    }
    // Which tile each stream is, by the stream's index in the file.
    let mut tile_of = std::collections::HashMap::new();
    for t in 0..(*grid).nb_tiles as usize {
        let offset = *(*grid).offsets.add(t);
        if (offset.idx as usize) < (*group).nb_streams as usize {
            let stream = *(*group).streams.add(offset.idx as usize);
            tile_of.insert((*stream).index, (offset.horizontal.max(0) as usize, offset.vertical.max(0) as usize));
        }
    }
    let mut canvas = vec![0u8; cw * ch * 4];
    let mut placed = 0usize;
    let mut packet = ffi::av_packet_alloc();
    let mut frame = ffi::av_frame_alloc();
    while placed < tile_of.len() && ffi::av_read_frame(format, packet) >= 0 {
        if let Some(&(x, y)) = tile_of.get(&(*packet).stream_index) {
            ffi::avcodec_send_packet(context, packet);
            let mut got = ffi::avcodec_receive_frame(context, frame) >= 0;
            if !got {
                ffi::avcodec_send_packet(context, ptr::null());
                got = ffi::avcodec_receive_frame(context, frame) >= 0;
                ffi::avcodec_flush_buffers(context);
            }
            if got {
                if let Some(tile) = shrink(frame, (*frame).width as u32) {
                    let (tw, th) = (tile.width as usize, tile.height as usize);
                    for row in 0..th.min(ch.saturating_sub(y)) {
                        let take = tw.min(cw.saturating_sub(x)) * 4;
                        let to = ((y + row) * cw + x) * 4;
                        canvas[to..to + take].copy_from_slice(&tile.rgba[row * tw * 4..row * tw * 4 + take]);
                    }
                    placed += 1;
                }
                ffi::av_frame_unref(frame);
            }
        }
        ffi::av_packet_unref(packet);
    }
    ffi::av_frame_free(&mut frame);
    ffi::av_packet_free(&mut packet);
    ffi::avcodec_free_context(&mut context);
    if placed == 0 {
        return None;
    }
    // Cropped to the picture the grid is of.
    let (ox, oy) = ((*grid).horizontal_offset.max(0) as usize, (*grid).vertical_offset.max(0) as usize);
    let (w, h) = (((*grid).width.max(1) as usize).min(cw - ox.min(cw)), ((*grid).height.max(1) as usize).min(ch - oy.min(ch)));
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let from = ((oy + row) * cw + ox) * 4;
        rgba.extend_from_slice(&canvas[from..from + w * 4]);
    }
    let whole = Still { width: w as u32, height: h as u32, rgba };
    Some(if whole.width <= width { whole } else { smaller(&whole, width) })
}

/// A picture made smaller in memory: a box filter, which is what shrinking by a lot wants.
fn smaller(still: &Still, width: u32) -> Still {
    let (fw, fh) = (still.width as usize, still.height as usize);
    let tw = (width as usize).max(1);
    let th = ((fh * tw) / fw).max(1);
    let mut rgba = vec![0u8; tw * th * 4];
    for y in 0..th {
        let (y0, y1) = (y * fh / th, ((y + 1) * fh / th).max(y * fh / th + 1));
        for x in 0..tw {
            let (x0, x1) = (x * fw / tw, ((x + 1) * fw / tw).max(x * fw / tw + 1));
            let mut sum = [0u32; 4];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = (sy * fw + sx) * 4;
                    for c in 0..4 {
                        sum[c] += still.rgba[i + c] as u32;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let o = (y * tw + x) * 4;
            for c in 0..4 {
                rgba[o + c] = (sum[c] / n) as u8;
            }
        }
    }
    Still { width: tw as u32, height: th as u32, rgba }
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
