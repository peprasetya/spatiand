//! A film on a web server, fetched the way a browser fetches one: ahead of where the demuxer is
//! reading, a stretch at a time, into memory, and handed to FFmpeg from there.
//!
//! FFmpeg's own `http:` keeps one connection open for the whole film and reads it only as fast
//! as the decoders want: most of the time the connection sits stalled, the server's sending
//! blocked on it. And whenever the demuxer jumps further than the socket's buffer -- to a sound
//! stored a few megabytes from its pictures, to an index at the end -- it closes it and asks
//! again from the new place. A server in front of cloud storage (Vibrefy reading Google Drive)
//! does badly at both: a stalled or abandoned request has had Drive read ahead for it, and
//! that is thrown away; it pulls far more than it sends, and the film stutters.
//!
//! So, as a browser does: each place the demuxer reads from (a "head" -- usually one, two for a
//! file whose sound is stored apart from its pictures) is fetched ahead by asking for exactly
//! the stretch that fills about half a minute of the film past it (`Range: bytes=a-b`), as fast
//! as the line goes, and then nothing more until half of that has been read. A request is never
//! left stalled, and is only cut short when a seek leaves it behind. What has been fetched is
//! held in memory (never on disk) while it is near where reading is, so stepping back costs
//! nothing. Paused, a film costs nothing once its half minute is in hand.
//!
//! What cannot be fetched this way is left to FFmpeg: a server that does not answer a `Range`
//! with its part of a known length, a playlist (HLS, DASH), anything not `http(s)`.

use std::collections::HashMap;
use std::ffi::{c_int, c_void};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use rsmpeg::ffi;

/// What is held is held in blocks of this many bytes.
const BLOCK: u64 = 64 * 1024;
/// How far ahead of reading to fetch, in the film's seconds, and within what.
const SECONDS_AHEAD: f64 = 30.0;
const LEAST_AHEAD: u64 = 8 * 1024 * 1024;
const MOST_AHEAD: u64 = 64 * 1024 * 1024;
/// While the film is being opened its pace is not known, and the demuxer jumps about -- to an
/// index at the end, back to the start -- so only this much is fetched past each place.
const OPENING_AHEAD: u64 = 1024 * 1024;
/// ...unless the demuxer reads on and on from one place, as it does an index: then each request
/// is four times what it has read there so far, up to this; and an index at the end of the file
/// is asked for to the end at once. A long film's index is tens of megabytes, and was otherwise
/// fetched a megabyte at a time, a server's wait before each.
const OPENING_MOST: u64 = 16 * 1024 * 1024;
/// The least asked for at once, once open: fewer, larger requests.
const LEAST_ASKED: u64 = 4 * 1024 * 1024;
/// What is kept behind reading, for a demuxer that steps back.
const KEPT_BEHIND: u64 = 16 * 1024 * 1024;
/// How often in a row the server may fail before reading gives up.
const TRIES: u32 = 6;
/// How many places at once reading is followed from.
const HEADS: usize = 3;
/// A head not read from for this long while another has been is one a seek has left.
const LEFT: Duration = Duration::from_secs(1);

/// Where the film is and how it is to be asked for.
struct Ask {
    url: String,
    headers: Vec<(String, String)>,
}

/// A place reading goes on from, and when it last did.
struct Head {
    id: u64,
    /// Where it began.
    from: u64,
    at: u64,
    used: Instant,
}

/// One request under way: bytes `from..to` (exclusive), of which those before `at` are here.
struct Run {
    head: u64,
    from: u64,
    at: u64,
    to: u64,
    cancel: Arc<AtomicBool>,
}

struct State {
    blocks: HashMap<u64, (Vec<u8>, Instant)>,
    heads: Vec<Head>,
    runs: Vec<Run>,
    next_head: u64,
    /// Failures in a row, and when the server may next be asked.
    failures: u32,
    retry: Instant,
    failed: Option<String>,
    ahead: u64,
}

struct Shared {
    ask: Ask,
    size: u64,
    agent: ureq::Agent,
    state: Mutex<State>,
    changed: Condvar,
    closed: AtomicBool,
}

/// A film on a server, being fetched. Dropping it, or [`Remote::close`], stops the fetching.
pub struct Remote {
    shared: Arc<Shared>,
}

/// Whether a film at this address is one for FFmpeg's own protocols: a playlist names other
/// files to fetch, which only its demuxer knows how to.
fn playlist(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
    path.ends_with(".m3u8") || path.ends_with(".m3u") || path.ends_with(".mpd")
}

