//! An extension's languages: `_locales/<lang>/messages.json`, `i18n.getMessage`, and the
//! `__MSG_name__` its manifest and style sheets are written with -- as Chrome and Firefox do it.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

/// One language's messages, by name in lower case (names are not case-sensitive).
#[derive(Debug, Clone, Default)]
pub struct Messages {
    /// The languages they come from, best first (`en_GB`, `en`, the default).
    pub chain: Vec<HashMap<String, Message>>,
    /// The language chosen, as `i18n.getUILanguage()` says it (`en-GB`).
    pub ui_language: String,
}

#[derive(Debug, Clone, Default)]
pub struct Message {
    pub text: String,
    /// `$name$` placeholders: their content (`$1`, or text).
    pub placeholders: HashMap<String, String>,
}

fn read_language(root: &Path, language: &str) -> Option<HashMap<String, Message>> {
    let text = std::fs::read_to_string(root.join("_locales").join(language).join("messages.json")).ok()?;
    let json: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let map = json.as_object()?;
    Some(
        map.iter()
            .filter_map(|(name, m)| {
                let text = m.get("message")?.as_str()?.to_string();
                let placeholders = m
                    .get("placeholders")
                    .and_then(Value::as_object)
                    .map(|ps| ps.iter().filter_map(|(k, v)| Some((k.to_lowercase(), v.get("content")?.as_str()?.to_string()))).collect())
                    .unwrap_or_default();
                Some((name.to_lowercase(), Message { text, placeholders }))
            })
            .collect(),
    )
}

impl Messages {
    /// The messages of the extension in `root`, for the wearer's language (`en_US`, `de`...).
    pub fn load(root: &Path, ui_language: &str, default_locale: Option<&str>) -> Messages {
        let language = ui_language.replace('-', "_");
        let mut names = vec![language.clone()];
        if let Some((base, _)) = language.split_once('_') {
            names.push(base.to_string());
        }
        if let Some(d) = default_locale {
            names.push(d.to_string());
        }
        let mut chain = Vec::new();
        let mut seen = Vec::new();
        for name in names {
            if seen.contains(&name) {
                continue;
            }
            seen.push(name.clone());
            if let Some(messages) = read_language(root, &name) {
                chain.push(messages);
            }
        }
        Messages { chain, ui_language: ui_language.replace('_', "-") }
    }

    fn find(&self, name: &str) -> Option<&Message> {
        let name = name.to_lowercase();
        self.chain.iter().find_map(|m| m.get(&name))
    }

    /// `i18n.getMessage(name, substitutions)`: "" for a message there is not.
    pub fn get(&self, name: &str, substitutions: &[String], extension_id: &str) -> String {
        match name {
            "@@extension_id" => return extension_id.to_string(),
            "@@ui_locale" => return self.ui_language.replace('-', "_"),
            "@@bidi_dir" => return "ltr".into(),
            "@@bidi_reversed_dir" => return "rtl".into(),
            "@@bidi_start_edge" => return "left".into(),
            "@@bidi_end_edge" => return "right".into(),
            _ => {}
        }
        let Some(message) = self.find(name) else { return String::new() };
        let numbered = |text: &str| -> String {
            let mut out = String::new();
            let mut chars = text.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '$' {
                    match chars.peek() {
                        Some('$') => {
                            chars.next();
                            out.push('$');
                        }
                        Some(d) if d.is_ascii_digit() => {
                            let n = d.to_digit(10).unwrap() as usize;
                            chars.next();
                            if n >= 1 {
                                out.push_str(substitutions.get(n - 1).map(String::as_str).unwrap_or(""));
                            }
                        }
                        _ => out.push('$'),
                    }
                } else {
                    out.push(c);
                }
            }
            out
        };
        // Named placeholders first ($name$, case aside), then $1..$9 and $$.
        let mut text = String::new();
        let mut rest = message.text.as_str();
        while let Some(start) = rest.find('$') {
            let after = &rest[start + 1..];
            match after.find('$') {
                Some(end) if end > 0 && after[..end].chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '@') => {
                    let name = after[..end].to_lowercase();
                    if let Some(content) = message.placeholders.get(&name) {
                        // Its content as written: its $1..$9 are given below, with the rest.
                        text.push_str(&rest[..start]);
                        text.push_str(content);
                        rest = &after[end + 1..];
                        continue;
                    }
                    text.push_str(&rest[..start + 1]);
                    rest = after;
                }
                _ => {
                    text.push_str(&rest[..start + 1]);
                    rest = after;
                }
            }
        }
        text.push_str(rest);
        numbered(&text)
    }

    /// Every message, for an extension context's `i18n.getMessage` to answer itself.
    pub fn table(&self, extension_id: &str) -> serde_json::Map<String, Value> {
        let mut names: Vec<&String> = self.chain.iter().flat_map(|m| m.keys()).collect();
        names.sort();
        names.dedup();
        let mut out = serde_json::Map::new();
        for name in names {
            let m = self.find(name).cloned().unwrap_or_default();
            out.insert(
                name.clone(),
                serde_json::json!({ "message": m.text, "placeholders": m.placeholders }),
            );
        }
        out.insert("@@extension_id".into(), serde_json::json!({ "message": extension_id, "placeholders": {} }));
        out.insert("@@ui_locale".into(), serde_json::json!({ "message": self.ui_language.replace('-', "_"), "placeholders": {} }));
        out
    }
}

