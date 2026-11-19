//! One picture out of a film, for a library to show in its place -- or a picture file FFmpeg
//! reads (HEIF, AVIF, JPEG XL, JPEG 2000) whole, at its own depth, for a viewer to save.
//!
//! Decoded in software, a little way in -- a film's first frame is black more often than not
//! -- and made small. Nothing is kept anywhere but in the memory of whoever asked: a library
//! shows these for as long as it is open and makes them again the next time.
//!
//! Its colours are made RGB as the picture says they are: the matrix of its YUV (BT.601, 709,
//! 2020) and its range (full, as phones write their photographs, or studio).

use std::ffi::{c_int, CString};
use std::ptr;

use rsmpeg::ffi;

/// A small picture: four bytes a pixel, red first, top row first.
pub struct Still {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A picture whole, at its own size and the right way up, its samples sixteen bits wide whatever
/// they were in the file: what a ten-bit HEIF or AVIF, or a sixteen-bit JPEG XL, keeps of itself.
pub struct Deep {
    pub width: u32,
    pub height: u32,
    /// Four samples a pixel, red first, top row first.
    pub rgba: Vec<u16>,
    /// How many bits each sample had in the file: 8, 10, 12, 16.
    pub bits: u8,
    /// Whether it has see-through parts at all.
    pub alpha: bool,
    /// Its colours' profile, where FFmpeg found one.
    pub icc: Option<Vec<u8>>,
    /// Its colours' code points (ITU-T H.273): primaries and transfer; 2, unspecified.
    pub primaries: u8,
    pub transfer: u8,
}

/// A frame of the film at `path`, no wider than `width`. `None` for anything that will not
/// give one, without saying why: a tile with no picture is all that comes of it.
pub fn grab(path: &str, width: u32) -> Option<Still> {
    grab_named(CString::new(path).ok()?, width)
}

/// The same, of a file on this machine by the system's own name for it: one that is not UTF-8
/// is opened as it is, where text would have been a different name.
pub fn grab_file(path: &std::path::Path, width: u32) -> Option<Still> {
    grab_named(file_name(path)?, width)
}

/// The picture at `path` (a file, or an address FFmpeg reads) whole, at sixteen bits.
pub fn whole(path: &str) -> Option<Deep> {
    whole_named(CString::new(path).ok()?)
}

/// The same, of a file on this machine.
pub fn whole_file(path: &std::path::Path) -> Option<Deep> {
    whole_named(file_name(path)?)
}

fn file_name(path: &std::path::Path) -> Option<CString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        CString::new(path.as_os_str().as_bytes()).ok()
    }
    #[cfg(not(unix))]
    CString::new(path.to_str()?).ok()
}

fn grab_named(name: CString, width: u32) -> Option<Still> {
    let (w, h, rgba, _) = opened(name, |format| unsafe { from_open::<u8>(format, Some(width)) })?;
    Some(Still { width: w as u32, height: h as u32, rgba })
}

fn whole_named(name: CString) -> Option<Deep> {
    let (w, h, rgba, about) = opened(name, |format| unsafe { from_open::<u16>(format, None) })?;
    Some(Deep { width: w as u32, height: h as u32, rgba, bits: about.bits, alpha: about.alpha, icc: about.icc, primaries: about.primaries, transfer: about.transfer })
}

fn opened<T>(name: CString, read: impl FnOnce(*mut ffi::AVFormatContext) -> Option<T>) -> Option<T> {
    unsafe {
        let mut format = ptr::null_mut();
        if ffi::avformat_open_input(&mut format, name.as_ptr(), ptr::null(), ptr::null_mut()) < 0 {
            return None;
        }
        let read = read(format);
        ffi::avformat_close_input(&mut format);
        read
    }
}

/// A sample of a picture in memory: eight bits (RGBA) or sixteen (RGBA64, this machine's order).
trait Sample: Copy + Default + Into<u32> + TryFrom<u32> {
    /// A frame as RGB with alpha, no wider than `width`.
    unsafe fn pixels(frame: *const ffi::AVFrame, width: u32) -> Option<(usize, usize, Vec<Self>)>;
}

