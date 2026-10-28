//! hemera, as much of it as speech needs: what was said, and a quick word from a small model.
//!
//! OpenAI's API, as hemera speaks it (`docs/media-api.md` in the hemera repository). Blocking
//! calls, made from a thread of their own: a phrase is a second of work at most, and a thread
//! per dictation is cheaper to reason about than a runtime.
//!
//! hemera's errors are `{"error": "<message fit to show a user>"}`, so that message is what an
//! error here says.

use std::time::Duration;

use serde_json::{json, Value};

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
    /// `base` is the server's address, `http://deepmagpie:12434`; a trailing slash is fine.
    pub fn new(base: &str) -> Hemera {
        let agent = ureq::AgentBuilder::new()
            // Not reachable in this long is not reachable: away from home without Tailscale.
            .timeout_connect(Duration::from_secs(3))
            .build();
        Hemera { base: base.trim_end_matches('/').to_string(), agent }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// What was said in `pcm` (16-bit mono at `rate`).
    ///
    /// `language` only when one language is spoken: with it left out Whisper decides, which is
    /// what keeps a sentence that mixes Indonesian and English as it was said. Given `en`,
    /// Whisper *translated* the Indonesian half (`docs/ai-integrations.md`).
    pub fn transcribe(&self, model: &str, pcm: &[i16], rate: u32, language: Option<&str>, prompt: &str) -> Result<String, String> {
        let boundary = "spatiand-voice-a7c3f9e1";
        let mut body = Vec::with_capacity(pcm.len() * 2 + 2048);
        let mut field = |name: &str, value: &str| {
            body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
        };
        field("model", model);
        field("temperature", "0");
        field("response_format", "json");
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
        Ok(reply.get("text").and_then(Value::as_str).unwrap_or_default().trim().to_string())
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

    /// hemera's models of one kind (`text`, `transcription`, `speech`), by name.
    pub fn models(&self, kind: &str) -> Result<Vec<String>, String> {
        let reply = self
            .agent
            .get(&format!("{}/v1/models", self.base))
            .timeout(Duration::from_secs(5))
            .call()
            .map_err(|e| failure(e, &self.base))?;
        let list: Value = reply.into_json().map_err(|e| format!("hemera's list of models was not readable: {e}"))?;
        Ok(list
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|m| m.pointer("/hemera/kind").and_then(Value::as_str) == Some(kind))
            .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string))
            .collect())
    }

    fn post(&self, path: &str, content_type: &str, body: &[u8], timeout: Duration) -> Result<Value, String> {
        let reply = self
            .agent
            .post(&format!("{}{path}", self.base))
            .set("Content-Type", content_type)
            .timeout(timeout)
            .send_bytes(body)
            .map_err(|e| failure(e, &self.base))?;
        reply.into_json::<Value>().map_err(|e| format!("hemera's answer was not readable: {e}"))
    }
}

/// What went wrong, in words fit to show: hemera's own message when it gave one.
fn failure(error: ureq::Error, base: &str) -> String {
    match error {
        ureq::Error::Status(code, reply) => {
            let said = reply.into_json::<Value>().ok().and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string));
            said.unwrap_or_else(|| format!("hemera answered {code}"))
        }
        ureq::Error::Transport(t) => format!("hemera at {base} could not be reached ({t})"),
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
    fn a_wav_reads_back_as_written() {
        let pcm: Vec<i16> = (0..1000).map(|i| (i * 37 % 2000) as i16 - 1000).collect();
        let (back, rate) = read_wav(&wav(&pcm, 16_000)).unwrap();
        assert_eq!(rate, 16_000);
        assert_eq!(back, pcm);
    }
}
