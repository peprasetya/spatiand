//! Hearing the wearer over its own voice: the assistant's voice taken back out of the microphone,
//! and a gate that tells the wearer speaking from what is left of it.
//!
//! The glasses' speakers sit a few centimetres from their microphone, so while the assistant
//! speaks the microphone hears it about as loud as the wearer -- and it must not take itself for
//! the wearer and stop (the owner, 2026-11-15: "let user interrupt with voice if the spatipal is
//! speaking"). **The same on every platform**, because it is done against what the assistant
//! itself plays, which it knows exactly, sample by sample: not against what the machine plays,
//! which only the platform's own canceller sees, and then only for sound played through it.
//!
//! The way through:
//!
//! 1. [`Played`]: what was handed to the room, on a timeline of 16 kHz samples, with when it
//!    began to be heard.
//! 2. [`MicClock`]: the microphone on the same timeline, from when its pieces arrive.
//! 3. [`DelayFinder`]: how long the room, the sound device, the air and the microphone take
//!    between the two -- 50 ms to a second, unknown and not the same on any two machines --
//!    found by matching the loudness of the two over the last four seconds.
//! 4. [`Canceller`]: an adaptive filter, frequency domain and in partitions (the same family as
//!    Speex's and WebRTC's), that learns the path from what was played to what the microphone
//!    hears -- the room's spatial filters, the speakers, the frame of the glasses -- and takes it
//!    away. It learns only while the wearer is quiet.
//! 5. [`Gate`]: what is left is the wearer's when it is a voice (a pitch), well above the room's
//!    own floor, and well above what is left of the assistant's voice -- which it learns as it
//!    goes: how loud what is left is for how loud the voice played was, the loud end of it.
//! 6. [`BargeIn`]: enough of that, close enough together, while it is speaking, and it stops.
//!
//! Everything works in blocks of [`BLOCK`] samples (16 ms).

use std::collections::VecDeque;
use std::time::Instant;

/// Samples a block: 16 ms at 16 kHz.
pub const BLOCK: usize = 256;
/// The rate everything here is at.
pub const RATE: u32 = crate::RATE;
const FFT: usize = 2 * BLOCK;
/// Partitions of the filter: 12 blocks, 192 ms of the path after the delay.
const PARTS: usize = 12;
/// The filter starts this many blocks before the delay found, so that a delay found a block too
/// late is still covered.
const LEAD: usize = 3;
/// The longest delay looked for, in blocks: about a second.
const MOST_DELAY: usize = 64;

// --- a small FFT, radix 2 ---

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct C {
    re: f32,
    im: f32,
}

impl C {
    fn mul(self, o: C) -> C {
        C { re: self.re * o.re - self.im * o.im, im: self.re * o.im + self.im * o.re }
    }
    fn conj(self) -> C {
        C { re: self.re, im: -self.im }
    }
    fn norm(self) -> f32 {
        self.re * self.re + self.im * self.im
    }
}

struct Fft {
    twiddles: Vec<C>,
    reversed: Vec<usize>,
}

impl Fft {
    fn new(n: usize) -> Fft {
        assert!(n.is_power_of_two());
        let bits = n.trailing_zeros();
        let reversed = (0..n).map(|i| i.reverse_bits() >> (usize::BITS - bits)).collect();
        let twiddles = (0..n / 2)
            .map(|k| {
                let a = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
                C { re: a.cos() as f32, im: a.sin() as f32 }
            })
            .collect();
        Fft { twiddles, reversed }
    }

    fn forward(&self, x: &mut [C]) {
        let n = x.len();
        for i in 0..n {
            let j = self.reversed[i];
            if i < j {
                x.swap(i, j);
            }
        }
        let mut size = 2;
        while size <= n {
            let step = n / size;
            for start in (0..n).step_by(size) {
                for k in 0..size / 2 {
                    let t = self.twiddles[k * step].mul(x[start + k + size / 2]);
                    let u = x[start + k];
                    x[start + k] = C { re: u.re + t.re, im: u.im + t.im };
                    x[start + k + size / 2] = C { re: u.re - t.re, im: u.im - t.im };
                }
            }
            size *= 2;
        }
    }

    fn inverse(&self, x: &mut [C]) {
        for v in x.iter_mut() {
            *v = v.conj();
        }
        self.forward(x);
        let scale = 1.0 / x.len() as f32;
        for v in x.iter_mut() {
            *v = C { re: v.re * scale, im: -v.im * scale };
        }
    }
}

// --- what was played ---

/// What the assistant handed the room to be heard, on a timeline of 16 kHz samples counted from
/// `epoch`: sample `i` is heard at `epoch + i / 16000` s (as near as the player knows -- the
/// room and the device add their own delay, which [`DelayFinder`] finds). The last few seconds.
pub struct Played {
    epoch: Instant,
    ring: Vec<f32>,
    /// One past the last sample written.
    end: u64,
}

const RING: usize = 1 << 17; // 8 s

impl Played {
    pub fn new(epoch: Instant) -> Played {
        Played { epoch, ring: vec![0.0; RING], end: 0 }
    }

    pub fn epoch(&self) -> Instant {
        self.epoch
    }

    /// The timeline's sample for a moment.
    pub fn index_at(&self, at: Instant) -> u64 {
        (at.saturating_duration_since(self.epoch).as_secs_f64() * RATE as f64).round() as u64
    }

    /// `pcm`, at `rate` a second, begins to be heard at `at`.
    pub fn push(&mut self, at: Instant, pcm: &[f32], rate: u32) {
        if pcm.is_empty() {
            return;
        }
        let start = self.index_at(at);
        let step = rate as f64 / RATE as f64;
        let n = ((pcm.len() as f64) / step).floor() as usize;
        // Silence where nothing was played since the last.
        if start > self.end {
            let gap = (start - self.end).min(RING as u64);
            for i in 0..gap {
                let at = (start - gap + i) as usize % RING;
                self.ring[at] = 0.0;
            }
        }
        for k in 0..n {
            let x = k as f64 * step;
            let (a, f) = (x.floor() as usize, (x - x.floor()) as f32);
            let s0 = pcm[a.min(pcm.len() - 1)];
            let s1 = pcm[(a + 1).min(pcm.len() - 1)];
            self.ring[(start as usize + k) % RING] = s0 + (s1 - s0) * f;
        }
        self.end = self.end.max(start + n as u64);
    }

