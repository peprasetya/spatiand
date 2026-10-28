//! Spoken symbols, for dictation into a terminal.
//!
//! Whisper writes what it hears: "l s slash e t c" comes back as `LS slash ETC`. Turning that
//! into `ls /etc` needs to know that the focus is a terminal, which is why it is done here and
//! not by hemera. These rules do the fixed part -- symbol words in English and Indonesian,
//! letters spelled out, where a symbol joins its neighbours -- and the small model does the rest
//! with the terminal's context (`dictation.rs`). Where the rules cannot know ("cd slash etc" is
//! `cd /etc`, "etc slash httpd" is `etc/httpd`, and both are word, slash, word), they guess the
//! commoner, and the model is given both what was heard and the guess.

/// How a symbol sits against the words beside it.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Join {
    /// `a.b`
    Both,
    /// `a -b`: a space before, none after.
    Right,
    /// `a, b`
    Left,
    /// `a | b`
    Neither,
    /// `/`: like `Both`, except straight after the command (`cd /etc`).
    Path,
    /// `-`, `--`: a flag (`ls -la`) when what follows is short, else a word joiner (`my-file`).
    Dash,
    /// `"`: opens to the right, closes to the left.
    Quote,
}

/// Symbol words, longest first so that "titik dua" is a colon and not a dot and a two.
const SYMBOLS: &[(&str, &str, Join)] = &[
    ("lebih besar dari", ">", Join::Neither),
    ("lebih kecil dari", "<", Join::Neither),
    ("double ampersand", "&&", Join::Neither),
    ("forward slash", "/", Join::Path),
    ("garis miring", "/", Join::Path),
    ("garis bawah", "_", Join::Both),
    ("tanda hubung", "-", Join::Dash),
    ("tanda kutip", "\"", Join::Quote),
    ("double quote", "\"", Join::Quote),
    ("single quote", "'", Join::Both),
    ("titik dua", ":", Join::Both),
    ("titik koma", ";", Join::Left),
    ("sama dengan", "=", Join::Both),
    ("greater than", ">", Join::Neither),
    ("less than", "<", Join::Neither),
    ("open paren", "(", Join::Right),
    ("close paren", ")", Join::Left),
    ("kurung buka", "(", Join::Right),
    ("kurung tutup", ")", Join::Left),
    ("question mark", "?", Join::Left),
    ("tanda tanya", "?", Join::Left),
    ("exclamation mark", "!", Join::Left),
    ("tanda seru", "!", Join::Left),
    ("double dash", "--", Join::Dash),
    ("dash dash", "--", Join::Dash),
    ("at sign", "@", Join::Both),
    ("dollar sign", "$", Join::Right),
    ("back slash", "\\", Join::Both),
    ("backslash", "\\", Join::Both),
    ("slash", "/", Join::Path),
    ("dash", "-", Join::Dash),
    ("minus", "-", Join::Dash),
    ("strip", "-", Join::Dash),
    ("dot", ".", Join::Both),
    ("titik", ".", Join::Both),
    ("underscore", "_", Join::Both),
    ("tilde", "~", Join::Right),
    ("pipe", "|", Join::Neither),
    ("colon", ":", Join::Both),
    ("semicolon", ";", Join::Left),
    ("equals", "=", Join::Both),
    ("ampersand", "&", Join::Neither),
    ("asterisk", "*", Join::Both),
    ("star", "*", Join::Both),
    ("bintang", "*", Join::Both),
    ("hash", "#", Join::Right),
    ("pagar", "#", Join::Right),
    ("dollar", "$", Join::Right),
    ("percent", "%", Join::Both),
    ("persen", "%", Join::Both),
    ("caret", "^", Join::Both),
    ("comma", ",", Join::Left),
    ("koma", ",", Join::Left),
    ("quote", "\"", Join::Quote),
];

