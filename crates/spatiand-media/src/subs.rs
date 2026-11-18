//! Words under a film.
//!
//! A subtitle is a stretch of time and something to show during it: text, or a picture (the
//! kind a disc carries -- PGS, VobSub -- which is drawn letters, not letters). Both come from
//! the film's own tracks through FFmpeg, and text also from a file beside the film; either
//! way what reaches the player is a [`Cue`].

/// What to show.
#[derive(Debug, Clone, PartialEq)]
pub enum Words {
    /// Lines of text.
    Text(String),
    /// A picture, four bytes a pixel, red first, alpha not multiplied in; and where it goes:
    /// its left, top, width and height as fractions of the film's frame.
    Picture { rgba: Vec<u8>, width: u32, height: u32, place: [f32; 4] },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    /// Seconds from the start of the film.
    pub start: f64,
    pub end: f64,
    pub words: Words,
}

/// The text of an ASS event as plain lines: its fields before the text dropped, its styling
/// overrides (`{\i1}`) and drawing commands thrown away, `\N` made a new line.
///
/// FFmpeg hands every text subtitle over as ASS whatever it was in the file, so this is the
/// one place text subtitles are read.
pub fn plain(ass: &str) -> String {
    // "ReadOrder,Layer,Style,Name,MarginL,MarginR,MarginV,Effect,Text": the text is after
    // the eighth comma, and may have commas of its own.
    let text = ass.splitn(9, ',').nth(8).unwrap_or(ass);
    let mut out = String::new();
    let mut depth = 0;
    // Inside a drawing (`{\p1}` to `{\p0}`): what is between is a path, not words. It was
    // shown as "m 0 0 l 10 10".
    let mut drawing = false;
    let mut block = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => {
                depth += 1;
                block.clear();
            }
            '}' if depth > 0 => {
                depth -= 1;
                if let Some(scale) = drawing_scale(&block) {
                    drawing = scale > 0;
                }
            }
            _ if depth > 0 => block.push(c),
            _ if drawing => {}
            '\\' => match chars.peek() {
                Some('N') | Some('n') => {
                    chars.next();
                    out.push('\n');
                }
                Some('h') => {
                    chars.next();
                    out.push(' ');
                }
                _ => out.push(c),
            },
            _ => out.push(c),
        }
    }
    tidy(&out)
}

/// The scale of the last `\p` in an override block, if it has one: 0 ends a drawing, more begins
/// one.
fn drawing_scale(block: &str) -> Option<u32> {
    block.split('\\').filter_map(|tag| tag.strip_prefix('p')).filter_map(|n| n.trim().parse().ok()).last()
}

fn tidy(text: &str) -> String {
    text.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<_>>().join("\n")
}

/// The markup an `.srt` file is allowed (`<i>`, `<font ...>`) taken out, and the ASS overrides
/// many have as well (`{\an8}`, put it at the top; `{\i1}`). Only what is a tag: a `<` that does
/// not begin one ("x < 3", "<3") is a letter -- it took the rest of the cue with it.
fn unmarked(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        let after = &rest[c.len_utf8()..];
        let tag = match c {
            '<' => {
                let name = after.strip_prefix('/').unwrap_or(after);
                let closed = after.find('>').filter(|&end| !after[..end].contains(['<', '\n']));
                closed.filter(|_| name.starts_with(|c: char| c.is_ascii_alphabetic()))
            }
            '{' if after.starts_with('\\') => after.find('}').filter(|&end| !after[..end].contains('\n')),
            _ => None,
        };
        match tag {
            Some(end) => rest = &after[end + 1..],
            None => {
                out.push(c);
                rest = after;
            }
        }
    }
    out
}

fn stamp(text: &str) -> Option<f64> {
    // 00:01:02,345 -- or with a full stop, which some files have.
    let text = text.trim().replace(',', ".");
    let mut parts = text.split(':');
    let (h, m, s) = (parts.next()?, parts.next()?, parts.next()?);
    Some(h.trim().parse::<f64>().ok()? * 3600.0 + m.trim().parse::<f64>().ok()? * 60.0 + s.trim().parse::<f64>().ok()?)
}

/// A subtitle file's bytes as text, whatever it was saved as: UTF-8 (with its mark or without),
/// UTF-16 with its mark, either way round, or -- not being UTF-8 -- Windows' Western code page
/// (Latin-1 and its additions), which most old subtitles for Western films are in. Read as
/// UTF-8 whatever it was, a UTF-16 file was nothing and an old one's accents were marks.
pub fn text_of(bytes: &[u8]) -> String {
    let utf16 = |rest: &[u8], big: bool| {
        let units: Vec<u16> = rest.chunks_exact(2).map(|p| if big { u16::from_be_bytes([p[0], p[1]]) } else { u16::from_le_bytes([p[0], p[1]]) }).collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, false),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, true),
        _ => match std::str::from_utf8(bytes) {
            Ok(text) => text.to_string(),
            Err(_) => bytes.iter().map(|&b| windows_1252(b)).collect(),
        },
    }
}

/// A byte of Windows-1252 as the character it is.
fn windows_1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20ac}', '\u{81}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}', '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8d}', '\u{17d}', '\u{8f}',
        '\u{90}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}', '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{9d}', '\u{17e}', '\u{178}',
    ];
    match byte {
        0x80..=0x9F => HIGH[(byte - 0x80) as usize],
        _ => byte as char,
    }
}

