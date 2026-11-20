//! The text in a picture, read by hemera: `POST /v1/ocr` (hemera's own reference is
//! `docs/ocr-api.md` in its repository, and `docs/integrate-spatiand.md` section 4).
//!
//! The picture goes as it is, at its own size -- hemera never shrinks it, so small print stays
//! readable -- as PNG for a screenshot or a film's frame, as its own file for a photograph. What
//! comes back is every line in reading order, with its four corners, its words and where each of
//! its characters (grapheme clusters) is along it, in the pixels of the picture as it was sent:
//! EXIF orientation is not applied by hemera, so a photograph stored on its side is read on its
//! side and its coordinates are its stored pixels'.
//!
//! **Called off by hanging up.** Each reading has a connection of its own ([`Cancel`]): the wearer
//! moving to another picture, or a film playing on, shuts it, and hemera stops its work when its
//! connection closes.
//!
//! **Nothing of a picture or its text is logged**, here or by hemera: sizes, times and counts only.
//! An ID card read passes through memory and nowhere else.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::Hemera;

/// The most hemera reads: 50 MB, and 50 million pixels. A larger picture is refused (413).
pub const MOST_BYTES: usize = 52_428_800;
pub const MOST_PIXELS: u64 = 50_000_000;

/// How long a reading is given, from connecting to the answer's last byte: hemera's own limit
/// is 60 s (then 504), and the picture has to get there first.
const WITHIN: Duration = Duration::from_secs(90);

/// How long a 503's `Retry-After` is waited out, at most, before the one retry: hemera says when
/// its engine will be ready; longer than this is not worth the wearer's waiting.
const RETRY_AT_MOST: Duration = Duration::from_secs(20);

/// The most of an answer read: the text of a page and its every character's place is a few
/// hundred kilobytes; far more is not an answer of hemera's.
const ANSWER_LIMIT: u64 = 64 << 20;

/// What the picture is, as it is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Png,
    Jpeg,
    Webp,
    Tiff,
}

impl Kind {
    pub fn content_type(self) -> &'static str {
        match self {
            Kind::Png => "image/png",
            Kind::Jpeg => "image/jpeg",
            Kind::Webp => "image/webp",
            Kind::Tiff => "image/tiff",
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Kind::Png => "picture.png",
            Kind::Jpeg => "picture.jpg",
            Kind::Webp => "picture.webp",
            Kind::Tiff => "picture.tiff",
        }
    }

    /// The kind a file's bytes are, by how they begin: what hemera reads (PNG, JPEG, WebP, TIFF),
    /// or `None` for anything else, which is to be sent as a PNG of it instead.
    pub fn of(bytes: &[u8]) -> Option<Kind> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Kind::Png)
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Kind::Jpeg)
        } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            Some(Kind::Webp)
        } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
            Some(Kind::Tiff)
        } else {
            None
        }
    }
}

/// How fine hemera is to say where the text is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Granularity {
    Lines,
    Words,
    /// Lines, their words, and each character's place: what choosing text by the character needs.
    #[default]
    Chars,
}

/// What a picture is read with: hemera's options, as `docs/ocr-api.md` names them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Options {
    /// BCP-47 tags of the languages expected, best first, ending in `auto` for any other script to
    /// be found by itself; empty is `["auto"]`.
    pub languages: Vec<String>,
    pub granularity: Granularity,
    /// Only this part, `[x, y, w, h]` in the picture's pixels; the answer's places are still the
    /// whole picture's.
    pub region: Option<[u32; 4]>,
    /// How far the text is turned clockwise, where it is known (a screenshot, a film's frame: 0);
    /// `None` for hemera to find out.
    pub orientation: Option<u16>,
    /// A model by name from `GET /v1/ocr`; `None` for hemera's own choice.
    pub model: Option<String>,
}

