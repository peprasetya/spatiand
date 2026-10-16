//! The address bar: one field for an address or a search.
//!
//! What is typed is an address when it says so (a scheme), or when it looks like one -- a host
//! with a dot and a top-level domain, `localhost`, an IP address, a host with a port -- and has
//! no spaces in it. Anything else is a search with the profile's search engine. An address with
//! no scheme is tried over HTTPS, except a machine on this network (`localhost`, an IP address,
//! a name with no dot), which is asked over HTTP as a browser has always done.

use serde::{Deserialize, Serialize};

/// A search engine: its name and its address with `{}` where the words go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchEngine {
    pub name: String,
    pub template: String,
}

impl SearchEngine {
    pub fn new(name: &str, template: &str) -> SearchEngine {
        SearchEngine { name: name.to_string(), template: template.to_string() }
    }

    /// The address that searches for `words`.
    pub fn url_for(&self, words: &str) -> String {
        let query = form_encode(words.trim());
        if self.template.contains("{}") {
            self.template.replace("{}", &query)
        } else {
            format!("{}{}", self.template, query)
        }
    }

    /// The words of a search this engine made, read back from its address: so that the bar can
    /// show what was searched for rather than the engine's address.
    pub fn words_in(&self, url: &str) -> Option<String> {
        let (before, after) = self.template.split_once("{}")?;
        let rest = url.strip_prefix(before)?;
        let end = if after.is_empty() { rest.find('&').unwrap_or(rest.len()) } else { rest.find(after)? };
        Some(form_decode(&rest[..end]))
    }
}

/// The search engines offered in Settings, the first the default.
pub fn search_engines() -> Vec<SearchEngine> {
    vec![
        SearchEngine::new("DuckDuckGo", "https://duckduckgo.com/?q={}"),
        SearchEngine::new("Google", "https://www.google.com/search?q={}"),
        SearchEngine::new("Bing", "https://www.bing.com/search?q={}"),
        SearchEngine::new("Startpage", "https://www.startpage.com/do/search?q={}"),
        SearchEngine::new("Brave Search", "https://search.brave.com/search?q={}"),
        SearchEngine::new("Ecosia", "https://www.ecosia.org/search?q={}"),
        SearchEngine::new("Kagi", "https://kagi.com/search?q={}"),
    ]
}

/// What was typed, understood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Typed {
    /// An address, made whole: `example.com` is `https://example.com/`.
    Address(String),
    Search(String),
}

impl Typed {
    /// The address to load.
    pub fn url(&self, engine: &SearchEngine) -> String {
        match self {
            Typed::Address(url) => url.clone(),
            Typed::Search(words) => engine.url_for(words),
        }
    }
}

/// Schemes taken as they are typed.
const SCHEMES: &[&str] = &["http", "https", "file", "about", "data", "view-source", "blob", "ftp", "mailto", "chrome", "chrome-extension", "webkit-extension", "safari-web-extension", "moz-extension", "spatimesh-extension"];

/// Top-level domains that are not two letters long (two letters are countries, and are all
/// taken as domains). Not the whole list: the ones a name is likely to end in.
const TLDS: &[&str] = &[
    "com", "org", "net", "edu", "gov", "mil", "int", "info", "biz", "name", "pro", "mobi", "app", "dev", "page", "blog", "shop", "store",
    "online", "site", "website", "tech", "cloud", "xyz", "club", "live", "news", "media", "studio", "design", "art", "music", "game", "games",
    "zone", "space", "world", "today", "life", "link", "click", "help", "email", "wiki", "work", "works", "company", "agency", "services",
    "solutions", "network", "systems", "software", "digital", "social", "group", "team", "center", "host", "hosting", "codes", "tools",
    "travel", "museum", "aero", "coop", "jobs", "tel", "asia", "cat", "post", "local", "localhost", "lan", "home", "internal",
    "onion", "eth", "porn", "bike", "photo", "photos", "pics", "video", "movie", "film", "tv", "radio", "fm", "stream", "chat", "one", "run",
    "build", "garden", "farm", "earth", "eco", "green", "energy", "science", "academy", "school", "education", "university", "london",
    "berlin", "paris", "tokyo", "nyc", "amsterdam", "wien", "swiss", "bayern", "nrw", "hamburg", "koeln", "quebec", "scot", "wales",
    "africa", "arpa", "google", "microsoft", "apple", "amazon", "goog", "ninja", "rocks", "guru", "expert", "plus", "fun", "lol", "wtf",
];