    /// Samples from `from` on into `out`: nothing (zeros) where nothing was played, or it is too
    /// long ago to be kept.
    pub fn read(&self, from: u64, out: &mut [f32]) {
        for (k, o) in out.iter_mut().enumerate() {
            let i = from + k as u64;
            *o = if i < self.end && i + (RING as u64) > self.end { self.ring[i as usize % RING] } else { 0.0 };
        }
    }

    /// The sample at a fractional place, for a simulated room.
    fn at(&self, x: f64) -> f32 {
        if x < 0.0 {
            return 0.0;
        }
        let a = x.floor() as u64;
        let f = (x - a as f64) as f32;
        let mut two = [0.0; 2];
        self.read(a, &mut two);
        two[0] + (two[1] - two[0]) * f
    }

    /// Whether anything was played after `index`.
    pub fn played_since(&self, index: u64) -> bool {
        self.end > index
    }
}

// --- the microphone's clock ---

/// Where on [`Played`]'s timeline the microphone's samples fall. A microphone's samples come
/// steadily, at its own rate, but are read in pieces whenever the thread gets to them: the
/// earliest a piece could have arrived is the truest. So the samples are counted from the
/// earliest arrival seen, **steadily** -- one sample, one place, with no jumps for a late read,
/// which would look to the canceller like the room's delay changing. Only when every arrival for
/// two seconds has been later than the count says by more than 10 ms (samples lost, or a device
/// clock running slow) does the count move on to them.
pub struct MicClock {
    count: u64,
    anchor: Option<i64>,
    seen: VecDeque<(Instant, i64)>,
}

/// How far behind the count arrivals may be, in samples, before the count follows them.
const SLIP: i64 = RATE as i64 / 100;

impl Default for MicClock {
    fn default() -> Self {
        MicClock::new()
    }
}

impl MicClock {
    pub fn new() -> MicClock {
        MicClock { count: 0, anchor: None, seen: VecDeque::new() }
    }

    /// `n` samples arrived at `at` (the end of them). The timeline index of the first of them.
    pub fn arrived(&mut self, played: &Played, at: Instant, n: usize) -> u64 {
        self.count += n as u64;
        let offset = played.index_at(at) as i64 - self.count as i64;
        self.seen.push_back((at, offset));
        while self.seen.front().is_some_and(|(t, _)| at.duration_since(*t).as_secs_f32() > 2.0) {
            self.seen.pop_front();
        }
        let least = self.seen.iter().map(|(_, o)| *o).min().unwrap_or(offset);
        let anchor = match self.anchor {
            // Earlier than ever: that is where they are.
            Some(a) if offset < a => offset,
            Some(a) if least > a + SLIP && self.seen.front().is_some_and(|(t, _)| at.duration_since(*t).as_secs_f32() > 1.5) => {
                log::info!("echo: the microphone fell {} ms behind its count: following it", (least - a) * 1000 / RATE as i64);
                least
            }
            Some(a) => a,
            None => offset,
        };
        self.anchor = Some(anchor);
        (anchor + self.count as i64 - n as i64).max(0) as u64
    }
}

// --- the delay ---

/// How long between a sample played and its echo in the microphone, in blocks, found by matching
/// their loudness: the log of each block's energy, the microphone's against the played's at
/// each delay up to a second, over the last four seconds. Asked again every half second while
/// there is voice to match; believed when the match is good, and the same twice running.
pub struct DelayFinder {
    mic: VecDeque<f32>,
    played: VecDeque<f32>,
    since: usize,
    candidate: Option<usize>,
    pub delay: Option<usize>,
    pub confidence: f32,
}

const HISTORY: usize = 256;

impl Default for DelayFinder {
    fn default() -> Self {
        DelayFinder::new()
    }
}

impl DelayFinder {
    pub fn new() -> DelayFinder {
        DelayFinder { mic: VecDeque::new(), played: VecDeque::new(), since: 0, candidate: None, delay: None, confidence: 0.0 }
    }

    /// One block's energy, of the microphone and of what was played at the same moment. `true`
    /// when the delay changed.
    pub fn block(&mut self, mic: f32, played: f32) -> bool {
        self.mic.push_back((mic + 1e-9).log10());
        self.played.push_back((played + 1e-9).log10());
        if self.mic.len() > HISTORY {
            self.mic.pop_front();
            self.played.pop_front();
        }
        self.since += 1;
        if self.since < 31 || self.mic.len() < MOST_DELAY + 32 {
            return false;
        }
        self.since = 0;
        let Some((lag, r)) = self.best() else { return false };
        self.confidence = r;
        if r < 0.45 {
            return false;
        }
        let agreed = self.candidate.is_some_and(|c| c.abs_diff(lag) <= 1);
        self.candidate = Some(lag);
        // Twice running, the first time too: a delay believed too soon puts its echo where it is
        // not, and its own voice then sounds like the wearer.
        if agreed && self.delay.is_none_or(|d| d.abs_diff(lag) > 1) {
            self.delay = Some(lag);
            return true;
        }
        false
    }

    /// The delay that matches best, and how well (a correlation, -1 to 1). Only blocks where
    /// something loud enough was played count: silence matches everything.
    fn best(&self) -> Option<(usize, f32)> {
        let n = self.mic.len();
        let mic: Vec<f32> = self.mic.iter().copied().collect();
        let played: Vec<f32> = self.played.iter().copied().collect();
        let loudest = played.iter().copied().fold(f32::MIN, f32::max);
        // Blocks within 40 dB of the loudest played.
        let floor = loudest - 4.0;
        let active = played.iter().filter(|&&p| p > floor).count();
        if active < 30 {
            return None;
        }
        let mut best: Option<(usize, f32)> = None;
        for lag in 0..=MOST_DELAY {
            let pairs: Vec<(f32, f32)> = (MOST_DELAY..n).filter(|&k| played[k - lag] > floor).map(|k| (mic[k], played[k - lag].max(floor))).collect();
            if pairs.len() < 24 {
                continue;
            }
            let r = correlation(&pairs);
            if best.is_none_or(|(_, b)| r > b) {
                best = Some((lag, r));
            }
        }
        best
    }
}

fn correlation(pairs: &[(f32, f32)]) -> f32 {
    let n = pairs.len() as f32;
    let (ma, mb) = pairs.iter().fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
    let (ma, mb) = (ma / n, mb / n);
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in pairs {
        ab += (x - ma) * (y - mb);
        aa += (x - ma) * (x - ma);
        bb += (y - mb) * (y - mb);
    }
    if aa <= 1e-9 || bb <= 1e-9 {
        return 0.0;
    }
    ab / (aa * bb).sqrt()
}

