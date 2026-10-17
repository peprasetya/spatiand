//! A web page's MediaSource, played by the player like a file (SpatiMesh, docs/spatimesh.md,
//! "Media").
//!
//! A page streaming a film (YouTube, most players) does not give an address: it makes a
//! MediaSource, a SourceBuffer for each of its tracks (one for the picture, one for the sound),
//! and appends pieces of a fragmented MP4 or a WebM to each as it fetches them -- first an
//! initialization segment saying what the track is, then media segments of a few seconds each,
//! in whatever order and from whatever quality it chooses. This is the window's half of that:
//!
//! * each SourceBuffer's bytes are read by FFmpeg's own demuxer, on a thread of its own, as they
//!   come (`Parser`). A new initialization segment starts a new demuxer from it (a page
//!   changing quality appends one), and the coded frames of each are kept, by time, in the
//!   track's buffer -- what a frame appended over replaces, removed first, as the MSE
//!   specification has it;
//! * an append is finished when the demuxer has read everything given it and is waiting for
//!   more, which is what the page's `updateend` waits for;
//! * the player reads the tracks' frames in order of their times from wherever it is asked to
//!   be ([`crate::player::Player::open_source`]), waiting where nothing has been appended yet.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::{c_int, c_void};
use std::ptr;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use rsmpeg::ffi;

/// A track's frames are kept to times in microseconds, whatever the container said.
pub(crate) const MICROS: ffi::AVRational = ffi::AVRational { num: 1, den: 1_000_000 };

/// How much of a SourceBuffer's frames may be held before the oldest are let go of.
const QUOTA: usize = 160 << 20;

/// What a track is, from its initialization segment: FFmpeg's description of it, owned.
pub(crate) struct Params(pub *mut ffi::AVCodecParameters);
unsafe impl Send for Params {}
unsafe impl Sync for Params {}
impl Drop for Params {
    fn drop(&mut self) {
        unsafe { ffi::avcodec_parameters_free(&mut self.0) };
    }
}

impl Params {
    fn copy_of(from: *const ffi::AVCodecParameters) -> Arc<Params> {
        unsafe {
            let to = ffi::avcodec_parameters_alloc();
            ffi::avcodec_parameters_copy(to, from);
            Arc::new(Params(to))
        }
    }

    pub(crate) fn kind(&self) -> ffi::AVMediaType {
        unsafe { (*self.0).codec_type }
    }

    pub(crate) fn extradata(&self) -> &[u8] {
        unsafe {
            if (*self.0).extradata.is_null() || (*self.0).extradata_size <= 0 {
                &[]
            } else {
                std::slice::from_raw_parts((*self.0).extradata, (*self.0).extradata_size as usize)
            }
        }
    }
}

/// One coded frame: its bytes, its times in microseconds, and what it was coded with.
pub(crate) struct Frame {
    pub data: Vec<u8>,
    pub pts: i64,
    pub dts: i64,
    pub duration: i64,
    pub key: bool,
    pub params: Arc<Params>,
}

/// A track's frames by decode time.
#[derive(Default)]
pub(crate) struct Track {
    pub frames: BTreeMap<i64, Frame>,
    pub params: Option<Arc<Params>>,
    bytes: usize,
}

impl Track {
    /// Put frames of one media segment in, after taking out what they cover (MSE's "coded frame
    /// removal" for an overlapping append): [first pts, last pts + duration).
    fn insert(&mut self, batch: Vec<Frame>) {
        let Some(start) = batch.iter().map(|f| f.pts).min() else { return };
        let end = batch.iter().map(|f| f.pts + f.duration.max(1)).max().unwrap_or(start);
        self.remove(start, end);
        for frame in batch {
            self.bytes += frame.data.len();
            self.params = Some(frame.params.clone());
            self.frames.insert(frame.dts, frame);
        }
    }

