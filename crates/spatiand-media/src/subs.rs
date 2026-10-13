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
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => depth += 1,
            '}' if depth > 0 => depth -= 1,
            _ if depth > 0 => {}
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

fn tidy(text: &str) -> String {
    text.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<_>>().join("\n")
}

/// The markup an `.srt` file is allowed (`<i>`, `<font ...>`) taken out.
fn unmarked(text: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in text.chars() {
        match c {
            '<' => inside = true,
            '>' if inside => inside = false,
            _ if !inside => out.push(c),
            _ => {}
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

/// The cues of an `.srt` file. Whatever in it cannot be read is left out, and the rest kept:
/// one bad block is not a reason to show no subtitles at all.
pub fn srt(text: &str) -> Vec<Cue> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut cues = Vec::new();
    for block in text.split("\n\n") {
        let mut lines = block.lines().skip_while(|line| !line.contains("-->"));
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
        // A drawing, which is all override and no words.
        assert_eq!(plain("3,0,Sign,,0,0,0,,{\\p1}m 0 0 l 10 10{\\p0}"), "m 0 0 l 10 10");
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