// --- the canceller ---

/// The echo path learnt and taken away: a partitioned-block frequency-domain adaptive filter
/// (overlap-save, its gradient constrained), [`PARTS`] blocks long, normalised in each frequency
/// by the power of what was played.
pub struct Canceller {
    fft: Fft,
    /// What was played, as spectra of two blocks, the newest first.
    spectra: VecDeque<Vec<C>>,
    weights: Vec<Vec<C>>,
    last: Vec<f32>,
    step: f32,
    scratch: Vec<C>,
    /// The microphone's energy and what was left of it, the last half second.
    checked: VecDeque<(f32, f32)>,
}

impl Default for Canceller {
    fn default() -> Self {
        Canceller::new()
    }
}

impl Canceller {
    pub fn new() -> Canceller {
        Canceller {
            fft: Fft::new(FFT),
            spectra: (0..PARTS).map(|_| vec![C::default(); FFT]).collect(),
            weights: (0..PARTS).map(|_| vec![C::default(); FFT]).collect(),
            last: vec![0.0; BLOCK],
            step: 0.5,
            scratch: vec![C::default(); FFT],
            checked: VecDeque::new(),
        }
    }

    /// Forget what was learnt: the path changed (a new delay).
    pub fn reset(&mut self) {
        *self = Canceller::new();
    }

    /// One block: `played`, the reference at the filter's start, and `mic`, what the microphone
    /// heard. What is left of the microphone goes into `left`, the echo it took away into
    /// `echo`; it learns from this block if `learn`.
    pub fn block(&mut self, played: &[f32], mic: &[f32], learn: bool, left: &mut [f32], echo: &mut [f32]) {
        debug_assert!(played.len() == BLOCK && mic.len() == BLOCK);
        // The newest spectrum: the last block and this one.
        let mut x = self.spectra.pop_back().unwrap_or_else(|| vec![C::default(); FFT]);
        for k in 0..BLOCK {
            x[k] = C { re: self.last[k], im: 0.0 };
            x[BLOCK + k] = C { re: played[k], im: 0.0 };
        }
        self.fft.forward(&mut x);
        self.spectra.push_front(x);
        self.last.copy_from_slice(played);

        // What it expects to hear.
        let y = &mut self.scratch;
        y.iter_mut().for_each(|v| *v = C::default());
        for (w, x) in self.weights.iter().zip(&self.spectra) {
            for f in 0..FFT {
                let p = w[f].mul(x[f]);
                y[f].re += p.re;
                y[f].im += p.im;
            }
        }
        self.fft.inverse(y);
        let mut energy_mic = 0.0;
        let mut energy_left = 0.0;
        for k in 0..BLOCK {
            echo[k] = y[BLOCK + k].re;
            left[k] = mic[k] - echo[k];
            energy_mic += mic[k] * mic[k];
            energy_left += left[k] * left[k];
        }
        // Gone wrong -- adding more than it takes away, over half a second: start again. Not
        // block by block: as a syllable begins the microphone has not heard it yet.
        self.checked.push_back((energy_mic, energy_left));
        if self.checked.len() > 32 {
            self.checked.pop_front();
        }
        let (heard, kept) = self.checked.iter().fold((0.0, 0.0), |(a, b), (m, l)| (a + m, b + l));
        if self.checked.len() == 32 && kept > 2.0 * heard + 1e-5 {
            log::warn!("echo: the canceller went wrong ({:.1} dB added): starting again", 10.0 * (kept / heard.max(1e-12)).log10());
            self.reset();
            left.copy_from_slice(mic);
            echo.iter_mut().for_each(|v| *v = 0.0);
            return;
        }
        if !learn {
            return;
        }
        // What is left, as a spectrum of zeros then this block.
        let mut e = vec![C::default(); FFT];
        for k in 0..BLOCK {
            e[BLOCK + k] = C { re: left[k], im: 0.0 };
        }
        self.fft.forward(&mut e);
        // The power of what was played in each frequency, over the filter's length.
        let mut power = vec![0.0f32; FFT];
        for x in &self.spectra {
            for f in 0..FFT {
                power[f] += x[f].norm();
            }
        }
        let mean = power.iter().sum::<f32>() / FFT as f32;
        let floor = mean * 0.01 + 1e-6;
        let mut g = vec![C::default(); FFT];
        for (w, x) in self.weights.iter_mut().zip(&self.spectra) {
            for f in 0..FFT {
                let scale = self.step / (power[f] + floor);
                let p = x[f].conj().mul(e[f]);
                g[f] = C { re: p.re * scale, im: p.im * scale };
            }
            // Constrained to a linear (not circular) filter: its second half zeroed.
            self.fft.inverse(&mut g);
            for v in g[BLOCK..].iter_mut() {
                *v = C::default();
            }
            self.fft.forward(&mut g);
            for f in 0..FFT {
                w[f].re += g[f].re;
                w[f].im += g[f].im;
            }
        }
    }
}

// --- the gate ---

/// The bands the gate listens in, Hz: about a critical band each, from a low man's voice to the
/// top of speech.
const EDGES: [f32; BANDS + 1] = [
    100.0, 200.0, 300.0, 400.0, 510.0, 630.0, 770.0, 920.0, 1080.0, 1270.0, 1480.0, 1720.0, 2000.0, 2320.0, 2700.0, 3150.0, 3700.0, 4400.0, 5300.0, 6400.0, 7700.0,
];
const BANDS: usize = 20;
/// The bands a voice is looked for in: 100 Hz to 3.7 kHz.
const SPEECH: std::ops::Range<usize> = 0..16;

/// A block's power in each band, a sample, from two blocks windowed.
pub type Bands = [f32; BANDS];

fn bands_of(fft: &Fft, before: &[f32], now: &[f32]) -> Bands {
    let mut x: Vec<C> = before
        .iter()
        .chain(now)
        .enumerate()
        .map(|(i, v)| C { re: v * (0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FFT as f32).cos()), im: 0.0 })
        .collect();
    fft.forward(&mut x);
    let mut out = [0.0; BANDS];
    let hz = RATE as f32 / FFT as f32;
    for (b, o) in out.iter_mut().enumerate() {
        let (lo, hi) = ((EDGES[b] / hz).round() as usize, ((EDGES[b + 1] / hz).round() as usize).min(FFT / 2));
        let n = (hi - lo).max(1) as f32;
        *o = x[lo..hi].iter().map(|c| c.norm()).sum::<f32>() / n / FFT as f32;
    }
    out
}