impl Sample for u8 {
    /// By the scaler, told the matrix and the range of the frame's YUV, which it otherwise takes
    /// for BT.601 at studio range whatever it is: a phone's full-range photograph came out too
    /// contrasty, and a BT.709 one's colours off.
    unsafe fn pixels(frame: *const ffi::AVFrame, width: u32) -> Option<(usize, usize, Vec<u8>)> {
        let (w, h) = fitted(frame, width)?;
        let scaler = ffi::sws_getContext((*frame).width, (*frame).height, (*frame).format, w, h, ffi::AV_PIX_FMT_RGBA, ffi::SWS_BILINEAR as c_int, ptr::null_mut(), ptr::null_mut(), ptr::null());
        if scaler.is_null() {
            return None;
        }
        if let Some((matrix, full)) = yuv(frame) {
            let coefficients = ffi::sws_getCoefficients(matrix as c_int);
            let rgb = ffi::sws_getCoefficients(ffi::SWS_CS_DEFAULT as c_int);
            ffi::sws_setColorspaceDetails(scaler, coefficients, full as c_int, rgb, 1, 0, 1 << 16, 1 << 16);
        }
        let px = scaled(scaler, frame, w, h, 4);
        ffi::sws_freeContext(scaler);
        Some((w as usize, h as usize, px))
    }
}

impl Sample for u16 {
    /// An RGB frame by the scaler. A YUV one by the scaler only as far as YUV of sixteen bits,
    /// every pixel its own chroma -- its values as they were, the scaler told nothing of range --
    /// and made RGB here, by its own matrix and range: the scaler's sixteen-bit RGB, told them,
    /// gave nonsense (green and blue lost) where its eight-bit RGB is right.
    unsafe fn pixels(frame: *const ffi::AVFrame, width: u32) -> Option<(usize, usize, Vec<u16>)> {
        let (w, h) = fitted(frame, width)?;
        let Some((matrix, full)) = yuv(frame) else {
            let to = if cfg!(target_endian = "little") { ffi::AV_PIX_FMT_RGBA64LE } else { ffi::AV_PIX_FMT_RGBA64BE };
            let scaler = ffi::sws_getContext((*frame).width, (*frame).height, (*frame).format, w, h, to, ffi::SWS_BILINEAR as c_int, ptr::null_mut(), ptr::null_mut(), ptr::null());
            if scaler.is_null() {
                return None;
            }
            let px = scaled(scaler, frame, w, h, 4);
            ffi::sws_freeContext(scaler);
            return Some((w as usize, h as usize, px));
        };
        // A full-range ("J") format is the same samples as its studio one: named so, the scaler
        // leaves them be.
        let from = match (*frame).format {
            ffi::AV_PIX_FMT_YUVJ420P => ffi::AV_PIX_FMT_YUV420P,
            ffi::AV_PIX_FMT_YUVJ422P => ffi::AV_PIX_FMT_YUV422P,
            ffi::AV_PIX_FMT_YUVJ444P => ffi::AV_PIX_FMT_YUV444P,
            ffi::AV_PIX_FMT_YUVJ440P => ffi::AV_PIX_FMT_YUV440P,
            ffi::AV_PIX_FMT_YUVJ411P => ffi::AV_PIX_FMT_YUV411P,
            other => other,
        };
        let desc = ffi::av_pix_fmt_desc_get((*frame).format);
        let bits = (*desc).comp[0].depth.clamp(1, 16) as u32;
        let scaler = ffi::sws_getContext((*frame).width, (*frame).height, from, w, h, ffi::AV_PIX_FMT_YUVA444P16LE, ffi::SWS_BILINEAR as c_int, ptr::null_mut(), ptr::null_mut(), ptr::null());
        if scaler.is_null() {
            return None;
        }
        let (n, size) = ((w * h) as usize, std::mem::size_of::<u16>());
        let mut planes: Vec<Vec<u16>> = (0..4).map(|_| vec![0u16; n]).collect();
        let pointers = [planes[0].as_mut_ptr() as *mut u8, planes[1].as_mut_ptr() as *mut u8, planes[2].as_mut_ptr() as *mut u8, planes[3].as_mut_ptr() as *mut u8];
        let strides = [w * size as c_int; 4];
        ffi::sws_scale(scaler, (*frame).data.as_ptr() as *const *const u8, (*frame).linesize.as_ptr(), 0, (*frame).height, pointers.as_ptr(), strides.as_ptr());
        ffi::sws_freeContext(scaler);
        let alpha = !desc.is_null() && (*desc).flags & ffi::AV_PIX_FMT_FLAG_ALPHA as u64 != 0;
        let mut px = vec![0u16; n * 4];
        let to_rgb = Yuv::new(matrix, full, bits);
        for i in 0..n {
            // Back to the samples' own code values: the scaler widened them to sixteen bits.
            let code = |v: u16| u16::from_le(v) as u32 >> (16 - bits);
            let rgb = to_rgb.rgb(code(planes[0][i]), code(planes[1][i]), code(planes[2][i]));
            px[i * 4..i * 4 + 3].copy_from_slice(&rgb);
            px[i * 4 + 3] = if alpha { u16::from_le(planes[3][i]) } else { u16::MAX };
        }
        Some((w as usize, h as usize, px))
    }
}