impl Options {
    /// The wearer's two languages (the AI settings' first and second), then `auto`: lines in a
    /// script neither of them is written in are still found. Empty or repeated ones are left out.
    pub fn languages(first: &str, second: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for tag in [first, second, "auto"] {
            let tag = tag.trim();
            if !tag.is_empty() && !out.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
                out.push(tag.to_string());
            }
        }
        out
    }

    /// The `options` part, as JSON.
    pub fn to_json(&self) -> String {
        let languages = if self.languages.is_empty() { vec!["auto".to_string()] } else { self.languages.clone() };
        let granularity = match self.granularity {
            Granularity::Lines => "lines",
            Granularity::Words => "words",
            Granularity::Chars => "chars",
        };
        let options = serde_json::json!({
            "languages": languages,
            "granularity": granularity,
            "region": self.region,
            "orientation": match self.orientation {
                Some(turned) => serde_json::json!(turned),
                None => serde_json::json!("auto"),
            },
            "model": self.model.as_deref().filter(|m| !m.trim().is_empty()),
        });
        options.to_string()
    }
}

/// Which way a line is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Left to right: Latin, Cyrillic, Thai, Devanagari, horizontal CJK.
    #[default]
    Ltr,
    /// Right to left: Arabic, Hebrew.
    Rtl,
    /// Top to bottom, the columns right to left: vertical Japanese and Chinese.
    Ttb,
}

/// A point, in the picture's pixels as it was sent.
pub type Point = [f32; 2];

/// The picture as hemera read it.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Default)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// What hemera read.
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
pub struct Answer {
    pub version: u32,
    pub model: String,
    pub image: Size,
    /// How far the picture's text is turned clockwise, as found or as given.
    pub orientation: u16,
    /// `total` is hemera's whole answer; the rest are its engine's steps.
    pub timing_ms: serde_json::Map<String, serde_json::Value>,
    /// Everything in reading order: lines joined by a line's end, blocks by an empty line.
    pub text: String,
    /// Paragraphs, columns, a form's rows: in reading order.
    pub blocks: Vec<Block>,
}

impl Answer {
    /// How long hemera took, by its own clock, in milliseconds.
    pub fn total_ms(&self) -> f64 {
        self.timing_ms.get("total").and_then(serde_json::Value::as_f64).unwrap_or(0.0)
    }