/// Whether what is left after the canceller is the wearer's voice -- judged **band by band**,
/// so that the wearer is heard where its own voice is not (between its harmonics, below its
/// pitch, in its pauses), and with no trust in the canceller: what is left of its voice in each
/// band is learnt from what was left for what was played, the loud end of it, whatever the
/// canceller managed. A clock that drifts, a path that changes as the head turns, small
/// speakers that distort -- the canceller takes less away, the gate learns that, and asks more
/// of the wearer.
pub struct Gate {
    /// The quietest in each band in half-second steps, the last eight seconds: the room's floor --
    /// long enough that a stretch of the wearer and its voice together is not taken for it.
    floors: VecDeque<Bands>,
    quietest: Bands,
    blocks: usize,
    /// What was left in each band for what was played there, while the wearer was not speaking:
    /// the last five seconds or so.
    ratios: Vec<VecDeque<f32>>,
    /// The same for the microphone as it came, before the canceller.
    raw: Vec<VecDeque<f32>>,
    /// For the log: the block's level, the floor, what was expected of its voice (a sample's
    /// power, over the speech bands), how much like a voice it was, and how much of it was more
    /// than its voice.
    pub level: f32,
    pub floor: f32,
    pub expected: f32,
    pub voiced: f32,
    pub excess: f32,
    /// How much the canceller has been taking away of its voice, dB, smoothed over a second or so.
    pub taken: f32,
}

/// How far above the loud end of its voice a band must be to be more than it: after the
/// canceller (what is left of its voice varies most), and in the microphone as it came.
const FINE_MARGIN: f32 = 3.0;
const RAW_MARGIN: f32 = 2.0;
/// What a band needs to have learnt before what is left of its voice there is known.
const RATIOS_NEEDED: usize = 40;
/// Below this, a band of what was played is nothing: what it plays is exact, so its silence is.
const PLAYED_NOTHING: f32 = 1e-10;

impl Default for Gate {
    fn default() -> Self {
        Gate::new()
    }
}

impl Gate {
    pub fn new() -> Gate {
        Gate {
            floors: VecDeque::new(),
            quietest: [f32::MAX; BANDS],
            blocks: 0,
            ratios: (0..BANDS).map(|_| VecDeque::new()).collect(),
            raw: (0..BANDS).map(|_| VecDeque::new()).collect(),
            level: 0.0,
            floor: 0.0,
            expected: 0.0,
            voiced: 0.0,
            excess: 0.0,
            taken: 0.0,
        }
    }

    /// What was left of its voice is to be learnt again: the canceller started again.
    pub fn forget_voice(&mut self) {
        self.ratios.iter_mut().for_each(VecDeque::clear);
        self.raw.iter_mut().for_each(VecDeque::clear);
    }

    /// Whether enough of its voice has been heard to judge what is left of it.
    pub fn ready(&self) -> bool {
        self.ratios[SPEECH].iter().filter(|r| r.len() >= RATIOS_NEEDED).count() >= SPEECH.len() / 2
    }

    /// One block: what is left in each band (`left`), the loudest played in each band around
    /// where its echo would be (`played`; `None` while where is not known and anything played
    /// may be heard), and the block's samples with the one before (for its pitch). Whether it is
    /// the wearer's voice.
    pub fn block(&mut self, left: &Bands, mic: &Bands, played: Option<&Bands>, playing: bool, unsure: bool, before: &[f32], now: &[f32]) -> Verdict {
        for b in 0..BANDS {
            self.quietest[b] = self.quietest[b].min(left[b]);
        }
        self.blocks += 1;
        if self.blocks % 31 == 0 {
            self.floors.push_back(self.quietest);
            self.quietest = [f32::MAX; BANDS];
            if self.floors.len() > 16 {
                self.floors.pop_front();
            }
        }
        let mut floor = self.quietest;
        for f in &self.floors {
            for b in 0..BANDS {
                floor[b] = floor[b].min(f[b]);
            }
        }
        let floor = floor.map(|f| f.clamp(1e-12, 1.0));
        let voiced = voicing(before, now);
        // Against what is left after the canceller, and against the microphone as it came:
        // the first is sensitive, the second does not depend on the canceller having kept up.
        // What was played, each band at least a thousandth of the loudest: a band it hardly plays
        // in still hears a little of it (small speakers spread a voice), and what was learnt of
        // such a band, for next to nothing played, would expect a roar when it is played at last.
        let effective: Option<Bands> = played.map(|p| {
            let loudest = p.iter().copied().fold(0.0f32, f32::max);
            p.map(|v| v.max(loudest * 1e-3))
        });
        let played = effective.as_ref();
        // Unsure where its echo falls (the delay not found yet), the loudest it has been, not the
        // loud end: what is compared with is the loudest of a whole second, which its echo is seldom.
        let loud_end = if unsure { 1.0 } else { 0.9 };
        let fine = judge(left, &floor, &self.ratios, played, playing, FINE_MARGIN, loud_end);
        let raw = judge(mic, &floor, &self.raw, played, playing, RAW_MARGIN, loud_end);
        self.level = fine.total;
        self.floor = fine.floors;
        self.expected = fine.expected;
        self.voiced = voiced;
        self.excess = if fine.total > 0.0 { fine.more / fine.total } else { 0.0 };
        // Speaking over its voice: the microphone well above its voice, whatever the canceller
        // made of it. The canceller does not learn then, and neither does this.
        let over = raw.bands_more >= 2 && raw.more > 0.2 * raw.total;
        // How much the canceller has been taking away, while only its voice was heard, for the log.
        if playing && !over && raw.total > 4.0 * raw.floors {
            let taken = 10.0 * (raw.total / fine.total.max(1e-12)).log10();
            self.taken += (taken.clamp(-10.0, 40.0) - self.taken) * 0.03;
        }
        // **While it plays, both must hear the wearer**: what is left after the canceller, and the
        // microphone as it came. What is left alone is not enough, even when the canceller has
        // been doing well -- on one clock it took most of its voice away, and its slips, where a
        // sentence began, were then heard as the wearer four times in a minute and a half (real
        // speech, 2026-11-15). So the canceller makes the wearer's words clearer for Whisper, and
        // the decision to stop rests on the microphone well above its own voice.
        let voice = fine.total > 6.0 * fine.floors && fine.bands_more >= 3 && fine.more > 0.3 * fine.total && voiced > 0.4 && (over || !playing);
        if let (Some(p), false) = (played, over || voice) {
            for b in 0..BANDS {
                if playing && p[b] > PLAYED_NOTHING {
                    for (ratios, heard) in [(&mut self.ratios[b], left[b]), (&mut self.raw[b], mic[b])] {
                        ratios.push_back(heard / p[b]);
                        if ratios.len() > 300 {
                            ratios.pop_front();
                        }
                    }
                }
            }
        }
        Verdict { voice, over }
    }
}

