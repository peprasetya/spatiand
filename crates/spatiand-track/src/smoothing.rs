//! Taking the shake out without adding lag.
//!
//! Ported from HoloFrame's `ViewSmoothing.swift`.
//!
//! The view follows head pose one-to-one, so involuntary motion — breathing, pulse, the desk
//! being knocked — moves the image several pixels and reads as jitter. A fixed low-pass
//! filter removes it at the cost of lag on real movement, which is worse: a laggy view feels
//! detached from your head and is the classic route to discomfort.
//!
//! The One Euro filter (Casiez, Roussel & Vogel, CHI 2012) solves exactly this. Its cutoff
//! frequency rises with the speed of the signal, so it filters heavily when you are nearly
//! still — where jitter is all there is to see — and barely at all when you turn, where lag
//! would be felt and jitter would not be noticed.
//!
//! The room's view goes a step further ([`Steady`]): a head that only sways, inside a band,
//! moves nothing at all, and the band is let go of the moment the head turns.

/// Exponential smoothing with a cutoff that adapts to how fast the value is changing.
#[derive(Debug, Clone)]
pub struct OneEuroFilter {
    /// Cutoff in Hz while stationary. Lower means steadier but slower to start moving.
    pub min_cutoff: f64,
    /// How much the cutoff opens up with speed. Higher means less lag when turning.
    pub beta: f64,
    /// Cutoff for the speed estimate itself, so a noisy derivative cannot flap the cutoff.
    pub derivative_cutoff: f64,
    previous: Option<f64>,
    previous_derivative: f64,
}

impl OneEuroFilter {
    pub fn new(min_cutoff: f64, beta: f64) -> Self {
        Self {
            min_cutoff,
            beta,
            derivative_cutoff: 1.0,
            previous: None,
            previous_derivative: 0.0,
        }
    }

    fn alpha(cutoff: f64, dt: f64) -> f64 {
        let tau = 1.0 / (2.0 * std::f64::consts::PI * cutoff);
        1.0 / (1.0 + tau / dt)
    }

    pub fn filter(&mut self, value: f64, dt: f64) -> f64 {
        if dt <= 0.0 {
            return self.previous.unwrap_or(value);
        }
        let Some(last) = self.previous else {
            self.previous = Some(value);
            return value;
        };

        let rate = (value - last) / dt;
        let a_d = Self::alpha(self.derivative_cutoff, dt);
        let smoothed_rate = a_d * rate + (1.0 - a_d) * self.previous_derivative;
        self.previous_derivative = smoothed_rate;

        // The whole trick: open the cutoff in proportion to speed.
        let cutoff = self.min_cutoff + self.beta * smoothed_rate.abs();
        let a = Self::alpha(cutoff, dt);
        let smoothed = a * value + (1.0 - a) * last;
        self.previous = Some(smoothed);
        smoothed
    }

    pub fn reset(&mut self) {
        self.previous = None;
        self.previous_derivative = 0.0;
    }

    /// Speed of the filtered signal, in units per second.
    pub fn speed(&self) -> f64 {
        self.previous_derivative.abs()
    }
}

/// Three One Euro filters, with yaw unwrapped so crossing the ±180° seam does not register
/// as a 360°/frame lurch and blow the cutoff wide open.
#[derive(Debug, Clone)]
pub struct PoseSmoother {
    yaw: OneEuroFilter,
    pitch: OneEuroFilter,
    roll: OneEuroFilter,
    unwrapped_yaw: Option<f64>,
}

impl PoseSmoother {
    pub fn new(min_cutoff: f64, beta: f64) -> Self {
        Self {
            yaw: OneEuroFilter::new(min_cutoff, beta),
            pitch: OneEuroFilter::new(min_cutoff, beta),
            roll: OneEuroFilter::new(min_cutoff, beta),
            unwrapped_yaw: None,
        }
    }

    /// Highest angular speed across the three axes, degrees/second.
    pub fn speed(&self) -> f64 {
        self.yaw
            .speed()
            .max(self.pitch.speed())
            .max(self.roll.speed())
    }

    /// Smooth one (yaw, pitch, roll) triple in degrees.
    pub fn filter(&mut self, yaw: f64, pitch: f64, roll: f64, dt: f64) -> (f64, f64, f64) {
        // Accumulate yaw continuously rather than filtering the wrapped value.
        let mut continuous = yaw;
        if let Some(last) = self.unwrapped_yaw {
            let mut delta = yaw - last.rem_euclid(360.0);
            while delta > 180.0 {
                delta -= 360.0;
            }
            while delta < -180.0 {
                delta += 360.0;
            }
            continuous = last + delta;
        }
        self.unwrapped_yaw = Some(continuous);

        (
            self.yaw.filter(continuous, dt),
            self.pitch.filter(pitch, dt),
            self.roll.filter(roll, dt),
        )
    }

