//! The left pad of a recording (`tools/capture-pad-trace.py`), read once a frame as the room reads
//! it, through the room's own `SteadyPad` and `PadScroll`: how evenly a slow scroll comes out.
//!
//!     cargo run --release -p spatiand-input --example replay_scroll -- trace.csv [frames.csv]
//!
//! For each way of reading the pad it prints, over the scroll gestures in the recording (from a
//! gesture's first step to its last): how many frames sent nothing between frames that did
//! (the page standing still mid-scroll), the longest such run, and how unevenly the steps came
//! (the frame-to-frame change of a step, against the step itself; 0 for a steady scroll). With a
//! second path it writes every frame's step for each way, for a plot.

use std::time::{Duration, Instant};

use spatiand_input::{Pad, PadScroll, Scroll, ScrollSpread, SteadyPad};

const FRAME: f64 = 1.0 / 72.0;

struct Way {
    name: String,
    steady: Option<SteadyPad>,
    scroll: PadScroll,
    /// Each step spread over the frames after it: this much of what is still owed goes each frame,
    /// to try other shares than the room's.
    spread: Option<f32>,
    owed: f32,
    /// The room's own: `ScrollSpread`.
    room: Option<ScrollSpread>,
    steps: Vec<f32>,
}

fn way(name: &str, steady: bool, spread: Option<f32>) -> Way {
    Way { name: name.into(), steady: steady.then(SteadyPad::default), scroll: PadScroll::default(), spread, owed: 0.0, room: None, steps: vec![] }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("a recording");
    let text = std::fs::read_to_string(path).expect("readable");
    let mut rows = text.lines();
    let header: Vec<&str> = rows.next().unwrap().split(',').collect();
    let col = |name: &str| header.iter().position(|h| *h == name).unwrap();
    let (ct, cx, cy, ctouch, cclick, cp) = (col("t"), col("lx"), col("ly"), col("ltouch"), col("lclick"), col("lpressure"));
    let reports: Vec<(f64, Pad)> = rows
        .filter_map(|line| {
            let f: Vec<&str> = line.split(',').collect();
            let raw = |i: usize| f[i].parse::<f64>().ok();
            Some((
                raw(ct)?,
                Pad {
                    x: raw(cx)? as f32 / i16::MAX as f32,
                    y: raw(cy)? as f32 / i16::MAX as f32,
                    touched: raw(ctouch)? != 0.0,
                    clicked: raw(cclick)? != 0.0,
                    pressure: raw(cp)? as u16,
                },
            ))
        })
        .collect();

    let mut ways = vec![way("as the pad reports it", false, None), way("held (steady.rs)", true, None)];
    ways.push(Way { room: Some(ScrollSpread::default()), ..way("as the room reads it", true, None) });
    for k in [0.5f32, 0.35, 0.25, 0.15] {
        ways.push(way(&format!("held, spread {k}"), true, Some(k)));
    }
    for k in [0.35f32, 0.25] {
        ways.push(way(&format!("raw, spread {k}"), false, Some(k)));
    }

    // Once a frame, the last report before it: the compositor's poll.
    let start = Instant::now();
    let mut next = reports.first().map(|r| r.0).unwrap_or(0.0);
    let mut last: Option<Pad> = None;
    let mut frame = 0u32;
    for (t, pad) in &reports {
        while *t >= next {
            if let Some(p) = last {
                let now = start + Duration::from_secs_f64(frame as f64 * FRAME);
                for way in ways.iter_mut() {
                    let read = match way.steady.as_mut() {
                        Some(s) => s.update(&p, now),
                        None => p,
                    };
                    let step = match way.scroll.update(&read) {
                        Scroll::By { dy, .. } => dy,
                        _ => 0.0,
                    };
                    let step = if let Some(room) = way.room.as_mut() {
                        room.step(0.0, step).1
                    } else {
                        step
                    };
                    let step = match way.spread {
                        Some(k) => {
                            way.owed += step;
                            let out = way.owed * k;
                            way.owed -= out;
                            if out.abs() < 1e-5 { 0.0 } else { out }
                        }
                        None => step,
                    };
                    way.steps.push(step);
                }
                frame += 1;
            }
            next += FRAME;
        }
        last = Some(*pad);
    }

    for way in &ways {
        let (stalls, longest, unevenness, gestures, frames) = measure(&way.steps);
        // Turns: a step the other way to the one before it, among steps of less than 0.01 -- the
        // page going back and forth, which the band was made for.
        let mut turns = 0;
        let mut last = 0.0f32;
        for s in &way.steps {
            if *s != 0.0 {
                if s * last < 0.0 && s.abs() < 0.01 {
                    turns += 1;
                }
                last = *s;
            }
        }
        println!(
            "{:<22} {gestures} gestures, {frames} frames: {:>2.0}% sent nothing mid-scroll, at most {longest} in a row ({:.0} ms); unevenness {unevenness:.2}; small turns {turns}",
            way.name,
            100.0 * stalls as f64 / frames.max(1) as f64,
            longest as f64 * FRAME * 1000.0,
        );
    }
    if let Some(out) = args.get(2) {
        let mut csv = String::from("frame");
        for way in &ways {
            csv += &format!(",{}", way.name.replace([' ', ','], "_"));
        }
        csv.push('\n');
        for i in 0..ways[0].steps.len() {
            csv += &i.to_string();
            for way in &ways {
                csv += &format!(",{}", way.steps[i]);
            }
            csv.push('\n');
        }
        std::fs::write(out, csv).expect("writable");
    }
}

/// Over each gesture -- a run of frames from a step to the last step before half a second of none --
/// the frames that sent nothing, the longest run of them, and the steps' unevenness.
fn measure(steps: &[f32]) -> (usize, usize, f64, usize, usize) {
    let gap = (0.5 / FRAME) as usize;
    let (mut stalls, mut longest, mut gestures, mut frames) = (0, 0, 0, 0);
    let (mut change, mut travel) = (0.0f64, 0.0f64);
    let mut i = 0;
    while i < steps.len() {
        if steps[i] == 0.0 {
            i += 1;
            continue;
        }
        // A gesture: on until half a second of nothing.
        let begin = i;
        let mut end = i;
        let mut quiet = 0;
        let mut j = i;
        while j < steps.len() && quiet < gap {
            if steps[j] != 0.0 {
                end = j;
                quiet = 0;
            } else {
                quiet += 1;
            }
            j += 1;
        }
        let g = &steps[begin..=end];
        if g.len() >= 10 {
            gestures += 1;
            frames += g.len();
            let mut run = 0;
            for w in g.windows(2) {
                change += (w[1] - w[0]).abs() as f64;
            }
            for s in g {
                travel += s.abs() as f64;
                if *s == 0.0 {
                    stalls += 1;
                    run += 1;
                    longest = longest.max(run);
                } else {
                    run = 0;
                }
            }
        }
        i = end + 1;
    }
    (stalls, longest, if travel > 0.0 { change / travel } else { 0.0 }, gestures, frames)
}
