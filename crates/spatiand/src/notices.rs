//! The room's card for the system's own notifications, one logic on every platform: who a
//! notification is from on the first line, what it says on the second, for as long as it lasts,
//! and pressed, what it opens is opened. Fed by
//!
//! * the Deck's notification server (`crate::notifications`: desktop applications' D-Bus
//!   `Notify`), each for the time it asked for;
//! * the Beam Pro's notification listener (`spatiand-android`'s `notices`), each for [`GLANCE`];
//! * the Mac's banners, read by its app (mac/Sources/Spatiand/Notices.swift, `sp_notice`), for as
//!   long as the app shows them (it takes them away after the same eight seconds).
//!
//! The rules, the Mac's first:
//!
//! * a notification is shown when it comes, in place of whatever was there; posted again
//!   unchanged (an update that says nothing new, one that sits there waiting), it is not shown
//!   again;
//! * the words are [`words`]: markup taken out, entities put back, each line cut at [`LINE`];
//! * taken away by its sender, it goes; pressed, it goes, and its sender opens what it opens.
//!
//! The card goes under every other card the room shows, the frame loops asking each first:
//! SpatiKey's, dictation's, the assistant's, the install card, then this, then a host's word.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long a notification that says nothing of its own time is on the card: the Mac's eight
/// seconds, a glance's worth.
#[allow(dead_code)] // the Beam Pro's and the Mac's
pub const GLANCE: Duration = Duration::from_secs(8);
/// Characters on a line of the card, before it is cut short.
pub const LINE: usize = 120;
/// How many notifications are remembered as seen.
const REMEMBERED: usize = 64;

struct Shown {
    key: String,
    text: String,
    until: Instant,
}

/// The card's notification, and the ones already seen, by the key its sender knows it by.
pub struct Card {
    shown: Option<Shown>,
    seen: VecDeque<String>,
}

impl Default for Card {
    fn default() -> Self {
        Self::new()
    }
}

/// What posting a notification came to.
#[derive(Debug, PartialEq, Eq)]
pub struct Posted {
    /// It is on the card now.
    pub shown: bool,
    /// The other notification it took the card from, which is over.
    pub displaced: Option<String>,
}

impl Card {
    pub const fn new() -> Card {
        Card { shown: None, seen: VecDeque::new() }
    }

    /// A notification was posted, or posted again, saying `words` (see [`words`]), to stay for
    /// `lasting`. Posted again with what it said already, the card is left as it is; posted with
    /// nothing to say, it is not shown (and is off the card, if it was there).
    pub fn post(&mut self, key: &str, words: String, lasting: Duration, now: Instant) -> Posted {
        let seen = format!("{key}\u{1}{words}");
        if !words.is_empty() && self.seen.contains(&seen) {
            let shown = self.shown.as_ref().is_some_and(|s| s.key == key && now < s.until);
            return Posted { shown, displaced: None };
        }
        let displaced = self.shown.take().filter(|old| old.key != key && now < old.until).map(|old| old.key);
        if words.is_empty() {
            return Posted { shown: false, displaced };
        }
        self.seen.push_back(seen);
        while self.seen.len() > REMEMBERED {
            self.seen.pop_front();
        }
        self.shown = Some(Shown { key: key.to_string(), text: words, until: now + lasting });
        Posted { shown: true, displaced }
    }

    /// A notification was taken away by its sender: off the card, if it is the one there (true
    /// then). Its having been seen is forgotten, so the same words posted later show again.
    pub fn remove(&mut self, key: &str) -> bool {
        self.forget(key);
        if self.shown.as_ref().is_some_and(|s| s.key == key) {
            self.shown = None;
            return true;
        }
        false
    }

    /// The notification is over for its sender, though it was not taken away: the Deck's server
    /// tells the application so when its card expires or is pressed (`NotificationClosed`), and
    /// the same words sent again are then a new notification, to be shown. (A Mac banner or an
    /// Android notification is still there after its card goes, and posted again unchanged is
    /// not shown again.)
    #[allow(dead_code)] // the Deck's
    pub fn forget(&mut self, key: &str) {
        let prefix = format!("{key}\u{1}");
        self.seen.retain(|s| !s.starts_with(&prefix));
    }

