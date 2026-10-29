//! Which site a page is, by the Public Suffix List (data/public_suffix_list.dat, the copy in
//! libpsl 0.23.3, MPL-2.0).
//!
//! Hosts are compared as browsers write them in origins: a domain in lowercase punycode, an
//! IPv4 address in dotted decimal, an IPv6 address compressed and in lowercase. The list itself
//! is written in Unicode, so its rules are turned into punycode as it is read (once, on first
//! use), and a host or domain given in Unicode is turned into punycode before it is looked up,
//! and answered in punycode.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::OnceLock;

/// The host of an origin ("https://Login.Example.com:8443" -> "login.example.com"), lowercased,
/// IPv6 without brackets. None if it is not an origin with a host.
///
/// A domain comes back in punycode, as browsers write it ("https://bücher.example" ->
/// "xn--bcher-kva.example"). A name and password before the host ("https://github.com@evil.com")
/// are refused rather than read past; a path after it is ignored.
pub fn host_of(origin: &str) -> Option<String> {
    Origin::parse(origin).map(|origin| origin.host.bare())
}

/// https, or http to localhost, *.localhost, 127.0.0.0/8 or ::1: what WebAuthn and filling accept.
pub fn is_secure_origin(origin: &str) -> bool {
    Origin::parse(origin).is_some_and(|origin| origin.is_secure())
}

/// Is `domain` itself a public suffix ("com", "co.uk", "github.io", and any single unknown label)?
///
/// An IP address is not (it is no domain at all; [`registrable_domain`] answers None for it all
/// the same). Something that is not a host at all, such as an empty string or a name with an
/// empty label, is answered yes: nothing can be registered as it either.
pub fn is_public_suffix(domain: &str) -> bool {
    match Host::parse(domain) {
        Some(Host::Domain(domain)) => {
            let starts = label_starts(&domain);
            suffix_labels(&domain, &starts) == starts.len()
        }
        Some(Host::Ip(_)) => false,
        None => true,
    }
}

/// The registrable domain (eTLD+1) of a host: "a.b.example.co.uk" -> "example.co.uk". None for
/// an IP address, "localhost", or a host that is itself a public suffix.
pub fn registrable_domain(host: &str) -> Option<String> {
    match Host::parse(host)? {
        Host::Domain(domain) => registrable(&domain).map(str::to_string),
        Host::Ip(_) => None,
    }
}

/// May a page at `origin` use the relying party id `rp_id`? As Chrome decides it:
///
/// * The origin must be secure ([`is_secure_origin`]) and must not be an IP address. Chrome
///   refuses an address outright, loopback ones included, though the standard would let one
///   name itself.
/// * The rp id must be the origin's host, or a parent domain of it that is not a public suffix.
///   Nor may it reach into the host's own public suffix: a page on bucket.s3.amazonaws.com
///   cannot claim amazonaws.com, as the HTML standard's "is a registrable domain suffix of or is
///   equal to" says.
/// * So "localhost" only for localhost itself: *.localhost cannot claim it, being a single
///   unknown label.
///
/// `rp_id` is taken as the page gave it and must already be what will be hashed: lowercase, and
/// in punycode, as `location.hostname` writes it.
pub fn rp_id_ok(rp_id: &str, origin: &str) -> bool {
    let Some(origin) = Origin::parse(origin) else { return false };
    if !origin.is_secure() {
        return false;
    }
    let Host::Domain(host) = &origin.host else { return false };
    if Host::parse(rp_id) != Some(Host::Domain(rp_id.to_string())) {
        return false;
    }
    if rp_id == host.as_str() {
        return true;
    }
    let parent = host.len() > rp_id.len() && host.ends_with(rp_id) && host[..host.len() - rp_id.len()].ends_with('.');
    // Both are whole labels from the right of the host, so the rp id holds the host's
    // registrable domain exactly when it is at least as long.
    parent && !is_public_suffix(rp_id) && registrable(host).is_some_and(|site| rp_id.len() >= site.len())
}