/// `__MSG_name__` in `text` given by `message`; unknown ones become "" as in Chrome.
pub fn substitute(text: &str, message: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("__MSG_") {
        let after = &rest[start + 6..];
        match after.find("__") {
            Some(end) if after[..end].chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '@') => {
                out.push_str(&rest[..start]);
                out.push_str(&message(&after[..end]).unwrap_or_default());
                rest = &after[end + 2..];
            }
            _ => {
                out.push_str(&rest[..start + 6]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("spatimesh-locale-{}", crate::store::new_id()));
        for (lang, json) in [
            ("en", r#"{"greeting":{"message":"Hello, $who$! You have $1 new $$ messages.","placeholders":{"who":{"content":"$2"}}},
                      "only_en":{"message":"English"}}"#),
            ("de", r#"{"greeting":{"message":"Hallo, $WHO$!","placeholders":{"who":{"content":"$1"}}}}"#),
            ("pt_BR", r#"{"only_br":{"message":"Brasil"}}"#),
        ] {
            std::fs::create_dir_all(dir.join("_locales").join(lang)).unwrap();
            std::fs::write(dir.join("_locales").join(lang).join("messages.json"), json).unwrap();
        }
        dir
    }

    #[test]
    fn messages_as_chrome_gives_them() {
        let dir = folder();
        let en = Messages::load(&dir, "en-US", Some("en"));
        assert_eq!(en.get("greeting", &["3".into(), "Ann".into()], "x"), "Hello, Ann! You have 3 new $ messages.");
        assert_eq!(en.get("GREETING", &["3".into(), "Ann".into()], "x"), "Hello, Ann! You have 3 new $ messages.", "names are not case-sensitive");
        assert_eq!(en.get("missing", &[], "x"), "");
        assert_eq!(en.get("@@extension_id", &[], "abc"), "abc");
        let de = Messages::load(&dir, "de", Some("en"));
        assert_eq!(de.get("greeting", &["Bea".into()], "x"), "Hallo, Bea!");
        assert_eq!(de.get("only_en", &[], "x"), "English", "the default language fills in");
        let br = Messages::load(&dir, "pt-BR", Some("en"));
        assert_eq!(br.get("only_br", &[], "x"), "Brasil");
        assert_eq!(br.ui_language, "pt-BR");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn manifest_and_style_sheet_text() {
        let m = |n: &str| (n == "name").then(|| "Tidy".to_string());
        assert_eq!(substitute("__MSG_name__ (beta)", &m), "Tidy (beta)");
        assert_eq!(substitute("a __MSG_unknown__ b", &m), "a  b");
        assert_eq!(substitute("no messages __MSG_ here", &m), "no messages __MSG_ here");
    }
}