    pub fn reset(&mut self) {
        self.yaw.reset();
        self.pitch.reset();
        self.roll.reset();
        self.unwrapped_yaw = None;
    }
}

/// How firmly the room is held still against the head's small movements: the wearer's choice in
/// the settings (`steadiness` in `prefs.toml`), the same on the Deck, the Beam Pro and the Mac.
///
/// A body sitting still is never still. It breathes, it sways, it shakes with a laugh, a cough,
/// a thumb pressing a pad; the glasses report all of it, and a room that followed all of it shook
/// with the wearer -- and the pointer with it, since every pointer is a place in the view
/// (`pointer::press_hold`). XREAL's own room lets small movements go and follows larger ones a
/// moment late. The owner, 2026-11-17: "a small movement and the screen shakes with the shake of
/// my body... when selecting I have to make sure my head doesn't move at all."
///
/// Each level is a [`Hold`] for the view and a look back for a press ([`Self::press_settle`]):
/// one setting for both, since a pointer held still in a room that is not is no help.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Steadiness {
    /// The room follows the head as tracked, every movement of it.
    Off,
    /// A pulse, a tremor and the least of a sway held out; a look of a degree moves the room.
    Light,
    /// The sway of a body sitting, breathing and talking held out too. The default.
    #[default]
    Medium,
    /// A body that moves more than that, at the price of a look of two degrees moving nothing.
    Strong,
}

impl Steadiness {
    /// In the order the settings step through them.
    pub const ALL: [Steadiness; 4] = [Self::Off, Self::Light, Self::Medium, Self::Strong];

    /// Where in [`Self::ALL`].
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(2)
    }

    /// The next, from the strongest round to off.
    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    /// As `prefs.toml` and `SPATIAND_STEADY` write it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Light => "light",
            Self::Medium => "medium",
            Self::Strong => "strong",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name().eq_ignore_ascii_case(name.trim()))
    }

    /// How the view is held, or nothing for a view that is the head.
    ///
    /// The figures are degrees and seconds, so they mean the same at the Deck's 72 frames a
    /// second as at the others' 60. What each was measured to do, with a made-up head and with a
    /// recorded one, is in the tests below and in `examples/steadiness.rs`.
    pub fn hold(self) -> Option<Hold> {
        let hold = |band: f64, turning_band: f64, turn_speed: (f64, f64), reopen: f64, settle: (f64, f64)| Hold {
            band,
            turning_band,
            turn_speed,
            close: 0.35,
            reopen,
            settle,
        };
        match self {
            Self::Off => None,
            Self::Light => Some(hold(0.5, 0.1, (10.0, 30.0), 2.0, (5.0, 0.5))),
            Self::Medium => Some(hold(1.2, 0.2, (15.0, 40.0), 4.0, (4.0, 0.5))),
            Self::Strong => Some(hold(2.5, 0.3, (20.0, 50.0), 8.0, (3.0, 0.4))),
        }
    }

    /// How far back a press looks for where the pointer rested before pressing moved it, in
    /// milliseconds, and how far it may have gone from there and still be put back, in degrees.
    /// Nothing when off. See the pointer's `press_hold`.
    pub fn press_settle(self) -> (u32, f64) {
        match self {
            Self::Off => (0, 0.0),
            Self::Light => (80, 0.6),
            Self::Medium => (120, 1.0),
            Self::Strong => (150, 1.5),
        }
    }
}

/// What a level of [`Steadiness`] holds the view against, in degrees and seconds. See [`Steady`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hold {
    /// Within this many degrees of where the view is held, the head moves nothing.
    pub band: f64,
    /// What the band closes to while the head turns: how far behind a turn the view keeps.
    pub turning_band: f64,
    /// Head speeds, degrees a second, from which pushing the band starts to count as a turn, and
    /// by which it is one altogether. Above a sway's: a body swaying two degrees once a second
    /// moves at under thirteen.
    pub turn_speed: (f64, f64),
    /// How much a turn closes the band by, for each degree it pushes it.
    pub close: f64,
    /// How fast the band opens again round a head that has slowed, degrees a second.
    pub reopen: f64,
    /// How fast the view drawn settles on where it is held: a cutoff in Hz at rest, and how many
    /// more for each degree a second the held view moves.
    pub settle: (f64, f64),
}

/// A gap in the frames longer than this is a stall -- the loop held up, the glasses asleep --
/// and not a movement to hold or smooth: the view starts again from the head.
const STALL_SECONDS: f64 = 0.25;