/// What a gate made of a block: the wearer's voice is in it; the microphone is well above what
/// its own voice would make it, so the canceller must not learn from it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Verdict {
    pub voice: bool,
    pub over: bool,
}

struct Judged {
    total: f32,
    floors: f32,
    expected: f32,
    more: f32,
    bands_more: usize,
}

/// How much of `heard` is more than its voice would make it: band by band, its voice expected at
/// the loud end of what `ratios` learnt (`f32::MAX` where nothing is learnt yet, so nothing there
/// is more than it).
fn judge(heard: &Bands, floor: &Bands, ratios: &[VecDeque<f32>], played: Option<&Bands>, playing: bool, margin: f32, loud_end: f32) -> Judged {
    let mut j = Judged { total: 0.0, floors: 0.0, expected: 0.0, more: 0.0, bands_more: 0 };
    for b in SPEECH {
        let expected = match played {
            _ if !playing => 0.0,
            None => f32::MAX,
            Some(p) if p[b] <= PLAYED_NOTHING => 0.0,
            Some(p) if ratios[b].len() >= RATIOS_NEEDED => p[b] * percentile(&ratios[b], loud_end),
            Some(_) => f32::MAX,
        };
        j.total += heard[b];
        j.floors += floor[b];
        if expected < f32::MAX {
            if j.expected < f32::MAX {
                j.expected += expected;
            }
            let over = heard[b] - margin * expected - 2.0 * floor[b];
            if over > 0.0 {
                j.more += over;
                j.bands_more += 1;
            }
        } else {
            j.expected = f32::MAX;
        }
    }
    j
}

fn percentile(values: &VecDeque<f32>, p: f32) -> f32 {
    let mut v: Vec<f32> = values.iter().copied().collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[((v.len() - 1) as f32 * p) as usize]
}

/// How much like a voice a block is: its strongest repetition at a speaking pitch (80-400 Hz),
/// 0 to 1, over the block and the one before.
fn voicing(before: &[f32], now: &[f32]) -> f32 {
    let x: Vec<f32> = before.iter().chain(now).copied().collect();
    let n = x.len();
    let mut best = 0.0f32;
    for lag in (RATE as usize / 400)..=(RATE as usize / 80) {
        let (mut ab, mut aa, mut bb) = (0.0f32, 0.0f32, 0.0f32);
        for i in 0..n - lag {
            ab += x[i] * x[i + lag];
            aa += x[i] * x[i];
            bb += x[i + lag] * x[i + lag];
        }
        if aa > 0.0 && bb > 0.0 {
            best = best.max(ab / (aa * bb).sqrt());
        }
    }
    best
}

// --- deciding to stop ---

/// The wearer has begun speaking over it: enough blocks of voice, close enough together, while
/// it speaks -- [`NEEDED`] of the last [`WINDOW`], [`RUN`] of them running.
#[derive(Default)]
pub struct BargeIn {
    recent: VecDeque<bool>,
    run: usize,
}

pub const WINDOW: usize = 16;
pub const NEEDED: usize = 6;
pub const RUN: usize = 3;

impl BargeIn {
    /// One block: whether it is speaking (or about to), and whether the gate heard a voice. `true`
    /// the moment it should stop.
    pub fn block(&mut self, speaking: bool, voice: bool) -> bool {
        if !speaking {
            self.recent.clear();
            self.run = 0;
            return false;
        }
        self.recent.push_back(voice);
        if self.recent.len() > WINDOW {
            self.recent.pop_front();
        }
        self.run = if voice { self.run + 1 } else { 0 };
        let heard = self.recent.iter().filter(|v| **v).count();
        let longest = longest_run(&self.recent);
        if heard >= NEEDED && longest >= RUN {
            self.recent.clear();
            self.run = 0;
            return true;
        }
        false
    }
}

fn longest_run(v: &VecDeque<bool>) -> usize {
    let (mut best, mut now) = (0, 0);
    for &b in v {
        now = if b { now + 1 } else { 0 };
        best = best.max(now);
    }
    best
}

// --- all of it ---

/// What [`Echo::block`] made of a block.
pub struct Heard {
    /// The microphone with its voice taken out.
    pub left: Vec<f32>,
    /// The wearer's voice is in it.
    pub voice: bool,
    /// The wearer has begun speaking over it: stop.
    pub barge_in: bool,
}

/// The whole of it, for one microphone: blocks in, what is left and whether to stop out.
pub struct Echo {
    clock: MicClock,
    delay: DelayFinder,
    canceller: Canceller,
    gate: Gate,
    barge: BargeIn,
    pending: Vec<f32>,
    pending_index: u64,
    before: Vec<f32>,
    before_mic: Vec<f32>,
    /// The last block was the wearer over its voice: the canceller does not learn from the next.
    over: bool,
    /// How many blocks it has played in, all told.
    played_blocks: usize,
    fft: Fft,
    /// The bands of what was played, by the block they start at.
    cache: VecDeque<(u64, Bands)>,
    /// What was said, block by block, for the log: the last decision's numbers.
    pub last: String,
}

impl Default for Echo {
    fn default() -> Self {
        Echo::new()
    }
}

impl Echo {
    pub fn new() -> Echo {
        Echo {
            clock: MicClock::new(),
            delay: DelayFinder::new(),
            canceller: Canceller::new(),
            gate: Gate::new(),
            barge: BargeIn::default(),
            pending: Vec::new(),
            pending_index: 0,
            before: vec![0.0; BLOCK],
            before_mic: vec![0.0; BLOCK],
            over: false,
            played_blocks: 0,
            fft: Fft::new(FFT),
            cache: VecDeque::new(),
            last: String::new(),
        }
    }

    /// The delay found, in milliseconds.
    pub fn delay_ms(&self) -> Option<u32> {
        self.delay.delay.map(|d| (d * BLOCK * 1000 / RATE as usize) as u32)
    }