/// Understand what was typed in the bar. `None` for nothing at all.
pub fn understand(typed: &str) -> Option<Typed> {
    let text = typed.trim();
    if text.is_empty() {
        return None;
    }
    // A scheme, said outright.
    if let Some((scheme, rest)) = text.split_once(':') {
        let scheme_lower = scheme.to_ascii_lowercase();
        if SCHEMES.contains(&scheme_lower.as_str()) && !text.contains(char::is_whitespace) {
            // `localhost:8080` and `example.com:443/x` are a host and a port, not a scheme.
            if !rest.chars().next().is_some_and(|c| c.is_ascii_digit()) || scheme_lower == "about" {
                return Some(Typed::Address(text.to_string()));
            }
        }
        // `javascript:` from the bar is a search: typed or pasted there, it is a way to make the
        // wearer run something on the page in front of them.
    }
    if text.contains(char::is_whitespace) {
        return Some(Typed::Search(text.to_string()));
    }
    match looks_like_address(text) {
        Some(secure) => Some(Typed::Address(format!("{}://{}", if secure { "https" } else { "http" }, with_path(text)))),
        None => Some(Typed::Search(text.to_string())),
    }
}

/// `example.com` with the slash a browser puts after a bare host.
fn with_path(text: &str) -> String {
    let host_end = text.find(['/', '?', '#']).unwrap_or(text.len());
    if host_end == text.len() {
        format!("{text}/")
    } else {
        text.to_string()
    }
}

/// Whether text with no scheme and no spaces is an address, and if so whether it is asked
/// for over HTTPS (true) or plain HTTP (a machine on this network).
fn looks_like_address(text: &str) -> Option<bool> {
    let host_end = text.find(['/', '?', '#']).unwrap_or(text.len());
    let authority = &text[..host_end];
    let after = &text[host_end..];
    // `someone@example.com` is someone's address for mail, which the bar searches for.
    if authority.contains('@') {
        return None;
    }
    let host_port = authority;
    // [::1] and [::1]:8080.
    if let Some(rest) = host_port.strip_prefix('[') {
        let (inside, tail) = rest.split_once(']')?;
        if inside.parse::<std::net::Ipv6Addr>().is_ok() && (tail.is_empty() || port(tail.strip_prefix(':')?)) {
            return Some(false);
        }
        return None;
    }
    let (host, has_port) = match host_port.rsplit_once(':') {
        Some((host, p)) if port(p) => (host, true),
        Some(_) => return None,
        None => (host_port, false),
    };
    if host.is_empty() || !host.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '.' || c == '_') {
        return None;
    }
    if host.parse::<std::net::Ipv4Addr>().is_ok() {
        return Some(false);
    }
    let lower = host.to_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") {
        return Some(false);
    }
    let labels: Vec<&str> = lower.split('.').collect();
    if labels.iter().any(|l| l.is_empty() || l.starts_with('-') || l.ends_with('-')) {
        return None;
    }
    if labels.len() == 1 {
        // A single name is a search -- unless it says it is a machine, with a port or a path
        // after it: `printer:631`, `nas/`.
        return (has_port || after.starts_with('/')).then_some(false);
    }
    let top = labels[labels.len() - 1];
    let known = (top.len() == 2 && top.chars().all(|c| c.is_ascii_alphabetic()))
        || TLDS.contains(&top)
        || top.starts_with("xn--")
        || !top.is_ascii();
    if !known {
        // `readme.txt`, `1.5`: words with a dot in them.
        return None;
    }
    let local = matches!(top, "local" | "lan" | "home" | "internal");
    Some(!local)
}

