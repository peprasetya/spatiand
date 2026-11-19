//! A picture's text translated between the wearer's two languages (the owner, 2026-11-19: "the
//! selection in primary, translate to secondary. other, translate to primary... anything translate
//! to english... if english translate to indonesia").
//!
//! **The rule is decided here, not by the model.** The AI settings' everyday model (a 4B model)
//! was told the rule itself -- "if the text is in English, translate it into Indonesian;
//! otherwise into English" -- and gave Indonesian back unchanged, a lone "Diskon" as it was, and
//! Japanese in Indonesian. Asked only which language the text is in (a two-letter code), and then
//! to translate into the one language that answer makes it, it went the right way for every test
//! text. So two requests, thinking off: [`asked_language`], then [`asked_translation`].
//!
//! **Lines and digits as they were.** The lines go numbered and come back numbered, each put back
//! where its line was (blocks an empty line apart, as copied); digits are never to be changed -- an
//! ID number translated is the same number. Called off as a reading is ([`Cancel`]).

use std::time::Duration;

use serde_json::{json, Value};

use super::ocr::{post, refusal, Cancel, Failure};
use super::Hemera;

/// How long each of the two requests is given.
const WITHIN: Duration = Duration::from_secs(60);

/// The language the translation goes into, by the rule: text in the `first` language goes into the
/// `second`, anything else -- the second, or a third -- into the `first`. Two-letter names, as the AI
/// settings keep them; `written` is the model's word for the text's language.
pub fn target(written: &str, first: &str, second: &str) -> String {
    let written = written.trim().trim_matches(|c: char| !c.is_alphabetic()).to_ascii_lowercase();
    let code: String = written.chars().take(2).collect();
    // A name rather than a code, now and then: "Indonesian", "Bahasa".
    let named = spatiand_ai::LANGUAGES.iter().find(|(_, name)| name.eq_ignore_ascii_case(&written)).map(|(c, _)| c.to_string());
    let written = named.unwrap_or(if written == "bahasa" { "id".into() } else { code });
    if written.eq_ignore_ascii_case(first) { second.to_string() } else { first.to_string() }
}

/// The first question: which language the text is in.
pub fn asked_language(model: &str, text: &str) -> Value {
    json!({
        "model": model,
        "messages": [
            {"role": "system", "content": "Which language is the text written in? Answer with its two-letter ISO 639-1 code only (en, id, ja, zh, ar, ...), nothing else."},
            {"role": "user", "content": text},
        ],
        "temperature": 0,
        "max_tokens": 8,
        "stream": false,
        "chat_template_kwargs": {"enable_thinking": false},
    })
}

/// The text's lines that have something in them, numbered from 1: `1| first line`.
fn numbered(text: &str) -> String {
    text.lines().filter(|l| !l.trim().is_empty()).enumerate().map(|(i, l)| format!("{}| {}", i + 1, l.trim())).collect::<Vec<_>>().join("\n")
}

/// The second question: the text, its lines numbered, into `target` (a two-letter name).
pub fn asked_translation(model: &str, text: &str, target: &str) -> Value {
    let into = spatiand_ai::language_name(target);
    // Enough for the translation of a page, and a little over what a language that takes more
    // words than the other needs; never without end.
    let most = (text.chars().count() * 4 + 256).min(8192);
    json!({
        "model": model,
        "messages": [
            {"role": "system", "content": format!(
                "Translate the text into {into}, line by line. Its lines are numbered: answer with the same numbered lines, \"<number>| <translation>\", \
                 one for each line, nothing else. Translate every word, a form's labels and field names too. Never change a digit: numbers, times, dates \
                 and codes stay exactly as they are written (3 pm stays 3 pm)."
            )},
            {"role": "user", "content": numbered(text)},
        ],
        "temperature": 0,
        "max_tokens": most,
        "stream": false,
        "chat_template_kwargs": {"enable_thinking": false},
    })
}

/// A model's reply's text, thinking taken off.
fn reply_text(reply: &Value) -> Option<&str> {
    let text = reply.pointer("/choices/0/message/content").and_then(Value::as_str)?.trim();
    Some(text.strip_prefix("<think>").and_then(|t| t.split_once("</think>").map(|(_, after)| after.trim())).unwrap_or(text))
}