/// A head that goes further than this between two frames has been recentred, not turned: no head
/// turns 20 degrees in a sixtieth of a second.
const JUMP_DEGREES: f64 = 20.0;

/// The view held still while the head only sways, and let go the moment it turns.
///
/// Three parts, each in degrees and seconds:
///
/// 1. **A band.** Where the view is held moves only by as much as the head has gone past
///    [`Hold::band`] from it, as gears with play between them. A head swaying inside the band
///    moves nothing, to the last bit, and there is no creep afterwards: nothing at all moves a
///    view whose head is inside its band. A sway a little wider pushes the band once and then
///    sways inside it.
/// 2. **A turn closes the band.** A head pushing it at a turn's speed ([`Hold::turn_speed`])
///    closes it to [`Hold::turning_band`], half a degree for each degree it pushes, so the view
///    makes up the band within the first few degrees of a turn and ends the turn where the head
///    is; once the head slows, the band opens round it again ([`Hold::reopen`]), and is then as
///    wide either side. A sway, however wide, is too slow to close it.
/// 3. **The view drawn settles on where it is held** rather than jumping when the band is pushed,
///    over a time that shortens the faster the held view moves, as a One Euro filter's does
///    ([`Hold::settle`]): a few hundredths of a second from rest, next to nothing in a turn.
///
/// Done on the orientation itself rather than on three angles, so there is no seam to cross,
/// nothing special about looking straight up, and roll is held as yaw and pitch are.
#[derive(Debug, Clone)]
pub struct Steady {
    hold: Hold,
    /// The head last frame, where the view is held, and the view drawn.
    head: Option<glam::DQuat>,
    held: Option<glam::DQuat>,
    view: Option<glam::DQuat>,
    /// How fast the head turns, and the held view, degrees a second, evened out.
    speed: f64,
    held_speed: f64,
    /// The band as it is now: [`Hold::band`], or less in a turn.
    band: f64,
}

impl Steady {
    pub fn new(hold: Hold) -> Self {
        Self { hold, head: None, held: None, view: None, speed: 0.0, held_speed: 0.0, band: hold.band }
    }

    /// Hold by other figures from now on, from where the view is now.
    pub fn set_hold(&mut self, hold: Hold) {
        self.hold = hold;
        self.band = self.band.clamp(hold.turning_band.min(hold.band), hold.band);
    }

    /// The band as it is now, in degrees.
    pub fn band(&self) -> f64 {
        self.band
    }

    /// The orientation to draw with, given the head's now and how long since the last.
    pub fn filter(&mut self, head: glam::DQuat, dt: f64) -> glam::DQuat {
        let (Some(last), Some(held), Some(view)) = (self.head, self.held, self.view) else {
            return self.restart(head);
        };
        if dt <= 0.0 {
            return view;
        }
        let step = apart(last, head);
        if dt > STALL_SECONDS || step > JUMP_DEGREES {
            return self.restart(head);
        }
        self.head = Some(head);
        let hold = self.hold;
        // How fast the head is turning, evened out over a few frames so that one noisy sample,
        // or the jolt of a press, does not count as a turn.
        self.speed += OneEuroFilter::alpha(4.0, dt) * (step / dt - self.speed);
        let turning = ramp(self.speed, hold.turn_speed);

        // The band: moved by as much as the head has gone past it, and no more.
        let push = (apart(held, head) - self.band).max(0.0);
        let held_now = if push > 0.0 { toward(held, head, push) } else { held };
        // Closed by a turn pushing it, opened again round a head that has slowed.
        let low = hold.turning_band.min(hold.band);
        self.band = (self.band - turning * hold.close * push + (1.0 - turning) * hold.reopen * dt).clamp(low, hold.band);

        // The view drawn settles on the held one, the faster the faster that moves.
        self.held_speed += OneEuroFilter::alpha(10.0, dt) * (apart(held, held_now) / dt - self.held_speed);
        let cutoff = hold.settle.0 + hold.settle.1 * self.held_speed;
        let gap = apart(view, held_now);
        let view = if gap < 1e-6 { held_now } else { toward(view, held_now, gap * OneEuroFilter::alpha(cutoff, dt)) };
        self.held = Some(held_now);
        self.view = Some(view);
        view
    }

    fn restart(&mut self, head: glam::DQuat) -> glam::DQuat {
        self.head = Some(head);
        self.held = Some(head);
        self.view = Some(head);
        self.speed = 0.0;
        self.held_speed = 0.0;
        self.band = self.hold.band;
        head
    }

    /// Forget the head: the next frame starts from wherever it is then.
    pub fn reset(&mut self) {
        self.head = None;
        self.held = None;
        self.view = None;
    }
}