    /// Take out the frames presented in [start, end), and for a picture the frames after them
    /// up to the next keyframe, which cannot be decoded without them.
    pub(crate) fn remove(&mut self, start: i64, end: i64) {
        let doomed: Vec<i64> = self.frames.iter().filter(|(_, f)| f.pts >= start && f.pts < end).map(|(d, _)| *d).collect();
        if doomed.is_empty() {
            return;
        }
        let last = *doomed.iter().max().unwrap();
        let mut gone = doomed;
        let video = self.params.as_ref().is_some_and(|p| p.kind() == ffi::AVMEDIA_TYPE_VIDEO);
        if video {
            for (dts, frame) in self.frames.range(last + 1..) {
                if frame.key {
                    break;
                }
                gone.push(*dts);
            }
        }
        for dts in gone {
            if let Some(f) = self.frames.remove(&dts) {
                self.bytes -= f.data.len();
            }
        }
    }

    /// What is held, as ranges of presentation time in seconds: frames closer together than
    /// a little more than a frame's length are one range.
    pub(crate) fn ranges(&self) -> Vec<(f64, f64)> {
        let mut spans: Vec<(i64, i64)> = self.frames.values().map(|f| (f.pts, f.pts + f.duration.max(1))).collect();
        spans.sort_unstable();
        let mut out: Vec<(i64, i64)> = Vec::new();
        for (s, e) in spans {
            match out.last_mut() {
                // A gap under a tenth of a second is not a gap a player stops for.
                Some(last) if s <= last.1 + 100_000 => last.1 = last.1.max(e),
                _ => out.push((s, e)),
            }
        }
        out.into_iter().map(|(s, e)| (s as f64 / 1e6, e as f64 / 1e6)).collect()
    }
}

/// A SourceBuffer's bytes as they arrive, for its demuxer to read; and whether the demuxer has
/// read all of them and is waiting.
#[derive(Default)]
struct Pipe {
    bytes: VecDeque<u8>,
    /// The demuxer is waiting for more: everything appended has been read.
    idle: bool,
    /// This demuxer is to stop: a new initialization segment has started another, or the
    /// SourceBuffer has gone.
    closed: bool,
    /// Bytes read so far, for `timestampOffset`s set between appends.
    read: u64,
    /// Where each append began, in bytes read, and the offset in force for it (microseconds).
    offsets: Vec<(u64, i64)>,
}

struct PipeShared {
    pipe: Mutex<Pipe>,
    more: Condvar,
}

/// One SourceBuffer: its tracks' frames and the demuxer filling them.
pub(crate) struct Buffer {
    pub tracks: Vec<Track>,
    /// The demuxer of the newest initialization segment, if it is still running.
    current: Option<Arc<PipeShared>>,
    container: Container,
    /// Bytes of the top-level element or box being read, not yet ended, across appends.
    scan: Scan,
    pub offset: i64,
    /// An append is being taken in.
    pub busy: bool,
    pub error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Container {
    Mp4,
    WebM,
}

/// Where the next top-level box or element of an append begins, carried across appends, to
/// see where an initialization segment starts.
#[derive(Default)]
struct Scan {
    /// Bytes still to skip before the next top-level header.
    skip: u64,
    /// The start of a header split between appends.
    partial: Vec<u8>,
    /// Inside a WebM Segment: its children are what is looked at.
    in_segment: bool,
    /// The last top-level box began an initialization segment: an MP4's `moov` after its
    /// `ftyp` is the same one.
    after_init: bool,
}

/// The MediaSource: its buffers, its length, and whether the page has said it is complete.
pub struct Source {
    pub(crate) state: Mutex<State>,
    pub(crate) changed: Condvar,
}

#[derive(Default)]
pub(crate) struct State {
    pub buffers: Vec<Option<Buffer>>,
    pub duration: f64,
    pub ended: bool,
    pub closed: bool,
}

impl Source {
    pub fn new() -> Arc<Source> {
        Arc::new(Source { state: Mutex::new(State { duration: f64::NAN, ..Default::default() }), changed: Condvar::new() })
    }

    /// A SourceBuffer for `mime` (`video/mp4; codecs="avc1.64001f"`); its number, or why not.
    pub fn add_buffer(&self, mime: &str) -> Result<usize, String> {
        let container = container_of(mime).ok_or_else(|| format!("{mime} is not a container this can read"))?;
        let mut state = self.state.lock().unwrap();
        state.buffers.push(Some(Buffer {
            tracks: Vec::new(),
            current: None,
            container,
            scan: Scan::default(),
            offset: 0,
            busy: false,
            error: None,
        }));
        Ok(state.buffers.len() - 1)
    }

