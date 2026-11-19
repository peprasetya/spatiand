//! Live Text against pictures whose text is known: hemera's test set
//! (`/media/shared/AIModels/ocr/testset/`, or `LIVE_TEXT_TESTSET`), whose `truth.json` has every
//! line's text and corners -- generated pictures only, never a real document. Where the set is not
//! on the machine, these say so and pass.
//!
//! The geometry is tried on the truth alone (its lines given evenly placed graphemes); the
//! readings themselves against the real hemera, by hand:
//! `cargo test -p spatiand-app --lib -- --ignored live_text::truth --nocapture`.

use std::path::PathBuf;

use serde_json::Value;

use super::text::{Line, ReadText, Spot};
use super::{LiveText, Mapping, Picture};
use crate::ui::Rect;
use spatiand_voice::hemera::ocr::{self, Direction};

fn testset() -> Option<PathBuf> {
    let dir = std::env::var_os("LIVE_TEXT_TESTSET").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/media/shared/AIModels/ocr/testset"));
    dir.join("truth.json").is_file().then_some(dir)
}

fn truth() -> Option<(PathBuf, Value)> {
    let dir = testset()?;
    let text = std::fs::read_to_string(dir.join("truth.json")).ok()?;
    Some((dir, serde_json::from_str(&text).ok()?))
}

fn polygon(value: &Value) -> Vec<[f32; 2]> {
    value.as_array().into_iter().flatten().filter_map(|p| Some([p.get(0)?.as_f64()? as f32, p.get(1)?.as_f64()? as f32])).collect()
}

fn direction(value: &Value) -> Direction {
    match value.as_str() {
        Some("rtl") => Direction::Rtl,
        Some("ttb") => Direction::Ttb,
        _ => Direction::Ltr,
    }
}

/// A truth line as hemera would send it: its graphemes spread evenly along its top side.
fn as_read(line: &Value) -> ocr::Line {
    use unicode_segmentation::UnicodeSegmentation;
    let text = line["text"].as_str().unwrap_or_default().to_string();
    let polygon = polygon(&line["polygon"]);
    let direction = direction(&line["direction"]);
    let top_end = if direction == Direction::Rtl { polygon[3] } else { polygon[1] };
    let long = (top_end[0] - polygon[0][0]).hypot(top_end[1] - polygon[0][1]);
    let n = text.graphemes(true).count().max(1);
    let chars = (0..n).map(|i| [long * i as f32 / n as f32, long * (i + 1) as f32 / n as f32]).collect();
    ocr::Line { text, polygon, direction, confidence: 0.99, chars, ..Default::default() }
}

fn near(a: [f32; 2], b: [f32; 2], within: f32) -> bool {
    (a[0] - b[0]).abs() <= within && (a[1] - b[1]).abs() <= within
}

fn centre(q: [[f32; 2]; 4]) -> [f32; 2] {
    [(q[0][0] + q[1][0] + q[2][0] + q[3][0]) / 4.0, (q[0][1] + q[1][1] + q[2][1] + q[3][1]) / 4.0]
}

