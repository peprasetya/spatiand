//! hemera, as much of it as speech needs: what was said, and a quick word from a small model.
//!
//! OpenAI's API, as hemera speaks it (`docs/media-api.md` in the hemera repository). Blocking
//! calls, made from a thread of their own: a phrase is a second of work at most, and a thread
//! per dictation is cheaper to reason about than a runtime.
//!
//! hemera's errors are `{"error": "<message fit to show a user>"}`, so that message is what an
//! error here says.
//!
//! **No address, no hemera**: a fresh install's AI settings name no server, and a `Hemera` made
//! from an empty address fails every request at once with [`NOT_SET`], sending nothing anywhere.
//! What would ask by itself looks at [`Hemera::is_set`] first and does not ask.

use std::time::Duration;

use serde_json::{json, Value};

/// The text in pictures: read (`POST /v1/ocr`), each line, word and character with where it is.
pub mod ocr;
/// A picture's text, translated between the wearer's two languages.
pub mod translate;

/// What every request says when hemera's address is not set, in words the wearer understands,
/// fit to go inside a sentence ("The text could not be read: ..."): no full stop.
pub const NOT_SET: &str = "hemera's address is not set (Settings \u{2192} AI)";

/// One hemera server.
#[derive(Clone)]
pub struct Hemera {
    base: String,
    agent: ureq::Agent,
}

/// A message of a conversation, OpenAI's way.
#[derive(Clone, Debug)]
pub struct Message {
    pub role: &'static str,
    pub content: String,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Message {
        Message { role: "system", content: content.into() }
    }

    pub fn user(content: impl Into<String>) -> Message {
        Message { role: "user", content: content.into() }
    }
}

impl Hemera {
    /// `base` is the server's address, `http://name:12434`; a trailing slash is fine. Empty (or
    /// blank): no hemera -- every request fails at once with [`NOT_SET`], and nothing is sent.
    pub fn new(base: &str) -> Hemera {
        // Connections kept for the next request: over Wi-Fi each new one costs a round trip, and a
        // turn of the assistant's asks for a few at once.
        Hemera { base: base.trim().trim_end_matches('/').to_string(), agent: agent(4) }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// Whether there is a hemera to ask: its address is set.
    pub fn is_set(&self) -> bool {
        !self.base.is_empty()
    }

    /// `path` on this hemera -- or, with no address, [`NOT_SET`]: the one guard every request
    /// goes through before anything is sent.
    fn url(&self, path: &str) -> Result<String, String> {
        if self.is_set() {
            Ok(format!("{}{path}", self.base))
        } else {
            Err(NOT_SET.to_string())
        }
    }

    /// What was said in `pcm` (16-bit mono at `rate`).
    ///
    /// `language` only when one language is spoken: with it left out Whisper decides, which is
    /// what keeps a sentence that mixes Indonesian and English as it was said. Given `en`,
    /// Whisper *translated* the Indonesian half (`docs/ai-integrations.md`).
    pub fn transcribe(&self, model: &str, pcm: &[i16], rate: u32, language: Option<&str>, prompt: &str) -> Result<String, String> {
        Ok(self.transcribe_surely(model, pcm, rate, language, prompt)?.0)
    }

    /// As [`Hemera::transcribe`], with how sure Whisper was of its words, 0 to 1, on average: it
    /// writes words given noise too, but says of them that it barely believes them.
    pub fn transcribe_surely(&self, model: &str, pcm: &[i16], rate: u32, language: Option<&str>, prompt: &str) -> Result<(String, f32), String> {
        let boundary = "spatiand-voice-a7c3f9e1";
        let mut body = Vec::with_capacity(pcm.len() * 2 + 2048);
        let mut field = |name: &str, value: &str| {
            body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
        };
        field("model", model);
        field("temperature", "0");
        field("response_format", "verbose_json");
        if let Some(language) = language {
            field("language", language);
        }
        if !prompt.is_empty() {
            field("prompt", prompt);
        }
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"phrase.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(&wav(pcm, rate));
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let reply = self.post(
            "/v1/audio/transcriptions",
            &format!("multipart/form-data; boundary={boundary}"),
            &body,
            Duration::from_secs(30),
        )?;
        let text = reply.get("text").and_then(Value::as_str).unwrap_or_default().trim().to_string();
        let words: Vec<f64> = reply
            .get("segments")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|segment| segment.get("words").and_then(Value::as_array).into_iter().flatten())
            .filter(|word| word.get("word").and_then(Value::as_str).is_some_and(|w| w.chars().any(char::is_alphanumeric)))
            .filter_map(|word| word.get("probability").and_then(Value::as_f64))
            .collect();
        // Told nothing of its words, it is taken at its word.
        let sure = if words.is_empty() { 1.0 } else { words.iter().sum::<f64>() / words.len() as f64 };
        Ok((text, sure as f32))
    }

