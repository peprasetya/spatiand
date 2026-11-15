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

/// **One voice for every sentence.** A voice described in words is not a voice: hemera hands the
/// description to VoxCPM2 in front of each request's text (`(a friendly young voice...)Sure,
/// here is...`), and the model makes a voice up for that text -- a new one for every sentence,
/// the same only when the sentence is (its seed is fixed). Spoken a sentence at a time, a long
/// answer was a choir: the median pitch of five sentences of one answer ran from 139 to 238 Hz,
/// and hemera's own speaker model heard the Indonesian one as somebody else (2026-11-15).
///
/// So the description is made into a voice **once**: one recording, made from the description,
/// kept in hemera's voice library under a name that comes from the model and the description
/// ([`made_voice_name`]), and every sentence after is spoken in that recorded voice -- on every
/// device, in every answer, as long as the description stays the same. Changing it makes another.
pub fn made_voice_name(model: &str, described: &str) -> String {
    // FNV-1a: the same name on every machine, from nothing but the two strings.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in model.bytes().chain([0]).chain(described.trim().bytes()) {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("spatipal-{:012x}", hash & 0xffff_ffff_ffff)
}

/// What the made voice says in its recording: about ten seconds, natural and in both of the
/// wearer's languages, so that the voice it lends is the same in either.
pub const MADE_VOICE_LINES: &str = "Hi, I'm here whenever you need me. Tell me what you'd like to do, and I'll help you with it, step by step. Kalau mau, kita bisa ngobrol pakai bahasa Indonesia juga. Just say my name, and I'll listen.";

/// The library voice the description `described` was made into, made now if hemera does not have
/// it yet: its name. Making it is one sentence spoken in the described voice and kept (about
/// two seconds); after that it is a look in the library.
pub fn made_voice(hemera: &Hemera, model: &str, described: &str) -> Result<String, String> {
    let name = made_voice_name(model, described);
    if hemera.get_json(&format!("/v1/audio/voices/{name}")).is_ok_and(|v| v.get("id").is_some()) {
        return Ok(name);
    }
    let made = Voice { model: model.to_string(), name: None, described: Some(described.to_string()) };
    let mut pcm = Vec::new();
    say(hemera, &made, MADE_VOICE_LINES, |piece| {
        pcm.extend_from_slice(piece);
        true
    })?;
    if pcm.len() < SPEECH_RATE as usize * 3 {
        return Err(format!("the voice described came out too short to keep ({:.1} s)", pcm.len() as f32 / SPEECH_RATE as f32));
    }
    let wav = crate::hemera::wav(&pcm, SPEECH_RATE);
    let consent = "Synthetic: made by the speech model from SpatiPal's voice description, not a recording of a real person.";
    hemera.post_form(
        "/v1/audio/voices",
        &[("name", &name), ("consent", consent), ("transcript", MADE_VOICE_LINES), ("owner", "SpatiPal")],
        ("voice.wav", "audio/wav", &wav),
        Duration::from_secs(60),
    )?;
    log::info!("voice: made the voice described ({:?}) into {name}, {:.1} s of it", described.trim(), pcm.len() as f32 / SPEECH_RATE as f32);
    Ok(name)
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
    fn a_described_voice_has_one_name_everywhere() {
        let d = "A friendly young voice, light and warm, speaking at an easy pace.";
        // The same on every machine and in every answer: nothing but the model and the words.
        assert_eq!(made_voice_name("voxcpm2", d), made_voice_name("voxcpm2", &format!("  {d} ")));
        assert_eq!(made_voice_name("voxcpm2", d), "spatipal-6cada70dcd77");
        // Another description, or another model, is another voice.
        assert_ne!(made_voice_name("voxcpm2", d), made_voice_name("voxcpm2", "An old man, slow and calm."));
        assert_ne!(made_voice_name("voxcpm2", d), made_voice_name("voxcpm2-npu", d));
        // A name hemera's library takes: lowercase letters, digits and dashes.
        assert!(made_voice_name("voxcpm2", d).chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
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
