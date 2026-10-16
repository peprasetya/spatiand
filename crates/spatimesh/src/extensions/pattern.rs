//! Match patterns and globs: which pages an extension's content scripts go into and which hosts
//! it may reach, as Chrome and Firefox read them (`<scheme>://<host><path>`, `<all_urls>`).

/// What `<all_urls>` covers.
const ALL_URL_SCHEMES: &[&str] = &["http", "https", "ws", "wss", "ftp", "data", "file", "urn"];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Host {
    Any,
    /// `*.example.com`: the domain and everything under it.
    Domain(String),
    Exact(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    /// None for `*` (http and https, and ws and wss as Firefox has it) and `<all_urls>`.
    scheme: Option<String>,
    all_urls: bool,
    host: Host,
    port: Option<u16>,
    path: String,
}

impl Pattern {
    pub fn parse(text: &str) -> Result<Pattern, String> {
        let bad = || format!("not a match pattern: {text}");
        if text == "<all_urls>" {
            return Ok(Pattern { scheme: None, all_urls: true, host: Host::Any, port: None, path: "/*".into() });
        }
        let (scheme, rest) = text.split_once("://").ok_or_else(bad)?;
        let scheme = match scheme {
            "*" => None,
            s if s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '+' || c == '.') && !s.is_empty() => Some(s.to_ascii_lowercase()),
            _ => return Err(bad()),
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => return Err(bad()),
        };
        // An IPv6 address is in brackets, colons and all; a port follows a colon after them.
        let (host_text, port_text) = if authority.starts_with('[') {
            match authority.find("]:") {
                Some(i) => (&authority[..=i], Some(&authority[i + 2..])),
                None => (authority, None),
            }
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (h, Some(p)),
                None => (authority, None),
            }
        };
        let port = match port_text {
            None | Some("*") => None,
            Some(p) => Some(p.parse::<u16>().map_err(|_| bad())?),
        };
        let host = match host_text {
            "*" => Host::Any,
            "" if scheme.as_deref() == Some("file") => Host::Any,
            "" => return Err(bad()),
            h if h.starts_with("*.") && !h[2..].contains('*') => Host::Domain(h[2..].to_ascii_lowercase()),
            h if !h.contains('*') => Host::Exact(h.to_ascii_lowercase()),
            _ => return Err(bad()),
        };
        Ok(Pattern { scheme, all_urls: false, host, port, path: path.to_string() })
    }

    pub fn matches(&self, url: &str) -> bool {
        let Ok(u) = url::Url::parse(url) else { return false };
        let scheme = u.scheme();
        let scheme_ok = match (&self.scheme, self.all_urls) {
            (_, true) => ALL_URL_SCHEMES.contains(&scheme),
            (None, false) => matches!(scheme, "http" | "https" | "ws" | "wss"),
            (Some(s), false) => s == scheme,
        };
        if !scheme_ok {
            return false;
        }
        if self.all_urls && matches!(scheme, "data" | "urn") {
            return true;
        }
        let host = u.host_str().unwrap_or("").trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
        let host_ok = match &self.host {
            Host::Any => true,
            Host::Domain(d) => host == *d || host.ends_with(&format!(".{d}")),
            Host::Exact(h) => host == h.trim_start_matches('[').trim_end_matches(']'),
        };
        if !host_ok || self.port.is_some_and(|p| u.port_or_known_default() != Some(p)) {
            return false;
        }
        let mut path = u.path().to_string();
        if let Some(q) = u.query() {
            path.push('?');
            path.push_str(q);
        }
        wildcard(&self.path, &path, false)
    }

    /// Whether every URL this matches the other matches too: for whether a host permission
    /// covers what an extension asks to reach.
    pub fn within(&self, wider: &Pattern) -> bool {
        if wider.all_urls {
            return true;
        }
        if self.all_urls {
            return false;
        }
        let scheme_ok = match (&wider.scheme, &self.scheme) {
            (None, None) => true,
            (None, Some(s)) => matches!(s.as_str(), "http" | "https" | "ws" | "wss"),
            (Some(_), None) => false,
            (Some(a), Some(b)) => a == b,
        };
        let host_ok = match (&wider.host, &self.host) {
            (Host::Any, _) => true,
            (Host::Domain(d), Host::Domain(e)) | (Host::Domain(d), Host::Exact(e)) => e == d || e.ends_with(&format!(".{d}")),
            (Host::Exact(a), Host::Exact(b)) => a == b,
            _ => false,
        };
        scheme_ok && host_ok && (wider.port.is_none() || wider.port == self.port) && wildcard(&wider.path, &self.path, false)
    }
}