    pub fn remove_buffer(&self, id: usize) {
        let mut state = self.state.lock().unwrap();
        if let Some(slot) = state.buffers.get_mut(id) {
            if let Some(buffer) = slot.take() {
                buffer.close();
            }
        }
        self.changed.notify_all();
    }

    /// `changeType`: what is appended from now on is in another container (or codec).
    pub fn change_type(&self, id: usize, mime: &str) -> Result<(), String> {
        let container = container_of(mime).ok_or_else(|| format!("{mime} is not a container this can read"))?;
        let mut state = self.state.lock().unwrap();
        let buffer = state.buffers.get_mut(id).and_then(|b| b.as_mut()).ok_or("no such SourceBuffer")?;
        buffer.container = container;
        buffer.scan = Scan::default();
        Ok(())
    }

    /// Give a SourceBuffer bytes. Taken in on its demuxer's thread: [`Source::append_done`]
    /// says when.
    pub fn append(self: &Arc<Source>, id: usize, bytes: &[u8]) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        let buffer = state.buffers.get_mut(id).and_then(|b| b.as_mut()).ok_or("no such SourceBuffer")?;
        if let Some(e) = buffer.error.clone() {
            return Err(e);
        }
        if buffer.size() > QUOTA {
            return Err("QuotaExceededError".into());
        }
        buffer.busy = true;
        // Where in these bytes an initialization segment starts, if one does: a new demuxer
        // from there, the old one left to finish what it was given before.
        let starts = buffer.scan_for_init(bytes);
        let mut at = 0;
        for start in starts.into_iter().chain(std::iter::once(bytes.len())) {
            if start > at {
                if buffer.current.is_none() {
                    // Media before any initialization segment: nothing can read it.
                    at = start;
                    continue;
                }
                buffer.feed(&bytes[at..start]);
            }
            if start < bytes.len() {
                if let Some(old) = buffer.current.take() {
                    old.close();
                }
                let pipe = Arc::new(PipeShared { pipe: Mutex::new(Pipe::default()), more: Condvar::new() });
                buffer.current = Some(pipe.clone());
                let offset = buffer.offset;
                pipe.pipe.lock().unwrap().offsets.push((0, offset));
                let source = Arc::downgrade(self);
                let container = buffer.container;
                let _ = std::thread::Builder::new().name("mse-demux".into()).spawn(move || demux(pipe, id, container, source));
            }
            at = start;
        }
        Ok(())
    }

    /// Whether the last append to a SourceBuffer has been taken in (and if it failed, why).
    pub fn append_done(&self, id: usize) -> Option<Result<(), String>> {
        let mut state = self.state.lock().unwrap();
        let buffer = state.buffers.get_mut(id).and_then(|b| b.as_mut())?;
        if let Some(e) = buffer.error.clone() {
            buffer.busy = false;
            return Some(Err(e));
        }
        if !buffer.busy {
            return Some(Ok(()));
        }
        let idle = buffer.current.as_ref().is_none_or(|p| {
            let pipe = p.pipe.lock().unwrap();
            pipe.idle && pipe.bytes.is_empty()
        });
        if idle {
            buffer.busy = false;
            Some(Ok(()))
        } else {
            None
        }
    }

    /// `abort()`: what was being taken in is dropped, and the next append starts afresh.
    pub fn abort(&self, id: usize) {
        let mut state = self.state.lock().unwrap();
        if let Some(buffer) = state.buffers.get_mut(id).and_then(|b| b.as_mut()) {
            if let Some(p) = buffer.current.as_ref() {
                p.pipe.lock().unwrap().bytes.clear();
            }
            buffer.busy = false;
            buffer.scan = Scan::default();
        }
    }

    pub fn remove(&self, id: usize, start: f64, end: f64) {
        let mut state = self.state.lock().unwrap();
        if let Some(buffer) = state.buffers.get_mut(id).and_then(|b| b.as_mut()) {
            let (s, e) = ((start * 1e6) as i64, if end.is_finite() { (end * 1e6) as i64 } else { i64::MAX });
            for track in &mut buffer.tracks {
                track.remove(s, e);
                track.bytes = track.frames.values().map(|f| f.data.len()).sum();
            }
        }
        self.changed.notify_all();
    }

