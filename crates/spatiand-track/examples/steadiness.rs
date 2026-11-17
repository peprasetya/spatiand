//! What each level of steadiness does to a head: made up, or recorded from a real one.
//!
//! ```text
//! steadiness                    made-up heads: a body's sway, turns, a press's jolt
//! steadiness <imu.bin> [rate]   a recording, replayed through the tracker at `rate` frames a
//!                               second (72, the Deck's; 60 for the Mac and the Beam Pro)
//! ```
//!
//! The recording is what `SPATIAND_IMU_RECORD` writes (HoloFrame's format): ten little-endian f64
//! per sample, raw. It is run under the XREAL Air's axis map, and the room's view is made from it
//! as a frame loop makes it: the tracker's prediction a frame ahead, then each level's
//! [`Steady`]. What matters is said in degrees; at a window's default distance a degree is about
//! 46 of its pixels, so a hundredth of a degree is half a pixel.

use glam::{DQuat, DVec3};
use spatiand_hmd::ImuSample;
use spatiand_track::smoothing::motion::{head, turn, turn_time, Sway};
use spatiand_track::{AxisMap, HeadTracker, Steadiness, Steady, TrackerConfig};

const LEVELS: [Steadiness; 3] = [Steadiness::Light, Steadiness::Medium, Steadiness::Strong];

fn apart(a: DQuat, b: DQuat) -> f64 {
    let d = a.conjugate() * b;
    2.0 * d.xyz().length().atan2(d.w.abs()).to_degrees()
}

fn facing(q: DQuat) -> f64 {
    let f = q * DVec3::X;
    -f.y.atan2(f.x).to_degrees()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first() {
        Some(path) => recorded(path, args.get(1).and_then(|r| r.parse().ok()).unwrap_or(72.0)),
        None => made_up(),
    }
}