/// A frame's size, no wider than `width` (and at least two pixels each way).
unsafe fn fitted(frame: *const ffi::AVFrame, width: u32) -> Option<(c_int, c_int)> {
    let (from_w, from_h) = ((*frame).width, (*frame).height);
    if from_w <= 0 || from_h <= 0 {
        return None;
    }
    let w = (width.min(i32::MAX as u32) as c_int).min(from_w).max(2);
    let h = ((w as i64 * from_h as i64 / from_w as i64) as c_int).max(2);
    Some((w, h))
}

/// The matrix of a YUV frame (an AVColorSpace) and whether it is at full range; `None` for RGB.
unsafe fn yuv(frame: *const ffi::AVFrame) -> Option<(ffi::AVColorSpace, bool)> {
    let desc = ffi::av_pix_fmt_desc_get((*frame).format);
    if desc.is_null() || (*desc).nb_components < 3 || (*desc).flags & ffi::AV_PIX_FMT_FLAG_RGB as u64 != 0 {
        return None;
    }
    let j = matches!((*frame).format, ffi::AV_PIX_FMT_YUVJ420P | ffi::AV_PIX_FMT_YUVJ422P | ffi::AV_PIX_FMT_YUVJ444P | ffi::AV_PIX_FMT_YUVJ440P | ffi::AV_PIX_FMT_YUVJ411P);
    Some(((*frame).colorspace, j || (*frame).color_range == ffi::AVCOL_RANGE_JPEG))
}

/// The frame scaled into one interleaved plane of `channels` samples a pixel.
unsafe fn scaled<T: Sample>(scaler: *mut ffi::SwsContext, frame: *const ffi::AVFrame, w: c_int, h: c_int, channels: c_int) -> Vec<T> {
    let size = std::mem::size_of::<T>() as c_int;
    let mut px = vec![T::default(); (w * h * channels) as usize];
    let planes = [px.as_mut_ptr() as *mut u8, ptr::null_mut(), ptr::null_mut(), ptr::null_mut()];
    let strides = [w * channels * size, 0, 0, 0];
    ffi::sws_scale(scaler, (*frame).data.as_ptr() as *const *const u8, (*frame).linesize.as_ptr(), 0, (*frame).height, planes.as_ptr(), strides.as_ptr());
    px
}

/// YUV made RGB, as ITU-T H.273 has it: the matrix's red and blue shares, and the range.
#[derive(Debug, Clone, Copy)]
struct Yuv {
    kr: f32,
    kb: f32,
    /// Green, blue and red as they are (the identity matrix).
    identity: bool,
    /// What black is and how far it is to white, and the middle of the chroma and its reach.
    black: f32,
    span: f32,
    middle: f32,
    reach: f32,
}

impl Yuv {
    fn new(matrix: ffi::AVColorSpace, full: bool, bits: u32) -> Yuv {
        let (kr, kb) = match matrix {
            ffi::AVCOL_SPC_BT709 => (0.2126, 0.0722),
            ffi::AVCOL_SPC_FCC => (0.30, 0.11),
            ffi::AVCOL_SPC_SMPTE240M => (0.212, 0.087),
            ffi::AVCOL_SPC_BT2020_NCL | ffi::AVCOL_SPC_BT2020_CL => (0.2627, 0.0593),
            // BT.601 (470BG, 170M), and what says nothing: HEIF's and AVIF's own default.
            _ => (0.299, 0.114),
        };
        let unit = (1u32 << bits.saturating_sub(8)) as f32;
        let max = ((1u64 << bits) - 1) as f32;
        let (black, span, reach) = if full { (0.0, max, max) } else { (16.0 * unit, 219.0 * unit, 224.0 * unit) };
        Yuv { kr, kb, identity: matrix == ffi::AVCOL_SPC_RGB, black, span, middle: (1u32 << (bits - 1)) as f32, reach }
    }