    /// How many lines were read.
    pub fn line_count(&self) -> usize {
        self.blocks.iter().map(|b| b.lines.len()).sum()
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
pub struct Block {
    pub polygon: Vec<Point>,
    pub lines: Vec<Line>,
}

/// One line: four corners, clockwise from its own top-left in its reading direction (top-left for
/// `ltr`; top-right for `rtl` and `ttb`). The reading direction runs along its top side: `ltr`
/// from corner 0 to 1, `rtl` from 0 to 3, `ttb` from 0 to 1, downwards.
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
pub struct Line {
    pub text: String,
    pub polygon: Vec<Point>,
    /// Degrees the line is turned clockwise from upright.
    pub angle: f32,
    pub direction: Direction,
    /// ISO 15924: `Latn`, `Arab`, `Jpan`, ...; `Zyyy` for digits and signs only.
    pub script: String,
    pub language: Option<String>,
    /// 0 to 1: how sure the recogniser was, over the line's characters. Under 0.5 it is left out.
    pub confidence: f32,
    pub words: Vec<Word>,
    /// One `[start, end]` per extended grapheme cluster of `text`: pixels along the reading
    /// direction from corner 0.
    pub chars: Vec<[f32; 2]>,
    /// The places are spread evenly along the line rather than read.
    pub chars_estimated: bool,
}

/// A run of characters between spaces: `start` and `end` are grapheme indices into its line's
/// `chars` (`end` not included).
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
pub struct Word {
    pub text: String,
    pub polygon: Vec<Point>,
    pub start: usize,
    pub end: usize,
    pub confidence: f32,
}

/// Why a picture's text, or a translation, did not come.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// Called off: the wearer moved on.
    Cancelled,
    /// hemera said no, and why, in words fit to show: 400 a picture or options it cannot read,
    /// 413 too large, 503 its engine starting, 504 too long reading, 500 or 502 its engine failed.
    Refused { status: u16, message: String },
    /// hemera was not reached, or stopped answering.
    Unreachable(String),
    /// What came back was not an answer of hemera's.
    Unreadable(String),
    /// There is no hemera to ask: its address is not set ([`super::NOT_SET`]). Nothing was sent.
    NotSet,
}

impl Failure {
    /// What went wrong, in words fit to show, without a full stop at the end.
    pub fn words(&self) -> String {
        match self {
            Failure::Cancelled => "it was called off".into(),
            Failure::NotSet => super::NOT_SET.into(),
            Failure::Refused { message, .. } => message.trim().trim_end_matches('.').to_string(),
            Failure::Unreachable(said) | Failure::Unreadable(said) => said.trim().trim_end_matches('.').to_string(),
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.words())
    }
}

/// What a reply's status and body come to: the answer, or hemera's refusal in its own words.
pub fn answer_of(status: u16, body: &[u8]) -> Result<Answer, Failure> {
    if status >= 400 {
        return Err(refusal(status, body));
    }
    serde_json::from_slice::<Answer>(body).map_err(|e| Failure::Unreadable(format!("hemera's answer was not readable ({e})")))
}

/// A 4xx or 5xx, said as hemera said it (`{"error": "..."}`), or by its status where it said
/// nothing readable.
pub(super) fn refusal(status: u16, body: &[u8]) -> Failure {
    let said = serde_json::from_slice::<serde_json::Value>(body).ok().and_then(|v| v.get("error").and_then(serde_json::Value::as_str).map(str::to_string));
    let message = said.filter(|m| !m.trim().is_empty()).unwrap_or_else(|| match status {
        413 => "the picture is too large for hemera to read".into(),
        503 => "hemera's reader is not ready".into(),
        504 => "hemera took too long reading it".into(),
        _ => format!("hemera answered {status}"),
    });
    Failure::Refused { status, message }
}

/// How long a `Retry-After` says to wait: a number of seconds (hemera's way). An HTTP date, or
/// nothing, is two seconds.
pub fn retry_after(value: Option<&str>) -> Duration {
    value
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map_or(Duration::from_secs(2), Duration::from_secs_f64)
        .min(RETRY_AT_MOST)
}

/// A reading, or a translation, that may be called off from another thread: its connection is
/// its own, and calling it off shuts that connection -- hemera notices, and stops.
#[derive(Clone, Default)]
pub struct Cancel(Arc<Called>);

#[derive(Default)]
struct Called {
    off: AtomicBool,
    /// The connection of the request going on now, to be shut.
    agent: Mutex<Option<ureq::Agent>>,
}

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }

    /// Call it off: what is going on stops now, and nothing more is asked.
    pub fn cancel(&self) {
        self.0.off.store(true, Ordering::SeqCst);
        if let Some(agent) = self.0.agent.lock().unwrap().take() {
            agent.shutdown();
        }
    }

    pub fn cancelled(&self) -> bool {
        self.0.off.load(Ordering::SeqCst)
    }

    /// `agent` is the request going on: shut if this is called off. `false` if it already was.
    fn hold(&self, agent: &ureq::Agent) -> bool {
        let mut held = self.0.agent.lock().unwrap();
        if self.cancelled() {
            return false;
        }
        *held = Some(agent.clone());
        true
    }

    fn release(&self) {
        self.0.agent.lock().unwrap().take();
    }

    /// Wait `long`, or less if it is called off meanwhile: whether it was.
    fn wait(&self, long: Duration) -> bool {
        let until = Instant::now() + long;
        while Instant::now() < until {
            if self.cancelled() {
                return true;
            }
            std::thread::sleep((until - Instant::now()).min(Duration::from_millis(50)));
        }
        self.cancelled()
    }
}

/// What came back: its status, its `Retry-After`, its body.
pub(super) struct Reply {
    pub status: u16,
    pub retry_after: Option<String>,
    pub body: Vec<u8>,
}