/// The cues of an `.srt` file. Whatever in it cannot be read is left out, and the rest kept:
/// one bad block is not a reason to show no subtitles at all.
pub fn srt(text: &str) -> Vec<Cue> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n");
    let mut cues = Vec::new();
    // Blocks are parted by a line with nothing on it -- or only spaces, which many files have
    // and which, parted by "\n\n" alone, ran two cues into one.
    let mut blocks: Vec<Vec<&str>> = vec![Vec::new()];
    for line in text.lines() {
        if line.trim().is_empty() {
            if blocks.last().is_some_and(|b| !b.is_empty()) {
                blocks.push(Vec::new());
            }
        } else {
            blocks.last_mut().unwrap().push(line);
        }
    }
    for block in blocks {
        let mut lines = block.into_iter().skip_while(|line| !line.contains("-->"));
        let Some(times) = lines.next() else { continue };
        let Some((from, to)) = times.split_once("-->") else { continue };
        // After the end time there may be where on the screen it goes; not ours to honour.
        let to = to.split_whitespace().next().unwrap_or("");
        let (Some(start), Some(end)) = (stamp(from), stamp(to)) else { continue };
        let words = tidy(&unmarked(&lines.collect::<Vec<_>>().join("\n")));
        if !words.is_empty() && end > start {
            cues.push(Cue { start, end, words: Words::Text(words) });
        }
    }
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    cues
}

/// The cue to show at `time`, of cues in order of their starts: the latest that has begun
/// and not ended.
pub fn at(cues: &[Cue], time: f64) -> Option<&Cue> {
    let before = cues.partition_point(|cue| cue.start <= time);
    cues[..before].iter().rev().take(4).find(|cue| cue.end > time)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ass_event_is_its_text_and_nothing_else() {
        assert_eq!(plain("12,0,Default,,0,0,0,,Hello, {\\i1}world{\\i0}\\Nand you"), "Hello, world\nand you");
        // A drawing, which is a path and no words: nothing of it is shown.
        assert_eq!(plain("3,0,Sign,,0,0,0,,{\\p1}m 0 0 l 10 10{\\p0}"), "");
        assert_eq!(plain("3,0,Sign,,0,0,0,,{\\an7\\p2}m 0 0 l 10 10 l 0 10{\\p0}Exit"), "Exit");
        assert_eq!(plain("3,0,Sign,,0,0,0,,{\\pos(10,20)\\pbo0}Sign"), "Sign");
        assert_eq!(plain("1,0,Default,,0,0,0,,  spaced\\hout  "), "spaced out");
    }

    #[test]
    fn an_srt_file_is_read_and_its_markup_dropped() {
        let cues = srt("\u{feff}1\r\n00:00:01,000 --> 00:00:03,500\r\n<i>Hello</i>\r\nthere\r\n\r\n2\r\n00:01:00.250 --> 00:01:02.000 X1:10\r\n- Who?\r\n\r\nnot a cue\r\n\r\n3\r\n00:00:05,000 --> 00:00:04,000\r\nbackwards\r\n");
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0], Cue { start: 1.0, end: 3.5, words: Words::Text("Hello\nthere".into()) });
        assert_eq!(cues[1].start, 60.25);
        assert_eq!(cues[1].words, Words::Text("- Who?".into()));
    }

    #[test]
    fn an_srt_file_s_other_markup_is_dropped_and_its_letters_kept() {
        // ASS overrides in an .srt, a blank line holding spaces, and a "<" that is a letter.
        let cues = srt("1\n00:00:01,000 --> 00:00:02,000\n{\\an8}Up <b>here</b>\n  \n2\n00:00:03,000 --> 00:00:04,000\nx < 3 and <3 too\nnext line\n");
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].words, Words::Text("Up here".into()));
        assert_eq!(cues[1].words, Words::Text("x < 3 and <3 too\nnext line".into()));
        assert_eq!(unmarked("<font color=\"#ff0\">a</font> {b} {\\i1}c{\\i0}"), "a {b} c");
    }

    #[test]
    fn a_subtitle_file_is_read_whatever_it_was_saved_as() {
        assert_eq!(text_of("Caf\u{e9}".as_bytes()), "Caf\u{e9}");
        assert_eq!(text_of(b"\xEF\xBB\xBFCaf\xC3\xA9"), "Caf\u{e9}");
        assert_eq!(text_of(b"\xFF\xFEC\0a\0f\0\xE9\0"), "Caf\u{e9}");
        assert_eq!(text_of(b"\xFE\xFF\0C\0a\0f\0\xE9"), "Caf\u{e9}");
        // Not UTF-8: Windows' Western code page, its quotes and its euro included.
        assert_eq!(text_of(b"Caf\xE9 \x93ol\xE9\x94 \x80"), "Caf\u{e9} \u{201c}ol\u{e9}\u{201d} \u{20ac}");
    }

    #[test]
    fn the_cue_on_show_is_the_latest_begun_and_not_ended() {
        let cue = |start, end, text: &str| Cue { start, end, words: Words::Text(text.into()) };
        let cues = [cue(1.0, 3.0, "a"), cue(2.5, 4.0, "b"), cue(10.0, 11.0, "c")];
        assert!(at(&cues, 0.5).is_none());
        assert_eq!(at(&cues, 2.0).unwrap().words, Words::Text("a".into()));
        assert_eq!(at(&cues, 2.7).unwrap().words, Words::Text("b".into()));
        assert!(at(&cues, 5.0).is_none());
        assert_eq!(at(&cues, 10.5).unwrap().words, Words::Text("c".into()));
    }
}