    /// One pixel's code values made sixteen-bit RGB.
    fn rgb(&self, y: u32, cb: u32, cr: u32) -> [u16; 3] {
        let y = (y as f32 - self.black) / self.span;
        let (cb, cr) = if self.identity {
            ((cb as f32 - self.black) / self.span, (cr as f32 - self.black) / self.span)
        } else {
            ((cb as f32 - self.middle) / self.reach, (cr as f32 - self.middle) / self.reach)
        };
        let (r, g, b) = if self.identity {
            (cr, y, cb)
        } else {
            let r = y + 2.0 * (1.0 - self.kr) * cr;
            let b = y + 2.0 * (1.0 - self.kb) * cb;
            (r, (y - self.kr * r - self.kb * b) / (1.0 - self.kr - self.kb), b)
        };
        let wide = |v: f32| (v.clamp(0.0, 1.0) * 65535.0).round() as u16;
        [wide(r), wide(g), wide(b)]
    }
}

/// What a picture's file said of it, beside its pixels.
#[derive(Default)]
struct About {
    bits: u8,
    alpha: bool,
    icc: Option<Vec<u8>>,
    primaries: u8,
    transfer: u8,
}

/// What is known of a decoded frame: its depth, whether it is see-through, its colours.
unsafe fn about(frame: *const ffi::AVFrame, side: Option<Vec<u8>>) -> About {
    let desc = ffi::av_pix_fmt_desc_get((*frame).format);
    let (bits, alpha) = if desc.is_null() { (8, false) } else { ((*desc).comp[0].depth as u8, (*desc).flags & ffi::AV_PIX_FMT_FLAG_ALPHA as u64 != 0) };
    let icc = {
        let data = ffi::av_frame_get_side_data(frame, ffi::AV_FRAME_DATA_ICC_PROFILE);
        if data.is_null() || (*data).data.is_null() || (*data).size == 0 { None } else { Some(std::slice::from_raw_parts((*data).data, (*data).size).to_vec()) }
    };
    About { bits, alpha, icc: icc.or(side), primaries: (*frame).color_primaries as u8, transfer: (*frame).color_trc as u8 }
}

/// A stream's (or a grid's) colour profile among its side data.
unsafe fn side_icc(data: *const ffi::AVPacketSideData, count: c_int) -> Option<Vec<u8>> {
    if data.is_null() || count <= 0 {
        return None;
    }
    let side = ffi::av_packet_side_data_get(data, count, ffi::AV_PKT_DATA_ICC_PROFILE);
    if side.is_null() || (*side).data.is_null() || (*side).size == 0 {
        return None;
    }
    Some(std::slice::from_raw_parts((*side).data, (*side).size).to_vec())
}