/// The translation of `text`, from the numbered lines the model gave back: each line where its
/// line was, empty lines where there were empty lines; a line it left out, as it was. A reply that
/// is not numbered lines at all is taken as the translation, quotation marks round it taken off.
pub fn translation_of(reply: &Value, text: &str) -> Option<String> {
    let said = reply_text(reply)?;
    let mut lines = std::collections::HashMap::new();
    for line in said.lines() {
        if let Some((n, rest)) = line.split_once('|') {
            if let Ok(n) = n.trim().parse::<usize>() {
                lines.insert(n, rest.trim().to_string());
            }
        }
    }
    let wanted = text.lines().filter(|l| !l.trim().is_empty()).count();
    if wanted > 0 && lines.len() * 2 >= wanted {
        let mut n = 0;
        let back: Vec<String> = text
            .lines()
            .map(|l| {
                if l.trim().is_empty() {
                    return String::new();
                }
                n += 1;
                lines.get(&n).cloned().unwrap_or_else(|| l.trim().to_string())
            })
            .collect();
        return Some(back.join("\n").trim().to_string()).filter(|t| !t.is_empty());
    }
    let unquoted = [('"', '"'), ('\u{201C}', '\u{201D}'), ('\u{300C}', '\u{300D}')]
        .iter()
        .find_map(|(open, close)| said.strip_prefix(*open).and_then(|t| t.strip_suffix(*close)).filter(|inner| !inner.contains(*open) && !inner.contains(*close)));
    Some(unquoted.unwrap_or(said).trim().to_string()).filter(|t| !t.is_empty())
}

impl Hemera {
    /// A chat request on a connection of its own, its answer as JSON.
    fn ask_alone(&self, request: &Value, cancel: &Cancel) -> Result<Value, Failure> {
        let reply = post(self, "/v1/chat/completions", "application/json", request.to_string().as_bytes(), WITHIN, cancel)?;
        if reply.status >= 400 {
            return Err(refusal(reply.status, &reply.body));
        }
        serde_json::from_slice(&reply.body).map_err(|e| Failure::Unreadable(format!("hemera's answer was not readable ({e})")))
    }