/// 0 below the range, 1 above it, smoothly between.
fn ramp(x: f64, (lo, hi): (f64, f64)) -> f64 {
    let t = ((x - lo) / (hi - lo).max(1e-9)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The angle between two orientations, in degrees. By the arc tangent of the rotation between
/// them, which keeps the thousandths of a degree an arc cosine near one loses.
fn apart(a: glam::DQuat, b: glam::DQuat) -> f64 {
    let d = a.conjugate() * b;
    2.0 * d.xyz().length().atan2(d.w.abs()).to_degrees()
}

/// `from` turned `degrees` towards `to` the shortest way; `to` itself if that is nearer.
fn toward(from: glam::DQuat, to: glam::DQuat, degrees: f64) -> glam::DQuat {
    let mut d = from.conjugate() * to;
    if d.w < 0.0 {
        d = -d;
    }
    let sin = d.xyz().length();
    let angle = 2.0 * sin.atan2(d.w);
    if sin < 1e-15 || angle <= degrees.to_radians() {
        return to;
    }
    (from * glam::DQuat::from_axis_angle(d.xyz() / sin, degrees.to_radians())).normalize()
}

/// [`Steady`] as a frame loop uses it: once a frame, on the orientation about to be drawn with
/// and written to clients' pose channel, timed by the clock between frames, at the wearer's
/// [`Steadiness`]. The same on every platform -- the Deck at 72 frames a second, the Mac and the
/// Beam Pro at 60 -- since what it holds back is in degrees and seconds, not in frames; `dt`
/// carries the difference. The glasses' sensors are read at about a thousand samples a second on
/// all three (hidraw, IOKit, Android's usbfs), well past the frame rate, so nothing in it depends
/// on which path they came by.
///
/// `SPATIAND_STEADY` overrides the setting, to compare: `0` or `off`, `light`, `medium`, `strong`.
#[derive(Debug, Clone)]
pub struct SteadyView {
    steady: Steady,
    level: Steadiness,
    forced: Option<Steadiness>,
    last: Option<std::time::Instant>,
}

impl SteadyView {
    /// At the wearer's level, unless `SPATIAND_STEADY` says otherwise.
    pub fn new(level: Steadiness) -> Self {
        let forced = std::env::var("SPATIAND_STEADY").ok().and_then(|v| match v.trim() {
            "0" => Some(Steadiness::Off),
            other => Steadiness::from_name(other),
        });
        if let Some(forced) = forced {
            log::info!("steadiness {} from SPATIAND_STEADY", forced.name());
        }
        Self::with(level, forced)
    }

    fn with(level: Steadiness, forced: Option<Steadiness>) -> Self {
        let hold = forced.unwrap_or(level).hold().or(Steadiness::Medium.hold()).expect("medium holds");
        Self { steady: Steady::new(hold), level, forced, last: None }
    }

    /// The level in effect.
    pub fn level(&self) -> Steadiness {
        self.forced.unwrap_or(self.level)
    }

    /// The wearer chose another level: held by it from the next frame, from where the view is.
    pub fn set_level(&mut self, level: Steadiness) {
        self.level = level;
        if let Some(hold) = self.level().hold() {
            self.steady.set_hold(hold);
        }
    }

    /// The orientation to draw this frame with.
    pub fn filter(&mut self, head: glam::DQuat) -> glam::DQuat {
        let now = std::time::Instant::now();
        let dt = self.last.map_or(0.0, |last| now.duration_since(last).as_secs_f64());
        self.last = Some(now);
        self.filter_after(head, dt)
    }

    /// The same, `dt` seconds after the last frame.
    pub fn filter_after(&mut self, head: glam::DQuat, dt: f64) -> glam::DQuat {
        if self.level().hold().is_none() {
            // Off: the head, and nothing remembered, so that turned on again it starts there.
            self.steady.reset();
            return head;
        }
        self.steady.filter(head, dt)
    }

    /// Start again from the head: it has been recentred, and the view goes there at once.
    pub fn reset(&mut self) {
        self.steady.reset();
    }
}

/// Made-up heads to try [`Steady`] with, the same every run: a body's sway, a hand's tremor, the
/// sensors' noise, and turns. For the tests here and `examples/steadiness.rs`.
pub mod motion {
    use glam::DQuat;

    /// A head `right` and `up` degrees from straight ahead, leaning `roll` degrees (+X ahead,
    /// +Y left, +Z up, as the tracker's frame is).
    pub fn head(right: f64, up: f64, roll: f64) -> DQuat {
        DQuat::from_rotation_z(-right.to_radians())
            * DQuat::from_rotation_y(-up.to_radians())
            * DQuat::from_rotation_x(roll.to_radians())
    }

    /// A repeatable noise, -1 to 1.
    #[derive(Debug, Clone)]
    pub struct Noise(pub u64);

    impl Noise {
        pub fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 as f64 / u64::MAX as f64 - 0.5) * 2.0
        }
    }

    /// A body swaying `amplitude` degrees each way at `hz` -- sideways, and half that up and down
    /// at a different pace, and a little leaning -- with a tremor of `tremor` degrees at ten a
    /// second and `noise` degrees of the sensors' own on every sample. Degrees right, up and of
    /// roll at `t` seconds.
    #[derive(Debug, Clone)]
    pub struct Sway {
        pub amplitude: f64,
        pub hz: f64,
        pub tremor: f64,
        pub noise: f64,
        pub seed: Noise,
    }

    impl Sway {
        pub fn new(amplitude: f64, hz: f64) -> Self {
            Self { amplitude, hz, tremor: 0.03, noise: 0.005, seed: Noise(0x2545_f491_4f6c_dd1d) }
        }

        pub fn at(&mut self, t: f64) -> (f64, f64, f64) {
            use std::f64::consts::TAU;
            let a = self.amplitude;
            let tremor = |phase: f64| self.tremor * (TAU * 10.0 * t + phase).sin();
            let right = a * (TAU * self.hz * t).sin() + tremor(0.0) + self.noise * self.seed.next();
            let up = 0.5 * a * (TAU * self.hz * 0.73 * t + 1.0).sin() + tremor(2.0) + self.noise * self.seed.next();
            let roll = 0.3 * a * (TAU * self.hz * 0.51 * t + 2.0).sin() + self.noise * self.seed.next();
            (right, up, roll)
        }
    }

    /// A deliberate turn of `degrees` peaking at `peak` degrees a second, as a head makes one:
    /// smooth from rest to rest (minimum jerk). How far it has gone at `t` seconds from its start,
    /// and how long it takes.
    pub fn turn(degrees: f64, peak: f64, t: f64) -> f64 {
        let duration = turn_time(degrees, peak);
        let s = (t / duration).clamp(0.0, 1.0);
        degrees * s * s * s * (10.0 - 15.0 * s + 6.0 * s * s)
    }

    /// How long [`turn`] takes.
    pub fn turn_time(degrees: f64, peak: f64) -> f64 {
        1.875 * degrees.abs() / peak
    }
}