/// A POST of `body` to `path`, on a connection of its own that `cancel` shuts.
pub(super) fn post(hemera: &Hemera, path: &str, content_type: &str, body: &[u8], within: Duration, cancel: &Cancel) -> Result<Reply, Failure> {
    let url = hemera.url(path).map_err(|_| Failure::NotSet)?;
    let agent = super::agent(0);
    if !cancel.hold(&agent) {
        return Err(Failure::Cancelled);
    }
    let asked = agent.post(&url).header("Content-Type", content_type);
    let sent = super::within(asked, within).send(body);
    let reply = sent.and_then(|reply| {
        let status = reply.status().as_u16();
        let retry_after = reply.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
        let body = reply.into_body().into_with_config().limit(ANSWER_LIMIT).read_to_vec()?;
        Ok(Reply { status, retry_after, body })
    });
    cancel.release();
    match reply {
        _ if cancel.cancelled() => Err(Failure::Cancelled),
        Ok(reply) => Ok(reply),
        Err(ureq::Error::Timeout(t)) => Err(Failure::Unreachable(format!("hemera at {} did not answer in time ({t})", hemera.base))),
        Err(ureq::Error::Io(e)) => Err(Failure::Unreachable(format!("hemera at {} could not be reached ({e})", hemera.base))),
        Err(e) => Err(Failure::Unreachable(format!("hemera at {} could not be reached ({e})", hemera.base))),
    }
}

/// The form hemera is sent: the picture as `image`, and the options.
fn form(picture: &[u8], kind: Kind, options: &Options) -> (String, Vec<u8>) {
    // Long enough, and of this moment, that a picture's own bytes do not hold it by chance.
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let boundary = format!("spatiand-ocr-{stamp:032x}-{:08x}", std::process::id());
    let mut body = Vec::with_capacity(picture.len() + 1024);
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"options\"\r\nContent-Type: application/json\r\n\r\n").as_bytes());
    body.extend_from_slice(options.to_json().as_bytes());
    body.extend_from_slice(
        format!("\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n", kind.file_name(), kind.content_type()).as_bytes(),
    );
    body.extend_from_slice(picture);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