    /// `text` translated by the rule ([`target`]) with `model`: the language it went into (two
    /// letters) and the translation. Logged: how long it was, both ways, which way it went, and
    /// how long it took. Never the text.
    pub fn translate(&self, model: &str, text: &str, first: &str, second: &str, cancel: &Cancel) -> Result<(String, String), Failure> {
        let started = std::time::Instant::now();
        let written = self.ask_alone(&asked_language(model, text), cancel)?;
        let written = reply_text(&written).unwrap_or_default().to_string();
        let into = target(&written, first, second);
        let reply = self.ask_alone(&asked_translation(model, text, &into), cancel)?;
        let translated = translation_of(&reply, text).ok_or_else(|| Failure::Unreadable("hemera's answer had no text in it".into()))?;
        log::info!(
            "translate: {} characters, written in {:?}, into {into}: {} characters, with {model} in {:.2} s",
            text.chars().count(),
            written.chars().take(12).collect::<String>(),
            translated.chars().count(),
            started.elapsed().as_secs_f32()
        );
        Ok((into, translated))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rule_sends_the_first_language_to_the_second_and_anything_else_to_the_first() {
        // The owner's two: English into Indonesian, anything else into English.
        assert_eq!(target("en", "en", "id"), "id");
        assert_eq!(target("id", "en", "id"), "en");
        assert_eq!(target("ja", "en", "id"), "en");
        assert_eq!(target("ar", "en", "id"), "en");
        // Said as a model says it now and then.
        assert_eq!(target(" EN.\n", "en", "id"), "id");
        assert_eq!(target("English", "en", "id"), "id");
        assert_eq!(target("Bahasa", "id", "en"), "en");
        assert_eq!(target("Indonesian", "id", "ja"), "ja");
        // The other way round: Indonesian first.
        assert_eq!(target("id", "id", "en"), "en");
        assert_eq!(target("en", "id", "en"), "id");
        // Nothing said: the first, as for anything not in it.
        assert_eq!(target("", "en", "id"), "en");
    }

    #[test]
    fn the_questions_are_the_everyday_model_thinking_off() {
        let asked = asked_language("neohorse-1:4b", "Diskon");
        assert_eq!(asked["model"], "neohorse-1:4b");
        assert_eq!(asked["messages"][1]["content"], "Diskon");
        assert_eq!(asked["chat_template_kwargs"]["enable_thinking"], false);
        let asked = asked_translation("neohorse-1:4b", "NIK\n: 3171234567890001\n\nNama", "en");
        assert!(asked["messages"][0]["content"].as_str().unwrap().contains("into English, line by line"));
        assert_eq!(asked["messages"][1]["content"], "1| NIK\n2| : 3171234567890001\n3| Nama");
        assert_eq!(asked["temperature"], 0);
        assert_eq!(asked_translation("m", &"a".repeat(100_000), "en")["max_tokens"], 8192);
    }

    #[test]
    fn numbered_lines_come_back_where_their_lines_were() {
        let reply = |text: &str| json!({"choices": [{"message": {"content": text}}]});
        let text = "Kode verifikasi Anda: 482913.\nJangan berikan kode ini kepada siapa pun.\n\nSaldo akhir";
        let back = translation_of(&reply("1| Your verification code: 482913.\n2| Do not give this code to anyone.\n3| Final balance"), text);
        assert_eq!(back.as_deref(), Some("Your verification code: 482913.\nDo not give this code to anyone.\n\nFinal balance"));
        // A line left out is given as it was; out of order is put in order.
        let back = translation_of(&reply("3| Final balance\n1| Your verification code: 482913."), text);
        assert_eq!(back.as_deref(), Some("Your verification code: 482913.\nJangan berikan kode ini kepada siapa pun.\n\nFinal balance"));
        // Not numbered at all: the reply as it is, quotation marks round it taken off.
        assert_eq!(translation_of(&reply("\"Discount\""), "Diskon").as_deref(), Some("Discount"));
        assert_eq!(translation_of(&reply("<think>\n</think>\n\n1| Good morning"), "Selamat pagi").as_deref(), Some("Good morning"));
        assert_eq!(translation_of(&reply("  "), "a"), None);
        assert_eq!(translation_of(&json!({"choices": []}), "a"), None);
    }

    /// The rule, by the real hemera and its everyday model (`HEMERA`, else deepmagpie's), each way:
    /// Indonesian into English, English into Indonesian, anything else into English -- its lines
    /// and digits as they were. By hand: `cargo test -p spatiand-voice -- --ignored translate --nocapture`.
    #[test]
    #[ignore]
    fn the_everyday_model_translates_each_way_and_keeps_lines_and_digits() {
        let hemera = Hemera::new(&std::env::var("HEMERA").unwrap_or_else(|_| "http://deepmagpie:12434".into()));
        let model = std::env::var("MODEL").unwrap_or_else(|_| "neohorse-1:4b".into());
        let cases = [
            ("Kode verifikasi Anda: 482913.\nJangan berikan kode ini kepada siapa pun.", "en", "482913"),
            ("Meeting moved to 3 pm, see you\nat the lobby.", "id", "3"),
            ("春の朝、川沿いの道を歩くと、\n桜の花びらが静かに水面へ落ちていく。", "en", ""),
            ("NIK\n: 3171234567890001\nNama\n: RINA KARTIKA SARI", "en", "3171234567890001"),
            ("Diskon", "en", ""),
            ("مرحبا بكم في المعرض الدولي للكتاب", "en", ""),
            ("Open 24 hours", "id", "24"),
        ];
        for (text, into, digits) in cases {
            let started = std::time::Instant::now();
            let (went, out) = hemera.translate(&model, text, "en", "id", &Cancel::new()).unwrap();
            println!("{:.2} s into {went}\n  {text:?}\n  -> {out:?}", started.elapsed().as_secs_f32());
            assert_eq!(went, into, "{text:?}");
            assert!(out.contains(digits), "its digits kept: {out:?}");
            assert_eq!(out.lines().count(), text.lines().count(), "its lines kept: {out:?}");
        }
    }
}