    pub fn set_offset(&self, id: usize, seconds: f64) {
        let mut state = self.state.lock().unwrap();
        if let Some(buffer) = state.buffers.get_mut(id).and_then(|b| b.as_mut()) {
            buffer.offset = (seconds * 1e6) as i64;
            if let Some(p) = buffer.current.as_ref() {
                let mut pipe = p.pipe.lock().unwrap();
                let at = pipe.read + pipe.bytes.len() as u64;
                pipe.offsets.push((at, buffer.offset));
            }
        }
    }

    /// What a SourceBuffer holds: the ranges all its tracks have.
    pub fn buffered(&self, id: usize) -> Vec<(f64, f64)> {
        let state = self.state.lock().unwrap();
        state.buffers.get(id).and_then(|b| b.as_ref()).map(|b| intersect_all(b.tracks.iter().map(|t| t.ranges()).collect())).unwrap_or_default()
    }

    /// What the media element holds: the ranges every SourceBuffer has.
    pub fn buffered_all(&self) -> Vec<(f64, f64)> {
        let state = self.state.lock().unwrap();
        let mut lists = Vec::new();
        for buffer in state.buffers.iter().flatten() {
            for track in &buffer.tracks {
                lists.push(track.ranges());
            }
        }
        let mut ranges = intersect_all(lists);
        // Ended, the last range runs to the end of the longest track, as the spec has it.
        if state.ended {
            if let Some(last) = ranges.last_mut() {
                let end = state.buffers.iter().flatten().flat_map(|b| b.tracks.iter()).flat_map(|t| t.ranges().last().map(|r| r.1)).fold(last.1, f64::max);
                last.1 = end;
            }
        }
        ranges
    }

    pub fn set_duration(&self, seconds: f64) {
        self.state.lock().unwrap().duration = seconds;
        self.changed.notify_all();
    }

    pub fn duration(&self) -> f64 {
        self.state.lock().unwrap().duration
    }

    /// The page says everything has been appended (`endOfStream()`), or takes it back by
    /// appending again.
    pub fn set_ended(&self, ended: bool) {
        let mut state = self.state.lock().unwrap();
        state.ended = ended;
        // Ended, the length is where the last of what was appended ends, if that is later
        // than the page said (MSE's end of stream algorithm).
        if ended {
            let end = state.buffers.iter().flatten().flat_map(|b| b.tracks.iter()).flat_map(|t| t.ranges().last().map(|r| r.1)).fold(0.0, f64::max);
            if end > 0.0 && (state.duration.is_nan() || end > state.duration || state.duration.is_infinite()) {
                state.duration = end;
            }
        }
        self.changed.notify_all();
    }

    /// Whether the tracks a player needs are known: a picture and/or a sound, each from its
    /// initialization segment.
    pub fn ready(&self) -> bool {
        let state = self.state.lock().unwrap();
        let buffers: Vec<&Buffer> = state.buffers.iter().flatten().collect();
        !buffers.is_empty() && buffers.iter().all(|b| b.tracks.iter().any(|t| t.params.is_some()))
    }

    /// Let go of the demuxers: the element has gone, or another source is playing.
    pub fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        for buffer in state.buffers.iter_mut().flatten() {
            if let Some(p) = buffer.current.take() {
                p.close();
            }
        }
        self.changed.notify_all();
    }

    /// Let go of frames well behind where the player is, for a SourceBuffer near its quota.
    pub fn evict_before(&self, seconds: f64) {
        let mut state = self.state.lock().unwrap();
        let before = ((seconds - 30.0) * 1e6) as i64;
        for buffer in state.buffers.iter_mut().flatten() {
            if buffer.size() > QUOTA * 3 / 4 {
                for track in &mut buffer.tracks {
                    track.remove(i64::MIN, before);
                    track.bytes = track.frames.values().map(|f| f.data.len()).sum();
                }
            }
        }
    }
}

impl Buffer {
    fn size(&self) -> usize {
        self.tracks.iter().map(|t| t.bytes).sum()
    }

    fn feed(&mut self, bytes: &[u8]) {
        if let Some(p) = self.current.as_ref() {
            let mut pipe = p.pipe.lock().unwrap();
            pipe.bytes.extend(bytes);
            pipe.idle = false;
            p.more.notify_all();
        }
    }

    fn close(self) {
        if let Some(p) = self.current {
            p.close();
        }
    }

