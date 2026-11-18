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
    grab_named(CString::new(path).ok()?, width)
}

/// The same, of a file on this machine by the system's own name for it: one that is not UTF-8
/// is opened as it is, where text would have been a different name.
pub fn grab_file(path: &std::path::Path, width: u32) -> Option<Still> {
    #[cfg(unix)]
    let name = {
        use std::os::unix::ffi::OsStrExt;
        CString::new(path.as_os_str().as_bytes()).ok()?
    };
    #[cfg(not(unix))]
    let name = CString::new(path.to_str()?).ok()?;
    grab_named(name, width)
}

fn grab_named(name: CString, width: u32) -> Option<Still> {
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
                let grid = (*group).params.tile_grid;
                let upright = Upright::of(side_matrix((*grid).coded_side_data, (*grid).nb_coded_side_data));
                return Some(upright.apply(&still));
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
    let upright = Upright::of(side_matrix((*(*stream).codecpar).coded_side_data, (*(*stream).codecpar).nb_coded_side_data));
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
                // A decoder that is full takes no more until a frame is taken from it: then the
                // packet is given again, not lost.
                let mut sent = ffi::avcodec_send_packet(context, packet);
                loop {
                    if ffi::avcodec_receive_frame(context, frame) >= 0 {
                        still = shrink(frame, width);
                        break;
                    }
                    if sent != again() {
                        break;
                    }
                    sent = ffi::avcodec_send_packet(context, packet);
                }
                if still.is_some() {
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
    still.map(|still| upright.apply(&still))
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
    // Where each tile given to the decoder goes, in the order given: a decoder may hold a frame
    // back for a while, and gives them up in that order.
    let mut waiting = std::collections::VecDeque::new();
    // Every frame the decoder has ready, each put where its tile goes.
    let take = |context: *mut ffi::AVCodecContext, waiting: &mut std::collections::VecDeque<(usize, usize)>, canvas: &mut Vec<u8>, placed: &mut usize| {
        while ffi::avcodec_receive_frame(context, frame) >= 0 {
            if let (Some((x, y)), Some(tile)) = (waiting.pop_front(), shrink(frame, (*frame).width as u32)) {
                let (tw, th) = (tile.width as usize, tile.height as usize);
                for row in 0..th.min(ch.saturating_sub(y)) {
                    let take = tw.min(cw.saturating_sub(x)) * 4;
                    let to = ((y + row) * cw + x) * 4;
                    canvas[to..to + take].copy_from_slice(&tile.rgba[row * tw * 4..row * tw * 4 + take]);
                }
                *placed += 1;
            }
            ffi::av_frame_unref(frame);
        }
    };
    let mut given = 0usize;
    while given < tile_of.len() && ffi::av_read_frame(format, packet) >= 0 {
        if let Some(&at) = tile_of.get(&(*packet).stream_index) {
            loop {
                let sent = ffi::avcodec_send_packet(context, packet);
                if sent >= 0 {
                    waiting.push_back(at);
                    given += 1;
                }
                take(context, &mut waiting, &mut canvas, &mut placed);
                // Full, it has given up what it had: the packet again. Otherwise on to the next.
                if sent != again() {
                    break;
                }
            }
        }
        ffi::av_packet_unref(packet);
    }
    // What it still holds, once it is told there is no more.
    ffi::avcodec_send_packet(context, ptr::null());
    take(context, &mut waiting, &mut canvas, &mut placed);
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

/// What a decoder says when it cannot take a packet until a frame is taken from it.
fn again() -> c_int {
    -(ffi::EAGAIN as c_int)
}

/// The display matrix among a stream's (or a grid's) side data, if it has one.
pub(crate) unsafe fn side_matrix(data: *const ffi::AVPacketSideData, count: c_int) -> Option<[i32; 9]> {
    if data.is_null() || count <= 0 {
        return None;
    }
    let side = ffi::av_packet_side_data_get(data, count, ffi::AV_PKT_DATA_DISPLAYMATRIX);
    if side.is_null() || (*side).data.is_null() || (*side).size < 36 {
        return None;
    }
    let mut matrix = [0i32; 9];
    ptr::copy_nonoverlapping((*side).data as *const i32, matrix.as_mut_ptr(), 9);
    Some(matrix)
}

/// How a picture is put the right way up: across and down swapped first, then mirrored left for
/// right, then upside down -- which between them make every quarter turn, mirrored or not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Upright {
    pub swap: bool,
    pub mirror: bool,
    pub flip: bool,
}

impl Upright {
    /// What a display matrix asks -- a phone held upright, an iPhone's HEIF turned with `irot`,
    /// mirrored with `imir` -- as FFmpeg's own player reads it (fftools' autorotate). Anything but
    /// a quarter turn, mirrored or not, is shown as it is.
    pub fn of(matrix: Option<[i32; 9]>) -> Upright {
        let Some(m) = matrix else { return Upright::default() };
        let mut theta = -unsafe { ffi::av_display_rotation_get(m.as_ptr()) }.round();
        if theta.is_nan() {
            return Upright::default();
        }
        theta -= 360.0 * (theta / 360.0 + 0.9 / 360.0).floor();
        let near = |angle: f64| (theta - angle).abs() < 1.0;
        let (swap, mirror, flip) = if near(90.0) {
            // transpose=cclock_flip if m[3] > 0, else transpose=clock.
            if m[3] > 0 { (true, false, false) } else { (true, true, false) }
        } else if near(180.0) {
            (false, m[0] < 0, m[4] < 0)
        } else if near(270.0) {
            // transpose=clock_flip if m[3] < 0, else transpose=cclock.
            if m[3] < 0 { (true, true, true) } else { (true, false, true) }
        } else if near(0.0) || near(360.0) {
            (false, false, m[4] < 0)
        } else {
            (false, false, false)
        };
        Upright { swap, mirror, flip }
    }

    pub fn apply(self, still: &Still) -> Still {
        if self == Upright::default() {
            return Still { width: still.width, height: still.height, rgba: still.rgba.clone() };
        }
        let (w, h) = (still.width as usize, still.height as usize);
        let (ow, oh) = if self.swap { (h, w) } else { (w, h) };
        let mut rgba = vec![0u8; ow * oh * 4];
        for y in 0..oh {
            for x in 0..ow {
                let (x1, y1) = (if self.mirror { ow - 1 - x } else { x }, if self.flip { oh - 1 - y } else { y });
                let (sx, sy) = if self.swap { (y1, x1) } else { (x1, y1) };
                let from = (sy * w + sx) * 4;
                let to = (y * ow + x) * 4;
                rgba[to..to + 4].copy_from_slice(&still.rgba[from..from + 4]);
            }
        }
        Still { width: ow as u32, height: oh as u32, rgba }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A display matrix turning a picture `angle` degrees anticlockwise, as FFmpeg makes one,
    /// mirrored left for right if asked.
    fn matrix(angle: f64, mirrored: bool) -> [i32; 9] {
        let mut m = [0i32; 9];
        unsafe {
            ffi::av_display_rotation_set(m.as_mut_ptr(), angle);
            if mirrored {
                ffi::av_display_matrix_flip(m.as_mut_ptr(), 1, 0);
            }
        }
        m
    }

    /// Two by one: A then B, as red 1 and 2.
    fn two() -> Still {
        Still { width: 2, height: 1, rgba: vec![1, 0, 0, 255, 2, 0, 0, 255] }
    }

    fn reds(still: &Still) -> (u32, u32, Vec<u8>) {
        (still.width, still.height, still.rgba.chunks_exact(4).map(|p| p[0]).collect())
    }

    #[test]
    fn a_picture_is_put_upright_as_its_display_matrix_says() {
        // None, or none to speak of: as it is.
        assert_eq!(reds(&Upright::of(None).apply(&two())), (2, 1, vec![1, 2]));
        assert_eq!(reds(&Upright::of(Some(matrix(0.0, false))).apply(&two())), (2, 1, vec![1, 2]));
        // A phone held upright: ffprobe says "rotation of -90" (av_display_rotation_set of 90),
        // a quarter clockwise to put right, and its left end goes up.
        assert_eq!(reds(&Upright::of(Some(matrix(90.0, false))).apply(&two())), (1, 2, vec![1, 2]));
        // The other way, and half a turn.
        assert_eq!(reds(&Upright::of(Some(matrix(-90.0, false))).apply(&two())), (1, 2, vec![2, 1]));
        assert_eq!(reds(&Upright::of(Some(matrix(180.0, false))).apply(&two())), (2, 1, vec![2, 1]));
        // Mirrored only.
        assert_eq!(reds(&Upright::of(Some(matrix(0.0, true))).apply(&two())), (2, 1, vec![2, 1]));
    }
}
