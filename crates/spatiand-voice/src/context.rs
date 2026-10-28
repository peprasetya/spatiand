//! What the focused window is about, and Whisper's prompt made from it.
//!
//! A window describes itself (`spatiand_xr_surface_v1.describe`; Mac windows through
//! Accessibility; anything else by its title), and the same description serves dictation --
//! which words to lean towards -- and the assistant, reading a window.

use serde::{Deserialize, Serialize};

/// A window's account of itself, as JSON on the wire.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Description {
    /// `terminal`, `web`, `files`, `video`, `viewer`, `chat`, `text`; empty when not known.
    pub kind: String,
    /// The application, by its name.
    pub app: String,
    pub title: String,
    /// What it shows, as text: a terminal's screen, a page's words, a field being typed in.
    pub text: String,
    /// Names, commands and words likely to be said: file names, a page's headings.
    pub vocabulary: Vec<String>,
    /// A terminal's or a file manager's folder.
    pub directory: String,
    /// A terminal's last commands, the last one last.
    pub commands: Vec<String>,
}

impl Description {
    /// Just a title and an application: all that is known of most windows.
    pub fn titled(app: &str, title: &str) -> Description {
        Description { app: app.to_string(), title: title.to_string(), ..Default::default() }
    }

    pub fn is_terminal(&self) -> bool {
        self.kind == "terminal"
    }

    /// A window's account of itself, as it writes it (`spatiand_xr_surface_v1.describe`).
    pub fn from_json(json: &str) -> Option<Description> {
        serde_json::from_str(json).ok()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Whisper reads about the last 150 words of its prompt; this keeps a little under that.
const PROMPT_CHARS: usize = 800;

/// Whisper's prompt for a phrase said into `focus`: what it is about, then its words, then the
/// wearer's own last phrases -- **last**, because Whisper leans hardest on the end, and the
/// wearer's own way of mixing Indonesian and English is the steer that matters on every phrase.
/// Before there are any, `primer`: a sentence in the way they speak.
///
/// **Written as sentences.** Whisper copies the prompt's style as well as its words: given a
/// bare list of capitalised names it wrote back "terus coba Scroll pakai Left Path", with no
/// full stop anywhere.
pub fn whisper_prompt(focus: &Description, own_words: &[String], recent: &[String], primer: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !focus.title.is_empty() {
        parts.push(focus.title.clone());
    }
    if !focus.directory.is_empty() {
        parts.push(focus.directory.clone());
    }
    if !focus.commands.is_empty() {
        let last: Vec<&str> = focus.commands.iter().rev().take(6).rev().map(String::as_str).collect();
        parts.push(last.join("; "));
    } else if !focus.text.is_empty() {
        parts.push(tail_words(&focus.text, 50));
    }
    let mut names: Vec<&str> = Vec::new();
    for w in focus.vocabulary.iter().chain(own_words) {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(w)) {
            names.push(w);
        }
    }
    if !names.is_empty() {
        parts.push(names.iter().take(40).copied().collect::<Vec<_>>().join(", "));
    }
    if !recent.is_empty() {
        parts.push(recent.iter().rev().take(4).rev().cloned().collect::<Vec<_>>().join(" "));
    } else if !primer.is_empty() {
        parts.push(primer.to_string());
    }
    let prompt = parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| if p.ends_with(['.', '!', '?']) { p.to_string() } else { format!("{p}.") })
        .collect::<Vec<_>>()
        .join(" ");
    // Over the length, the start goes: the end is what Whisper reads.
    let chars: Vec<char> = prompt.chars().collect();
    if chars.len() > PROMPT_CHARS {
        chars[chars.len() - PROMPT_CHARS..].iter().collect()
    } else {
        prompt
    }
}

/// The commands typed at the prompts in a terminal's text, in order: what follows a prompt's mark
/// (`%` for zsh, `$` for bash, `#` for root, `❯` and `>` for the fancier ones) on a line where
/// what comes before it is short enough to be a prompt. For a terminal that says only its text
/// (a Mac's, through Accessibility).
pub fn prompt_commands(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            for mark in [" % ", "$ ", "# ", "\u{276F} ", "> "] {
                if let Some(at) = line.find(mark) {
                    let before = &line[..at];
                    if before.trim().is_empty() || before.chars().count() > 80 || before.split_whitespace().count() > 6 {
                        continue;
                    }
                    let command = line[at + mark.len()..].trim();
                    return (!command.is_empty()).then(|| command.to_string());
                }
            }
            None
        })
        .collect()
}

/// The last `n` words of `text`.
pub fn tail_words(text: &str, n: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    words[words.len().saturating_sub(n)..].join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wearers_own_phrases_come_last() {
        let focus = Description {
            kind: "terminal".into(),
            title: "~/Documents/Linux/Spatiand".into(),
            commands: vec!["git status".into(), "cargo build --release".into()],
            vocabulary: vec!["Cargo.toml".into(), "crates".into()],
            ..Default::default()
        };
        let p = whisper_prompt(&focus, &["Spatiand".into(), "crates".into()], &["Oke, aku push dulu ya.".into()], "Primer.");
        assert!(p.ends_with("Oke, aku push dulu ya."), "{p}");
        assert!(p.contains("git status; cargo build --release"));
        assert_eq!(p.matches("crates").count(), 1, "{p}");
    }

    #[test]
    fn a_long_prompt_loses_its_start() {
        let focus = Description { text: "kata ".repeat(400), ..Default::default() };
        let p = whisper_prompt(&focus, &[], &["the end".into()], "");
        assert!(p.chars().count() <= PROMPT_CHARS);
        assert!(p.ends_with("the end."));
    }
}

#[cfg(test)]
mod prompt_tests {
    use super::prompt_commands;

    #[test]
    fn a_terminals_commands_are_read_off_its_prompts() {
        let text = "Last login: Wed\npaulus@darkPhoenix ~ % cd Documents\npaulus@darkPhoenix Documents % ls\nLinux  notes.txt\n> quoted\npaulus@darkPhoenix Documents % ";
        assert_eq!(prompt_commands(text), vec!["cd Documents".to_string(), "ls".to_string()]);
    }
}

#[cfg(test)]
mod primer_tests {
    use super::*;

    #[test]
    fn with_nothing_said_yet_the_primer_ends_it() {
        let p = whisper_prompt(&Description::titled("Notes", "Notes"), &["Spatiand".into()], &[], "Oke, aku cek dulu ya.");
        assert_eq!(p, "Notes. Spatiand. Oke, aku cek dulu ya.");
    }
}