/// The headers a player's options ask for, or `None` if the options say something only
/// FFmpeg's own `http:` would understand.
fn headers(options: &[(String, String)]) -> Option<Vec<(String, String)>> {
    let mut headers = Vec::new();
    for (key, value) in options {
        match key.as_str() {
            "user_agent" => headers.push(("User-Agent".to_string(), value.clone())),
            "referer" => headers.push(("Referer".to_string(), value.clone())),
            "cookies" => headers.push(("Cookie".to_string(), value.trim_end_matches(['\r', '\n']).replace('\n', "; "))),
            "headers" => {
                for line in value.split("\r\n").flat_map(|l| l.split('\n')) {
                    if let Some((name, value)) = line.split_once(':') {
                        headers.push((name.trim().to_string(), value.trim().to_string()));
                    }
                }
            }
            _ => return None,
        }
    }
    Some(headers)
}

impl Shared {
    #[allow(clippy::result_large_err)]
    fn get(&self, from: u64, to: u64) -> Result<ureq::Response, ureq::Error> {
        let mut request = self.agent.get(&self.ask.url).set("Range", &format!("bytes={from}-{}", to - 1));
        for (name, value) in &self.ask.headers {
            request = request.set(name, value);
        }
        request.call()
    }

    fn block_len(&self, block: u64) -> usize {
        BLOCK.min(self.size.saturating_sub(block * BLOCK)) as usize
    }

    /// Whether `block` is here, or on its way in a request under way.
    fn covered(&self, state: &State, block: u64) -> bool {
        state.blocks.get(&block).is_some_and(|(data, _)| data.len() == self.block_len(block))
            || state.runs.iter().any(|run| run.at <= block * BLOCK && block * BLOCK < run.to)
    }

    /// Where what is here or coming, from `at` on without a gap, ends; and the next block past
    /// that gap that is here (where asking stops).
    fn reach(&self, state: &State, at: u64) -> (u64, u64) {
        let last = self.size.div_ceil(BLOCK);
        let mut block = at / BLOCK;
        while block < last && self.covered(state, block) {
            block += 1;
        }
        let gap = block;
        let mut next = gap;
        while next < last && !self.covered(state, next) && (next - gap) * BLOCK < MOST_AHEAD {
            next += 1;
        }
        ((gap * BLOCK).min(self.size), (next * BLOCK).min(self.size))
    }

    /// Start whatever requests reading now wants, and cut short those a seek has left behind.
    fn plan(self: &Arc<Shared>, state: &mut State) {
        if self.closed.load(Ordering::Relaxed) {
            return;
        }
        let newest = state.heads.iter().map(|head| head.used).max();
        let Some(newest) = newest else { return };
        let left = |used: Instant| newest.duration_since(used) > LEFT;
        // A head left behind by a seek has its request cut: the line is wanted where reading is.
        let gone: Vec<u64> = state.heads.iter().filter(|head| left(head.used)).map(|head| head.id).collect();
        state.runs.retain(|run| {
            let keep = !gone.contains(&run.head);
            if !keep {
                run.cancel.store(true, Ordering::Relaxed);
            }
            keep
        });
        if Instant::now() < state.retry {
            return;
        }
        let lively: Vec<(u64, u64, u64)> = state.heads.iter().filter(|head| !left(head.used)).map(|head| (head.id, head.from, head.at)).collect();
        let shared_ahead = (state.ahead / lively.len().max(1) as u64).max(OPENING_AHEAD.min(state.ahead));
        for (id, from, at) in lively {
            let ahead = if state.ahead > OPENING_AHEAD {
                shared_ahead
            } else if self.size - at <= OPENING_MOST {
                // Near the end while opening is an index stored there: all of it, at once.
                self.size - at
            } else {
                (at.saturating_sub(from) * 4).clamp(OPENING_AHEAD, OPENING_MOST)
            };
            if state.runs.iter().any(|run| run.head == id) {
                continue;
            }
            let (have, stop) = self.reach(state, at);
            // Asked again when half of what is ahead has been read, not at every block.
            let opening = state.ahead <= OPENING_AHEAD;
            if have >= self.size || have.saturating_sub(at) >= ahead / 2 {
                continue;
            }
            let least = if opening { BLOCK } else { LEAST_ASKED };
            let to = (at + ahead).max(have + least).min(stop).min(self.size);
            if to <= have {
                continue;
            }
            // A block left part filled by a request that failed is asked for whole again.
            for block in have / BLOCK..to.div_ceil(BLOCK) {
                state.blocks.remove(&block);
            }
            let cancel = Arc::new(AtomicBool::new(false));
            state.runs.push(Run { head: id, from: have, at: have, to, cancel: cancel.clone() });
            let shared = self.clone();
            let _ = std::thread::Builder::new().name("film-fetch".into()).spawn(move || shared.run(id, have, to, cancel));
        }
    }