/// The picture of an open file: its pixels (four samples each), their size, and what is known
/// of it. Made no wider than `width`, or whole if `None` -- a still, which is not sought into.
unsafe fn from_open<T: Sample>(format: *mut ffi::AVFormatContext, width: Option<u32>) -> Option<(usize, usize, Vec<T>, About)> {
    if ffi::avformat_find_stream_info(format, ptr::null_mut()) < 0 {
        return None;
    }
    // A phone's HEIF is a grid of small pictures, each a stream of its own, put together.
    for g in 0..(*format).nb_stream_groups as usize {
        let group = *(*format).stream_groups.add(g);
        if (*group).type_ == ffi::AV_STREAM_GROUP_PARAMS_TILE_GRID {
            if let Some((w, h, px, mut about)) = from_grid::<T>(format, group) {
                let grid = (*group).params.tile_grid;
                about.icc = about.icc.or_else(|| side_icc((*grid).coded_side_data, (*grid).nb_coded_side_data));
                let upright = Upright::of(side_matrix((*grid).coded_side_data, (*grid).nb_coded_side_data));
                let (w, h, px) = upright.apply_to(w, h, &px);
                return Some(match width {
                    Some(width) if w > width as usize => {
                        let (w, h, px) = smaller(w, h, &px, width);
                        (w, h, px, about)
                    }
                    _ => (w, h, px, about),
                });
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
    let codecpar = (*stream).codecpar;
    let upright = Upright::of(side_matrix((*codecpar).coded_side_data, (*codecpar).nb_coded_side_data));
    let side = side_icc((*codecpar).coded_side_data, (*codecpar).nb_coded_side_data);
    let mut context = ffi::avcodec_alloc_context3(codec);
    ffi::avcodec_parameters_to_context(context, codecpar);
    (*context).thread_count = 2;
    let mut still = None;
    if ffi::avcodec_open2(context, codec, ptr::null_mut()) >= 0 {
        // A tenth of the way in, or ten seconds, whichever is sooner: past the studio's cards
        // without being somewhere that gives the film away.
        if width.is_some() && (*format).duration > 0 {
            let to = ((*format).duration / 10).min(10 * ffi::AV_TIME_BASE as i64);
            let start = if (*format).start_time != ffi::AV_NOPTS_VALUE { (*format).start_time } else { 0 };
            ffi::av_seek_frame(format, -1, start + to, ffi::AVSEEK_FLAG_BACKWARD as c_int);
        }
        let mut packet = ffi::av_packet_alloc();
        let mut frame = ffi::av_frame_alloc();
        let take = |frame: *mut ffi::AVFrame| T::pixels(frame, width.unwrap_or(u32::MAX)).map(|(w, h, px)| (w, h, px, about(frame, side.clone())));
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
                        still = take(frame);
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
                still = take(frame);
            }
        }
        ffi::av_frame_free(&mut frame);
        ffi::av_packet_free(&mut packet);
    }
    ffi::avcodec_free_context(&mut context);
    still.map(|(w, h, px, about)| {
        let (w, h, px) = upright.apply_to(w, h, &px);
        (w, h, px, about)
    })
}

/// The tiles of a grid, each decoded and put where the grid says, the whole cropped to the
/// picture it is.
unsafe fn from_grid<T: Sample>(format: *mut ffi::AVFormatContext, group: *mut ffi::AVStreamGroup) -> Option<(usize, usize, Vec<T>, About)> {
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
    let side = side_icc((*(*first).codecpar).coded_side_data, (*(*first).codecpar).nb_coded_side_data);
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
    let mut canvas = vec![T::default(); cw * ch * 4];
    let mut placed = 0usize;
    let mut about_it: Option<About> = None;
    let mut packet = ffi::av_packet_alloc();
    let mut frame = ffi::av_frame_alloc();
    // Where each tile given to the decoder goes, in the order given: a decoder may hold a frame
    // back for a while, and gives them up in that order.
    let mut waiting = std::collections::VecDeque::new();
    // Every frame the decoder has ready, each put where its tile goes.
    let take = |context: *mut ffi::AVCodecContext, waiting: &mut std::collections::VecDeque<(usize, usize)>, canvas: &mut Vec<T>, placed: &mut usize, about_it: &mut Option<About>| {
        while ffi::avcodec_receive_frame(context, frame) >= 0 {
            if let (Some((x, y)), Some((tw, th, tile))) = (waiting.pop_front(), T::pixels(frame, (*frame).width as u32)) {
                for row in 0..th.min(ch.saturating_sub(y)) {
                    let take = tw.min(cw.saturating_sub(x)) * 4;
                    let to = ((y + row) * cw + x) * 4;
                    canvas[to..to + take].copy_from_slice(&tile[row * tw * 4..row * tw * 4 + take]);
                }
                *placed += 1;
                if about_it.is_none() {
                    *about_it = Some(about(frame, side.clone()));
                }
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
                take(context, &mut waiting, &mut canvas, &mut placed, &mut about_it);
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
    take(context, &mut waiting, &mut canvas, &mut placed, &mut about_it);
    ffi::av_frame_free(&mut frame);
    ffi::av_packet_free(&mut packet);
    ffi::avcodec_free_context(&mut context);
    if placed == 0 {
        return None;
    }
    // Cropped to the picture the grid is of.
    let (ox, oy) = ((*grid).horizontal_offset.max(0) as usize, (*grid).vertical_offset.max(0) as usize);
    let (w, h) = (((*grid).width.max(1) as usize).min(cw - ox.min(cw)), ((*grid).height.max(1) as usize).min(ch - oy.min(ch)));
    let mut px = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let from = ((oy + row) * cw + ox) * 4;
        px.extend_from_slice(&canvas[from..from + w * 4]);
    }
    Some((w, h, px, about_it.unwrap_or_default()))
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
        let (w, h, rgba) = self.apply_to(still.width as usize, still.height as usize, &still.rgba);
        Still { width: w as u32, height: h as u32, rgba }
    }

    /// The same, of `w` by `h` pixels of four samples each, whatever a sample is.
    pub fn apply_to<T: Copy + Default>(self, w: usize, h: usize, px: &[T]) -> (usize, usize, Vec<T>) {
        if self == Upright::default() {
            return (w, h, px.to_vec());
        }
        let (ow, oh) = if self.swap { (h, w) } else { (w, h) };
        let mut out = vec![T::default(); ow * oh * 4];
        for y in 0..oh {
            for x in 0..ow {
                let (x1, y1) = (if self.mirror { ow - 1 - x } else { x }, if self.flip { oh - 1 - y } else { y });
                let (sx, sy) = if self.swap { (y1, x1) } else { (x1, y1) };
                let from = (sy * w + sx) * 4;
                let to = (y * ow + x) * 4;
                out[to..to + 4].copy_from_slice(&px[from..from + 4]);
            }
        }
        (ow, oh, out)
    }
}

/// A picture made smaller in memory, `fw` by `fh` pixels of four samples each, to `width`
/// across: a box filter, which is what shrinking by a lot wants.
fn smaller<T: Sample>(fw: usize, fh: usize, px: &[T], width: u32) -> (usize, usize, Vec<T>) {
    let tw = (width as usize).max(1);
    let th = ((fh * tw) / fw).max(1);
    let mut out = vec![T::default(); tw * th * 4];
    for y in 0..th {
        let (y0, y1) = (y * fh / th, ((y + 1) * fh / th).max(y * fh / th + 1));
        for x in 0..tw {
            let (x0, x1) = (x * fw / tw, ((x + 1) * fw / tw).max(x * fw / tw + 1));
            let mut sum = [0u64; 4];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = (sy * fw + sx) * 4;
                    for c in 0..4 {
                        sum[c] += px[i + c].into() as u64;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            let o = (y * tw + x) * 4;
            for c in 0..4 {
                out[o + c] = T::try_from((sum[c] / n) as u32).unwrap_or_default();
            }
        }
    }
    (tw, th, out)
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
    fn yuv_is_made_rgb_by_its_own_matrix_and_range() {
        // BT.601 at full range, eight bits: Y 128 and no chroma is the middle grey; red as a
        // phone writes it.
        let full = Yuv::new(ffi::AVCOL_SPC_SMPTE170M, true, 8);
        assert_eq!(full.rgb(128, 128, 128), [32896, 32896, 32896]);
        let [r, g, b] = full.rgb(76, 85, 255).map(|v| (v as f32 / 257.0).round() as i32);
        assert!((r - 255).abs() <= 1 && g.abs() <= 1 && b.abs() <= 1, "{r} {g} {b}");
        // Studio range: 16 is black, 235 white; ten bits, four times each.
        let studio = Yuv::new(ffi::AVCOL_SPC_BT709, false, 10);
        assert_eq!(studio.rgb(64, 512, 512), [0, 0, 0]);
        assert_eq!(studio.rgb(940, 512, 512), [65535, 65535, 65535]);
        // BT.709's matrix is not BT.601's: the same samples, another colour.
        assert_ne!(Yuv::new(ffi::AVCOL_SPC_BT709, true, 8).rgb(76, 85, 255), full.rgb(76, 85, 255));
        // The identity matrix: green, blue and red as they are.
        assert_eq!(Yuv::new(ffi::AVCOL_SPC_RGB, true, 8).rgb(255, 0, 0), [0, 65535, 0]);
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
        // Sixteen bits a sample, the same.
        let deep: Vec<u16> = vec![1, 0, 0, 65535, 2, 0, 0, 65535];
        let (w, h, px) = Upright::of(Some(matrix(90.0, false))).apply_to(2, 1, &deep);
        assert_eq!((w, h, px[0], px[4]), (1, 2, 1, 2));
    }
}