/// Whether `text` matches `pattern`, where `*` is any run of characters and, for globs, `?` is
/// any one.
pub fn wildcard(pattern: &str, text: &str, question_marks: bool) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == t[ti] || (question_marks && p[pi] == '?')) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// A set of patterns, read once: those that do not parse are left out (as browsers do, with a
/// warning).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Patterns(Vec<Pattern>);

impl Patterns {
    pub fn new(texts: &[String]) -> Patterns {
        Patterns(
            texts
                .iter()
                .filter_map(|t| match Pattern::parse(t) {
                    Ok(p) => Some(p),
                    Err(e) => {
                        log::warn!("{e}");
                        None
                    }
                })
                .collect(),
        )
    }

    pub fn matches(&self, url: &str) -> bool {
        self.0.iter().any(|p| p.matches(url))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether these cover all of `asked`.
    pub fn cover(&self, asked: &Pattern) -> bool {
        self.0.iter().any(|p| asked.within(p))
    }
}

/// Whether a content script's patterns and globs take this URL (Chrome's rule: one of
/// `matches`, none of `exclude_matches`, one of `include_globs` if any, none of `exclude_globs`).
pub fn script_wants(matches: &Patterns, excludes: &Patterns, include_globs: &[String], exclude_globs: &[String], url: &str) -> bool {
    matches.matches(url)
        && !excludes.matches(url)
        && (include_globs.is_empty() || include_globs.iter().any(|g| wildcard(g, url, true)))
        && !exclude_globs.iter().any(|g| wildcard(g, url, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, url: &str) -> bool {
        Pattern::parse(p).unwrap().matches(url)
    }

    #[test]
    fn patterns_as_chrome_reads_them() {
        assert!(m("<all_urls>", "https://a.b/c"));
        assert!(m("<all_urls>", "file:///home/x.html"));
        assert!(!m("<all_urls>", "chrome-extension://abc/x.html"));
        assert!(m("*://*/*", "http://example.com/"));
        assert!(!m("*://*/*", "file:///x"));
        assert!(m("https://*.example.com/*", "https://example.com/x"));
        assert!(m("https://*.example.com/*", "https://a.b.example.com/x?y=1"));
        assert!(!m("https://*.example.com/*", "https://notexample.com/"));
        assert!(!m("https://*.example.com/*", "http://example.com/"));
        assert!(m("https://example.com/foo*", "https://example.com/foobar"));
        assert!(!m("https://example.com/foo*", "https://example.com/bar"));
        assert!(m("https://www.google.com/search?*", "https://www.google.com/search?q=1"), "the query is part of the path");
        assert!(m("http://localhost:8080/*", "http://localhost:8080/a"));
        assert!(!m("http://localhost:8080/*", "http://localhost:9090/a"));
        assert!(m("http://127.0.0.1/*", "http://127.0.0.1:5000/"), "no port in the pattern: any");
        assert!(m("file:///*", "file:///home/me/page.html"));
        assert!(m("http://[::1]/*", "http://[::1]:8000/x"));
        assert!(m("http://[::1]:8000/*", "http://[::1]:8000/x"));
        assert!(Pattern::parse("https://*foo.com/*").is_err());
        assert!(Pattern::parse("https://example.com").is_err(), "a path is required");
        assert!(Pattern::parse("nonsense").is_err());
    }

    #[test]
    fn globs_and_content_scripts() {
        assert!(wildcard("https://???.example.com/*", "https://www.example.com/a", true));
        assert!(!wildcard("https://???.example.com/*", "https://w.example.com/a", true));
        let matches = Patterns::new(&["*://*.example.com/*".into()]);
        let excludes = Patterns::new(&["*://*.example.com/private/*".into()]);
        assert!(script_wants(&matches, &excludes, &[], &[], "https://example.com/a"));
        assert!(!script_wants(&matches, &excludes, &[], &[], "https://example.com/private/a"));
        assert!(!script_wants(&matches, &excludes, &["*news*".into()], &[], "https://example.com/a"));
        assert!(script_wants(&matches, &excludes, &["*news*".into()], &[], "https://news.example.com/a"));
        assert!(!script_wants(&matches, &excludes, &[], &["*.example.com/a*".into()], "https://news.example.com/a"));
    }

    #[test]
    fn what_a_permission_covers() {
        let granted = Patterns::new(&["https://*.example.com/*".into()]);
        assert!(granted.cover(&Pattern::parse("https://a.example.com/x/*").unwrap()));
        assert!(!granted.cover(&Pattern::parse("https://example.org/*").unwrap()));
        assert!(!granted.cover(&Pattern::parse("<all_urls>").unwrap()));
        assert!(Patterns::new(&["<all_urls>".into()]).cover(&Pattern::parse("*://*/*").unwrap()));
    }
}