    /// A short answer from a text model, thinking off: the whole reply at once.
    pub fn complete(&self, model: &str, messages: &[Message], max_tokens: u32, timeout: Duration) -> Result<String, String> {
        let messages: Vec<Value> = messages.iter().map(|m| json!({"role": m.role, "content": m.content})).collect();
        let request = json!({
            "model": model,
            "messages": messages,
            "temperature": 0,
            "max_tokens": max_tokens,
            "stream": false,
            // Qwen's way of asking for an answer without a page of reasoning first; models
            // that do not think ignore it.
            "chat_template_kwargs": {"enable_thinking": false},
        });
        let reply = self.post("/v1/chat/completions", "application/json", request.to_string().as_bytes(), timeout)?;
        reply
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .ok_or_else(|| "hemera's answer had no text in it".to_string())
    }

    /// A GET of hemera's, as JSON.
    pub fn get_json(&self, path: &str) -> Result<Value, String> {
        let reply = answered(within(self.agent.get(&self.url(path)?), Duration::from_secs(5)).call(), &self.base)?;
        json_of(reply).map_err(|e| format!("hemera's answer was not readable: {e}"))
    }

    /// hemera's models of one kind (`text`, `transcription`, `speech`), by name.
    pub fn models(&self, kind: &str) -> Result<Vec<String>, String> {
        let reply = answered(within(self.agent.get(&self.url("/v1/models")?), Duration::from_secs(5)).call(), &self.base)?;
        let list: Value = json_of(reply).map_err(|e| format!("hemera's list of models was not readable: {e}"))?;
        Ok(list
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|m| m.pointer("/hemera/kind").and_then(Value::as_str) == Some(kind))
            .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string))
            .collect())
    }

    /// A POST of JSON, answered in JSON.
    pub fn post_json(&self, path: &str, request: &Value, timeout: Duration) -> Result<Value, String> {
        self.post(path, "application/json", request.to_string().as_bytes(), timeout)
    }

    /// A POST of JSON, asked again while hemera says it is still getting its model ready (503),
    /// after the wait its `Retry-After` gives -- `tries` times in all.
    pub fn post_json_waiting(&self, path: &str, request: &Value, timeout: Duration, tries: u32) -> Result<Value, String> {
        let url = self.url(path)?;
        let mut tried = 0;
        loop {
            tried += 1;
            let asked = self.agent.post(&url).header("Content-Type", "application/json");
            match within(asked, timeout).send(request.to_string().as_bytes()) {
                Ok(reply) if reply.status().as_u16() == 503 && tried < tries => {
                    let wait = reply.headers().get("Retry-After").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(10);
                    std::thread::sleep(Duration::from_secs(wait.clamp(1, 60)));
                }
                reply => return answered(reply, &self.base).and_then(|r| json_of(r).map_err(|e| format!("hemera's answer was not readable: {e}"))),
            }
        }
    }

    /// A DELETE of hemera's: what it holds of something made, let go.
    pub fn delete(&self, path: &str) -> Result<(), String> {
        answered(within(self.agent.delete(&self.url(path)?), Duration::from_secs(5)).call(), &self.base).map(|_| ())
    }

    /// A POST of a form (`multipart/form-data`): text fields, and one file as `file`. Answered in
    /// JSON.
    pub fn post_form(&self, path: &str, fields: &[(&str, &str)], file: (&str, &str, &[u8]), timeout: Duration) -> Result<Value, String> {
        let boundary = "spatiand-voice-5b1e0c47";
        let mut body = Vec::with_capacity(file.2.len() + 2048);
        for (name, value) in fields {
            body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
        }
        let (filename, content_type, bytes) = file;
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        self.post(path, &format!("multipart/form-data; boundary={boundary}"), &body, timeout)
    }

    /// A request whose answer is read as it comes (server-sent events): the body, to be read
    /// line by line. Dropping it stops the answer.
    pub fn post_stream(&self, path: &str, request: &Value, timeout: Duration) -> Result<Box<dyn std::io::Read + Send + Sync>, String> {
        let asked = self.agent.post(&self.url(path)?).header("Content-Type", "application/json").header("Accept", "text/event-stream");
        let reply = answered(within(asked, timeout).send(request.to_string()), &self.base)?;
        // No limit on what is read, as ureq 2's `into_reader` had none: speech is as long as it is.
        Ok(Box::new(reply.into_body().into_reader()))
    }

    fn post(&self, path: &str, content_type: &str, body: &[u8], timeout: Duration) -> Result<Value, String> {
        let request = self.agent.post(&self.url(path)?).header("Content-Type", content_type);
        let reply = answered(within(request, timeout).send(body), &self.base)?;
        json_of(reply).map_err(|e| format!("hemera's answer was not readable: {e}"))
    }
}