fn made_up() {
    let rate = 72.0;
    let dt = 1.0 / rate;
    println!("A body swaying, with a tremor of 0.03 degrees at 10 Hz and the sensors' noise, {rate} frames a second.");
    println!("How far the view moves, degrees a second, once settled (20 s after 20 s); the head's own beside it.\n");
    let hzs = [0.2, 0.5, 1.0, 2.0, 3.0];
    print!("{:>8} {:>6}", "level", "each");
    for hz in hzs {
        print!(" {:>13}", format!("{hz} Hz"));
    }
    println!();
    for amplitude in [0.25, 0.5, 1.0, 1.5, 2.0] {
        for level in [Steadiness::Off].into_iter().chain(LEVELS) {
            print!("{:>8} {:>5.2}°", level.name(), amplitude);
            for hz in hzs {
                let mut sway = Sway::new(amplitude, hz);
                let mut f = level.hold().map(Steady::new);
                let mut last: Option<DQuat> = None;
                let (mut travel, mut most) = (0.0, 0.0f64);
                for i in 0..(40.0 * rate) as usize {
                    let t = i as f64 * dt;
                    let (r, u, roll) = sway.at(t);
                    let h = head(r, u, roll);
                    let view = f.as_mut().map_or(h, |f| f.filter(h, dt));
                    if t >= 20.0 {
                        if let Some(last) = last {
                            let step = apart(last, view);
                            travel += step;
                            most = most.max(step);
                        }
                        last = Some(view);
                    }
                }
                print!(" {:>6.3} {:>6.4}", travel / 20.0, most);
            }
            println!();
        }
        println!();
    }
    println!("(each cell: degrees a second all told, and the most in one frame)\n");

    println!("Deliberate turns from rest to rest (minimum jerk), {rate} frames a second.");
    println!("starts: ms after the head that the view has moved a tenth of a degree; lag: ms after the head");
    println!("that it is 90% of the way; behind: the most degrees it is behind; fastest: its fastest against");
    println!("the head's; past: degrees beyond the end. Then again with a sway of 0.2 degrees on top: off,");
    println!("degrees from the head 0.5 s after it stops, and crept, degrees moved from then to 2 s after.\n");
    println!(
        "{:>8} {:>12} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}",
        "level", "turn", "starts", "lag", "behind", "fastest", "past", "off", "crept"
    );
    // When a series of (time, value) first reaches `at`, between frames.
    let reaches = |series: &[(f64, f64)], at: f64| {
        series.windows(2).find(|w| w[1].1 >= at).map(|w| {
            let ((t0, v0), (t1, v1)) = (w[0], w[1]);
            if v1 > v0 { t0 + (t1 - t0) * ((at - v0) / (v1 - v0)).clamp(0.0, 1.0) } else { t1 }
        })
    };
    for (degrees, peak) in [(5.0, 30.0), (20.0, 50.0), (30.0, 100.0), (45.0, 150.0), (90.0, 200.0)] {
        for level in [Steadiness::Off].into_iter().chain(LEVELS) {
            let start = 1.0;
            let duration = turn_time(degrees, peak);
            let frames = ((start + duration + 2.0) * rate) as usize;
            // Clean, for the timing.
            let mut f = level.hold().map(Steady::new);
            let (mut heads, mut views) = (Vec::new(), Vec::new());
            let (mut behind, mut fastest, mut past) = (0.0f64, 0.0f64, 0.0f64);
            let mut last: Option<DQuat> = None;
            for i in 0..frames {
                let t = i as f64 * dt;
                let along = turn(degrees, peak, t - start);
                let h = head(along, 0.0, 0.0);
                let view = f.as_mut().map_or(h, |f| f.filter(h, dt));
                heads.push((t, along));
                views.push((t, facing(view)));
                behind = behind.max(along - facing(view));
                past = past.max(facing(view) - degrees);
                if let Some(last) = last {
                    fastest = fastest.max(apart(last, view) / dt);
                }
                last = Some(view);
            }
            let at = |series: &[(f64, f64)], value: f64| reaches(series, value).unwrap_or(f64::NAN);
            let starts = (at(&views, 0.1) - at(&heads, 0.1)) * 1000.0;
            let lag = (at(&views, 0.9 * degrees) - at(&heads, 0.9 * degrees)) * 1000.0;
            // With a sway, for what is left after.
            let mut f = level.hold().map(Steady::new);
            let mut sway = Sway::new(0.2, 0.7);
            let (mut off, mut crept) = (None, 0.0);
            let mut last: Option<DQuat> = None;
            for i in 0..frames {
                let t = i as f64 * dt;
                let (r, u, roll) = sway.at(t);
                let h = head(turn(degrees, peak, t - start) + r, u, roll);
                let view = f.as_mut().map_or(h, |f| f.filter(h, dt));
                let after = t - start - duration;
                if after >= 0.5 && off.is_none() {
                    off = Some(apart(view, h));
                }
                if after > 0.5 {
                    if let Some(last) = last {
                        crept += apart(last, view);
                    }
                }
                last = Some(view);
            }
            println!(
                "{:>8} {:>12} {:>7.1} {:>7.1} {:>7.2} {:>7.2} {:>7.3} {:>7.3} {:>7.4}",
                level.name(),
                format!("{degrees}° {peak}/s"),
                starts,
                lag,
                behind,
                fastest / peak,
                past,
                off.unwrap_or(f64::NAN),
                crept
            );
        }
        println!();
    }

    println!("A press that jolts the head: a sway of 0.3 degrees, then a bump of 0.5 to 1.5 degrees over");
    println!("80 ms (up, as a thumb pressing down tips it) and back over 150 ms. How far the view, and with");
    println!("it the pointer, has moved from where it was when the bump began: at the press (the bump's");
    println!("height), and at most while it lasts. The look back a press makes on top of this is the pointer's.\n");
    println!("{:>8} {:>6} {:>10} {:>10}", "level", "jolt", "at press", "at most");
    for jolt in [0.5, 1.0, 1.5] {
        for level in [Steadiness::Off].into_iter().chain(LEVELS) {
            let mut f = level.hold().map(Steady::new);
            let mut sway = Sway::new(0.3, 0.8);
            let begin = 5.0;
            let (mut before, mut at_press, mut most) = (None, 0.0, 0.0f64);
            for i in 0..(6.0 * rate) as usize {
                let t = i as f64 * dt;
                let s = t - begin;
                let bump = if s < 0.0 {
                    0.0
                } else if s < 0.08 {
                    jolt * (std::f64::consts::FRAC_PI_2 * s / 0.08).sin()
                } else if s < 0.23 {
                    jolt * (std::f64::consts::FRAC_PI_2 * (0.23 - s) / 0.15).sin()
                } else {
                    0.0
                };
                let (r, u, roll) = sway.at(t);
                let h = head(r, u - bump, roll);
                let view = f.as_mut().map_or(h, |f| f.filter(h, dt));
                if s < 0.0 {
                    before = Some(view);
                } else if s < 0.3 {
                    let moved = apart(before.unwrap(), view);
                    most = most.max(moved);
                    if s < 0.08 + dt / 2.0 && s >= 0.08 - dt / 2.0 {
                        at_press = moved;
                    }
                }
            }
            println!("{:>8} {:>5.1}° {:>10.3} {:>10.3}", level.name(), jolt, at_press, most);
        }
        println!();
    }
}