fn port(text: &str) -> bool {
    !text.is_empty() && text.len() <= 5 && text.chars().all(|c| c.is_ascii_digit()) && text.parse::<u32>().is_ok_and(|p| p > 0 && p < 65536)
}

/// Words as a URL's query wants them: letters and digits as they are, a space a `+`,
/// everything else in `%XX` of its UTF-8.
pub fn form_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn form_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() && bytes[i + 1].is_ascii_hexdigit() && bytes[i + 2].is_ascii_hexdigit() => {
                let hex = |b: u8| (b as char).to_digit(16).unwrap_or(0) as u8;
                out.push(hex(bytes[i + 1]) * 16 + hex(bytes[i + 2]));
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What the bar shows for a page's address: the address itself, but for a search made from
/// the bar, the words searched for.
pub fn shown(url: &str, engine: &SearchEngine) -> String {
    engine.words_in(url).unwrap_or_else(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(text: &str) -> String {
        match understand(text) {
            Some(Typed::Address(a)) => a,
            other => panic!("{text:?} was {other:?}, not an address"),
        }
    }

    fn search(text: &str) {
        assert!(matches!(understand(text), Some(Typed::Search(_))), "{text:?} was {:?}, not a search", understand(text));
    }

    #[test]
    fn addresses() {
        assert_eq!(address("example.com"), "https://example.com/");
        assert_eq!(address("  example.com  "), "https://example.com/");
        assert_eq!(address("www.example.co.uk/page?x=1"), "https://www.example.co.uk/page?x=1");
        assert_eq!(address("https://example.com"), "https://example.com");
        assert_eq!(address("HTTP://Example.com/A"), "HTTP://Example.com/A");
        assert_eq!(address("localhost"), "http://localhost/");
        assert_eq!(address("localhost:8080/x"), "http://localhost:8080/x");
        assert_eq!(address("192.168.1.10"), "http://192.168.1.10/");
        assert_eq!(address("192.168.1.10:8096"), "http://192.168.1.10:8096/");
        assert_eq!(address("[::1]:8080"), "http://[::1]:8080/");
        assert_eq!(address("nas/"), "http://nas/");
        assert_eq!(address("printer:631"), "http://printer:631/");
        assert_eq!(address("deepmagpie.local"), "http://deepmagpie.local/");
        assert_eq!(address("about:blank"), "about:blank");
        assert_eq!(address("file:///home/deck/a.html"), "file:///home/deck/a.html");
        assert_eq!(address("github.io"), "https://github.io/");
        assert_eq!(address("example.dev#top"), "https://example.dev#top");
        assert_eq!(address("bücher.de"), "https://bücher.de/");
    }

    #[test]
    fn searches() {
        search("weather");
        search("rust traits");
        search("readme.txt");
        search("1.5");
        search("what is example.com");
        search("javascript:alert(1)");
        search("a:b");
        search("-x.com");
        search("someone@example.com");
    }

    #[test]
    fn search_urls() {
        let ddg = &search_engines()[0];
        assert_eq!(understand("rust traits").unwrap().url(ddg), "https://duckduckgo.com/?q=rust+traits");
        assert_eq!(ddg.url_for("a&b=c ü"), "https://duckduckgo.com/?q=a%26b%3Dc+%C3%BC");
        assert_eq!(ddg.words_in("https://duckduckgo.com/?q=a%26b%3Dc+%C3%BC").as_deref(), Some("a&b=c ü"));
        assert_eq!(shown("https://duckduckgo.com/?q=rust+traits&ia=web", ddg), "rust traits");
        assert_eq!(shown("https://example.com/", ddg), "https://example.com/");
        let google = &search_engines()[1];
        assert_eq!(google.url_for("x"), "https://www.google.com/search?q=x");
    }

    #[test]
    fn decoding_survives_nonsense() {
        assert_eq!(form_decode("100%"), "100%");
        assert_eq!(form_decode("%zz"), "%zz");
        assert_eq!(form_decode("%4"), "%4");
        assert_eq!(form_decode("a+b%20c"), "a b c");
    }
}