    /// One request: bytes `from..to`, into blocks as they come.
    fn run(self: Arc<Shared>, head: u64, from: u64, to: u64, cancel: Arc<AtomicBool>) {
        let stopped = || cancel.load(Ordering::Relaxed) || self.closed.load(Ordering::Relaxed);
        let outcome = self.get(from, to).map_err(|e| e.to_string()).and_then(|response| {
            if response.status() != 206 {
                return Err(format!("asked for part of it, the server sent {}", response.status()));
            }
            // A server that sends some other part than the one asked for is not believed.
            let begins = response.header("Content-Range").and_then(|range| range.trim_start_matches("bytes").trim().split('-').next()?.parse::<u64>().ok());
            if begins != Some(from) {
                return Err(format!("asked for bytes from {from}, the server sent {:?}", response.header("Content-Range")));
            }
            let mut reader = response.into_reader().take(to - from);
            let mut chunk = vec![0u8; BLOCK as usize];
            let mut at = from;
            while at < to {
                if stopped() {
                    return Ok(at);
                }
                let got = reader.read(&mut chunk).map_err(|e| e.to_string())?;
                if got == 0 {
                    return Err("the connection ended early".into());
                }
                let mut state = self.state.lock().unwrap();
                let mut taken = 0;
                while taken < got {
                    let block = at / BLOCK;
                    let length = self.block_len(block);
                    let (data, used) = state.blocks.entry(block).or_insert_with(|| (Vec::with_capacity(length), Instant::now()));
                    // Another request may have filled this block already, or part of it: only
                    // what is missing is added, and the bytes passed over still count as taken.
                    let inside = (at - block * BLOCK) as usize;
                    let count = (length - inside).min(got - taken);
                    if data.len() == inside {
                        data.extend_from_slice(&chunk[taken..taken + count]);
                    }
                    *used = Instant::now();
                    taken += count;
                    at += count as u64;
                }
                if let Some(run) = state.runs.iter_mut().find(|run| Arc::ptr_eq(&run.cancel, &cancel)) {
                    run.at = at;
                }
                drop(state);
                self.changed.notify_all();
            }
            Ok(at)
        });
        let mut state = self.state.lock().unwrap();
        state.runs.retain(|run| !Arc::ptr_eq(&run.cancel, &cancel));
        match outcome {
            Ok(_) => state.failures = 0,
            Err(why) if !stopped() => {
                state.failures += 1;
                if state.failures >= TRIES {
                    state.failed = Some(why);
                } else {
                    log::warn!("{}: bytes {from}-{to} (for head {head}): {why}; asking again", self.ask.url);
                    state.retry = Instant::now() + Duration::from_millis(250 << state.failures.min(4));
                }
            }
            Err(_) => {}
        }
        self.forget(&mut state);
        self.plan(&mut state);
        drop(state);
        self.changed.notify_all();
    }

    /// Let go of blocks no head will want again, the least lately used first, while more is
    /// held than reading needs.
    fn forget(&self, state: &mut State) {
        let budget = (state.ahead + KEPT_BEHIND) / BLOCK;
        if state.blocks.len() as u64 <= budget {
            return;
        }
        let ahead = state.ahead;
        let wanted = |state: &State, block: u64| {
            let at = block * BLOCK;
            state.heads.iter().any(|head| at + BLOCK > head.at && at < head.at + ahead) || state.runs.iter().any(|run| at + BLOCK > run.from && at < run.to)
        };
        let mut old: Vec<(Instant, u64)> = state.blocks.iter().filter(|(&block, _)| !wanted(state, block)).map(|(&block, (_, used))| (*used, block)).collect();
        old.sort();
        let over = state.blocks.len() as u64 - budget;
        for (_, block) in old.into_iter().take(over as usize) {
            state.blocks.remove(&block);
        }
    }