fn recorded(path: &str, rate: f64) {
    let bytes = std::fs::read(path).expect("readable recording");
    let samples: Vec<ImuSample> = bytes
        .chunks_exact(80)
        .map(|c| {
            let f = |i: usize| f64::from_le_bytes(c[i * 8..i * 8 + 8].try_into().unwrap());
            ImuSample {
                timestamp_ns: f(0) as u64,
                gyro: DVec3::new(f(1), f(2), f(3)),
                accel: DVec3::new(f(4), f(5), f(6)),
                mag: DVec3::new(f(7), f(8), f(9)),
                temperature_c: None,
            }
        })
        .collect();
    let first = samples.first().map_or(0, |s| s.timestamp_ns);
    let last = samples.last().map_or(0, |s| s.timestamp_ns);
    println!("{} samples over {:.1} s, framed at {rate} a second", samples.len(), (last - first) as f64 * 1e-9);

    // The head each frame, as a frame loop would have drawn it before any steadying.
    let mut tracker = HeadTracker::new(AxisMap::XREAL_AIR, TrackerConfig::default());
    let frame_ns = (1e9 / rate) as u64;
    let mut next = first + frame_ns;
    let mut heads = Vec::new();
    for s in &samples {
        tracker.integrate(s);
        while s.timestamp_ns >= next {
            next += frame_ns;
            if tracker.has_samples() {
                heads.push(tracker.predicted_orientation(
                    spatiand_track::DEFAULT_PREDICTION_SECONDS,
                    spatiand_track::DEFAULT_PREDICTION_MAX_DEGREES,
                ));
            }
        }
    }
    let dt = 1.0 / rate;
    // Each frame by how fast the head went, evened over a tenth of a second either side.
    let speeds: Vec<f64> = heads.windows(2).map(|w| apart(w[0], w[1]) / dt).collect();
    let span = (0.1 * rate) as usize;
    let even: Vec<f64> = (0..speeds.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(span), (i + span + 1).min(speeds.len()));
            speeds[a..b].iter().sum::<f64>() / (b - a) as f64
        })
        .collect();
    let classes: [(&str, f64, f64); 4] =
        [("still (<2°/s)", 0.0, 2.0), ("sway (2-15°/s)", 2.0, 15.0), ("moving (15-60)", 15.0, 60.0), ("turning (>60)", 60.0, f64::MAX)];
    for (name, lo, hi) in classes {
        let n = even.iter().filter(|s| **s >= lo && **s < hi).count();
        println!("  {name:<16} {:5.1}% of frames", 100.0 * n as f64 / even.len().max(1) as f64);
    }
    println!();
    println!("Per class: the mean movement a frame (degrees), and the share of frames that moved more than");
    println!("a hundredth of a degree (half a pixel); for the moving classes, how far the view is from the head.\n");
    print!("{:>8}", "level");
    for (name, ..) in classes {
        print!(" {:>27}", name);
    }
    println!();
    let mut all_views: Vec<Vec<DQuat>> = Vec::new();
    for level in [Steadiness::Off].into_iter().chain(LEVELS) {
        let mut f = level.hold().map(Steady::new);
        let views: Vec<DQuat> = heads.iter().map(|h| f.as_mut().map_or(*h, |f| f.filter(*h, dt))).collect();
        print!("{:>8}", level.name());
        all_views.push(views.clone());
        for (_, lo, hi) in classes {
            let (mut n, mut moved, mut seen, mut off) = (0usize, 0.0, 0usize, 0.0);
            for i in 0..even.len() {
                if even[i] >= lo && even[i] < hi {
                    let step = apart(views[i], views[i + 1]);
                    n += 1;
                    moved += step;
                    seen += (step > 0.01) as usize;
                    off += apart(views[i + 1], heads[i + 1]);
                }
            }
            let n = n.max(1) as f64;
            print!(" {:>8.4} {:>5.1}% off {:>5.2}°", moved / n, 100.0 * seen as f64 / n, off / n);
        }
        println!();
    }

    // By how far the head strays: each second, the furthest it went from where it was on average
    // in that second. A sway is a small stray at any speed; a look is a larger one.
    println!();
    println!("Each second by how far the head strayed from its middle in it: the share of seconds, and the");
    println!("view's travel in them, degrees a second (raw head first).\n");
    let window = rate as usize;
    let bounds = [0.0, 0.25, 0.5, 1.0, 1.5, 2.5, 5.0, f64::MAX];
    let mut strays = Vec::new();
    for start in (0..heads.len().saturating_sub(window)).step_by(window) {
        let part = &heads[start..start + window];
        let middle = {
            let sum = part.iter().fold(glam::DVec4::ZERO, |acc, q| {
                let q = if q.dot(part[0]) < 0.0 { -*q } else { *q };
                acc + glam::DVec4::from(q)
            });
            DQuat::from_vec4(sum).normalize()
        };
        let stray = part.iter().map(|q| apart(middle, *q)).fold(0.0, f64::max);
        strays.push((start, stray));
    }
    print!("{:>12} {:>6}", "stray", "share");
    for level in [Steadiness::Off].into_iter().chain(LEVELS) {
        print!(" {:>8}", level.name());
    }
    println!();
    for pair in bounds.windows(2) {
        let (lo, hi) = (pair[0], pair[1]);
        let picked: Vec<usize> = strays.iter().filter(|(_, s)| *s >= lo && *s < hi).map(|(i, _)| *i).collect();
        let label = if hi == f64::MAX { format!(">{lo}°") } else { format!("{lo}-{hi}°") };
        print!("{:>12} {:>5.1}%", label, 100.0 * picked.len() as f64 / strays.len().max(1) as f64);
        for views in &all_views {
            let travel: f64 = picked
                .iter()
                .map(|&start| views[start..start + window].windows(2).map(|w| apart(w[0], w[1])).sum::<f64>())
                .sum();
            print!(" {:>8.3}", travel / picked.len().max(1) as f64);
        }
        println!();
    }
}