impl Hemera {
    /// The text in `picture` (its bytes, of `kind`), read with `options`. A 503 -- hemera's
    /// reader starting -- is tried once more after the `Retry-After` it gives; `cancel` calls the
    /// whole of it off. Logged: the picture's size, the times, how many lines. Never its text.
    pub fn read_text(&self, picture: &[u8], kind: Kind, options: &Options, cancel: &Cancel) -> Result<Answer, Failure> {
        // No hemera: nothing to send it to, and the picture is not copied into a form for nothing.
        if !self.is_set() {
            return Err(Failure::NotSet);
        }
        let started = Instant::now();
        let (content_type, body) = form(picture, kind, options);
        let mut reply = post(self, "/v1/ocr", &content_type, &body, WITHIN, cancel)?;
        if reply.status == 503 {
            let wait = retry_after(reply.retry_after.as_deref());
            log::info!("ocr: hemera's reader is starting: tried again in {:.1} s", wait.as_secs_f32());
            if cancel.wait(wait) {
                return Err(Failure::Cancelled);
            }
            reply = post(self, "/v1/ocr", &content_type, &body, WITHIN, cancel)?;
        }
        let answer = answer_of(reply.status, &reply.body);
        match &answer {
            Ok(read) => log::info!(
                "ocr: {} bytes of {}, {}x{}, read in {:.2} s (hemera {:.2} s): {} lines in {} blocks",
                picture.len(),
                kind.content_type(),
                read.image.width,
                read.image.height,
                started.elapsed().as_secs_f32(),
                read.total_ms() / 1000.0,
                read.line_count(),
                read.blocks.len()
            ),
            Err(failure) => log::warn!("ocr: {} bytes of {}: not read after {:.2} s ({failure:?})", picture.len(), kind.content_type(), started.elapsed().as_secs_f32()),
        }
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn options_are_hemera_s_json() {
        let options = Options { languages: Options::languages("en", "id"), ..Default::default() };
        let json: serde_json::Value = serde_json::from_str(&options.to_json()).unwrap();
        assert_eq!(json, serde_json::json!({"languages": ["en", "id", "auto"], "granularity": "chars", "region": null, "orientation": "auto", "model": null}));
        // A frame: upright, a region, a model named.
        let frame = Options { languages: vec!["id".into(), "auto".into()], region: Some([0, 780, 1920, 300]), orientation: Some(0), model: Some("pp-ocrv6".into()), granularity: Granularity::Words };
        let json: serde_json::Value = serde_json::from_str(&frame.to_json()).unwrap();
        assert_eq!(json, serde_json::json!({"languages": ["id", "auto"], "granularity": "words", "region": [0, 780, 1920, 300], "orientation": 0, "model": "pp-ocrv6"}));
        // Nothing given: auto. A blank model is hemera's choice.
        let json: serde_json::Value = serde_json::from_str(&Options { model: Some(" ".into()), ..Default::default() }.to_json()).unwrap();
        assert_eq!(json["languages"], serde_json::json!(["auto"]));
        assert_eq!(json["model"], serde_json::Value::Null);
    }

    #[test]
    fn the_two_languages_come_first_once_each_then_auto() {
        assert_eq!(Options::languages("en", "id"), ["en", "id", "auto"]);
        assert_eq!(Options::languages("id", "id"), ["id", "auto"]);
        assert_eq!(Options::languages("", "ja"), ["ja", "auto"]);
        assert_eq!(Options::languages("auto", ""), ["auto"]);
    }

    #[test]
    fn a_picture_s_kind_is_told_by_its_first_bytes() {
        assert_eq!(Kind::of(b"\x89PNG\r\n\x1a\nrest"), Some(Kind::Png));
        assert_eq!(Kind::of(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(Kind::Jpeg));
        assert_eq!(Kind::of(b"RIFF\x10\0\0\0WEBPVP8 "), Some(Kind::Webp));
        assert_eq!(Kind::of(b"II*\0...."), Some(Kind::Tiff));
        assert_eq!(Kind::of(b"MM\0*...."), Some(Kind::Tiff));
        assert_eq!(Kind::of(b"\0\0\0\x18ftypheic"), None);
        assert_eq!(Kind::of(b"GIF89a"), None);
    }

    /// hemera's own example answer (`docs/ocr-api.md`), cut short where it is cut short there.
    const ANSWER: &str = r#"{
      "version": 1, "model": "pp-ocrv6", "image": { "width": 1712, "height": 1080 }, "orientation": 0,
      "timing_ms": { "total": 1529, "orientation": 5, "detect": 753, "recognize": 702 },
      "text": "PROVINSI DKI JAKARTA\nKOTA JAKARTA SELATAN\n\nNIK",
      "blocks": [
        { "polygon": [[540, 38], [1236, 38], [1236, 156], [540, 156]],
          "lines": [
            { "text": "PROVINSI DKI JAKARTA",
              "polygon": [[556.2, 41.1], [1229.8, 41.1], [1229.8, 92.9], [556.2, 92.9]],
              "angle": 0, "direction": "ltr", "script": "Latn", "language": "id", "confidence": 0.998,
              "words": [ { "text": "PROVINSI", "polygon": [[561.7, 41.1], [781.2, 41.1], [781.2, 92.9], [561.7, 92.9]], "start": 0, "end": 8, "confidence": 0.999 } ],
              "chars": [[5.5, 33.9], [33.9, 62.0]],
              "chars_estimated": false },
            { "text": "مرحبا", "polygon": [[900, 100], [900, 140], [700, 140], [700, 100]], "angle": 0, "direction": "rtl",
              "script": "Arab", "language": null, "confidence": 0.9, "words": [], "chars": [], "chars_estimated": true,
              "a_field_of_a_later_hemera": 1 }
          ] }
      ]
    }"#;

    #[test]
    fn an_answer_is_read_with_every_field_and_a_later_one_s_extra_ignored() {
        let answer = answer_of(200, ANSWER.as_bytes()).unwrap();
        assert_eq!(answer.model, "pp-ocrv6");
        assert_eq!((answer.image.width, answer.image.height), (1712, 1080));
        assert_eq!(answer.total_ms(), 1529.0);
        assert_eq!(answer.line_count(), 2);
        let line = &answer.blocks[0].lines[0];
        assert_eq!(line.text, "PROVINSI DKI JAKARTA");
        assert_eq!(line.direction, Direction::Ltr);
        assert_eq!(line.polygon[1], [1229.8, 41.1]);
        assert_eq!(line.language.as_deref(), Some("id"));
        assert_eq!(line.words[0].end, 8);
        assert_eq!(line.chars[1], [33.9, 62.0]);
        let arabic = &answer.blocks[0].lines[1];
        assert_eq!(arabic.direction, Direction::Rtl);
        assert!(arabic.chars_estimated);
        assert_eq!(arabic.language, None);
    }

    #[test]
    fn no_text_is_an_answer_with_nothing_in_it() {
        let answer = answer_of(200, br#"{"version":1,"model":"pp-ocrv6","image":{"width":10,"height":10},"orientation":0,"timing_ms":{"total":30},"text":""}"#).unwrap();
        assert!(answer.text.is_empty());
        assert!(answer.blocks.is_empty());
    }

    #[test]
    fn a_refusal_is_said_in_hemera_s_own_words() {
        assert_eq!(answer_of(413, br#"{"error": "The picture is 61.2 million pixels; at most 50 million are read."}"#), Err(Failure::Refused { status: 413, message: "The picture is 61.2 million pixels; at most 50 million are read.".into() }));
        let refused = answer_of(400, br#"{"error": "options: granularity must be lines, words or chars"}"#).unwrap_err();
        assert_eq!(refused.words(), "options: granularity must be lines, words or chars");
        // Nothing readable said: by its status.
        assert_eq!(answer_of(502, b"<html>bad gateway</html>").unwrap_err().words(), "hemera answered 502");
        assert_eq!(answer_of(504, b"").unwrap_err().words(), "hemera took too long reading it");
        // Not an answer at all.
        assert!(matches!(answer_of(200, b"{\"blocks\": 7}"), Err(Failure::Unreadable(_))));
    }

    #[test]
    fn retry_after_is_seconds_and_never_long() {
        assert_eq!(retry_after(Some("3")), Duration::from_secs(3));
        assert_eq!(retry_after(Some(" 0.5 ")), Duration::from_millis(500));
        assert_eq!(retry_after(Some("Wed, 21 Oct 2026 07:28:00 GMT")), Duration::from_secs(2));
        assert_eq!(retry_after(None), Duration::from_secs(2));
        assert_eq!(retry_after(Some("600")), RETRY_AT_MOST);
    }

    #[test]
    fn the_form_carries_the_picture_whole_and_the_options() {
        let picture: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
        let (content_type, body) = form(&picture, Kind::Png, &Options::default());
        let boundary = content_type.strip_prefix("multipart/form-data; boundary=").unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("name=\"options\""));
        assert!(text.contains("name=\"image\"; filename=\"picture.png\"\r\nContent-Type: image/png"));
        assert!(text.ends_with(&format!("\r\n--{boundary}--\r\n")));
        assert!(body.windows(picture.len()).any(|w| w == picture.as_slice()), "the picture's bytes, whole");
    }

    /// A stand-in for hemera on this machine: each connection is answered by `answer`, given the
    /// request's bytes; it says how many connections came, and whether one closed under it.
    fn stand_in(answer: impl Fn(usize, &[u8]) -> Option<String> + Send + 'static) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (tell, told) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for (n, stream) in listener.incoming().enumerate() {
                let Ok(mut stream) = stream else { return };
                // The whole request: its head, and a body as long as it says.
                let mut got = Vec::new();
                let mut chunk = [0u8; 65536];
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                while let Ok(read) = stream.read(&mut chunk) {
                    if read == 0 {
                        break;
                    }
                    got.extend_from_slice(&chunk[..read]);
                    let head = got.windows(4).position(|w| w == b"\r\n\r\n");
                    if let Some(head) = head {
                        let length = String::from_utf8_lossy(&got[..head]).lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                        if got.len() >= head + 4 + length {
                            break;
                        }
                    }
                }
                match answer(n, &got) {
                    Some(reply) => {
                        let _ = stream.write_all(reply.as_bytes());
                        let _ = tell.send(format!("answered {n}"));
                    }
                    // Held, as hemera holds a reading: until the other end hangs up.
                    None => {
                        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                        let closed = matches!(stream.read(&mut chunk), Ok(0) | Err(_));
                        let _ = tell.send(format!("{} {n}", if closed { "hung up" } else { "still open" }));
                    }
                }
            }
        });
        (base, told)
    }

    fn reply(status: &str, headers: &str, body: &str) -> String {
        format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}", body.len())
    }