    /// Up to `into.len()` bytes from `at`, as soon as any are here; 0 at the end of the film.
    fn read(self: &Arc<Shared>, at: u64, into: &mut [u8]) -> Result<usize, String> {
        if at >= self.size || into.is_empty() {
            return Ok(0);
        }
        let block = at / BLOCK;
        let offset = (at - block * BLOCK) as usize;
        let mut state = self.state.lock().unwrap();
        // Reading here moves the head it continues, or begins one.
        let now = Instant::now();
        let near = state.ahead.max(LEAST_ASKED);
        match state.heads.iter_mut().find(|head| at + BLOCK >= head.at && at <= head.at + near) {
            Some(head) => {
                head.at = at;
                head.used = now;
            }
            None => {
                if state.heads.len() >= HEADS {
                    state.heads.sort_by_key(|head| std::cmp::Reverse(head.used));
                    state.heads.truncate(HEADS - 1);
                }
                state.next_head += 1;
                let id = state.next_head;
                state.heads.push(Head { id, from: at, at, used: now });
            }
        }
        // The same request may serve two heads that have come together: one is enough.
        state.heads.sort_by_key(|head| std::cmp::Reverse(head.used));
        let mut seen: Vec<u64> = Vec::new();
        state.heads.retain(|head| {
            let fresh = !seen.iter().any(|&other| other.abs_diff(head.at) < BLOCK);
            seen.push(head.at);
            fresh
        });
        let mut planned = false;
        loop {
            if self.closed.load(Ordering::Relaxed) {
                return Err("closed".into());
            }
            if let Some(why) = state.failed.clone() {
                return Err(why);
            }
            if let Some((data, used)) = state.blocks.get_mut(&block) {
                if data.len() > offset {
                    let count = (data.len() - offset).min(into.len());
                    into[..count].copy_from_slice(&data[offset..offset + count]);
                    *used = now;
                    if !planned {
                        self.plan(&mut state);
                    }
                    return Ok(count);
                }
            }
            self.plan(&mut state);
            planned = true;
            state = self.changed.wait_timeout(state, Duration::from_millis(100)).unwrap().0;
        }
    }
}

impl Remote {
    /// Begin fetching `url`, or `None` if it is not a film this can fetch (see the module's
    /// words), in which case FFmpeg is to open it itself.
    pub fn open(url: &str, options: &[(String, String)]) -> Option<Remote> {
        if !(url.starts_with("http://") || url.starts_with("https://")) || playlist(url) || std::env::var("SPATIAND_MEDIA_FETCH").as_deref() == Ok("ffmpeg") {
            return None;
        }
        let headers = headers(options)?;
        let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout_read(Duration::from_secs(30)).build();
        let shared = Shared {
            ask: Ask { url: url.to_string(), headers },
            size: 0,
            agent,
            state: Mutex::new(State {
                blocks: HashMap::new(),
                heads: Vec::new(),
                runs: Vec::new(),
                next_head: 0,
                failures: 0,
                retry: Instant::now(),
                failed: None,
                ahead: OPENING_AHEAD,
            }),
            changed: Condvar::new(),
            closed: AtomicBool::new(false),
        };
        // The start of the film, asked for as any other part, says whether the server answers
        // in parts and how long the whole is.
        let response = match shared.get(0, OPENING_AHEAD) {
            Ok(response) => response,
            Err(e) => {
                log::info!("{url}: {e}; FFmpeg is to fetch it");
                return None;
            }
        };
        let size = response.header("Content-Range").and_then(|range| range.rsplit_once('/')).and_then(|(_, total)| total.trim().parse::<u64>().ok());
        let kind = response.header("Content-Type").unwrap_or("").to_ascii_lowercase();
        let (Some(size), 206) = (size, response.status()) else {
            log::info!("{url}: the server does not send it in parts; FFmpeg is to fetch it");
            return None;
        };
        if kind.contains("mpegurl") || kind.contains("dash+xml") || size == 0 {
            return None;
        }
        let mut first = Vec::new();
        let _ = response.into_reader().take(OPENING_AHEAD.min(size)).read_to_end(&mut first);
        let shared = Arc::new(Shared { size, ..shared });
        {
            let mut state = shared.state.lock().unwrap();
            for (index, data) in first.chunks(BLOCK as usize).enumerate() {
                if data.len() == shared.block_len(index as u64) {
                    state.blocks.insert(index as u64, (data.to_vec(), Instant::now()));
                }
            }
        }
        log::info!("{url}: {:.1} MB, fetched ahead of reading", size as f64 / 1e6);
        Some(Remote { shared })
    }