/// Does an entry's site (a registrable domain, or a whole origin) cover `origin`? A domain covers
/// its own host and every host under it; an origin only itself.
///
/// A site with "://" in it is an origin, compared whole with the scheme's own port written or
/// not ("https://example.com:443" is "https://example.com"). A domain that is itself a public
/// suffix ("github.io", a single label like "nas") covers only its own host, never the sites
/// under it. Sites are compared as [`registrable_domain`] writes them, whatever case or script
/// they were typed in. The scheme of the page is not judged here: that is
/// [`is_secure_origin`]'s.
pub fn site_matches(site: &str, origin: &str) -> bool {
    let Some(page) = Origin::parse(origin) else { return false };
    let site = site.trim();
    if site.contains("://") {
        return Origin::parse(site).is_some_and(|site| site == page);
    }
    match (Host::parse(site), &page.host) {
        (Some(Host::Domain(domain)), Host::Domain(host)) => {
            *host == domain || (host.ends_with(&format!(".{domain}")) && !is_public_suffix(&domain))
        }
        (Some(Host::Ip(site)), Host::Ip(host)) => site == *host,
        _ => false,
    }
}

/// What to keep as the site of a login first used at `origin`: its registrable domain, or the
/// whole origin for an IP address, localhost or a host with a port other than the scheme's.
///
/// The whole origin too for *.localhost, a single-label host ("https://nas"), and a host that is
/// itself a public suffix: none has a registrable domain to share. Written as
/// [`site_matches`] reads it back; an empty string, which covers nothing, if `origin` is not an
/// origin at all.
pub fn site_for(origin: &str) -> String {
    let Some(origin) = Origin::parse(origin) else { return String::new() };
    if origin.port.is_none() && !origin.host.is_localhost() {
        if let Host::Domain(host) = &origin.host {
            if let Some(site) = registrable(host) {
                return site.to_string();
            }
        }
    }
    origin.serialize()
}

/// A host, made comparable.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Host {
    /// Lowercase punycode, with no empty label.
    Domain(String),
    Ip(IpAddr),
}

impl Host {
    /// Read a host as an origin carries it, or as a person writes one: an IPv6 address with or
    /// without brackets, an IPv4 address, or a domain in ASCII or Unicode. None for what a
    /// browser would not take as a host.
    fn parse(text: &str) -> Option<Host> {
        if let Some(inner) = text.strip_prefix('[') {
            return inner.strip_suffix(']')?.parse::<Ipv6Addr>().ok().map(|v6| Host::Ip(IpAddr::V6(v6)));
        }
        if text.contains(':') {
            return text.parse::<Ipv6Addr>().ok().map(|v6| Host::Ip(IpAddr::V6(v6)));
        }
        // Unicode into punycode, and lowercase; the characters no host holds (spaces, controls,
        // "%", "/", "@", "\" and the like) refused, as by browsers' URL parser.
        let ascii = idna::domain_to_ascii_cow(text.as_bytes(), idna::AsciiDenyList::URL).ok()?;
        // A host whose last label is a number is an IPv4 address to a browser, or nothing; only
        // the dotted decimal browsers write is read as one.
        if is_number(ascii.rsplit('.').next().unwrap_or_default()) {
            return ascii.parse::<Ipv4Addr>().ok().map(|v4| Host::Ip(IpAddr::V4(v4)));
        }
        if ascii.split('.').any(str::is_empty) {
            return None;
        }
        Some(Host::Domain(ascii.into_owned()))
    }

    /// As [`host_of`] gives it: IPv6 without brackets.
    fn bare(&self) -> String {
        match self {
            Host::Domain(domain) => domain.clone(),
            Host::Ip(ip) => ip.to_string(),
        }
    }

    /// As an origin is written: IPv6 in brackets.
    fn in_origin(&self) -> String {
        match self {
            Host::Ip(IpAddr::V6(v6)) => format!("[{v6}]"),
            other => other.bare(),
        }
    }

    fn is_localhost(&self) -> bool {
        matches!(self, Host::Domain(domain) if domain == "localhost" || domain.ends_with(".localhost"))
    }

    /// localhost, *.localhost, 127.0.0.0/8 or ::1.
    fn is_loopback(&self) -> bool {
        match self {
            Host::Ip(ip) => ip.is_loopback(),
            Host::Domain(_) => self.is_localhost(),
        }
    }
}