    /// The notification whose time on the card is over, taken off it.
    pub fn expire(&mut self, now: Instant) -> Option<String> {
        if self.shown.as_ref().is_some_and(|s| now >= s.until) {
            return self.shown.take().map(|s| s.key);
        }
        None
    }

    /// What the card says now; empty when nothing is on it. One whose time is over is taken off.
    pub fn text(&mut self, now: Instant) -> String {
        self.expire(now);
        self.shown.as_ref().map(|s| s.text.clone()).unwrap_or_default()
    }

    /// The card was pressed: the key of the notification on it, which is taken off.
    pub fn press(&mut self, now: Instant) -> Option<String> {
        self.expire(now);
        self.shown.take().map(|s| s.key)
    }

    /// Nothing on the card, and nothing remembered: its sender has gone.
    pub fn clear(&mut self) {
        self.shown = None;
        self.seen.clear();
    }
}

/// Text as it is to be read: tags taken out, the five entities and numbered characters put back,
/// and every run of white space one space. Only what looks like a tag -- `<` then a letter or
/// `/` -- is a tag, so "a < b" survives.
pub fn plain(text: &str) -> String {
    let mut stripped = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('<') {
        stripped.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let tag = after.chars().next().is_some_and(|c| c == '/' || c.is_ascii_alphabetic());
        match after.find('>') {
            Some(j) if tag => {
                stripped.push(' ');
                rest = &after[j + 1..];
            }
            _ => {
                stripped.push('<');
                rest = after;
            }
        }
    }
    stripped.push_str(rest);
    entities(&stripped).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        let found = after.find(';').filter(|j| *j <= 10).and_then(|j| {
            let name = &after[1..j];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32),
                n if n.starts_with('#') => n[1..].parse().ok().and_then(char::from_u32),
                _ => None,
            };
            c.map(|c| (c, j))
        });
        match found {
            Some((c, j)) => {
                out.push(c);
                rest = &after[j + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn limit(line: String) -> String {
    if line.chars().count() <= LINE {
        return line;
    }
    let mut cut: String = line.chars().take(LINE - 1).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('\u{2026}');
    cut
}

/// The card: who it is from, then what it says. An application that gave no name is named by its
/// summary (its title), and its body is what it says.
pub fn words(app: &str, summary: &str, body: &str) -> String {
    let (app, summary, body) = (plain(app), plain(summary), plain(body));
    let (first, second) = if !app.is_empty() {
        // `notify-send -a Firefox Firefox ...`: the name once is enough.
        let summary = if summary == app { String::new() } else { summary };
        let second = match (summary.is_empty(), body.is_empty()) {
            (false, false) => format!("{summary} \u{2014} {body}"),
            (false, true) => summary,
            (true, false) => body,
            (true, true) => String::new(),
        };
        (app, second)
    } else if !summary.is_empty() {
        (summary, body)
    } else {
        (body, String::new())
    };
    let (first, second) = (limit(first), limit(second));
    match (first.is_empty(), second.is_empty()) {
        (true, true) => String::new(),
        (false, true) => first,
        (true, false) => second,
        (false, false) => format!("{first}\n{second}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Duration = Duration::from_secs(1);

    #[test]
    fn markup_is_taken_out_and_entities_put_back() {
        assert_eq!(plain("film.mkv <b>is</b> ready &amp; waiting"), "film.mkv is ready & waiting");
        assert_eq!(plain("a < b and c > d"), "a < b and c > d");
        assert_eq!(plain("<a href=\"x\">link</a>&lt;tag&gt; &quot;q&quot; &apos;s&apos;"), "link <tag> \"q\" 's'");
        assert_eq!(plain("line\n\n  two<br/>three"), "line two three");
        assert_eq!(plain("&#233;t&#xE9; &bogus; & alone"), "été &bogus; & alone");
    }

    #[test]
    fn the_card_says_who_then_what() {
        assert_eq!(words("Firefox", "Download finished", "film.mkv"), "Firefox\nDownload finished \u{2014} film.mkv");
        assert_eq!(words("Firefox", "Download finished", ""), "Firefox\nDownload finished");
        assert_eq!(words("Firefox", "Firefox", "Done"), "Firefox\nDone");
        assert_eq!(words("", "Battery low", "5% left"), "Battery low\n5% left");
        assert_eq!(words("", "", "Only a body"), "Only a body");
        assert_eq!(words(" ", "<b></b>", ""), "");
        assert_eq!(words("Messages", "Alice", "See you at\n eight"), "Messages\nAlice \u{2014} See you at eight");
        let long = "word ".repeat(60);
        let said = words("App", &long, "");
        let second = said.lines().nth(1).unwrap();
        assert_eq!(second.chars().count(), LINE);
        assert!(second.ends_with("word\u{2026}"));
    }

    #[test]
    fn a_notification_is_shown_for_its_time_and_once() {
        let mut card = Card::new();
        let t = Instant::now();
        assert_eq!(card.post("k1", "Chat\nAlice".into(), GLANCE, t), Posted { shown: true, displaced: None });
        assert_eq!(card.text(t + T), "Chat\nAlice");
        // Posted again unchanged: left as it is, not shown again.
        assert!(card.post("k1", "Chat\nAlice".into(), GLANCE, t + 2 * T).shown);
        assert_eq!(card.expire(t + GLANCE), Some("k1".into()));
        assert_eq!(card.text(t + GLANCE), "");
        assert!(!card.post("k1", "Chat\nAlice".into(), GLANCE, t + GLANCE).shown);
        // Updated with something new: shown.
        assert!(card.post("k1", "Chat\nAlice \u{2014} there?".into(), GLANCE, t + GLANCE).shown);
        assert!(!card.text(t + GLANCE).is_empty());
    }

    #[test]
    fn a_newer_notification_takes_the_card_from_the_one_there() {
        let mut card = Card::new();
        let t = Instant::now();
        card.post("a", "A".into(), GLANCE, t);
        assert_eq!(card.post("b", "B".into(), Duration::from_secs(20), t + T).displaced, Some("a".into()));
        assert_eq!(card.text(t + 10 * T), "B");
        // The older one going leaves the card alone; the one there going takes it away.
        assert!(!card.remove("a"));
        assert!(card.remove("b"));
        assert_eq!(card.text(t + 10 * T), "");
        // Replaced by its sender with nothing to say: off the card, and nothing displaced.
        card.post("c", "C".into(), GLANCE, t);
        assert_eq!(card.post("c", String::new(), GLANCE, t), Posted { shown: false, displaced: None });
        assert_eq!(card.text(t), "");
    }

    #[test]
    fn pressing_the_card_names_its_notification_and_takes_it_away() {
        let mut card = Card::new();
        let t = Instant::now();
        assert_eq!(card.press(t), None);
        card.post("k", "Chat\nHi".into(), GLANCE, t);
        assert_eq!(card.press(t + T), Some("k".into()));
        assert_eq!(card.text(t + T), "");
        // A card whose time is over is nobody's to press.
        card.post("k2", "Chat\nYo".into(), GLANCE, t);
        assert_eq!(card.press(t + GLANCE), None);
    }

    #[test]
    fn a_notification_taken_away_and_posted_again_is_shown_again() {
        let mut card = Card::new();
        let t = Instant::now();
        card.post("k", "Alarm".into(), GLANCE, t);
        card.remove("k");
        assert!(card.post("k", "Alarm".into(), GLANCE, t + 60 * T).shown);
    }

    #[test]
    fn a_notification_its_sender_was_told_is_over_is_shown_again() {
        // The Deck: `notify-send -r 77 "Build finished"` twice, 10 s apart. The first expired
        // (or was pressed) and its application was told it closed, so the second is new.
        let mut card = Card::new();
        let t = Instant::now();
        assert!(card.post("77", "Build finished".into(), GLANCE, t).shown);
        assert_eq!(card.expire(t + GLANCE), Some("77".into()));
        card.forget("77");
        assert!(card.post("77", "Build finished".into(), GLANCE, t + 10 * T).shown);
        assert_eq!(card.press(t + 11 * T), Some("77".into()));
        card.forget("77");
        assert!(card.post("77", "Build finished".into(), GLANCE, t + 20 * T).shown);
        // Displaced by another and told it closed: the same.
        assert_eq!(card.post("78", "Other".into(), GLANCE, t + 21 * T).displaced, Some("77".into()));
        card.forget("77");
        assert!(card.post("77", "Build finished".into(), GLANCE, t + 22 * T).shown);
    }
}