    /// How fast the film goes, in its bytes a second: what decides how far ahead is fetched.
    pub fn set_rate(&self, bytes_a_second: f64) {
        if bytes_a_second.is_finite() && bytes_a_second > 0.0 {
            let ahead = ((bytes_a_second * SECONDS_AHEAD) as u64).clamp(LEAST_AHEAD, MOST_AHEAD);
            let mut state = self.shared.state.lock().unwrap();
            state.ahead = ahead;
            self.shared.plan(&mut state);
            drop(state);
            self.shared.changed.notify_all();
        }
    }

    pub fn size(&self) -> u64 {
        self.shared.size
    }

    /// Stop fetching, and let any read waiting give up.
    pub fn close(&self) {
        self.shared.closed.store(true, Ordering::Relaxed);
        for run in self.shared.state.lock().unwrap().runs.iter() {
            run.cancel.store(true, Ordering::Relaxed);
        }
        self.shared.changed.notify_all();
    }

    /// An FFmpeg reader over this film, for a format context's `pb`.
    pub fn io(&self) -> Io {
        let reader = Box::into_raw(Box::new(Reader { shared: self.shared.clone(), at: 0 }));
        unsafe {
            let size = 64 * 1024;
            let buffer = ffi::av_malloc(size) as *mut u8;
            let context = ffi::avio_alloc_context(buffer, size as c_int, 0, reader as *mut c_void, Some(read_packet), None, Some(seek));
            Io { context, reader }
        }
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.close();
    }
}

struct Reader {
    shared: Arc<Shared>,
    at: u64,
}

/// FFmpeg's reader over a [`Remote`], freed when dropped -- after the format context using it
/// is closed.
pub struct Io {
    pub context: *mut ffi::AVIOContext,
    reader: *mut Reader,
}
unsafe impl Send for Io {}

impl Drop for Io {
    fn drop(&mut self) {
        unsafe {
            if !self.context.is_null() {
                ffi::av_freep(&mut (*self.context).buffer as *mut *mut u8 as *mut c_void);
                ffi::avio_context_free(&mut self.context);
            }
            drop(Box::from_raw(self.reader));
        }
    }
}

unsafe extern "C" fn read_packet(opaque: *mut c_void, buffer: *mut u8, size: c_int) -> c_int {
    let reader = &mut *(opaque as *mut Reader);
    let into = std::slice::from_raw_parts_mut(buffer, size.max(0) as usize);
    match reader.shared.read(reader.at, into) {
        Ok(0) => ffi::AVERROR_EOF,
        Ok(count) => {
            reader.at += count as u64;
            count as c_int
        }
        Err(why) => {
            if !reader.shared.closed.load(Ordering::Relaxed) {
                log::warn!("{}: {why}", reader.shared.ask.url);
            }
            ffi::AVERROR(libc::EIO as u32)
        }
    }
}

unsafe extern "C" fn seek(opaque: *mut c_void, offset: i64, whence: c_int) -> i64 {
    let reader = &mut *(opaque as *mut Reader);
    let size = reader.shared.size as i64;
    let to = match whence & !(ffi::AVSEEK_FORCE as c_int) {
        w if w == ffi::AVSEEK_SIZE as c_int => return size,
        libc::SEEK_SET => offset,
        libc::SEEK_CUR => reader.at as i64 + offset,
        libc::SEEK_END => size + offset,
        _ => return -1,
    };
    if to < 0 {
        return -1;
    }
    reader.at = to as u64;
    to
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_only_ffmpeg_can_fetch_is_left_to_it() {
        assert!(playlist("https://h/live/index.m3u8?token=1"));
        assert!(playlist("http://h/a.mpd"));
        assert!(!playlist("https://h/stream/S/Films/a.mkv"));
        assert!(headers(&[("user_agent".into(), "x".into()), ("referer".into(), "http://p".into())]).is_some());
        assert!(headers(&[("reconnect".into(), "1".into())]).is_none());
        let said = headers(&[("headers".into(), "A: 1\r\nB: two\r\n".into())]).unwrap();
        assert_eq!(said, vec![("A".to_string(), "1".to_string()), ("B".to_string(), "two".to_string())]);
    }
}
