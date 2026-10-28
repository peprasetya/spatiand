//! Speaking, through hemera's speech models: the sound as it is made, so that the first words are
//! heard about 0.15 s after they are asked for rather than after the whole sentence is done.
//!
//! `POST /v1/audio/speech` with `stream_format: sse`: `speech.audio.delta` events carrying
//! base64 16-bit mono PCM at 24 kHz, whatever the model's own rate, then `speech.audio.done`.

use std::io::{BufRead, BufReader};
use std::time::Duration;

use serde_json::{json, Value};

use crate::hemera::Hemera;

/// The rate speech comes at.
pub const SPEECH_RATE: u32 = 24_000;

/// A voice: a name from hemera's voice library, or one described in words, or the model's own.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Voice {
    pub model: String,
    pub name: Option<String>,
    pub described: Option<String>,
}

/// Say `text`: `heard` is given the sound as it is made, and stops it by returning `false`.
pub fn say(hemera: &Hemera, voice: &Voice, text: &str, mut heard: impl FnMut(&[i16]) -> bool) -> Result<(), String> {
    let mut request = json!({"model": voice.model, "input": text, "stream_format": "sse"});
    if let Some(name) = voice.name.as_ref().filter(|n| !n.is_empty()) {
        request["voice"] = json!(name);
    } else if let Some(described) = voice.described.as_ref().filter(|d| !d.is_empty()) {
        request["instructions"] = json!(described);
    }
    let reply = hemera.post_stream("/v1/audio/speech", &request, Duration::from_secs(120))?;
    for line in BufReader::new(reply).lines() {
        let line = line.map_err(|e| format!("hemera's voice broke off: {e}"))?;
        let Some(data) = line.strip_prefix("data:").map(str::trim) else { continue };
        let Ok(event) = serde_json::from_str::<Value>(data) else { continue };
        match event.get("type").and_then(Value::as_str) {
            Some("speech.audio.delta") => {
                let Some(audio) = event.get("audio").and_then(Value::as_str) else { continue };
                let bytes = decode_base64(audio).ok_or("hemera's voice sent something that is not sound")?;
                let pcm: Vec<i16> = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
                if !heard(&pcm) {
                    return Ok(());
                }
            }
            Some("speech.audio.done") => break,
            Some("error") => return Err(event.pointer("/error/message").and_then(Value::as_str).unwrap_or("hemera could not speak").to_string()),
            _ => {}
        }
    }
    Ok(())
}

/// Base64, standard alphabet, padding optional. Small enough not to want a crate.
pub fn decode_base64(text: &str) -> Option<Vec<u8>> {
    fn value(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        })
    }
    let bytes: Vec<u8> = text.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= value(*c)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// Base64, for a picture sent to a model.
pub fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Where a sentence ends in text being written, so that speech can start on it: after `.`, `!`,
/// `?` or a line's end followed by a space or the end, and not inside a number (`3.5`).
pub fn sentence_end(text: &str) -> Option<usize> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (k, &(i, c)) in chars.iter().enumerate() {
        let next = chars.get(k + 1).map(|&(_, n)| n);
        let ends = match c {
            '.' | '!' | '?' | '\u{3002}' => next.is_some_and(char::is_whitespace),
            '\n' => true,
            _ => false,
        };
        // Long enough to be worth saying on its own: a stray "Ok." is not held back, but "Dr."
        // is rarely the end of a sentence.
        if ends && i >= 2 {
            return Some(i + c.len_utf8());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_goes_both_ways() {
        for bytes in [&b""[..], b"a", b"ab", b"abc", b"abcd", &[0, 255, 128, 7, 9]] {
            assert_eq!(decode_base64(&encode_base64(bytes)).unwrap(), bytes);
        }
        assert_eq!(decode_base64("JgA3AA==").unwrap(), vec![0x26, 0, 0x37, 0]);
    }

    #[test]
    fn sentences_end_where_they_end() {
        assert_eq!(sentence_end("Siap. Saya buka"), Some(5));
        assert_eq!(sentence_end("Versi 3.5 sudah"), None);
        assert_eq!(sentence_end("Done!"), None);
        assert_eq!(sentence_end("Done! Next"), Some(5));
        assert_eq!(sentence_end("one line\nnext"), Some(9));
    }
}