    /// Where, in an append, a new initialization segment begins: MP4's `ftyp` (or a `moov`
    /// with none before it), or WebM's EBML header, at the top level.
    fn scan_for_init(&mut self, bytes: &[u8]) -> Vec<usize> {
        let mut starts = Vec::new();
        let mut at = 0usize;
        let container = self.container;
        let scan = &mut self.scan;
        loop {
            if scan.skip > 0 {
                // A box that runs to the end of everything: nothing after it is looked at.
                if scan.skip == u64::MAX {
                    break;
                }
                let take = scan.skip.min((bytes.len() - at) as u64) as usize;
                at += take;
                scan.skip -= take as u64;
            }
            if at >= bytes.len() {
                break;
            }
            // A header, perhaps begun at the end of the last append.
            let carried = scan.partial.len();
            let mut head = std::mem::take(&mut scan.partial);
            head.extend_from_slice(&bytes[at..(at + 16).min(bytes.len())]);
            let parsed = match container {
                Container::Mp4 => mp4_header(&head),
                Container::WebM => webm_header(&head),
            };
            let Some((kind, header, body)) = parsed else {
                // Not all of it yet: kept for the next append.
                scan.partial = head;
                break;
            };
            if kind == Head::Init {
                if !scan.after_init {
                    starts.push(if carried > 0 { 0 } else { at });
                }
                scan.in_segment = false;
            }
            scan.after_init = kind == Head::Init;
            if kind == Head::Segment {
                scan.in_segment = true;
            }
            if header >= carried {
                at += header - carried;
            }
            let already = carried.saturating_sub(header) as u64;
            match (kind, body) {
                // Its children are the top level that matters.
                (Head::Segment, _) => {}
                (_, Some(len)) => scan.skip = len.saturating_sub(already),
                (_, None) => scan.skip = u64::MAX,
            }
        }
        starts
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Head {
    Init,
    Segment,
    Other,
}

/// An MP4 box header: what it is, the header's length, and its body's.
fn mp4_header(b: &[u8]) -> Option<(Head, usize, Option<u64>)> {
    if b.len() < 8 {
        return None;
    }
    let size = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64;
    let kind = &b[4..8];
    let (header, total) = match size {
        1 => {
            if b.len() < 16 {
                return None;
            }
            (16usize, Some(u64::from_be_bytes([b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]])))
        }
        0 => (8, None),
        n => (8, Some(n)),
    };
    let head = if kind == b"ftyp" || kind == b"moov" { Head::Init } else { Head::Other };
    Some((head, header, total.map(|t| t.saturating_sub(header as u64))))
}

/// A WebM element header: what it is, the header's length, and its body's (none for one of
/// unknown size).
fn webm_header(b: &[u8]) -> Option<(Head, usize, Option<u64>)> {
    let (id, id_len) = ebml_id(b)?;
    let (size, size_len, unknown) = ebml_size(&b[id_len..])?;
    let head = match id {
        0x1A45DFA3 => Head::Init,
        // The Segment: its children are what matters, so it is entered, not skipped.
        0x18538067 => Head::Segment,
        _ => Head::Other,
    };
    Some((head, id_len + size_len, if unknown { None } else { Some(size) }))
}

fn ebml_id(b: &[u8]) -> Option<(u32, usize)> {
    let first = *b.first()?;
    let len = first.leading_zeros() as usize + 1;
    if len > 4 || b.len() < len {
        return None;
    }
    let mut id = 0u32;
    for byte in &b[..len] {
        id = id << 8 | *byte as u32;
    }
    Some((id, len))
}

fn ebml_size(b: &[u8]) -> Option<(u64, usize, bool)> {
    let first = *b.first()?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 || b.len() < len {
        return None;
    }
    let mut value = (first as u64) & (0xFF >> len);
    let mut all_ones = value == (0xFF >> len) as u64;
    for byte in &b[1..len] {
        value = value << 8 | *byte as u64;
        all_ones &= *byte == 0xFF;
    }
    Some((value, len, all_ones))
}

fn container_of(mime: &str) -> Option<Container> {
    let base = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match base.as_str() {
        "video/mp4" | "audio/mp4" | "audio/aac" | "audio/mpeg" | "video/quicktime" => Some(Container::Mp4),
        "video/webm" | "audio/webm" | "video/x-matroska" | "audio/x-matroska" => Some(Container::WebM),
        _ => None,
    }
}

fn intersect_all(lists: Vec<Vec<(f64, f64)>>) -> Vec<(f64, f64)> {
    let mut it = lists.into_iter();
    let Some(mut acc) = it.next() else { return Vec::new() };
    for list in it {
        let mut out = Vec::new();
        for &(a0, a1) in &acc {
            for &(b0, b1) in &list {
                let (s, e) = (a0.max(b0), a1.min(b1));
                if e > s {
                    out.push((s, e));
                }
            }
        }
        acc = out;
    }
    acc
}

impl PipeShared {
    fn close(&self) {
        self.pipe.lock().unwrap().closed = true;
        self.more.notify_all();
    }
}

/// FFmpeg's reading of a SourceBuffer's bytes: given what has come, waiting for more, and done
/// when the pipe is closed.
unsafe extern "C" fn read_pipe(opaque: *mut c_void, buf: *mut u8, size: c_int) -> c_int {
    let pipe = &*(opaque as *const PipeShared);
    let mut held = pipe.pipe.lock().unwrap();
    loop {
        if !held.bytes.is_empty() {
            let n = (size as usize).min(held.bytes.len());
            let (a, b) = held.bytes.as_slices();
            let from_a = n.min(a.len());
            ptr::copy_nonoverlapping(a.as_ptr(), buf, from_a);
            if n > from_a {
                ptr::copy_nonoverlapping(b.as_ptr(), buf.add(from_a), n - from_a);
            }
            held.bytes.drain(..n);
            held.read += n as u64;
            return n as c_int;
        }
        if held.closed {
            return ffi::AVERROR_EOF;
        }
        held.idle = true;
        held = pipe.more.wait_timeout(held, Duration::from_millis(250)).unwrap().0;
    }
}

/// One initialization segment's demuxer: its tracks known when its header is read, then each
/// media segment's frames into the SourceBuffer's tracks as they are read.
fn demux(pipe: Arc<PipeShared>, id: usize, container: Container, source: std::sync::Weak<Source>) {
    unsafe {
        let size = 64 * 1024;
        let buffer = ffi::av_malloc(size) as *mut u8;
        let opaque = Arc::as_ptr(&pipe) as *mut c_void;
        let mut io = ffi::avio_alloc_context(buffer, size as c_int, 0, opaque, Some(read_pipe), None, None);
        if io.is_null() {
            return;
        }
        let mut format = ffi::avformat_alloc_context();
        (*format).pb = io;
        (*format).flags |= ffi::AVFMT_FLAG_CUSTOM_IO as c_int;
        let name = match container {
            Container::Mp4 => c"mp4",
            Container::WebM => c"matroska",
        };
        let input = ffi::av_find_input_format(name.as_ptr());
        let opened = ffi::avformat_open_input(&mut format, ptr::null(), input, ptr::null_mut());
        let fail = |why: String| {
            if let Some(source) = source.upgrade() {
                let mut state = source.state.lock().unwrap();
                if let Some(b) = state.buffers.get_mut(id).and_then(|b| b.as_mut()) {
                    b.error = Some(why);
                }
                source.changed.notify_all();
            }
        };
        if opened < 0 {
            if !pipe.pipe.lock().unwrap().closed {
                fail(format!("the initialization segment could not be read ({opened})"));
            }
            ffi::avio_context_free(&mut io);
            return;
        }
        let streams: Vec<*mut ffi::AVStream> = std::slice::from_raw_parts((*format).streams, (*format).nb_streams as usize).to_vec();
        let params: Vec<Arc<Params>> = streams.iter().map(|s| Params::copy_of((**s).codecpar)).collect();
        // The SourceBuffer's tracks, in the order of the segment's: made the first time, kept after.
        if let Some(source) = source.upgrade() {
            let mut state = source.state.lock().unwrap();
            if let Some(b) = state.buffers.get_mut(id).and_then(|b| b.as_mut()) {
                while b.tracks.len() < params.len() {
                    b.tracks.push(Track::default());
                }
                for (track, p) in b.tracks.iter_mut().zip(&params) {
                    if track.params.is_none() {
                        track.params = Some(p.clone());
                    }
                }
            }
            source.changed.notify_all();
        } else {
            ffi::avformat_close_input(&mut format);
            ffi::avio_context_free(&mut io);
            return;
        }
        let mut packet = ffi::av_packet_alloc();
        // Frames are put in a media segment at a time -- or every second's worth, for a long one
        // -- so that what one replaces is taken out once.
        let mut batch: Vec<Vec<Frame>> = (0..streams.len()).map(|_| Vec::new()).collect();
        let flush = |batch: &mut Vec<Vec<Frame>>| {
            let Some(source) = source.upgrade() else { return false };
            let mut state = source.state.lock().unwrap();
            let Some(b) = state.buffers.get_mut(id).and_then(|b| b.as_mut()) else { return false };
            for (index, frames) in batch.iter_mut().enumerate() {
                if !frames.is_empty() {
                    if let Some(track) = b.tracks.get_mut(index) {
                        track.insert(std::mem::take(frames));
                    }
                }
            }
            source.changed.notify_all();
            true
        };
        loop {
            let got = ffi::av_read_frame(format, packet);
            if got < 0 {
                break;
            }
            let index = (*packet).stream_index as usize;
            if index < streams.len() {
                let base = (*streams[index]).time_base;
                let rescale = |t: i64| if t == ffi::AV_NOPTS_VALUE { ffi::AV_NOPTS_VALUE } else { ffi::av_rescale_q(t, base, MICROS) };
                let offset = {
                    let pipe = pipe.pipe.lock().unwrap();
                    let at = (*packet).pos.max(0) as u64;
                    pipe.offsets.iter().rev().find(|(from, _)| *from <= at).map_or(0, |(_, o)| *o)
                };
                let pts = rescale((*packet).pts);
                let dts = rescale((*packet).dts);
                let (pts, dts) = match (pts, dts) {
                    (p, d) if p == ffi::AV_NOPTS_VALUE && d == ffi::AV_NOPTS_VALUE => (0, 0),
                    (p, d) if p == ffi::AV_NOPTS_VALUE => (d, d),
                    (p, d) if d == ffi::AV_NOPTS_VALUE => (p, p),
                    (p, d) => (p, d),
                };
                let data = std::slice::from_raw_parts((*packet).data, (*packet).size.max(0) as usize).to_vec();
                batch[index].push(Frame {
                    data,
                    pts: pts + offset,
                    dts: dts + offset,
                    duration: rescale((*packet).duration).max(0),
                    key: (*packet).flags & ffi::AV_PKT_FLAG_KEY as c_int != 0,
                    params: params[index].clone(),
                });
                // Whatever has come so far is in the track before the demuxer waits for more.
                let waiting = { pipe.pipe.lock().unwrap().bytes.is_empty() };
                if waiting || batch[index].len() >= 64 {
                    if !flush(&mut batch) {
                        break;
                    }
                }
            }
            ffi::av_packet_unref(packet);
        }
        flush(&mut batch);
        ffi::av_packet_free(&mut packet);
        ffi::avformat_close_input(&mut format);
        ffi::avio_context_free(&mut io);
    }
}

/// The next frame of each of a source's tracks after a moment, for the player's reader.
pub(crate) struct Cursor {
    pub buffer: usize,
    pub track: usize,
    /// The decode time of the last frame given; none before the first.
    pub last: Option<i64>,
    pub params: Option<Arc<Params>>,
}

impl State {
    pub(crate) fn track(&self, buffer: usize, track: usize) -> Option<&Track> {
        self.buffers.get(buffer)?.as_ref()?.tracks.get(track)
    }

    /// Where to start a track to present `micros`: for a picture its keyframe at or before it,
    /// for a sound the frame that covers it. Its decode time, as a cursor's `last` wants it
    /// (one before).
    pub(crate) fn start_of(&self, buffer: usize, track: usize, micros: i64, video: bool) -> Option<i64> {
        let t = self.track(buffer, track)?;
        let mut best: Option<i64> = None;
        for (dts, f) in t.frames.iter() {
            if f.pts > micros {
                if best.is_some() {
                    break;
                }
                continue;
            }
            if !video || f.key {
                best = Some(*dts);
            }
        }
        best.or_else(|| t.frames.keys().next().copied()).map(|d| d - 1)
    }
}
