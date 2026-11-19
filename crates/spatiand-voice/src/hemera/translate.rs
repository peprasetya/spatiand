//! A picture's text translated between the wearer's two languages (the owner, 2026-11-19: "the
//! selection in primary, translate to secondary. other, translate to primary... anything translate
//! to english... if english translate to indonesia").
//!
//! One chat request to the AI settings' everyday model (`/v1/chat/completions`), thinking off:
//! the model is told the rule and gives the translation only, its lines and its digits as they
//! were -- an ID number translated is the same number. Called off as a reading is ([`Cancel`]).

use std::time::Duration;

use serde_json::{json, Value};

use super::ocr::{post, refusal, Cancel, Failure};
use super::Hemera;

/// How long a translation is given.
const WITHIN: Duration = Duration::from_secs(60);

/// What the model is told: text in the `first` language goes into the `second`, anything else
/// into the `first` (two-letter names, as the AI settings keep them).
pub fn instruction(first: &str, second: &str) -> String {
    let (first, second) = (spatiand_ai::language_name(first), spatiand_ai::language_name(second));
    format!(
        "Translate the text you are given. If the text is in {first}, translate it into {second}; otherwise translate it into {first}. \
         Answer with the translation only: no quotation marks, notes or explanations, nothing before or after it. \
         Keep every line break where it is, and every digit, number, code, name and sign exactly as it is written."
    )
}

/// The request, as hemera is sent it.
pub fn request(model: &str, text: &str, first: &str, second: &str) -> Value {
    // Enough for the translation of a page, and a little over what a language that takes more
    // words than the other needs; never without end.
    let most = (text.chars().count() * 4 + 256).min(8192);
    json!({
        "model": model,
        "messages": [
            {"role": "system", "content": instruction(first, second)},
            {"role": "user", "content": text},
        ],
        "temperature": 0,
        "max_tokens": most,
        "stream": false,
        "chat_template_kwargs": {"enable_thinking": false},
    })
}

/// The translation, from the model's reply: its text, trimmed, and a pair of quotation marks
/// round all of it taken off (a small model's habit, told not to).
pub fn translation_of(reply: &Value) -> Option<String> {
    let text = reply.pointer("/choices/0/message/content").and_then(Value::as_str)?.trim();
    let text = text.strip_prefix("<think>").and_then(|t| t.split_once("</think>").map(|(_, after)| after.trim())).unwrap_or(text);
    let unquoted = [('"', '"'), ('\u{201C}', '\u{201D}'), ('\u{300C}', '\u{300D}')]
        .iter()
        .find_map(|(open, close)| text.strip_prefix(*open).and_then(|t| t.strip_suffix(*close)).filter(|inner| !inner.contains(*open) && !inner.contains(*close)));
    Some(unquoted.unwrap_or(text).trim().to_string()).filter(|t| !t.is_empty())
}

impl Hemera {
    /// `text` translated by the rule ([`instruction`]) with `model`. Logged: how long it was,
    /// both ways, and how long it took. Never the text.
    pub fn translate(&self, model: &str, text: &str, first: &str, second: &str, cancel: &Cancel) -> Result<String, Failure> {
        let started = std::time::Instant::now();
        let body = request(model, text, first, second).to_string();
        let reply = post(self, "/v1/chat/completions", "application/json", body.as_bytes(), WITHIN, cancel)?;
        if reply.status >= 400 {
            return Err(refusal(reply.status, &reply.body));
        }
        let json: Value = serde_json::from_slice(&reply.body).map_err(|e| Failure::Unreadable(format!("hemera's answer was not readable ({e})")))?;
        let translated = translation_of(&json).ok_or_else(|| Failure::Unreadable("hemera's answer had no text in it".into()))?;
        log::info!("translate: {} characters into {} with {model} in {:.2} s", text.chars().count(), translated.chars().count(), started.elapsed().as_secs_f32());
        Ok(translated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rule_names_the_two_languages_each_way() {
        let rule = instruction("en", "id");
        assert!(rule.contains("If the text is in English, translate it into Indonesian; otherwise translate it into English."), "{rule}");
        let rule = instruction("id", "ja");
        assert!(rule.contains("If the text is in Indonesian, translate it into Japanese; otherwise translate it into Indonesian."), "{rule}");
        // Its lines and digits as they are, and the translation alone.
        assert!(rule.contains("line break") && rule.contains("digit") && rule.contains("translation only"));
        // A language the list does not have is named by its code, not dropped.
        assert!(instruction("xx", "en").contains("If the text is in xx, translate it into English"));
    }

    #[test]
    fn the_request_is_the_everyday_model_thinking_off() {
        let asked = request("neohorse-1:4b", "NIK\n3171234567890001", "en", "id");
        assert_eq!(asked["model"], "neohorse-1:4b");
        assert_eq!(asked["messages"][1]["content"], "NIK\n3171234567890001");
        assert_eq!(asked["messages"][0]["role"], "system");
        assert_eq!(asked["temperature"], 0);
        assert_eq!(asked["chat_template_kwargs"]["enable_thinking"], false);
        assert!(asked["max_tokens"].as_u64().unwrap() >= 256);
        assert_eq!(request("m", &"a".repeat(100_000), "en", "id")["max_tokens"], 8192);
    }

    #[test]
    fn the_translation_is_the_model_s_text_without_quotes_round_it() {
        let reply = |text: &str| json!({"choices": [{"message": {"content": text}}]});
        assert_eq!(translation_of(&reply("  Selamat pagi\nDunia \n")).as_deref(), Some("Selamat pagi\nDunia"));
        assert_eq!(translation_of(&reply("\"Selamat pagi\"")).as_deref(), Some("Selamat pagi"));
        // Quotation marks inside are the text's own.
        assert_eq!(translation_of(&reply("\"Buka\" atau \"tutup\"")).as_deref(), Some("\"Buka\" atau \"tutup\""));
        assert_eq!(translation_of(&reply("<think>\n</think>\n\nGood morning")).as_deref(), Some("Good morning"));
        assert_eq!(translation_of(&reply("  ")), None);
        assert_eq!(translation_of(&json!({"choices": []})), None);
    }
}