#[test]
fn every_line_s_graphemes_lie_along_it_the_way_it_is_read() {
    let Some((_, truth)) = truth() else {
        eprintln!("the OCR test set is not on this machine: not tried");
        return;
    };
    // Right to left, down a column, turned three ways, a card photographed askew, a page slanted.
    for name in ["ar", "ja_vertical", "rotated_90", "rotated_180", "rotated_270", "ktp_photo", "slanted", "screenshot_light", "mixed_scripts"] {
        let picture = &truth[name];
        for (index, raw) in picture["lines"].as_array().unwrap().iter().enumerate() {
            let read = as_read(raw);
            let line = Line::of(&read, 0).unwrap();
            let p = &read.polygon;
            // All of it is the line's own corners, whichever way it is read.
            let whole = line.quad(0, line.len());
            let expected = match read.direction {
                Direction::Rtl => [p[0], p[3], p[2], p[1]],
                _ => [p[0], p[1], p[2], p[3]],
            };
            for (got, want) in whole.iter().zip(expected) {
                assert!(near(*got, want, 0.05), "{name} line {index}: {got:?} is not {want:?}");
            }
            if line.len() < 2 {
                continue;
            }
            // The first grapheme is where reading begins: right for Arabic, top for a column, and
            // for text turned on its side, where the turn puts it.
            let first = centre(line.quad(0, 1));
            let last = centre(line.quad(line.len() - 1, line.len()));
            let along = [last[0] - first[0], last[1] - first[1]];
            let reading = [expected[1][0] - expected[0][0], expected[1][1] - expected[0][1]];
            assert!(along[0] * reading[0] + along[1] * reading[1] > 0.0, "{name} line {index}: read the wrong way");
            match (name, read.direction) {
                (_, Direction::Rtl) => assert!(first[0] > last[0], "{name}: Arabic begins at the right"),
                (_, Direction::Ttb) => assert!(first[1] < last[1], "{name}: a column begins at its top"),
                ("rotated_90", _) => assert!(first[1] > last[1], "turned a quarter anticlockwise, it is read upwards"),
                ("rotated_180", _) => assert!(first[0] > last[0], "upside down, it is read right to left"),
                ("rotated_270", _) => assert!(first[1] < last[1], "turned a quarter clockwise, it is read downwards"),
                _ => {}
            }
            // Each grapheme's middle is found to be that grapheme, half way across.
            let read_text = ReadText::new((picture["width"].as_f64().unwrap() as f32, picture["height"].as_f64().unwrap() as f32), vec![line.clone()]);
            for g in [0, line.len() / 2, line.len() - 1] {
                let middle = centre(line.quad(g, g + 1));
                let (s, t) = line.locate(middle);
                assert!((t - 0.5).abs() < 0.02, "{name} line {index} grapheme {g}: across {t}");
                assert_eq!(line.grapheme_at(s), Some(g), "{name} line {index}: along {s}");
                assert_eq!(read_text.hit(middle, 0.0).map(|s| s.line), Some(0));
            }
            // A photo taken askew: a stretch of the line is the same fraction of its top and of its
            // bottom, the two not being parallel.
            if name == "ktp_photo" || name == "slanted" {
                let half = line.len() / 2;
                let q = line.quad(0, half);
                let fraction = |a: [f32; 2], b: [f32; 2], at: [f32; 2]| ((at[0] - a[0]).hypot(at[1] - a[1])) / (b[0] - a[0]).hypot(b[1] - a[1]);
                let (top, bottom) = (fraction(line.top.0, line.top.1, q[1]), fraction(line.bottom.0, line.bottom.1, q[2]));
                assert!((top - bottom).abs() < 1e-3, "{name} line {index}: {top} along the top, {bottom} along the bottom");
            }
        }
    }
}

