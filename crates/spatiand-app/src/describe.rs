//! A window's account of itself, for the room's speech (`Event::Describe`).
//!
//! The JSON the room reads (`spatiand_xr_surface_v1.describe`; `spatiand_voice::Description`
//! is the reading end). Written by hand rather than through serde, which no application needs
//! for anything else.

/// What a window shows. Every field may be left empty.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Description {
    /// `terminal`, `web`, `files`, `video`, `viewer`, `chat`, `text`.
    pub kind: String,
    /// The application, by its name.
    pub app: String,
    pub title: String,
    /// What it shows, as text: a terminal's screen, a page's words, the field being typed in.
    pub text: String,
    /// Names and words likely to be said: file names, a page's headings.
    pub vocabulary: Vec<String>,
    /// A terminal's or a file manager's folder.
    pub directory: String,
    /// A terminal's last commands, the last one last.
    pub commands: Vec<String>,
}

impl Description {
    pub fn to_json(&self) -> String {
        let list = |items: &[String]| format!("[{}]", items.iter().map(|i| quoted(i)).collect::<Vec<_>>().join(","));
        format!(
            "{{\"kind\":{},\"app\":{},\"title\":{},\"text\":{},\"vocabulary\":{},\"directory\":{},\"commands\":{}}}",
            quoted(&self.kind),
            quoted(&self.app),
            quoted(&self.title),
            quoted(&self.text),
            list(&self.vocabulary),
            quoted(&self.directory),
            list(&self.commands),
        )
    }
}

fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_is_json() {
        let d = Description {
            kind: "terminal".into(),
            text: "a \"quote\"\n\tand a tab\u{1}".into(),
            vocabulary: vec!["Cargo.toml".into(), "back\\slash".into()],
            ..Default::default()
        };
        assert_eq!(
            d.to_json(),
            r#"{"kind":"terminal","app":"","title":"","text":"a \"quote\"\n\tand a tab\u0001","vocabulary":["Cargo.toml","back\\slash"],"directory":"","commands":[]}"#
        );
    }
}