#[cfg(test)]
mod tests {
    use super::motion::{head, turn, turn_time, Noise, Sway};
    use super::*;

    #[test]
    fn first_sample_passes_through_untouched() {
        let mut f = OneEuroFilter::new(1.0, 0.007);
        assert_eq!(f.filter(42.0, 0.014), 42.0);
    }

    #[test]
    fn jitter_at_rest_is_attenuated() {
        let mut f = OneEuroFilter::new(1.0, 0.007);
        let dt = 1.0 / 72.0;
        f.filter(0.0, dt);
        // Alternating noise around zero, the shape involuntary head tremor takes.
        let mut last = 0.0;
        for i in 0..200 {
            last = f.filter(if i % 2 == 0 { 0.5 } else { -0.5 }, dt);
        }
        assert!(
            last.abs() < 0.25,
            "noise should be more than halved, got {last}"
        );
    }

    #[test]
    fn a_real_turn_is_tracked_without_much_lag() {
        // The point of One Euro: fast motion must NOT be smoothed into mush.
        let mut f = OneEuroFilter::new(1.0, 0.007);
        let dt = 1.0 / 72.0;
        let mut out = 0.0;
        for i in 0..72 {
            out = f.filter(i as f64 * 2.0, dt); // 144 deg/s sweep
        }
        let truth = 71.0 * 2.0;
        assert!(
            (out - truth).abs() < 12.0,
            "expected to stay near {truth}, got {out}"
        );
    }

    #[test]
    fn yaw_seam_does_not_produce_a_lurch() {
        let mut s = PoseSmoother::new(1.0, 0.007);
        let dt = 1.0 / 72.0;
        s.filter(179.0, 0.0, 0.0, dt);
        // Crossing +180 to -179 is a 2 degree move, not a 358 degree one. If unwrapping is
        // broken the filter sees an enormous rate and opens its cutoff wide.
        let (yaw, _, _) = s.filter(-179.0, 0.0, 0.0, dt);
        assert!(
            (yaw - 181.0).abs() < 2.0,
            "unwrapped yaw should continue past 180, got {yaw}"
        );
        assert!(
            s.speed() < 500.0,
            "seam must not blow up the speed estimate"
        );
    }

