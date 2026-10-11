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

/// The view held steady against a pulse, and let go of the moment the head really turns.
///
/// A heartbeat rocks the head a few hundredths of a degree, about once a second, and
/// breathing does the same more slowly. Followed one-to-one, that moves a window's picture
/// across the display's pixels by a fraction of one, and text that is being stared at
/// shimmers in time with it. Nothing is wrong with the tracking: it is reporting a head that
/// is moving. But a wearer looking at a page does not want to be shown their own pulse.
///
/// So the view follows a head that is nearly still only slowly -- for that little while a
/// window rides with the head rather than staying in the room, by an amount too small to see
/// as movement -- and follows in full once the head moves faster than a pulse moves it, or
/// has got further from the view than a pulse takes it. The second is what bounds the lag:
/// the view is never more than a fraction of a degree behind, however slowly the head creeps.
///
/// Done on the orientation itself rather than on three angles, so there is no seam to cross
/// and nothing special about looking straight up.
#[derive(Debug, Clone)]
pub struct Steady {
    /// How fast the view follows a still head, as a cutoff in Hz. A pulse is a little over
    /// 1 Hz, so a fifth of that leaves about a sixth of it.
    pub still_cutoff: f64,
    /// Turning slower than the first, in degrees a second, is stillness; faster than the
    /// second is a turn, followed in full. A pulse peaks under one.
    pub speed: (f64, f64),
    /// Nearer the view than the first, in degrees, is stillness; further than the second is
    /// followed in full.
    pub apart: (f64, f64),
    view: Option<glam::DQuat>,
    last: Option<glam::DQuat>,
    rate: f64,
}

impl Default for Steady {
    fn default() -> Self {
        Self { still_cutoff: 0.2, speed: (1.5, 6.0), apart: (0.15, 0.6), view: None, last: None, rate: 0.0 }
    }
}

impl Steady {
    /// The orientation to draw with, given the head's now and how long since the last.
    pub fn filter(&mut self, head: glam::DQuat, dt: f64) -> glam::DQuat {
        let (Some(view), Some(last)) = (self.view, self.last) else {
            self.view = Some(head);
            self.last = Some(head);
            return head;
        };
        // A stall -- the loop held up, the glasses asleep -- is not a movement to smooth over.
        if dt <= 0.0 || dt > 0.25 {
            self.view = Some(head);
            self.last = Some(head);
            self.rate = 0.0;
            return head;
        }
        self.last = Some(head);
        // How fast the head is turning, evened out over a few frames so that one noisy sample
        // does not open the filter.
        let now = last.angle_between(head).to_degrees() / dt;
        let a = OneEuroFilter::alpha(4.0, dt);
        self.rate += a * (now - self.rate);
        let apart = view.angle_between(head).to_degrees();
        let ramp = |x: f64, (lo, hi): (f64, f64)| {
            let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        let moving = ramp(self.rate.max(now), self.speed).max(ramp(apart, self.apart));
        let still = OneEuroFilter::alpha(self.still_cutoff, dt);
        let follow = still + (1.0 - still) * moving;
        let view = view.slerp(head, follow).normalize();
        self.view = Some(view);
        view
    }

    pub fn reset(&mut self) {
        self.view = None;
        self.last = None;
        self.rate = 0.0;
    }
}

#[cfg(test)]
mod tests {
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

    /// A head rocking as a pulse rocks it: how far the view moves, peak to peak, in degrees.
    fn swing(amplitude: f64, hz: f64) -> f64 {
        let mut f = Steady::default();
        let dt = 1.0 / 60.0;
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for i in 0..600 {
            let t = i as f64 * dt;
            let angle = amplitude * (2.0 * std::f64::consts::PI * hz * t).sin();
            let out = f.filter(glam::DQuat::from_rotation_z(angle.to_radians()), dt);
            if i > 240 {
                let yaw = out.to_euler(glam::EulerRot::ZYX).0.to_degrees();
                lo = lo.min(yaw);
                hi = hi.max(yaw);
            }
        }
        hi - lo
    }

    #[test]
    fn a_pulse_is_mostly_taken_out() {
        // Six hundredths of a degree each way, 72 beats a minute.
        let moved = swing(0.06, 1.2);
        assert!(moved < 0.12 * 0.25, "the view still swings {moved:.4} of 0.12 degrees");
    }

    #[test]
    fn a_turn_is_followed_closely_and_arrived_at() {
        let mut f = Steady::default();
        let dt = 1.0 / 60.0;
        f.filter(glam::DQuat::IDENTITY, dt);
        let mut worst: f64 = 0.0;
        // 60 degrees a second for half a second, then still.
        for i in 1..=90 {
            let angle = (i.min(30) as f64) * 1.0;
            let head = glam::DQuat::from_rotation_z(angle.to_radians());
            let out = f.filter(head, dt);
            worst = worst.max(out.angle_between(head).to_degrees());
            if i == 90 {
                assert!(out.angle_between(head).to_degrees() < 0.16, "left behind after the turn");
            }
        }
        assert!(worst < 0.7, "{worst:.3} degrees behind a turn");
    }

    #[test]
    fn a_slow_look_across_is_never_far_behind() {
        let mut f = Steady::default();
        let dt = 1.0 / 60.0;
        let mut worst: f64 = 0.0;
        // Three degrees a second: reading along a line.
        for i in 0..300 {
            let head = glam::DQuat::from_rotation_z((i as f64 * dt * 3.0).to_radians());
            let out = f.filter(head, dt);
            worst = worst.max(out.angle_between(head).to_degrees());
        }
        assert!(worst < 0.35, "{worst:.3} degrees behind a slow look");
    }
}