    /// Samples from the microphone (16 kHz, -1 to 1) that arrived at `at`, while it is or is not
    /// `speaking`. Whole blocks come out as they fill.
    pub fn hear(&mut self, played: &Played, at: Instant, mic: &[f32], speaking: bool) -> Vec<Heard> {
        let first = self.clock.arrived(played, at, mic.len());
        if self.pending.is_empty() {
            self.pending_index = first;
        }
        self.pending.extend_from_slice(mic);
        let mut out = Vec::new();
        while self.pending.len() >= BLOCK {
            let block: Vec<f32> = self.pending.drain(..BLOCK).collect();
            let index = self.pending_index;
            self.pending_index += BLOCK as u64;
            out.push(self.block(played, index, &block, speaking));
        }
        out
    }

    fn block(&mut self, played: &Played, index: u64, mic: &[f32], speaking: bool) -> Heard {
        let energy = |s: &[f32]| s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32;
        // What was played when this block was heard, for the delay's matching.
        let mut now = vec![0.0; BLOCK];
        played.read(index, &mut now);
        let lag = |blocks: usize| index.saturating_sub((blocks * BLOCK) as u64);
        if self.delay.block(energy(mic), energy(&now)) {
            log::info!("echo: its voice comes back {} ms after it is played (match {:.2})", self.delay_ms().unwrap_or(0), self.delay.confidence);
            self.canceller.reset();
            // What was left before is no guide to what will be.
            self.gate.forget_voice();
        }
        // Where its echo would be: a little before the delay to a little after, for the room's
        // own tail; anywhere in the last second while the delay is not known.
        let window = match self.delay.delay {
            Some(d) => d.saturating_sub(3)..=d + 10,
            None => 0..=MOST_DELAY,
        };
        let mut loudest = [0.0f32; BANDS];
        let mut playing = false;
        for b in window {
            let start = lag(b);
            if !played.played_since(start) {
                continue;
            }
            let bands = self.played_bands(played, start);
            playing |= bands.iter().any(|p| *p > PLAYED_NOTHING);
            for k in 0..BANDS {
                loudest[k] = loudest[k].max(bands[k]);
            }
        }
        let mut left = vec![0.0; BLOCK];
        match self.delay.delay {
            Some(d) => {
                let mut x = vec![0.0; BLOCK];
                played.read(lag(d.saturating_sub(LEAD)), &mut x);
                let mut echo = vec![0.0; BLOCK];
                // Learning needs something played, and the wearer quiet: the gate's last word.
                let learn = playing && !self.over;
                self.canceller.block(&x, mic, learn, &mut left, &mut echo);
            }
            None => left.copy_from_slice(mic),
        }
        let left_bands = bands_of(&self.fft, &self.before, &left);
        let mic_bands = bands_of(&self.fft, &self.before_mic, mic);
        // Where the delay is not known, the loudest of the whole last second stands for what may
        // be heard: cautious while its echo is still being found -- and the only way to hear the
        // wearer at all where its voice never reaches the microphone (earphones, a quiet room),
        // so that there is no echo to find.
        // But not before it has played for longer than any delay looked for: what is learnt of
        // its voice before its echo can have come back is nothing, and the echo then a roar.
        if playing {
            self.played_blocks += 1;
        }
        let window_full = self.delay.delay.is_some() || self.played_blocks > MOST_DELAY + 8;
        let verdict = self.gate.block(&left_bands, &mic_bands, window_full.then_some(&loudest), playing, self.delay.delay.is_none(), &self.before, &left);
        self.over = verdict.over;
        let voice = verdict.voice;
        self.before.copy_from_slice(&left);
        self.before_mic.copy_from_slice(mic);
        let barge_in = self.barge.block(speaking, voice);
        let db = |v: f32| if v == f32::MAX { f32::INFINITY } else { 10.0 * (v + 1e-12).log10() };
        self.last = format!(
            "taken away {:.1} dB, left {:.1} dB, floor {:.1} dB, its voice expected {:.1} dB, {:.0}% more than it, voiced {:.2}",
            self.gate.taken,
            db(self.gate.level),
            db(self.gate.floor),
            db(self.gate.expected),
            self.gate.excess * 100.0,
            self.gate.voiced
        );
        Heard { left, voice, barge_in }
    }

    /// The bands of what was played in the block starting at `start`, kept: the same blocks are
    /// asked for at each delay in turn.
    fn played_bands(&mut self, played: &Played, start: u64) -> Bands {
        if let Some((_, b)) = self.cache.iter().find(|(s, _)| *s == start) {
            return *b;
        }
        let (mut before, mut now) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
        played.read(start.saturating_sub(BLOCK as u64), &mut before);
        played.read(start, &mut now);
        let bands = bands_of(&self.fft, &before, &now);
        self.cache.push_back((start, bands));
        if self.cache.len() > MOST_DELAY + 16 {
            self.cache.pop_front();
        }
        bands
    }
}

// --- a room, for tests ---

/// A made-up room between the speakers and the microphone, for tests that have no speakers:
/// what was played comes back `delay` later, through a few reflections and a short tail, a
/// little distorted as small speakers are, at `gain`, on a clock `drift` parts in a million off.
pub struct SimulatedRoom {
    gain: f32,
    /// How hard small speakers squash it: 0 not at all.
    pub distortion: f32,
    delay: f64,
    drift: f64,
    taps: Vec<(f64, f32)>,
}