    #[test]
    fn a_reader_starting_is_waited_for_once_and_asked_again() {
        let (base, told) = stand_in(|n, request| {
            assert!(String::from_utf8_lossy(request).starts_with("POST /v1/ocr "));
            Some(if n == 0 { reply("503 Service Unavailable", "Retry-After: 1\r\n", r#"{"error": "The reader is starting."}"#) } else { reply("200 OK", "", ANSWER) })
        });
        let started = Instant::now();
        let answer = Hemera::new(&base).read_text(b"\x89PNG\r\n\x1a\n", Kind::Png, &Options::default(), &Cancel::new()).unwrap();
        assert_eq!(answer.line_count(), 2);
        assert!(started.elapsed() >= Duration::from_millis(950), "the second ask waited the second it was told");
        assert_eq!(told.recv().unwrap(), "answered 0");
        assert_eq!(told.recv().unwrap(), "answered 1");
        // Starting still the second time: that is the answer, said.
        let (base, _told) = stand_in(|_, _| Some(reply("503 Service Unavailable", "Retry-After: 0\r\n", r#"{"error": "No OCR model is configured."}"#)));
        let failed = Hemera::new(&base).read_text(b"x", Kind::Png, &Options::default(), &Cancel::new()).unwrap_err();
        assert_eq!(failed, Failure::Refused { status: 503, message: "No OCR model is configured.".into() });
    }

    #[test]
    fn calling_a_reading_off_hangs_up_on_hemera_at_once() {
        let (base, told) = stand_in(|_, _| None);
        let cancel = Cancel::new();
        let reading = {
            let cancel = cancel.clone();
            std::thread::spawn(move || Hemera::new(&base).read_text(&[7u8; 300_000], Kind::Png, &Options::default(), &cancel))
        };
        std::thread::sleep(Duration::from_millis(400));
        let called_off = Instant::now();
        cancel.cancel();
        assert_eq!(reading.join().unwrap().unwrap_err(), Failure::Cancelled);
        assert!(called_off.elapsed() < Duration::from_secs(1), "it stopped at once, not at a timeout");
        assert_eq!(told.recv_timeout(Duration::from_secs(5)).unwrap(), "hung up 0", "hemera's end saw its connection close");
        // Called off before it began: nothing is sent.
        let (base, told) = stand_in(|_, _| Some(reply("200 OK", "", ANSWER)));
        let cancel = Cancel::new();
        cancel.cancel();
        assert_eq!(Hemera::new(&base).read_text(b"x", Kind::Png, &Options::default(), &cancel), Err(Failure::Cancelled));
        assert!(told.recv_timeout(Duration::from_millis(300)).is_err());
    }

    #[test]
    fn with_no_address_nothing_is_read_or_translated_and_it_is_said_why() {
        let none = Hemera::new("");
        let read = none.read_text(b"\x89PNG\r\n\x1a\n", Kind::Png, &Options::default(), &Cancel::new()).unwrap_err();
        assert_eq!(read, Failure::NotSet);
        assert_eq!(read.words(), "hemera's address is not set (Settings \u{2192} AI)");
        assert_eq!(none.translate("neohorse-1:4b", "Diskon", "en", "id", &Cancel::new()), Err(Failure::NotSet));
    }

    #[test]
    fn hemera_not_there_is_said_so() {
        // A port nothing listens on.
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let failed = Hemera::new(&format!("http://{free}")).read_text(b"x", Kind::Png, &Options::default(), &Cancel::new()).unwrap_err();
        assert!(matches!(&failed, Failure::Unreachable(said) if said.contains("could not be reached")), "{failed:?}");
    }
}