/// Is `label` a number as the URL standard's IPv4 parser reads one: decimal digits, or 0x and
/// hexadecimal ones?
fn is_number(label: &str) -> bool {
    match label.strip_prefix("0x") {
        Some(hex) => hex.bytes().all(|b| b.is_ascii_hexdigit()),
        None => !label.is_empty() && label.bytes().all(|b| b.is_ascii_digit()),
    }
}

/// An origin, made comparable: the scheme in lowercase, the host as [`Host`] has it, and the
/// port only when it is not the scheme's own.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Origin {
    scheme: String,
    host: Host,
    port: Option<u16>,
}

impl Origin {
    /// Read "scheme://host[:port]". An origin has no path, but a page's address given in its
    /// place is read for its origin: the host ends where a path, query or fragment would begin.
    fn parse(text: &str) -> Option<Origin> {
        let (scheme, rest) = text.split_once("://")?;
        let scheme_ok = scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        if !scheme_ok {
            return None;
        }
        let scheme = scheme.to_ascii_lowercase();
        let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
        // A name and password before the host are refused rather than skipped:
        // "https://github.com@evil.com" is evil.com made to look like github.com. So is a
        // backslash, which browsers read as a slash.
        if authority.contains(['@', '\\']) {
            return None;
        }
        let (host, port) = if authority.starts_with('[') {
            let close = authority.find(']')?;
            let after = &authority[close + 1..];
            let port = if after.is_empty() { None } else { Some(after.strip_prefix(':')?) };
            (&authority[..=close], port)
        } else {
            match authority.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            }
        };
        let host = Host::parse(host)?;
        let port = match port {
            None => None,
            Some(digits) => {
                if digits.is_empty() || digits.len() > 5 || !digits.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                let port: u16 = digits.parse().ok()?;
                (Some(port) != default_port(&scheme)).then_some(port)
            }
        };
        Some(Origin { scheme, host, port })
    }

    /// As browsers serialize an origin.
    fn serialize(&self) -> String {
        match self.port {
            Some(port) => format!("{}://{}:{port}", self.scheme, self.host.in_origin()),
            None => format!("{}://{}", self.scheme, self.host.in_origin()),
        }
    }

    fn is_secure(&self) -> bool {
        match self.scheme.as_str() {
            "https" => true,
            "http" => self.host.is_loopback(),
            _ => false,
        }
    }
}

fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        "ftp" => Some(21),
        _ => None,
    }
}

// Which kinds of rule the list has for a name: "name", "*.name", "!name".
const NORMAL: u8 = 1;
const WILDCARD: u8 = 2;
const EXCEPTION: u8 = 4;

/// The list's rules: for each name (in punycode, without its "*." or "!"), the kinds of rule
/// written for it.
fn rules() -> &'static HashMap<String, u8> {
    static RULES: OnceLock<HashMap<String, u8>> = OnceLock::new();
    RULES.get_or_init(|| read_rules(include_str!("../data/public_suffix_list.dat")))
}

fn read_rules(list: &str) -> HashMap<String, u8> {
    let mut rules = HashMap::new();
    for line in list.lines() {
        // A rule is a line up to its first whitespace; "//" begins a comment.
        let Some(rule) = line.split_whitespace().next() else { continue };
        if rule.starts_with("//") {
            continue;
        }
        let (kind, name) = if let Some(name) = rule.strip_prefix('!') {
            (EXCEPTION, name)
        } else if let Some(name) = rule.strip_prefix("*.") {
            (WILDCARD, name)
        } else {
            (NORMAL, rule)
        };
        let name = if name.is_ascii() {
            name.to_ascii_lowercase()
        } else {
            match idna::domain_to_ascii(name) {
                Ok(ascii) => ascii,
                Err(_) => {
                    log::warn!("public suffix list: {rule} is not a domain name; left out");
                    continue;
                }
            }
        };
        *rules.entry(name).or_insert(0) |= kind;
    }
    rules
}

/// Where each label of `domain` begins, leftmost first.
fn label_starts(domain: &str) -> Vec<usize> {
    std::iter::once(0).chain(domain.match_indices('.').map(|(at, _)| at + 1)).collect()
}