/// A client of hemera's, keeping `idle` connections for the next request. A request that may be
/// called off has one of its own, keeping none ([`ocr::Cancel`]): shutting it down closes that
/// request's connection, and hemera stops its work when its connection closes.
fn agent(idle: usize) -> ureq::Agent {
    ureq::Agent::config_builder()
        // Not reachable in this long is not reachable: away from home without Tailscale.
        .timeout_connect(Some(Duration::from_secs(3)))
        .max_idle_connections_per_host(idle)
        // hemera's refusals say why in their body (`failure`), which ureq 3 drops from the
        // error it makes of a 4xx or 5xx; so those are answers, read by `answered`.
        .http_status_as_error(false)
        // As ureq 2 did, which ureq 3 does not by itself: no proxy from the environment,
        // and four redirects followed and an error at the fifth (ureq 3: ten).
        .proxy(None)
        .max_redirects(4)
        .build()
        .into()
}

/// The request given `timeout` for all of it, from connecting to the answer's last byte: what
/// ureq 2's `Request::timeout` was.
fn within<B>(request: ureq::RequestBuilder<B>, timeout: Duration) -> ureq::RequestBuilder<B> {
    request.config().timeout_global(Some(timeout)).build()
}

/// The most of a JSON answer of hemera's that is read. ureq 2 read JSON with no limit; ureq 3
/// stops at 10 MB unless told, and hemera's answers carry speech as base64 (a minute of it is
/// some 4 MB), so the limit is set far past anything it sends.
const JSON_LIMIT: u64 = 1024 * 1024 * 1024;

fn json_of(reply: ureq::http::Response<ureq::Body>) -> Result<Value, ureq::Error> {
    reply.into_body().into_with_config().limit(JSON_LIMIT).read_json()
}