#[derive(Clone, Debug, PartialEq)]
enum Piece {
    Word(String),
    Symbol(&'static str, Join),
    /// "space", said: a space that is meant.
    Space,
}

/// What was heard, as it would be typed into a terminal.
pub fn for_terminal(heard: &str) -> String {
    let pieces = join_letters(symbols(&words(heard)));
    let mut out = String::new();
    // Whether the next piece may join onto what is already written.
    let mut glue_next = false;
    let mut quote_open = false;
    let mut first_word: Option<usize> = None;
    let mut words_done = 0usize;
    for (i, piece) in pieces.iter().enumerate() {
        match piece {
            Piece::Space => {
                out.push(' ');
                glue_next = true;
            }
            Piece::Word(w) => {
                // An environment variable keeps its capitals; a flag's letter, said, is small.
                let keep = out.ends_with('$') || matches!(pieces.get(i + 1), Some(Piece::Symbol("=", _)));
                let after_dash = matches!(pieces.get(i.wrapping_sub(1)), Some(Piece::Symbol(_, Join::Dash)));
                let w = if keep {
                    w.clone()
                } else if after_dash && w.chars().count() == 1 {
                    w.to_lowercase()
                } else {
                    lower_shouted(w)
                };
                if !out.is_empty() && !glue_next && !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str(&w);
                glue_next = false;
                words_done += 1;
                if first_word.is_none() {
                    first_word = Some(i);
                }
            }
            Piece::Symbol(s, join) => {
                let next_short = matches!(pieces.get(i + 1), Some(Piece::Word(w)) if w.chars().count() <= 2);
                let (space_before, glue_after) = match join {
                    Join::Both => (false, true),
                    Join::Right => (true, true),
                    Join::Left => (false, false),
                    Join::Neither => (true, false),
                    // Straight after the command, a path starts: `cd /etc`.
                    Join::Path => (words_done == 1 && matches!(pieces.get(i.wrapping_sub(1)), Some(Piece::Word(_))), true),
                    Join::Dash => (next_short || *s == "--" || words_done <= 1, true),
                    Join::Quote => {
                        quote_open = !quote_open;
                        (quote_open, quote_open)
                    }
                };
                if space_before && !out.is_empty() && !glue_next && !out.ends_with(' ') {
                    out.push(' ');
                } else if !space_before && out.ends_with(' ') && !matches!(pieces.get(i.wrapping_sub(1)), Some(Piece::Space)) {
                    out.pop();
                }
                out.push_str(s);
                if !glue_after {
                    out.push(' ');
                }
                glue_next = glue_after;
            }
        }
    }
    unglue_command(out.trim())
}

/// Commands whose first argument is so often a path that Whisper, told it is in a terminal,
/// writes the two as one: "l s slash etc" came back as `ls/etc`.
const COMMANDS: &[&str] = &[
    "ls", "cd", "cat", "cp", "mv", "rm", "mkdir", "rmdir", "touch", "less", "more", "head", "tail", "vim", "vi", "nano", "open",
    "chmod", "chown", "find", "grep", "du", "df", "ssh", "scp", "rsync", "git", "cargo", "make", "python", "python3", "node",
    "npm", "sudo", "echo", "source", "tar", "unzip", "zip", "code", "systemctl", "journalctl", "ln", "file", "stat", "tree",
];

/// `ls/etc` is `ls /etc`, and `cd~/x` is `cd ~/x`.
fn unglue_command(line: &str) -> String {
    let first_end = line.find(['/', '~']).unwrap_or(0);
    let (first, rest) = line.split_at(first_end);
    if first_end > 0 && !first.contains(' ') && COMMANDS.contains(&first) {
        format!("{first} {rest}")
    } else {
        line.to_string()
    }
}

/// The words, without the punctuation Whisper puts between them: in a terminal a full stop at
/// the end of a phrase is never meant.
fn words(heard: &str) -> Vec<String> {
    heard
        .split_whitespace()
        .map(|w| w.trim_end_matches([',', '.', '?', '!']).trim_start_matches(['"', '\'']).trim_end_matches(['"', '\'']))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

fn symbols(words: &[String]) -> Vec<Piece> {
    let lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let mut out = Vec::new();
    let mut i = 0;
    'next: while i < words.len() {
        if lower[i] == "space" || lower[i] == "spasi" {
            out.push(Piece::Space);
            i += 1;
            continue;
        }
        for (said, symbol, join) in SYMBOLS {
            let n = said.split(' ').count();
            if i + n <= words.len() && lower[i..i + n].join(" ") == *said {
                out.push(Piece::Symbol(symbol, *join));
                i += n;
                continue 'next;
            }
        }
        // Whisper's own way of writing spelled letters: `L-S`, `L.S.`
        let w = &words[i];
        let parts: Vec<&str> = w.split(['-', '.']).filter(|p| !p.is_empty()).collect();
        if parts.len() >= 2 && parts.iter().all(|p| p.chars().count() == 1 && p.chars().all(char::is_alphabetic)) {
            out.push(Piece::Word(parts.concat().to_lowercase()));
        } else {
            out.push(Piece::Word(w.clone()));
        }
        i += 1;
    }
    out
}

/// Letters said one by one are one word: "l s" is `ls`.
fn join_letters(pieces: Vec<Piece>) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    let mut run = String::new();
    let single = |p: &Piece| matches!(p, Piece::Word(w) if w.chars().count() == 1 && w.chars().all(|c| c.is_ascii_alphabetic()));
    for (i, piece) in pieces.iter().enumerate() {
        let joins = single(piece) && (pieces.get(i + 1).is_some_and(single) || !run.is_empty());
        if joins {
            if let Piece::Word(w) = piece {
                run.push_str(&w.to_lowercase());
            }
            continue;
        }
        if !run.is_empty() {
            out.push(Piece::Word(std::mem::take(&mut run)));
        }
        out.push(piece.clone());
    }
    if !run.is_empty() {
        out.push(Piece::Word(run));
    }
    out
}

