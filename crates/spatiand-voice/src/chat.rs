//! A conversation with a model through hemera, as it is written: words, thinking and tool calls
//! as they come, so that speech can start on the first sentence and the wearer can watch (or
//! hear) a larger model think.
//!
//! OpenAI's streamed chat (`stream: true`): server-sent events, each a `chat.completion.chunk`
//! whose delta carries `content`, `reasoning_content` (what llama.cpp calls a model's thinking)
//! or pieces of `tool_calls`, the arguments arriving a few characters at a time by index.

use std::io::{BufRead, BufReader};
use std::time::Duration;

use serde_json::{json, Value};

use crate::hemera::Hemera;

/// What to ask.
#[derive(Clone, Debug)]
pub struct Ask {
    pub model: String,
    /// OpenAI's messages, as they go on the wire (an image is a content part).
    pub messages: Vec<Value>,
    /// OpenAI's tools: `{"type": "function", "function": {...}}`.
    pub tools: Vec<Value>,
    pub max_tokens: u32,
    pub temperature: f32,
    /// Whether the model may think before it answers. Off is quick; on is for the larger model.
    pub thinking: bool,
}

/// A piece of the answer, as it arrives.
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    Text(String),
    Thinking(String),
    /// A tool call has begun: which, by name.
    Calling(String),
}

/// One call of a tool, whole.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// JSON, as the model wrote it.
    pub arguments: String,
}

/// The answer, whole.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Answer {
    pub text: String,
    pub thinking: String,
    pub calls: Vec<ToolCall>,
    /// Stopped by whoever was listening, not by the model.
    pub cut: bool,
}

impl Answer {
    /// The answer as a message for the conversation: what the model said and called.
    pub fn message(&self) -> Value {
        let mut m = json!({"role": "assistant", "content": self.text});
        if !self.calls.is_empty() {
            m["tool_calls"] = Value::Array(
                self.calls
                    .iter()
                    .map(|c| json!({"id": c.id, "type": "function", "function": {"name": c.name, "arguments": c.arguments}}))
                    .collect(),
            );
        }
        m
    }
}

/// Ask, and hear the answer as it is written: `heard` is given each piece, and stops the answer
/// by returning `false`.
pub fn converse(hemera: &Hemera, ask: &Ask, mut heard: impl FnMut(&Piece) -> bool) -> Result<Answer, String> {
    let mut request = json!({
        "model": ask.model,
        "messages": ask.messages,
        "stream": true,
        "max_tokens": ask.max_tokens,
        "temperature": ask.temperature,
        "chat_template_kwargs": {"enable_thinking": ask.thinking},
    });
    if !ask.tools.is_empty() {
        request["tools"] = Value::Array(ask.tools.clone());
    }
    let reply = hemera.post_stream("/v1/chat/completions", &request, Duration::from_secs(600))?;
    let mut answer = Answer::default();
    let mut calls: Vec<ToolCall> = Vec::new();
    for line in BufReader::new(reply).lines() {
        let line = line.map_err(|e| format!("hemera's answer broke off: {e}"))?;
        let Some(data) = line.strip_prefix("data:").map(str::trim) else { continue };
        if data == "[DONE]" {
            break;
        }
        let Ok(event) = serde_json::from_str::<Value>(data) else { continue };
        if let Some(error) = event.get("error") {
            return Err(error.as_str().map(str::to_string).unwrap_or_else(|| error.to_string()));
        }
        let Some(delta) = event.pointer("/choices/0/delta") else { continue };
        let mut pieces = Vec::new();
        if let Some(t) = delta.get("reasoning_content").and_then(Value::as_str).filter(|t| !t.is_empty()) {
            answer.thinking.push_str(t);
            pieces.push(Piece::Thinking(t.to_string()));
        }
        if let Some(t) = delta.get("content").and_then(Value::as_str).filter(|t| !t.is_empty()) {
            answer.text.push_str(t);
            pieces.push(Piece::Text(t.to_string()));
        }
        for call in delta.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
            let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            while calls.len() <= index {
                calls.push(ToolCall::default());
            }
            let c = &mut calls[index];
            if let Some(id) = call.get("id").and_then(Value::as_str) {
                c.id = id.to_string();
            }
            if let Some(name) = call.pointer("/function/name").and_then(Value::as_str).filter(|n| !n.is_empty()) {
                c.name = name.to_string();
                pieces.push(Piece::Calling(name.to_string()));
            }
            if let Some(a) = call.pointer("/function/arguments").and_then(Value::as_str) {
                c.arguments.push_str(a);
            }
        }
        for piece in &pieces {
            if !heard(piece) {
                answer.cut = true;
                answer.calls = calls.into_iter().filter(|c| !c.name.is_empty()).collect();
                return Ok(answer);
            }
        }
    }
    // A model that wrote its tool call as text, `<tool_call>{...}</tool_call>`, as small ones
    // sometimes do, meant it all the same.
    if calls.is_empty() {
        if let Some((call, rest)) = written_call(&answer.text) {
            calls.push(call);
            answer.text = rest;
        }
    }
    for (i, c) in calls.iter_mut().enumerate() {
        if c.id.is_empty() {
            c.id = format!("call_{i}");
        }
        if c.arguments.trim().is_empty() {
            c.arguments = "{}".into();
        }
    }
    answer.calls = calls.into_iter().filter(|c| !c.name.is_empty()).collect();
    Ok(answer)
}

/// A tool call written into the text as Qwen's template writes it, and the text without it.
fn written_call(text: &str) -> Option<(ToolCall, String)> {
    let start = text.find("<tool_call>")?;
    let end = text[start..].find("</tool_call>").map(|e| start + e)?;
    let body: Value = serde_json::from_str(text[start + "<tool_call>".len()..end].trim()).ok()?;
    let name = body.get("name")?.as_str()?.to_string();
    let arguments = body.get("arguments").map(|a| a.to_string()).unwrap_or_else(|| "{}".into());
    let rest = format!("{}{}", &text[..start], &text[end + "</tool_call>".len()..]).trim().to_string();
    Some((ToolCall { id: String::new(), name, arguments }, rest))
}

/// A tool for the model: a name, what it is for, and its parameters as JSON Schema properties
/// (`required` lists the ones it must give).
pub fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": {"type": "object", "properties": properties, "required": required},
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_written_as_text_is_a_call() {
        let (call, rest) = written_call("Siap.\n<tool_call>\n{\"name\": \"open_app\", \"arguments\": {\"name\": \"SpatiQuill\"}}\n</tool_call>").unwrap();
        assert_eq!(call.name, "open_app");
        assert_eq!(call.arguments, "{\"name\":\"SpatiQuill\"}");
        assert_eq!(rest, "Siap.");
    }

    #[test]
    fn an_answer_becomes_a_message_with_its_calls() {
        let a = Answer { text: "ok".into(), calls: vec![ToolCall { id: "c1".into(), name: "recentre".into(), arguments: "{}".into() }], ..Default::default() };
        let m = a.message();
        assert_eq!(m["tool_calls"][0]["function"]["name"], "recentre");
        assert_eq!(m["role"], "assistant");
    }
}