    #[test]
    fn zero_dt_is_survivable() {
        let mut f = OneEuroFilter::new(1.0, 0.007);
        f.filter(1.0, 0.014);
        assert_eq!(f.filter(2.0, 0.0), 1.0, "must not divide by zero");
    }

    /// The frame rates the room runs at: the Mac's and the Beam Pro's 60, the Deck's 72, 90 for
    /// glasses that run faster, and the Deck's loop turning every few milliseconds while a frame
    /// is on its way, which takes the head each turn. What the hold does must not depend on which.
    const RATES: [f64; 4] = [60.0, 72.0, 90.0, 260.0];

    /// The levels that hold anything.
    const HOLDING: [Steadiness; 3] = [Steadiness::Light, Steadiness::Medium, Steadiness::Strong];

    fn steady(level: Steadiness) -> Steady {
        Steady::new(level.hold().unwrap())
    }

    /// Where a view is, as degrees right and up, for comparing with a made-up head.
    fn facing(q: glam::DQuat) -> (f64, f64) {
        let f = q * glam::DVec3::X;
        (-f.y.atan2(f.x).to_degrees(), f.z.asin().to_degrees())
    }

    /// A body swaying for `seconds` at `rate` frames a second: how far the view moved, the most in
    /// any one frame and all told, over the part after `settled` seconds, in degrees.
    fn swayed(level: Steadiness, mut sway: Sway, rate: f64, seconds: f64, settled: f64) -> (f64, f64) {
        let mut f = level.hold().map(Steady::new);
        let dt = 1.0 / rate;
        let mut last: Option<glam::DQuat> = None;
        let (mut most, mut all) = (0.0f64, 0.0);
        for i in 0..(seconds * rate) as usize {
            let t = i as f64 * dt;
            let (right, up, roll) = sway.at(t);
            let h = head(right, up, roll);
            let view = f.as_mut().map_or(h, |f| f.filter(h, dt));
            if t > settled {
                if let Some(last) = last {
                    let moved = apart(last, view);
                    most = most.max(moved);
                    all += moved;
                }
                last = Some(view);
            }
        }
        (most, all)
    }

    #[test]
    fn a_sway_inside_the_band_moves_nothing_at_all() {
        // The owner's complaint: the room shook with his body. A sway inside each level's band --
        // sideways by this much each way, half as much up and down and a third as much leaning,
        // each at its own pace, with a tremor and the sensors' noise on top -- at every frame
        // rate: once it has pushed the band where it sways (given time to have gone everywhere
        // it goes, which at a fifth of a beat a second is a while), nothing moves, not by a
        // thousandth of a degree in twenty seconds.
        for (level, amplitude) in [(Steadiness::Light, 0.25), (Steadiness::Medium, 0.75), (Steadiness::Strong, 1.5)] {
            for hz in [0.2, 0.5, 1.0, 2.0, 3.0] {
                for rate in RATES {
                    let (most, all) = swayed(level, Sway::new(amplitude, hz), rate, 40.0, 20.0);
                    assert!(
                        all < 1e-3,
                        "{level:?}: a sway of {amplitude} degrees at {hz} Hz moved the view {all:.4} \
                         degrees in 20 s at {rate} Hz (at most {most:.5} in a frame)"
                    );
                }
            }
        }
    }

    #[test]
    fn a_pulse_is_held_out_by_the_lightest() {
        // What the room held out before there were levels: six hundredths of a degree each way,
        // 72 beats a minute, with the sensors' noise.
        for rate in RATES {
            let mut pulse = Sway::new(0.06, 1.2);
            pulse.tremor = 0.0;
            let (_, all) = swayed(Steadiness::Light, pulse, rate, 10.0, 2.0);
            assert!(all < 1e-4, "at {rate} Hz a pulse still moves the view {all:.5} degrees");
        }
    }

    #[test]
    fn a_wider_sway_is_cut_down_by_the_band() {
        // A body swaying wider than the band: the view moves only by about what is past it.
        // Measured in how far it swings from side to side against the head's own, and in how far
        // it moves all told.
        for level in HOLDING {
            let band = level.hold().unwrap().band;
            let amplitude = band * 1.6;
            let mut sway = Sway::new(amplitude, 0.5);
            let mut f = steady(level);
            let dt = 1.0 / 72.0;
            let (mut lo, mut hi) = (f64::MAX, f64::MIN);
            let (mut head_lo, mut head_hi) = (f64::MAX, f64::MIN);
            for i in 0..(20.0 / dt) as usize {
                let t = i as f64 * dt;
                let (right, up, roll) = sway.at(t);
                let h = head(right, up, roll);
                let (x, _) = facing(f.filter(h, dt));
                if t > 5.0 {
                    lo = lo.min(x);
                    hi = hi.max(x);
                    head_lo = head_lo.min(facing(h).0);
                    head_hi = head_hi.max(facing(h).0);
                }
            }
            let (swing, head_swing) = (hi - lo, head_hi - head_lo);
            assert!(swing < head_swing * 0.55, "{level:?}: the view swings {swing:.3} of the head's {head_swing:.3}");
            let (_, all) = swayed(level, Sway::new(amplitude, 0.5), 72.0, 40.0, 20.0);
            let (_, head_all) = swayed(Steadiness::Off, Sway::new(amplitude, 0.5), 72.0, 40.0, 20.0);
            assert!(all < head_all * 0.5, "{level:?}: the view moved {all:.2} of the head's {head_all:.2}");
        }
    }