#[test]
fn choosing_across_a_picture_s_lines_copies_them_as_its_truth_has_them() {
    let Some((_, truth)) = truth() else {
        eprintln!("the OCR test set is not on this machine: not tried");
        return;
    };
    for name in ["ar", "ja_vertical", "ktp_card", "rotated_90"] {
        let picture = &truth[name];
        let lines: Vec<Line> = picture["lines"].as_array().unwrap().iter().filter_map(|raw| Line::of(&as_read(raw), 0)).collect();
        let texts: Vec<String> = lines.iter().map(|l| l.text.clone()).collect();
        let size = (picture["width"].as_f64().unwrap() as f32, picture["height"].as_f64().unwrap() as f32);
        let mut live = LiveText::new();
        live.set_text(Some(ReadText::new(size, lines.clone())));
        // Shown at half its size, 50 pixels in.
        live.place(Mapping::onto(size, Rect::new(50.0, 50.0, size.0 * 0.5, size.1 * 0.5)));
        let shown = |p: [f32; 2]| (50.0 + p[0] * 0.5, 50.0 + p[1] * 0.5);
        // A point a fraction of the way along a line, half way across it.
        let at = |line: &Line, along: f32| {
            let p = super::text::lerp(super::text::lerp(line.top.0, line.top.1, along), super::text::lerp(line.bottom.0, line.bottom.1, along), 0.5);
            shown(p)
        };
        // From a quarter into the second line's first grapheme to three quarters into the third
        // line's last, by the pointer: both lines, whole.
        let g = &lines[1].graphemes[0];
        let from = at(&lines[1], g.along.0 + (g.along.1 - g.along.0) * 0.25);
        let g = lines[2].graphemes.last().unwrap();
        let to = at(&lines[2], g.along.0 + (g.along.1 - g.along.0) * 0.75);
        live.input(&crate::ui::Input::Pointer { x: from.0, y: from.1 });
        live.input(&crate::ui::Input::Press { pressed: true, modifiers: Default::default() });
        live.input(&crate::ui::Input::Pointer { x: to.0, y: to.1 });
        live.input(&crate::ui::Input::Press { pressed: false, modifiers: Default::default() });
        assert_eq!(live.selected().unwrap_or_default(), format!("{}\n{}", texts[1], texts[2]), "{name}");
        let all = ReadText::new(size, lines).text(Spot::default(), Spot { line: 2, at: usize::MAX >> 1 });
        assert!(all.starts_with(&texts[0]));
    }
}

/// How many grapheme edits make `a` into `b`.
fn edits(a: &str, b: &str) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    let (a, b): (Vec<&str>, Vec<&str>) = (a.graphemes(true).collect(), b.graphemes(true).collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut last = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let was = row[j + 1];
            row[j + 1] = if x == y { last } else { 1 + last.min(row[j]).min(row[j + 1]) };
            last = was;
        }
    }
    row[b.len()]
}

/// How much of `a`'s ground `b` also covers, measured on a grid of `a`'s bounding box: 1 for a read
/// line wholly inside a true one.
fn inside_of(a: &[[f32; 2]], b: &[[f32; 2]]) -> f32 {
    let inside = |q: &[[f32; 2]], p: [f32; 2]| {
        let mut sign = 0.0f32;
        for i in 0..q.len() {
            let (u, v) = (q[i], q[(i + 1) % q.len()]);
            let c = (v[0] - u[0]) * (p[1] - u[1]) - (v[1] - u[1]) * (p[0] - u[0]);
            if c != 0.0 {
                if sign != 0.0 && c.signum() != sign {
                    return false;
                }
                sign = c.signum();
            }
        }
        true
    };
    let (x0, x1) = a.iter().fold((f32::MAX, f32::MIN), |(l, r), p| (l.min(p[0]), r.max(p[0])));
    let (y0, y1) = a.iter().fold((f32::MAX, f32::MIN), |(t, d), p| (t.min(p[1]), d.max(p[1])));
    let (mut both, mut mine) = (0usize, 0usize);
    for i in 0..40 {
        for j in 0..40 {
            let p = [x0 + (x1 - x0) * (i as f32 + 0.5) / 40.0, y0 + (y1 - y0) * (j as f32 + 0.5) / 40.0];
            if inside(a, p) {
                mine += 1;
                both += inside(b, p) as usize;
            }
        }
    }
    both as f32 / mine.max(1) as f32
}