/// The answer, or what went wrong in words fit to show: hemera's own message when it gave one.
/// A 4xx or 5xx is an error here, as ureq 2 made it, with its body read for the message.
fn answered(reply: Result<ureq::http::Response<ureq::Body>, ureq::Error>, base: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    match reply {
        Ok(reply) if reply.status().as_u16() >= 400 => {
            let code = reply.status().as_u16();
            let said = json_of(reply).ok().and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string));
            Err(said.unwrap_or_else(|| format!("hemera answered {code}")))
        }
        Ok(reply) => Ok(reply),
        // A socket's own error without the "io: " ureq 3 puts in front of it.
        Err(ureq::Error::Io(e)) => Err(format!("hemera at {base} could not be reached ({e})")),
        Err(ureq::Error::Timeout(t)) => Err(format!("hemera at {base} could not be reached (no answer in time: {t})")),
        Err(e) => Err(format!("hemera at {base} could not be reached ({e})")),
    }
}

/// A WAV file of 16-bit mono samples: what Whisper is sent.
pub fn wav(pcm: &[i16], rate: u32) -> Vec<u8> {
    let data = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// The samples of a 16-bit mono WAV file, and its rate. For tests, and phrases kept on disk.
pub fn read_wav(bytes: &[u8]) -> Option<(Vec<i16>, u32)> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut at = 12;
    let (mut rate, mut channels, mut bits) = (0u32, 0u16, 0u16);
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
        let body = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
        if id == b"fmt " && body.len() >= 16 {
            channels = u16::from_le_bytes([body[2], body[3]]);
            rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
            bits = u16::from_le_bytes([body[14], body[15]]);
        } else if id == b"data" {
            if bits != 16 || channels == 0 {
                return None;
            }
            let frames: Vec<i16> = body
                .chunks_exact(2 * channels as usize)
                .map(|f| i16::from_le_bytes([f[0], f[1]]))
                .collect();
            return Some((frames, rate));
        }
        at += 8 + size + (size & 1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_no_address_every_request_fails_at_once_and_says_why() {
        for blank in ["", "  ", "/"] {
            let hemera = Hemera::new(blank);
            assert!(!hemera.is_set(), "{blank:?}");
            let started = std::time::Instant::now();
            let said = [
                hemera.transcribe("whisper-large-v3-turbo", &[0; 1600], 16_000, None, "").unwrap_err(),
                hemera.complete("neohorse-1:4b", &[Message::user("hello")], 8, Duration::from_secs(30)).unwrap_err(),
                hemera.get_json("/v1/ocr").unwrap_err(),
                hemera.models("text").unwrap_err(),
                hemera.post_json("/v1/motion", &json!({}), Duration::from_secs(30)).unwrap_err(),
                hemera.post_json_waiting("/v1/motion", &json!({}), Duration::from_secs(30), 5).unwrap_err(),
                hemera.delete("/v1/motion/1").unwrap_err(),
                hemera.post_form("/v1/audio/voices", &[("name", "a")], ("a.wav", "audio/wav", b"RIFF"), Duration::from_secs(30)).err().unwrap(),
                hemera.post_stream("/v1/chat/completions", &json!({}), Duration::from_secs(30)).err().unwrap(),
            ];
            assert!(said.iter().all(|s| s == NOT_SET), "{said:?}");
            // Nothing tried: no name looked up, no connection waited for.
            assert!(started.elapsed() < Duration::from_millis(200), "{:?}", started.elapsed());
        }
        // An address is used as it is given, its trailing slash and blanks off.
        let hemera = Hemera::new(" http://hemera.local:12434/ ");
        assert!(hemera.is_set());
        assert_eq!(hemera.base(), "http://hemera.local:12434");
        assert_eq!(hemera.url("/v1/models").unwrap(), "http://hemera.local:12434/v1/models");
    }

    #[test]
    fn a_wav_reads_back_as_written() {
        let pcm: Vec<i16> = (0..1000).map(|i| (i * 37 % 2000) as i16 - 1000).collect();
        let (back, rate) = read_wav(&wav(&pcm, 16_000)).unwrap();
        assert_eq!(rate, 16_000);
        assert_eq!(back, pcm);
    }
}