/// How many of `domain`'s labels, counted from the right, are its public suffix, by
/// publicsuffix.org's algorithm: an exception rule prevails over every other, and counts one
/// label less than itself; failing that, the longest rule that matches, a wildcard matching any
/// one label; and with none, the default rule "*", the last label alone.
fn suffix_labels(domain: &str, starts: &[usize]) -> usize {
    let rules = rules();
    let count = starts.len();
    // The kinds of rule written for the name made of the labels from the i-th on.
    let kinds = |i: usize| rules.get(&domain[starts[i]..]).copied().unwrap_or(0);
    if let Some(i) = (0..count).find(|&i| kinds(i) & EXCEPTION != 0) {
        return count - i - 1;
    }
    for i in 0..count {
        if kinds(i) & NORMAL != 0 || (i + 1 < count && kinds(i + 1) & WILDCARD != 0) {
            return count - i;
        }
    }
    1
}

/// The registrable domain of a domain already in [`Host`]'s form: its public suffix and one
/// label more.
fn registrable(domain: &str) -> Option<&str> {
    let starts = label_starts(domain);
    let suffix = suffix_labels(domain, &starts);
    (suffix < starts.len()).then(|| &domain[starts[starts.len() - suffix - 1]..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_host_of_an_origin() {
        for (origin, host) in [
            ("https://Login.Example.com:8443", Some("login.example.com")),
            ("https://github.com", Some("github.com")),
            ("HTTPS://GITHUB.COM/login?next=/", Some("github.com")),
            ("https://[::1]:8443", Some("::1")),
            ("http://[2001:DB8:0:0:0:0:0:1]", Some("2001:db8::1")),
            ("http://127.0.0.1:3000", Some("127.0.0.1")),
            ("https://bücher.example", Some("xn--bcher-kva.example")),
            ("https://XN--BCHER-KVA.example", Some("xn--bcher-kva.example")),
            ("https://evil.com/@github.com", Some("evil.com")),
            // Not origins, or not ones a browser writes.
            ("https://github.com@evil.com", None),
            ("https://paulus:hunter2@github.com", None),
            ("https://evil.com\\@github.com", None),
            ("github.com", None),
            ("https://", None),
            ("https://:443", None),
            ("https://exa mple.com", None),
            ("https://example.com:", None),
            ("https://example.com:65536", None),
            ("https://example.com:44a", None),
            ("https://example..com", None),
            ("https://example.com.", None),
            ("https://127.1", None),
            ("https://example.123", None),
            ("https://[::1", None),
            ("https://[::1]x", None),
            ("https://[nonsense]", None),
            ("1https://example.com", None),
            ("null", None),
        ] {
            assert_eq!(host_of(origin).as_deref(), host, "{origin}");
        }
    }

    #[test]
    fn secure_is_https_or_this_machine() {
        for origin in [
            "https://example.com",
            "https://192.168.1.2:8443",
            "http://localhost",
            "http://localhost:3000",
            "http://app.localhost:5173",
            "http://127.0.0.1",
            "http://127.255.0.9:8080",
            "http://[::1]:8080",
            "HTTP://LOCALHOST",
        ] {
            assert!(is_secure_origin(origin), "{origin}");
        }
        for origin in [
            "http://example.com",
            "http://192.168.1.2",
            "http://128.0.0.1",
            "http://[::2]",
            "http://localhost.example.com",
            "http://notlocalhost",
            "ftp://localhost",
            "wss://example.com",
            "file:///etc/passwd",
            "null",
            "",
        ] {
            assert!(!is_secure_origin(origin), "{origin}");
        }
    }

    #[test]
    fn knows_the_public_suffixes() {
        for suffix in [
            "com",
            "co.uk",
            "uk",
            "github.io",
            "foo",
            "localhost",
            // *.ck: every name under ck is a suffix, and so is ck itself (the default rule).
            "ck",
            "anything.ck",
            // *.kawasaki.jp, with !city.kawasaki.jp its exception.
            "foo.kawasaki.jp",
            // The list's rules are Unicode; hosts may come either way.
            "公司.cn",
            "xn--55qx5d.cn",
            "個人.hk",
            "xn--gmqw5a.hk",
            "aéroport.ci",
            "xn--aroport-bya.ci",
            "CO.UK",
            "",
            "a..b",
        ] {
            assert!(is_public_suffix(suffix), "{suffix}");
        }
        for domain in [
            "example.com",
            "example.co.uk",
            "user.github.io",
            "www.ck",
            "city.kawasaki.jp",
            "kawasaki.jp",
            "shop.公司.cn",
            "shop.xn--55qx5d.cn",
            "192.168.1.2",
            "::1",
        ] {
            assert!(!is_public_suffix(domain), "{domain}");
        }
    }

    #[test]
    fn finds_the_registrable_domain() {
        for (host, site) in [
            ("a.b.example.co.uk", Some("example.co.uk")),
            ("example.co.uk", Some("example.co.uk")),
            ("co.uk", None),
            ("gist.github.com", Some("github.com")),
            ("Example.COM", Some("example.com")),
            ("user.github.io", Some("user.github.io")),
            ("a.user.github.io", Some("user.github.io")),
            ("github.io", None),
            ("foo.ck", None),
            ("bar.foo.ck", Some("bar.foo.ck")),
            ("www.ck", Some("www.ck")),
            ("a.www.ck", Some("www.ck")),
            ("city.kawasaki.jp", Some("city.kawasaki.jp")),
            ("www.city.kawasaki.jp", Some("city.kawasaki.jp")),
            ("foo.kawasaki.jp", None),
            ("a.foo.kawasaki.jp", Some("a.foo.kawasaki.jp")),
            ("bucket.s3.amazonaws.com", Some("bucket.s3.amazonaws.com")),
            ("www.shop.公司.cn", Some("shop.xn--55qx5d.cn")),
            ("a.shop.xn--55qx5d.cn", Some("shop.xn--55qx5d.cn")),
            ("www.bücher.example", Some("xn--bcher-kva.example")),
            ("intranet.corp", Some("intranet.corp")),
            ("localhost", None),
            ("app.localhost", Some("app.localhost")),
            ("nas", None),
            ("127.0.0.1", None),
            ("::1", None),
            ("[::1]", None),
            ("", None),
            ("exa mple.com", None),
        ] {
            assert_eq!(registrable_domain(host).as_deref(), site, "{host}");
        }
    }

    /// The list's own tests (publicsuffix.org's test_psl.txt, public domain), with the answers
    /// in punycode, as they are given here.
    #[test]
    fn passes_the_lists_own_tests() {
        for (host, site) in [
            // Mixed case; leading dots.
            ("COM", None),
            ("example.COM", Some("example.com")),
            ("WwW.example.COM", Some("example.com")),
            (".com", None),
            (".example", None),
            (".example.com", None),
            (".example.example", None),
            // An unlisted TLD.
            ("example", None),
            ("example.example", Some("example.example")),
            ("b.example.example", Some("example.example")),
            ("a.b.example.example", Some("example.example")),
            // A TLD with only one rule.
            ("biz", None),
            ("domain.biz", Some("domain.biz")),
            ("b.domain.biz", Some("domain.biz")),
            ("a.b.domain.biz", Some("domain.biz")),
            // A TLD with some two-level rules.
            ("com", None),
            ("example.com", Some("example.com")),
            ("b.example.com", Some("example.com")),
            ("a.b.example.com", Some("example.com")),
            ("uk.com", None),
            ("example.uk.com", Some("example.uk.com")),
            ("b.example.uk.com", Some("example.uk.com")),
            ("a.b.example.uk.com", Some("example.uk.com")),
            ("test.ac", Some("test.ac")),
            // A TLD with only a wildcard rule.
            ("mm", None),
            ("c.mm", None),
            ("b.c.mm", Some("b.c.mm")),
            ("a.b.c.mm", Some("b.c.mm")),
            // A more complex TLD.
            ("jp", None),
            ("test.jp", Some("test.jp")),
            ("www.test.jp", Some("test.jp")),
            ("ac.jp", None),
            ("test.ac.jp", Some("test.ac.jp")),
            ("www.test.ac.jp", Some("test.ac.jp")),
            ("kyoto.jp", None),
            ("test.kyoto.jp", Some("test.kyoto.jp")),
            ("ide.kyoto.jp", None),
            ("b.ide.kyoto.jp", Some("b.ide.kyoto.jp")),
            ("a.b.ide.kyoto.jp", Some("b.ide.kyoto.jp")),
            ("c.kobe.jp", None),
            ("b.c.kobe.jp", Some("b.c.kobe.jp")),
            ("a.b.c.kobe.jp", Some("b.c.kobe.jp")),
            ("city.kobe.jp", Some("city.kobe.jp")),
            ("www.city.kobe.jp", Some("city.kobe.jp")),
            // A wildcard rule with exceptions.
            ("ck", None),
            ("test.ck", None),
            ("b.test.ck", Some("b.test.ck")),
            ("a.b.test.ck", Some("b.test.ck")),
            ("www.ck", Some("www.ck")),
            ("www.www.ck", Some("www.ck")),
            // US K12.
            ("us", None),
            ("test.us", Some("test.us")),
            ("www.test.us", Some("test.us")),
            ("ak.us", None),
            ("test.ak.us", Some("test.ak.us")),
            ("www.test.ak.us", Some("test.ak.us")),
            ("k12.ak.us", None),
            ("test.k12.ak.us", Some("test.k12.ak.us")),
            ("www.test.k12.ak.us", Some("test.k12.ak.us")),
            // IDN labels.
            ("食狮.com.cn", Some("xn--85x722f.com.cn")),
            ("食狮.公司.cn", Some("xn--85x722f.xn--55qx5d.cn")),
            ("www.食狮.公司.cn", Some("xn--85x722f.xn--55qx5d.cn")),
            ("shishi.公司.cn", Some("shishi.xn--55qx5d.cn")),
            ("公司.cn", None),
            ("食狮.中国", Some("xn--85x722f.xn--fiqs8s")),
            ("www.食狮.中国", Some("xn--85x722f.xn--fiqs8s")),
            ("shishi.中国", Some("shishi.xn--fiqs8s")),
            ("中国", None),
            // The same, in punycode.
            ("xn--85x722f.com.cn", Some("xn--85x722f.com.cn")),
            ("xn--85x722f.xn--55qx5d.cn", Some("xn--85x722f.xn--55qx5d.cn")),
            ("www.xn--85x722f.xn--55qx5d.cn", Some("xn--85x722f.xn--55qx5d.cn")),
            ("shishi.xn--55qx5d.cn", Some("shishi.xn--55qx5d.cn")),
            ("xn--55qx5d.cn", None),
            ("xn--85x722f.xn--fiqs8s", Some("xn--85x722f.xn--fiqs8s")),
            ("www.xn--85x722f.xn--fiqs8s", Some("xn--85x722f.xn--fiqs8s")),
            ("shishi.xn--fiqs8s", Some("shishi.xn--fiqs8s")),
            ("xn--fiqs8s", None),
        ] {
            assert_eq!(registrable_domain(host).as_deref(), site, "{host}");
        }
    }

    #[test]
    fn rp_ids_are_judged_as_chrome_judges_them() {
        for (rp_id, origin) in [
            ("example.co.uk", "https://login.example.co.uk"),
            ("login.example.co.uk", "https://login.example.co.uk"),
            ("github.com", "https://github.com"),
            ("github.com", "https://GitHub.com:8443"),
            ("user.github.io", "https://www.user.github.io"),
            ("www.ck", "https://a.www.ck"),
            ("localhost", "http://localhost:3000"),
            ("localhost", "https://localhost"),
            ("a.localhost", "http://b.a.localhost"),
            ("shop.xn--55qx5d.cn", "https://www.shop.公司.cn"),
            // The host itself, though it is a public suffix, as Chrome allows.
            ("nas", "https://nas"),
        ] {
            assert!(rp_id_ok(rp_id, origin), "{rp_id} for {origin}");
        }
        for (rp_id, origin) in [
            ("co.uk", "https://login.example.co.uk"),
            ("uk", "https://login.example.co.uk"),
            ("evil.com", "https://example.com"),
            ("example.com", "https://evil.com"),
            ("ample.com", "https://example.com"),
            ("login.example.com", "https://example.com"),
            ("github.io", "https://user.github.io"),
            ("foo.ck", "https://a.foo.ck"),
            ("amazonaws.com", "https://bucket.s3.amazonaws.com"),
            ("localhost", "http://a.localhost"),
            ("example.com", "http://example.com"),
            ("example.com", "ftp://example.com"),
            // IP-address origins, refused as Chrome refuses them.
            ("127.0.0.1", "http://127.0.0.1"),
            ("192.168.1.2", "https://192.168.1.2"),
            ("::1", "http://[::1]"),
            // Not what will be hashed.
            ("GitHub.com", "https://github.com"),
            ("github.com.", "https://github.com"),
            (" github.com", "https://github.com"),
            ("", "https://github.com"),
            ("github.com", "null"),
        ] {
            assert!(!rp_id_ok(rp_id, origin), "{rp_id} for {origin}");
        }
    }

    #[test]
    fn a_site_covers_its_domain_or_exactly_its_origin() {
        for (site, origin) in [
            ("github.com", "https://github.com"),
            ("github.com", "https://gist.github.com"),
            ("github.com", "https://a.b.github.com:8443"),
            ("GitHub.COM", "https://github.com"),
            (" github.com ", "https://github.com"),
            ("shop.公司.cn", "https://www.shop.xn--55qx5d.cn"),
            ("github.io", "https://github.io"),
            ("nas", "https://nas"),
            ("192.168.1.2", "https://192.168.1.2:8443"),
            ("https://192.168.1.2:8443", "https://192.168.1.2:8443"),
            ("https://example.com:443", "https://example.com"),
            ("https://example.com", "https://EXAMPLE.com:443"),
            ("http://[::1]:8080", "http://[0:0:0:0:0:0:0:1]:8080"),
        ] {
            assert!(site_matches(site, origin), "{site} for {origin}");
        }
        for (site, origin) in [
            ("github.com", "https://evilgithub.com"),
            ("github.com", "https://github.com.evil.com"),
            ("gist.github.com", "https://github.com"),
            // A public suffix covers only itself.
            ("github.io", "https://user.github.io"),
            ("com", "https://example.com"),
            ("nas", "https://a.nas"),
            ("192.168.1.2", "https://192.168.1.20"),
            ("https://192.168.1.2:8443", "https://192.168.1.2"),
            ("https://192.168.1.2:8443", "https://192.168.1.2:8444"),
            ("https://example.com", "http://example.com"),
            ("https://example.com", "https://www.example.com"),
            ("github.com", "https://github.com@evil.com"),
            ("", "https://github.com"),
            ("github.com", "garbage"),
        ] {
            assert!(!site_matches(site, origin), "{site} for {origin}");
        }
    }

    #[test]
    fn a_new_logins_site_is_its_domain_or_its_whole_origin() {
        for (origin, site) in [
            ("https://github.com", "github.com"),
            ("https://gist.github.com", "github.com"),
            ("https://login.example.co.uk", "example.co.uk"),
            ("https://example.com:443", "example.com"),
            ("https://user.github.io", "user.github.io"),
            ("https://Shop.公司.cn", "shop.xn--55qx5d.cn"),
            ("https://192.168.1.2:8443", "https://192.168.1.2:8443"),
            ("https://192.168.1.2", "https://192.168.1.2"),
            ("http://localhost:3000", "http://localhost:3000"),
            ("http://app.localhost", "http://app.localhost"),
            ("https://[::1]:8443", "https://[::1]:8443"),
            ("https://nas", "https://nas"),
            ("https://example.com:8443", "https://example.com:8443"),
            ("https://github.io", "https://github.io"),
            ("not an origin", ""),
        ] {
            assert_eq!(site_for(origin), site, "{origin}");
        }
        // What it keeps covers the page it came from.
        for origin in ["https://gist.github.com", "https://192.168.1.2:8443", "http://localhost:3000", "https://nas"] {
            assert!(site_matches(&site_for(origin), origin), "{origin}");
        }
    }

    #[test]
    fn reads_every_kind_of_rule() {
        let small = read_rules("// a comment\n\nexample\n*.wild.example   trailing words\n!keep.wild.example\nbücher.example\n");
        assert_eq!(small.get("example"), Some(&NORMAL));
        assert_eq!(small.get("wild.example"), Some(&WILDCARD));
        assert_eq!(small.get("keep.wild.example"), Some(&EXCEPTION));
        assert_eq!(small.get("xn--bcher-kva.example"), Some(&NORMAL));
        assert_eq!(small.len(), 4);
        // The whole list: every rule read, none of them left in Unicode.
        assert!(rules().len() > 9000);
        assert!(rules().keys().all(|name| name.is_ascii()));
    }
}