    /// A turn of `degrees` peaking at `peak` degrees a second, from rest to rest, at `rate` frames
    /// a second: how many milliseconds after the head the view had come nine tenths of the way
    /// (between frames), and how far it went past the end, in degrees; then the same turn with a
    /// body's sway of a fifth of a degree on top: how far from the head the view was half a
    /// second after the head stopped, and how far it moved in all from then until two seconds
    /// after.
    fn turned(level: Steadiness, degrees: f64, peak: f64, rate: f64) -> (f64, f64, f64, f64) {
        let dt = 1.0 / rate;
        let duration = turn_time(degrees, peak);
        let start = 1.0;
        let frames = ((start + duration + 2.0) / dt) as usize;
        // When a series first comes nine tenths of the way, between frames.
        let reaches = |series: &[f64]| {
            let at = 0.9 * degrees;
            let i = series.iter().position(|x| *x >= at).unwrap();
            let (a, b) = (series[i - 1], series[i]);
            (i as f64 - 1.0 + (at - a) / (b - a)) * dt
        };
        let mut f = steady(level);
        let (mut heads, mut views) = (Vec::new(), Vec::new());
        let mut beyond = 0.0f64;
        for i in 0..frames {
            let along = turn(degrees, peak, i as f64 * dt - start);
            let (x, _) = facing(f.filter(head(along, 0.0, 0.0), dt));
            heads.push(along);
            views.push(x);
            beyond = beyond.max(x - degrees);
        }
        let lag = (reaches(&views) - reaches(&heads)) * 1000.0;

        let mut f = steady(level);
        let mut sway = Sway::new(0.2, 0.7);
        let (mut off_after, mut moved_after) = (None, 0.0);
        let mut last: Option<glam::DQuat> = None;
        for i in 0..frames {
            let t = i as f64 * dt;
            let (right, up, roll) = sway.at(t);
            let h = head(turn(degrees, peak, t - start) + right, up, roll);
            let view = f.filter(h, dt);
            let after = t - start - duration;
            if after >= 0.5 && off_after.is_none() {
                off_after = Some(apart(view, h));
            }
            if after > 0.5 {
                if let Some(last) = last {
                    moved_after += apart(last, view);
                }
            }
            last = Some(view);
        }
        (lag, beyond, off_after.unwrap(), moved_after)
    }

    #[test]
    fn a_turn_is_followed_closely_and_arrived_at() {
        // Twenty to ninety degrees at fifty to two hundred a second: nine tenths of the way at
        // most a frame or two after the head, nothing past the end, and still once the head is --
        // to the bit where the band has room for the sway after; the lightest's half degree has
        // little, and a sway of a fifth of a degree pushes it on by a pixel or two in 1.5 s.
        for level in HOLDING {
            for (degrees, peak) in [(20.0, 50.0), (30.0, 100.0), (45.0, 100.0), (90.0, 200.0)] {
                for rate in RATES {
                    let (lag, beyond, off, moved) = turned(level, degrees, peak, rate);
                    assert!(lag < 40.0, "{level:?} {degrees} at {peak}/s, {rate} Hz: {lag:.0} ms behind");
                    assert!(beyond < 0.05, "{level:?} {degrees} at {peak}/s, {rate} Hz: {beyond:.3} past the end");
                    assert!(off <= level.hold().unwrap().band + 0.05, "{level:?}: {off:.2} off the head after a turn");
                    let creep = if level == Steadiness::Light { 0.05 } else { 1e-3 };
                    assert!(moved < creep, "{level:?} {degrees} at {peak}/s, {rate} Hz: crept {moved:.4} after");
                }
            }
        }
    }

