//! A cookie jar for one account, pinned to one host.
//!
//! Ported from `data/AccountCookieJar.kt`. Vibrefy's whole authentication is a single
//! `JSESSIONID`, so a full cookie store would be machinery around one string. What does
//! matter is the pinning: a redirect somewhere else must never be handed a session that
//! belongs to this deployment, and the jar must be readable as a plain map so an account
//! can be written to disk and picked up again on the next run.

use std::collections::BTreeMap;

#[derive(Debug, Default, Clone)]
pub struct CookieJar {
    /// Lower-cased, so a redirect that changes only the case of the host still matches.
    host: Option<String>,
    cookies: BTreeMap<String, String>,
}

impl CookieJar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Restores a jar written out by [`CookieJar::snapshot`].
    pub fn restore(host: &str, cookies: BTreeMap<String, String>) -> Self {
        Self {
            host: Some(host.to_ascii_lowercase()),
            cookies,
        }
    }

    pub fn snapshot(&self) -> BTreeMap<String, String> {
        self.cookies.clone()
    }

    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    /// The `Cookie:` header for a request to `host`, or nothing at all when the host is
    /// not the one this jar belongs to.
    pub fn header_for(&self, host: &str) -> Option<String> {
        if !self.host.as_deref()?.eq_ignore_ascii_case(host) || self.cookies.is_empty() {
            return None;
        }
        Some(
            self.cookies
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; "),
        )
    }

    /// Takes in the `Set-Cookie` headers of one response. The first host seen claims the
    /// jar; anything else is ignored rather than merged.
    pub fn store(&mut self, host: &str, set_cookie: &[String]) {
        match &self.host {
            None => self.host = Some(host.to_ascii_lowercase()),
            Some(known) if known.eq_ignore_ascii_case(host) => {}
            Some(_) => return,
        }
        for header in set_cookie {
            let Some((name, value)) = parse_set_cookie(header) else {
                continue;
            };
            // A server ends a session by sending the cookie back empty; keeping it would
            // leave us presenting a dead id forever.
            if value.is_empty() {
                self.cookies.remove(&name);
            } else {
                self.cookies.insert(name, value);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    pub fn clear(&mut self) {
        self.cookies.clear();
    }
}

/// `JSESSIONID=abc123; Path=/; HttpOnly` -> `("JSESSIONID", "abc123")`.
///
/// Only the pair matters. Vibrefy's session cookie has no `Expires` - it is a session
/// cookie, and the four-hour idle timeout is enforced on the server, not by us.
fn parse_set_cookie(header: &str) -> Option<(String, String)> {
    let pair = header.split(';').next()?.trim();
    let (name, value) = pair.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some((name.to_string(), value.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jar_for(host: &str, headers: &[&str]) -> CookieJar {
        let mut jar = CookieJar::new();
        jar.store(
            host,
            &headers.iter().map(|h| h.to_string()).collect::<Vec<_>>(),
        );
        jar
    }

    #[test]
    fn keeps_the_pair_and_drops_the_attributes() {
        let jar = jar_for(
            "media.example.com",
            &["JSESSIONID=abc123; Path=/; HttpOnly; SameSite=Lax"],
        );
        assert_eq!(
            Some("JSESSIONID=abc123".to_string()),
            jar.header_for("media.example.com")
        );
    }

    /// The pin. A deployment that redirects to an identity provider must not hand that
    /// provider the session cookie.
    #[test]
    fn another_host_is_offered_nothing_and_stores_nothing() {
        let mut jar = jar_for("media.example.com", &["JSESSIONID=abc123"]);
        assert_eq!(None, jar.header_for("accounts.google.com"));

        jar.store("accounts.google.com", &["SID=someone-elses".to_string()]);
        assert_eq!(
            Some("JSESSIONID=abc123".to_string()),
            jar.header_for("media.example.com"),
            "the other host's cookie was not merged in"
        );
    }

    #[test]
    fn the_host_match_ignores_case() {
        let jar = jar_for("Media.Example.COM", &["JSESSIONID=abc123"]);
        assert!(jar.header_for("media.example.com").is_some());
    }

    /// Signing in rotates the session id. The new value has to replace the old one, or
    /// every stream URL built afterwards carries an id the server has already retired.
    #[test]
    fn a_rotated_session_replaces_the_one_before_it() {
        let mut jar = jar_for("media.example.com", &["JSESSIONID=before; Path=/"]);
        jar.store("media.example.com", &["JSESSIONID=after; Path=/".into()]);
        assert_eq!(
            Some("JSESSIONID=after".to_string()),
            jar.header_for("media.example.com")
        );
    }

    #[test]
    fn a_cookie_sent_back_empty_is_a_cookie_withdrawn() {
        let mut jar = jar_for("media.example.com", &["JSESSIONID=abc123"]);
        jar.store("media.example.com", &["JSESSIONID=; Max-Age=0".into()]);
        assert!(jar.is_empty());
        assert_eq!(None, jar.header_for("media.example.com"));
    }

    #[test]
    fn survives_a_round_trip_through_storage() {
        let jar = jar_for("media.example.com", &["JSESSIONID=abc123"]);
        let restored = CookieJar::restore("media.example.com", jar.snapshot());
        assert_eq!(
            jar.header_for("media.example.com"),
            restored.header_for("media.example.com")
        );
    }

    #[test]
    fn a_header_that_is_not_a_pair_is_ignored() {
        let jar = jar_for("media.example.com", &["nonsense", "=novalue", "a=b"]);
        assert_eq!(Some("a=b".to_string()), jar.header_for("media.example.com"));
    }
}