impl SimulatedRoom {
    pub fn new(gain: f32, delay_s: f64, drift_ppm: f64) -> SimulatedRoom {
        let mut taps = vec![(0.0, 1.0), (0.0021, 0.45), (0.0053, -0.3), (0.011, 0.18), (0.023, 0.1)];
        let mut seed = 7u32;
        for k in 0..40 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let r = (seed >> 8) as f32 / (1 << 24) as f32 - 0.5;
            let t = 0.025 + k as f64 * 0.0015;
            taps.push((t, r * 0.12 * (-(t as f32) / 0.03).exp()));
        }
        SimulatedRoom { gain, distortion: 0.5, delay: delay_s, drift: drift_ppm * 1e-6, taps }
    }

    /// The echo heard at timeline samples `from..from + out.len()`.
    pub fn echo(&self, played: &Played, from: u64, out: &mut [f32]) {
        for (k, o) in out.iter_mut().enumerate() {
            let t = (from + k as u64) as f64 * (1.0 + self.drift) - self.delay * RATE as f64;
            let mut s = 0.0;
            for (at, g) in &self.taps {
                s += g * played.at(t - at * RATE as f64);
            }
            let s = s * self.gain;
            *o = if self.distortion > 0.0 { (self.distortion * s).tanh() / self.distortion } else { s };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_fft_goes_there_and_back() {
        let fft = Fft::new(16);
        let original: Vec<C> = (0..16).map(|i| C { re: (i as f32 * 0.7).sin(), im: 0.0 }).collect();
        let mut x = original.clone();
        fft.forward(&mut x);
        fft.inverse(&mut x);
        for (a, b) in x.iter().zip(&original) {
            assert!((a.re - b.re).abs() < 1e-5 && a.im.abs() < 1e-5);
        }
        // A cosine at bin 3 is there and nowhere else.
        let mut c: Vec<C> = (0..16).map(|i| C { re: (2.0 * std::f32::consts::PI * 3.0 * i as f32 / 16.0).cos(), im: 0.0 }).collect();
        fft.forward(&mut c);
        assert!((c[3].re - 8.0).abs() < 1e-4 && c[5].norm() < 1e-6);
    }

    /// Something like speech: a voice with a pitch, syllables and pauses, `seed` for which.
    fn talk(seconds: f32, pitch: f32, seed: u32, rate: u32) -> Vec<f32> {
        let n = (seconds * rate as f32) as usize;
        let mut s = seed;
        let mut rand = move || {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 8) as f32 / (1 << 24) as f32
        };
        let mut out = Vec::with_capacity(n);
        let mut phase = 0.0f32;
        let mut syllable_left = 0usize;
        let (mut loud, mut f0) = (0.0f32, pitch);
        let mut formant = 0.0f32;
        for _ in 0..n {
            if syllable_left == 0 {
                syllable_left = ((0.08 + 0.2 * rand()) * rate as f32) as usize;
                loud = if rand() < 0.25 { 0.0 } else { 0.3 + 0.5 * rand() };
                f0 = pitch * (0.85 + 0.3 * rand());
                formant = 500.0 + 1500.0 * rand();
            }
            syllable_left -= 1;
            phase += f0 / rate as f32;
            let t = phase * 2.0 * std::f32::consts::PI;
            let mut v = 0.0;
            for h in 1..12 {
                let f = h as f32 * f0;
                let w = 1.0 / (1.0 + ((f - formant) / 400.0).powi(2));
                v += w * (t * h as f32).sin();
            }
            out.push(loud * v * 0.25 + (rand() - 0.5) * 0.01 * loud);
        }
        out
    }

    /// Its voice played through a made-up room, the wearer's (or not) on top, through the whole
    /// of it: the blocks at which it would have stopped.
    fn run(own: &[f32], wearer: Option<(&[f32], f32)>, gain: f32, delay: f64, distortion: f32, drift: f64) -> (Vec<usize>, Echo) {
        let epoch = Instant::now();
        let mut played = Played::new(epoch);
        let mut room = SimulatedRoom::new(gain, delay, drift);
        room.distortion = distortion;
        let mut echo = Echo::new();
        let piece = 320; // 20 ms, as the room's microphone comes
        let mut seed = 99u32;
        let mut stops = Vec::new();
        let total = own.len() / 24_000 * RATE as usize + RATE as usize;
        let mut mic_at = 0usize;
        let mut block = 0;
        // Played in 50 ms slices at 24 kHz, a little ahead, as the voice plays.
        let own_slices: Vec<&[f32]> = own.chunks(1200).collect();
        let mut next_slice = 0;
        while mic_at < total {
            let at = epoch + Duration::from_secs_f64((mic_at + piece) as f64 / RATE as f64 + 0.004);
            while next_slice < own_slices.len() && next_slice as f64 * 0.05 <= (mic_at as f64 / RATE as f64) + 0.2 {
                played.push(epoch + Duration::from_secs_f64(next_slice as f64 * 0.05), own_slices[next_slice], 24_000);
                next_slice += 1;
            }
            let mut mic = vec![0.0; piece];
            room.echo(&played, mic_at as u64, &mut mic);
            for (k, m) in mic.iter_mut().enumerate() {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *m += ((seed >> 8) as f32 / (1 << 24) as f32 - 0.5) * 0.002;
                if let Some((w, starts)) = wearer {
                    let i = mic_at + k;
                    let s = (starts * RATE as f32) as usize;
                    if i >= s && i - s < w.len() {
                        *m += w[i - s];
                    }
                }
            }
            let speaking = mic_at < own.len() * 2 / 3 + 3200;
            for heard in echo.hear(&played, at, &mic, speaking) {
                if std::env::var_os("ECHO_DEBUG").is_some() {
                    eprintln!("{:6.2} s voice {} ready {} delay {:?} {}", block as f32 * BLOCK as f32 / RATE as f32, heard.voice, echo.gate.ready(), echo.delay_ms(), echo.last);
                }
                if heard.barge_in {
                    stops.push(block);
                }
                block += 1;
            }
            mic_at += piece;
        }
        (stops, echo)
    }

    #[test]
    fn its_own_voice_coming_back_does_not_stop_it() {
        // Twelve seconds of its voice, back at the microphone at a wearer's loudness, and louder;
        // delays from a quick device to Bluetooth's; speakers clean and squashed; three voices.
        for seed in [1, 2, 3] {
            let own = talk(12.0, 190.0 + 20.0 * seed as f32, seed, 24_000);
            for (gain, delay, distortion) in [(1.0, 0.12, 0.5), (2.0, 0.31, 0.5), (1.0, 0.55, 0.0), (4.0, 0.2, 1.5)] {
                let (stops, echo) = run(&own, None, gain, delay, distortion, 150.0);
                assert!(stops.is_empty(), "seed {seed}, gain {gain}, delay {delay}: stopped at blocks {stops:?} ({})", echo.last);
                let found = echo.delay_ms().unwrap_or(0) as f64 / 1000.0;
                assert!((found - delay).abs() < 0.04, "delay {delay} found as {found}");
            }
        }
    }

    #[test]
    fn the_wearer_speaking_over_it_stops_it_at_once() {
        let own = talk(12.0, 210.0, 1, 24_000);
        // A lower voice than its own, and one as high; as loud as its voice at the microphone,
        // and half as loud.
        // On one clock (the glasses' own microphone and speakers), and on two clocks well apart.
        // As loud as its voice at the microphone, at any delay and on two clocks; softer (0.6), on
        // one clock with a quick device. (Softer still, or softer with a long delay, it is not
        // heard over its voice: the wearer must speak up, or wait for a pause.)
        for (pitch, loud, drift, delays) in [(120.0, 0.9, 150.0, &[0.12, 0.31][..]), (220.0, 0.9, 150.0, &[0.12, 0.31]), (120.0, 1.0, 0.0, &[0.12, 0.31, 0.55]), (120.0, 0.6, 0.0, &[0.12])] {
            let wearer: Vec<f32> = talk(2.5, pitch, 5, RATE).iter().map(|s| s * loud).collect();
            for &delay in delays {
                let gain = 1.0;
                let (stops, echo) = run(&own, Some((&wearer, 6.0)), gain, delay, 0.5, drift);
                let first = *stops.first().unwrap_or_else(|| panic!("pitch {pitch}, loud {loud}, delay {delay}, drift {drift}: never stopped ({})", echo.last));
                let at = first as f32 * BLOCK as f32 / RATE as f32;
                eprintln!("pitch {pitch}, loud {loud}, delay {delay}, drift {drift}: stopped {:.0} ms after the wearer began", (at - 6.0) * 1000.0);
                assert!(at > 6.0 && at < 6.6, "pitch {pitch}, loud {loud}: stopped at {at:.2} s, the wearer began at 6.0 s");
            }
        }
    }

    #[test]
    fn with_none_of_its_voice_in_the_microphone_the_wearer_still_stops_it() {
        // Earphones, or its voice somewhere the microphone does not hear: no echo to find.
        let own = talk(12.0, 210.0, 1, 24_000);
        let wearer: Vec<f32> = talk(2.5, 120.0, 5, RATE).iter().map(|s| s * 0.3).collect();
        let (stops, echo) = run(&own, Some((&wearer, 6.0)), 0.0, 0.12, 0.0, 0.0);
        let first = *stops.first().unwrap_or_else(|| panic!("never stopped ({})", echo.last));
        let at = first as f32 * BLOCK as f32 / RATE as f32;
        assert!(at > 6.0 && at < 6.5, "stopped at {at:.2} s");
        let (stops, _) = run(&own, None, 0.0, 0.12, 0.0, 0.0);
        assert!(stops.is_empty());
    }

    #[test]
    fn the_canceller_takes_most_of_its_voice_away() {
        // On one clock, the canceller takes nearly all of it; on two clocks a little apart (a
        // microphone and speakers on different devices) less, and the gate makes up for it.
        let own = talk(8.0, 210.0, 1, 24_000);
        for (distortion, drift, least) in [(0.0, 0.0, 12.0), (0.5, 0.0, 10.0), (0.0, 10.0, 6.0)] {
            let epoch = Instant::now();
            let mut played = Played::new(epoch);
            played.push(epoch, &own, 24_000);
            let mut room = SimulatedRoom::new(1.0, 0.1, drift);
            room.distortion = distortion;
            let mut c = Canceller::new();
            let (mut before, mut after) = (0.0f64, 0.0f64);
            let blocks = own.len() * 2 / 3 / BLOCK;
            for b in 0..blocks {
                let i = (b * BLOCK) as u64;
                let mut mic = vec![0.0; BLOCK];
                room.echo(&played, i, &mut mic);
                let mut x = vec![0.0; BLOCK];
                played.read(i.saturating_sub(((0.1 * RATE as f64) as u64).saturating_sub((LEAD * BLOCK) as u64)), &mut x);
                let (mut left, mut echo) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
                c.block(&x, &mic, true, &mut left, &mut echo);
                if b > blocks / 2 {
                    before += mic.iter().map(|v| (v * v) as f64).sum::<f64>();
                    after += left.iter().map(|v| (v * v) as f64).sum::<f64>();
                }
            }
            let erle = 10.0 * (before / after.max(1e-20)).log10();
            eprintln!("distortion {distortion}, drift {drift} ppm: {erle:.1} dB taken away");
            assert!(erle > least, "distortion {distortion}, drift {drift}: only {erle:.1} dB");
        }
    }

    /// Real speech: `ECHO_SPEECH=<folder of 24 kHz WAVs>` (its sentences, as the voice recorded
    /// them) and `ECHO_WEARER=<16 kHz WAV>,<seconds in>`. Run by hand.
    #[test]
    #[ignore]
    fn on_real_speech() {
        let Some(dir) = std::env::var_os("ECHO_SPEECH") else { return };
        let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).collect();
        files.sort();
        let mut own = Vec::new();
        for f in files {
            let (pcm, _) = crate::hemera::read_wav(&std::fs::read(f).unwrap()).unwrap();
            own.extend(pcm.iter().map(|s| *s as f32 / 32768.0));
            own.extend(std::iter::repeat(0.0).take(24_000 / 4));
        }
        let wearer = std::env::var("ECHO_WEARER").ok().map(|w| {
            let (path, at) = w.split_once(',').unwrap();
            let (pcm, _) = crate::hemera::read_wav(&std::fs::read(path).unwrap()).unwrap();
            (pcm.iter().map(|s| *s as f32 / 32768.0).collect::<Vec<f32>>(), at.parse::<f32>().unwrap())
        });
        let gain: f32 = std::env::var("ECHO_GAIN").ok().and_then(|g| g.parse().ok()).unwrap_or(1.0);
        let env = |k: &str, d: f64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        let (stops, echo) = run(&own, wearer.as_ref().map(|(w, at)| (&w[..], *at)), gain, env("ECHO_DELAY", 0.18), env("ECHO_DISTORTION", 0.5) as f32, env("ECHO_DRIFT", 40.0));
        eprintln!("{:.1} s of its voice; stopped at {:?} s ({})", own.len() as f32 / 24_000.0, stops.iter().map(|b| *b as f32 * BLOCK as f32 / RATE as f32).collect::<Vec<_>>(), echo.last);
    }

    #[test]
    fn a_wearer_speaking_while_it_is_quiet_is_not_a_barge_in() {
        let mut b = BargeIn::default();
        assert!((0..40).all(|_| !b.block(false, true)));
        // Scattered voice blocks are not enough; a run of them is.
        let mut b = BargeIn::default();
        assert!((0..40).all(|i| !b.block(true, i % 3 == 0)));
        let mut b = BargeIn::default();
        let fired: Vec<bool> = (0..12).map(|_| b.block(true, true)).collect();
        assert_eq!(fired.iter().position(|f| *f), Some(NEEDED - 1));
    }

    #[test]
    fn what_was_played_is_found_where_it_was_played() {
        let epoch = Instant::now();
        let mut p = Played::new(epoch);
        p.push(epoch + Duration::from_millis(100), &[0.5; 2400], 24_000);
        let mut out = vec![0.0; 1700];
        p.read(1500, &mut out);
        assert_eq!(out[0], 0.0);
        assert!((out[100] - 0.5).abs() < 1e-6 && (out[1699] - 0.5).abs() < 1e-6);
        assert!(p.played_since(3000) && !p.played_since(3200));
    }
}