    #[test]
    fn a_turn_ends_with_the_band_round_the_head() {
        // So what is held out after a turn is as much on the side the head turned to as on the
        // other: the view is within the turning band of the head as it stops.
        for level in HOLDING {
            let hold = level.hold().unwrap();
            let mut f = steady(level);
            let dt = 1.0 / 72.0;
            let duration = turn_time(40.0, 120.0);
            let mut end = glam::DQuat::IDENTITY;
            for i in 0..((duration + 1.0) / dt) as usize {
                let h = head(turn(40.0, 120.0, i as f64 * dt), 0.0, 0.0);
                end = f.filter(h, dt);
            }
            let off = apart(end, head(40.0, 0.0, 0.0));
            assert!(off < hold.turning_band + 0.15, "{level:?}: {off:.3} off the head after a turn");
            assert!((f.band() - hold.band).abs() < 1e-9, "{level:?}: the band did not reopen");
        }
    }

    #[test]
    fn nothing_creeps_once_the_head_stops_inside_the_band() {
        // A head that moves to most of the band away and stays: the view settles in a few
        // hundredths of a second and is then still to the last bit, for ever. No swim.
        for level in HOLDING {
            let band = level.hold().unwrap().band;
            for rate in RATES {
                let mut f = steady(level);
                let dt = 1.0 / rate;
                f.filter(glam::DQuat::IDENTITY, dt);
                let there = head(band * 0.9, 0.0, 0.0);
                let mut views = Vec::new();
                for _ in 0..(3.0 * rate) as usize {
                    views.push(f.filter(there, dt));
                }
                assert!(views.iter().all(|v| *v == glam::DQuat::IDENTITY), "{level:?} at {rate} Hz moved");
            }
        }
    }

    #[test]
    fn a_slow_look_across_is_followed_without_stopping() {
        // Reading along a line at three degrees a second: the view never more than the band
        // behind, and once it moves it keeps moving -- no stop and go.
        for level in HOLDING {
            let band = level.hold().unwrap().band;
            for rate in RATES {
                let mut f = steady(level);
                let dt = 1.0 / rate;
                let mut worst: f64 = 0.0;
                let mut last = glam::DQuat::IDENTITY;
                let mut moving = false;
                for i in 0..(10.0 * rate) as usize {
                    let h = head(i as f64 * dt * 3.0, 0.0, 0.0);
                    let view = f.filter(h, dt);
                    worst = worst.max(apart(view, h));
                    let step = apart(last, view);
                    if moving {
                        assert!(step > 3.0 * dt * 0.5, "{level:?} at {rate} Hz stopped on frame {i}");
                    }
                    moving |= step > 3.0 * dt * 0.9;
                    last = view;
                }
                assert!(worst < band + 0.15, "{level:?} at {rate} Hz {worst:.3} degrees behind a slow look");
            }
        }
    }

    #[test]
    fn a_recentre_or_a_stall_starts_again_from_the_head() {
        let mut f = steady(Steadiness::Strong);
        let dt = 1.0 / 72.0;
        f.filter(glam::DQuat::IDENTITY, dt);
        let recentred = head(35.0, -10.0, 0.0);
        assert_eq!(f.filter(recentred, dt), recentred);
        let later = head(36.0, -10.0, 0.0);
        assert_eq!(f.filter(later, 0.5), later);
    }

    #[test]
    fn switched_off_the_view_is_the_head() {
        let mut view = SteadyView::with(Steadiness::Off, None);
        let head = glam::DQuat::from_rotation_z(0.001);
        view.filter_after(glam::DQuat::IDENTITY, 0.0);
        assert_eq!(view.filter_after(head, 1.0 / 72.0), head);
        // And turned on again, it starts from the head.
        view.set_level(Steadiness::Strong);
        let next = glam::DQuat::from_rotation_z(0.002);
        assert_eq!(view.filter_after(next, 1.0 / 72.0), next);
        assert_eq!(view.filter_after(glam::DQuat::from_rotation_z(0.003), 1.0 / 72.0), next);
    }

    #[test]
    fn the_levels_step_round_and_are_written_by_name() {
        let mut level = Steadiness::default();
        assert_eq!(level, Steadiness::Medium);
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(level.name());
            level = level.next();
        }
        assert_eq!(seen, ["medium", "strong", "off", "light"]);
        for level in Steadiness::ALL {
            assert_eq!(Steadiness::from_name(level.name()), Some(level));
        }
        // The stronger, the wider the band and the further a press looks back.
        let bands: Vec<f64> = HOLDING.iter().map(|l| l.hold().unwrap().band).collect();
        assert!(bands.windows(2).all(|w| w[0] < w[1]));
        let looks: Vec<u32> = Steadiness::ALL.iter().map(|l| l.press_settle().0).collect();
        assert!(looks.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn the_noise_is_the_same_every_run() {
        let (mut a, mut b) = (Noise(7), Noise(7));
        assert_eq!(a.next(), b.next());
    }
}