/// `LS` is `ls`: in a terminal, a short word in capitals is almost always letters said one by one.
fn lower_shouted(w: &str) -> String {
    let letters = w.chars().filter(|c| c.is_alphabetic()).count();
    if (2..=6).contains(&letters) && w.chars().all(|c| !c.is_alphabetic() || c.is_uppercase()) {
        w.to_lowercase()
    } else {
        w.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::for_terminal as t;

    #[test]
    fn paths() {
        assert_eq!(t("LS slash ETC slash HTTPD"), "ls /etc/httpd");
        assert_eq!(t("cd tilde slash Documents slash Linux"), "cd ~/Documents/Linux");
        assert_eq!(t("cd garis miring etc"), "cd /etc");
        assert_eq!(t("cat file dot txt pipe grep error."), "cat file.txt | grep error");
        assert_eq!(t("ls/etc/httpd"), "ls /etc/httpd");
        assert_eq!(t("cd~/Documents"), "cd ~/Documents");
        assert_eq!(t("./build.sh"), "./build.sh");
    }

    #[test]
    fn flags_and_letters() {
        assert_eq!(t("ls dash l a"), "ls -la");
        assert_eq!(t("L-S dash L"), "ls -l");
        assert_eq!(t("git push dash dash force"), "git push --force");
        assert_eq!(t("git push double dash force"), "git push --force");
        assert_eq!(t("rm my dash file dot txt"), "rm my-file.txt");
    }

    #[test]
    fn quotes_and_signs() {
        assert_eq!(t("git commit dash m quote fix scroll quote"), "git commit -m \"fix scroll\"");
        assert_eq!(t("ssh paulus at sign deepmagpie"), "ssh paulus@deepmagpie");
        assert_eq!(t("echo dollar PATH"), "echo $PATH");
        assert_eq!(t("export NAME equals value"), "export NAME=value");
    }

    #[test]
    fn ordinary_words_pass() {
        assert_eq!(t("git status"), "git status");
        assert_eq!(t("cargo build space dash dash release"), "cargo build --release");
    }
}
