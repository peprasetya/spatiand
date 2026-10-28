//! Names Whisper cannot spell, put right afterwards.
//!
//! Context in the prompt fixes a word Whisper knows ("left pad" over "left path"), but not one it
//! has never seen: "Spatiand" comes back as "sepatian" whatever the prompt says, because nothing
//! it learnt from is spelled that way. So the words that matter here -- the focused window's
//! names, and the wearer's own list -- are matched against what came back, and a near miss is
//! replaced.
//!
//! **Near in sound and in spelling, both.** The spelling alone would turn "spatial" into
//! "Spatiand" (two letters apart); the sound alone would match far too much. A word's sound
//! here is its skeleton -- the first letter and the consonants after it, with letters that sound
//! alike made one -- and a replacement needs the skeletons within one change of each other and
//! the spellings within a quarter of the word. Several words heard as one name ("spati quill")
//! are tried together.

/// `text` with the near misses of `vocabulary` replaced by it.
pub fn correct(text: &str, vocabulary: &[String]) -> String {
    let known: Vec<(String, String, &str)> = vocabulary
        .iter()
        .map(|w| w.trim())
        .filter(|w| plain(w).chars().count() >= 5)
        .map(|w| (plain(w), skeleton(&plain(w)), w))
        .collect();
    if known.is_empty() {
        return text.to_string();
    }
    let spans = split(text);
    let words: Vec<usize> = spans.iter().enumerate().filter(|(_, s)| s.word).map(|(i, _)| i).collect();
    let mut replace: Vec<(usize, usize, String)> = Vec::new();
    let mut w = 0;
    while w < words.len() {
        let mut best: Option<(usize, f32, String)> = None;
        for n in 1..=3.min(words.len() - w) {
            // Only words with nothing but spaces between them make one name.
            let (first, last) = (words[w], words[w + n - 1]);
            if spans[first..=last].iter().any(|s| !s.word && s.text.trim() != "") {
                break;
            }
            let heard: String = (w..w + n).map(|k| spans[words[k]].text.as_str()).collect();
            let heard = plain(&heard);
            for (form, sound, original) in &known {
                if let Some(score) = closeness(&heard, form, sound, n) {
                    if best.as_ref().map_or(true, |b| score < b.1) {
                        best = Some((n, score, original.to_string()));
                    }
                }
            }
        }
        match best {
            Some((n, _, name)) => {
                replace.push((words[w], words[w + n - 1], name));
                w += n;
            }
            None => w += 1,
        }
    }
    let mut out = String::new();
    let mut i = 0;
    while i < spans.len() {
        if let Some((_, last, name)) = replace.iter().find(|(first, _, _)| *first == i) {
            out.push_str(name);
            i = last + 1;
        } else {
            out.push_str(&spans[i].text);
            i += 1;
        }
    }
    out
}

/// How far `heard` (made of `words` words) is from a known word (0 the same), if near enough to
/// be it.
fn closeness(heard: &str, form: &str, sound: &str, words: usize) -> Option<f32> {
    if heard == form {
        return Some(0.0);
    }
    // Words run together are a name only when they spell it, near enough to the letter: else
    // "sepatian di" is closer to "Spatiand" than "sepatian" is, and takes the "di" with it.
    if words > 1 {
        return (distance(heard, form) <= 1).then_some(0.05);
    }
    let length = heard.chars().count().max(form.chars().count());
    if heard.chars().count() < 4 {
        return None;
    }
    let spelled = distance(heard, form) as f32 / length as f32;
    let sounds = distance(&skeleton(heard), sound);
    (spelled <= 0.25 && sounds <= 1 && heard.chars().next() == form.chars().next()).then_some(spelled)
}

/// Lower case, letters and digits only.
fn plain(w: &str) -> String {
    w.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The first letter, then the consonants, with letters that sound alike made one and doubles one.
fn skeleton(w: &str) -> String {
    let mut out = String::new();
    for (i, c) in w.chars().enumerate() {
        let c = match c {
            'c' | 'q' => 'k',
            'v' => 'f',
            'z' => 's',
            other => other,
        };
        if i > 0 && "aeiouy".contains(c) {
            continue;
        }
        if out.ends_with(c) {
            continue;
        }
        out.push(c);
    }
    out
}

/// Edits from one to the other (Levenshtein), over characters.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut diagonal = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let above = row[j];
            row[j] = (diagonal + (a[i - 1] != b[j - 1]) as usize).min(row[j - 1] + 1).min(above + 1);
            diagonal = above;
        }
    }
    row[b.len()]
}

struct Span {
    text: String,
    word: bool,
}

/// Words and what is between them, in order, so that joining them gives the text back.
fn split(text: &str) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    for c in text.chars() {
        let word = c.is_alphanumeric();
        match spans.last_mut() {
            Some(s) if s.word == word => s.text.push(c),
            _ => spans.push(Span { text: c.to_string(), word }),
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn a_coined_name_is_put_right() {
        let names = v(&["Spatiand", "SpatiQuill", "Steam Deck"]);
        assert_eq!(correct("Restart sepatian di deck, terus coba scroll.", &names), "Restart Spatiand di deck, terus coba scroll.");
        assert_eq!(correct("buka spati quill dulu", &names), "buka SpatiQuill dulu");
        assert_eq!(correct("buka spatiquill", &names), "buka SpatiQuill");
    }

    #[test]
    fn real_words_near_a_name_stay() {
        let names = v(&["Spatiand", "SpatiShelf"]);
        assert_eq!(correct("spatial audio di sepatu", &names), "spatial audio di sepatu");
        assert_eq!(correct("the shelf is full", &names), "the shelf is full");
    }

    #[test]
    fn short_words_are_never_names() {
        assert_eq!(correct("git add dan push", &v(&["dang"])), "git add dan push");
    }
}