/// Each test picture read by the real hemera (the AI settings' address, or `HEMERA`), as the
/// applications send it: how long it took, and how each of its true lines came back -- the line
/// read where it is, chosen whole and copied, against its truth. Prints a table; fails only if
/// hemera could not be asked, or a line came back somewhere else altogether.
#[test]
#[ignore]
fn the_test_pictures_read_by_hemera_copy_as_their_truth() {
    let Some((dir, truth)) = truth() else {
        eprintln!("the OCR test set is not on this machine: not tried");
        return;
    };
    let base = std::env::var("HEMERA").unwrap_or_else(|_| crate::ai::settings().hemera);
    let hemera = spatiand_voice::Hemera::new(&base);
    let mut names: Vec<&String> = truth.as_object().unwrap().keys().collect();
    names.sort();
    if let Ok(only) = std::env::var("LIVE_TEXT_ONLY") {
        names.retain(|n| only.split(',').any(|o| o == n.as_str()));
    }
    let (mut all_chars, mut all_edits, mut all_bare, mut exact, mut lines_total) = (0usize, 0usize, 0usize, 0usize, 0usize);
    println!("{:<26} {:>11} {:>8} {:>8} {:>6} {:>7} {:>6} {:>8}", "picture", "size", "time s", "hemera", "lines", "exact", "CER %", "spaces %");
    for name in names {
        let entry = &truth[name.as_str()];
        let file = dir.join(entry["file"].as_str().unwrap());
        let bytes = std::fs::read(&file).unwrap();
        // As the applications send it: a photo, a screenshot and a frame as their own files (all
        // PNG or JPEG here); a screenshot and a frame known to be upright.
        let kind = entry["kind"].as_str().unwrap_or_default();
        let mut picture = Picture::file(bytes).expect("a kind hemera reads");
        picture.upright = matches!(kind, "screenshot" | "video");
        let options = ocr::Options { languages: ocr::Options::languages("en", "id"), orientation: picture.upright.then_some(0), ..Default::default() };
        let started = std::time::Instant::now();
        let answer = hemera.read_text(&picture.bytes, picture.kind, &options, &ocr::Cancel::new()).unwrap_or_else(|e| panic!("{name}: {e}"));
        let took = started.elapsed().as_secs_f32();
        let read = ReadText::of(&answer);
        let (mut chars, mut wrong, mut bare_wrong, mut same) = (0usize, 0usize, 0usize, 0usize);
        let truth_lines = entry["lines"].as_array().unwrap();
        for raw in truth_lines {
            let text = raw["text"].as_str().unwrap();
            let corners = polygon(&raw["polygon"]);
            // The lines read where this one is -- a receipt's row comes back as its item and its
            // price -- each chosen whole, as three presses choose it, and copied, in reading order.
            let copied: Vec<String> = read
                .lines
                .iter()
                .enumerate()
                .filter(|(_, l)| inside_of(&l.corners(), &corners) > 0.5)
                .map(|(index, _)| {
                    let (a, b) = read.whole(index);
                    read.text(a, b)
                })
                .collect();
            let copied = copied.join(" ");
            // A run of spaces is one: a receipt's columns are spaced out in its truth.
            let truth_text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let squeeze = |t: &str| t.split_whitespace().collect::<String>();
            let e = edits(&truth_text, &copied);
            let bare = edits(&squeeze(text), &squeeze(&copied));
            let n = truth_text.chars().count();
            chars += n;
            wrong += e.min(n);
            bare_wrong += bare.min(n);
            same += (e == 0) as usize;
            if e > 0 && std::env::var_os("LIVE_TEXT_SHOW").is_some() {
                println!("    {name}: {text:?} came as {copied:?}");
            }
        }
        println!(
            "{:<26} {:>11} {:>8.2} {:>8.2} {:>6} {:>4}/{:<2} {:>6.1} {:>8.1}",
            name,
            format!("{}x{}", answer.image.width, answer.image.height),
            took,
            answer.total_ms() / 1000.0,
            read.lines.len(),
            same,
            truth_lines.len(),
            100.0 * wrong as f32 / chars.max(1) as f32,
            100.0 * bare_wrong as f32 / chars.max(1) as f32
        );
        all_chars += chars;
        all_edits += wrong;
        all_bare += bare_wrong;
        exact += same;
        lines_total += truth_lines.len();
    }
    println!(
        "all: {exact} of {lines_total} lines exactly as their truth (a run of spaces as one); {:.2}% of characters wrong, {:.2}% leaving spaces aside",
        100.0 * all_edits as f32 / all_chars.max(1) as f32,
        100.0 * all_bare as f32 / all_chars.max(1) as f32
    );
}
