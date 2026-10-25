//! declarativeNetRequest: extensions' rules, read and matched as Chrome reads and matches them
//! (developer.chrome.com/docs/extensions/reference/api/declarativeNetRequest; Chromium's
//! components/url_pattern_index and extensions/browser/api/declarative_net_request), for an
//! engine that asks about each request ([`Rules::decide`], Steam's Chromium), and as WebKit's
//! content blockers for an engine that is given rules to apply itself ([`webkit_rules`]).

use std::collections::HashMap;

use regex::Regex;
use serde_json::{json, Map, Value};

/// What a request is for, as declarativeNetRequest names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceType {
    MainFrame,
    SubFrame,
    Stylesheet,
    Script,
    Image,
    Font,
    Object,
    XmlHttpRequest,
    Ping,
    CspReport,
    Media,
    Websocket,
    Webtransport,
    Webbundle,
    Other,
}

impl ResourceType {
    pub const ALL: [ResourceType; 15] = [
        ResourceType::MainFrame,
        ResourceType::SubFrame,
        ResourceType::Stylesheet,
        ResourceType::Script,
        ResourceType::Image,
        ResourceType::Font,
        ResourceType::Object,
        ResourceType::XmlHttpRequest,
        ResourceType::Ping,
        ResourceType::CspReport,
        ResourceType::Media,
        ResourceType::Websocket,
        ResourceType::Webtransport,
        ResourceType::Webbundle,
        ResourceType::Other,
    ];

    pub fn parse(name: &str) -> Option<ResourceType> {
        Some(match name {
            "main_frame" => ResourceType::MainFrame,
            "sub_frame" => ResourceType::SubFrame,
            "stylesheet" => ResourceType::Stylesheet,
            "script" => ResourceType::Script,
            "image" => ResourceType::Image,
            "font" => ResourceType::Font,
            "object" => ResourceType::Object,
            "xmlhttprequest" => ResourceType::XmlHttpRequest,
            "ping" => ResourceType::Ping,
            "csp_report" => ResourceType::CspReport,
            "media" => ResourceType::Media,
            "websocket" => ResourceType::Websocket,
            "webtransport" => ResourceType::Webtransport,
            "webbundle" => ResourceType::Webbundle,
            "other" => ResourceType::Other,
            _ => return None,
        })
    }

    /// Its name, as declarativeNetRequest writes it.
    pub fn name(self) -> &'static str {
        match self {
            ResourceType::MainFrame => "main_frame",
            ResourceType::SubFrame => "sub_frame",
            ResourceType::Stylesheet => "stylesheet",
            ResourceType::Script => "script",
            ResourceType::Image => "image",
            ResourceType::Font => "font",
            ResourceType::Object => "object",
            ResourceType::XmlHttpRequest => "xmlhttprequest",
            ResourceType::Ping => "ping",
            ResourceType::CspReport => "csp_report",
            ResourceType::Media => "media",
            ResourceType::Websocket => "websocket",
            ResourceType::Webtransport => "webtransport",
            ResourceType::Webbundle => "webbundle",
            ResourceType::Other => "other",
        }
    }

    pub fn is_frame(self) -> bool {
        matches!(self, ResourceType::MainFrame | ResourceType::SubFrame)
    }
}

/// What a rule does.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Block,
    Allow,
    AllowAllRequests,
    UpgradeScheme,
    /// The rule's `redirect` object.
    Redirect(Value),
    /// The rule's `requestHeaders` and `responseHeaders`.
    ModifyHeaders(Value, Value),
}

impl Action {
    /// Which wins among rules of one extension of the same priority: lower first (Chrome's
    /// order: allow and allowAllRequests, block, upgradeScheme, redirect).
    fn rank(&self) -> u8 {
        match self {
            Action::Allow | Action::AllowAllRequests => 0,
            Action::Block => 1,
            Action::UpgradeScheme => 2,
            Action::Redirect(_) => 3,
            Action::ModifyHeaders(..) => 4,
        }
    }
}

/// One rule, read.
#[derive(Debug, Clone)]
pub struct Rule {
    pub id: i64,
    pub priority: i64,
    pub action: Action,
    /// `urlFilter` as written, or `regexFilter`'s source; and the expression either is matched by.
    url_filter: Option<String>,
    regex_filter: Option<String>,
    url: Option<Regex>,
    case_sensitive: bool,
    initiator_domains: Option<Vec<String>>,
    excluded_initiator_domains: Vec<String>,
    request_domains: Option<Vec<String>>,
    excluded_request_domains: Vec<String>,
    resource_types: Option<Vec<ResourceType>>,
    excluded_resource_types: Vec<ResourceType>,
    request_methods: Option<Vec<String>>,
    excluded_request_methods: Vec<String>,
    /// `domainType`: Some(true) for third-party requests only, Some(false) first-party.
    third_party: Option<bool>,
    tab_ids: Option<Vec<i64>>,
    excluded_tab_ids: Vec<i64>,
}

fn strings(v: Option<&Value>) -> Option<Vec<String>> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
}

fn lower(v: Option<Vec<String>>) -> Option<Vec<String>> {
    v.map(|list| list.into_iter().map(|s| s.to_ascii_lowercase()).collect())
}

fn types(v: Option<&Value>) -> Option<Vec<ResourceType>> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).filter_map(ResourceType::parse).collect())
}

fn numbers(v: Option<&Value>) -> Option<Vec<i64>> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect())
}

/// `urlFilter`'s language as a regular expression's source: `*` any characters, `^` a separator
/// (any character but a letter, a digit or one of `_-.%`, or the end), `|` at either end an
/// anchor, `||` at the start the start of the host or of one of its subdomains.
pub fn url_filter_regex(filter: &str) -> String {
    let mut out = String::new();
    let mut rest = filter;
    if let Some(r) = rest.strip_prefix("||") {
        out.push_str(r"^[a-zA-Z][a-zA-Z0-9+.-]*://(?:[^/?#@]*@)?(?:[^/?#:@]*\.)?");
        rest = r;
    } else if let Some(r) = rest.strip_prefix('|') {
        out.push('^');
        rest = r;
    }
    let (body, end) = match rest.strip_suffix('|') {
        Some(b) => (b, true),
        None => (rest, false),
    };
    for c in body.chars() {
        match c {
            '*' => out.push_str(".*"),
            '^' => out.push_str(r"(?:[^a-zA-Z0-9_.%-]|$)"),
            c => out.push_str(&regex::escape(&c.to_string())),
        }
    }
    if end {
        out.push('$');
    }
    out
}

impl Rule {
    /// A rule as an extension wrote it; Err with why Chrome would refuse it.
    pub fn parse(v: &Value) -> Result<Rule, String> {
        let id = v.get("id").and_then(Value::as_i64).ok_or("A rule needs an id.")?;
        let priority = v.get("priority").and_then(Value::as_i64).unwrap_or(1);
        let a = v.get("action").ok_or_else(|| format!("Rule with id {id} has no action."))?;
        let action = match a.get("type").and_then(Value::as_str).unwrap_or("") {
            "block" => Action::Block,
            "allow" => Action::Allow,
            "allowAllRequests" => Action::AllowAllRequests,
            "upgradeScheme" => Action::UpgradeScheme,
            "redirect" => Action::Redirect(a.get("redirect").cloned().ok_or_else(|| format!("Rule with id {id} does not specify the redirect."))?),
            "modifyHeaders" => {
                Action::ModifyHeaders(a.get("requestHeaders").cloned().unwrap_or(Value::Null), a.get("responseHeaders").cloned().unwrap_or(Value::Null))
            }
            other => return Err(format!("Rule with id {id} has an unknown action type: {other}.")),
        };
        let c = v.get("condition").cloned().unwrap_or(Value::Object(Map::new()));
        let case_sensitive = c.get("isUrlFilterCaseSensitive").and_then(Value::as_bool).unwrap_or(false);
        let url_filter = c.get("urlFilter").and_then(Value::as_str).map(str::to_string);
        let regex_filter = c.get("regexFilter").and_then(Value::as_str).map(str::to_string);
        let source = match (&url_filter, &regex_filter) {
            (Some(_), Some(_)) => return Err(format!("Rule with id {id} specifies both urlFilter and regexFilter.")),
            (Some(f), None) => Some(url_filter_regex(f)),
            (None, Some(r)) => Some(r.clone()),
            (None, None) => None,
        };
        let url = match source {
            Some(s) => Some(
                regex::RegexBuilder::new(&s)
                    .case_insensitive(!case_sensitive)
                    .build()
                    .map_err(|e| format!("Rule with id {id} specifies an invalid regular expression: {e}"))?,
            ),
            None => None,
        };
        let domain_type = c.get("domainType").and_then(Value::as_str);
        Ok(Rule {
            id,
            priority,
            action,
            url_filter,
            regex_filter,
            url,
            case_sensitive,
            // `domains` and `excludedDomains` are what Manifest V3's first rules said.
            initiator_domains: lower(strings(c.get("initiatorDomains")).or_else(|| strings(c.get("domains")))),
            excluded_initiator_domains: lower(strings(c.get("excludedInitiatorDomains")).or_else(|| strings(c.get("excludedDomains")))).unwrap_or_default(),
            request_domains: lower(strings(c.get("requestDomains"))),
            excluded_request_domains: lower(strings(c.get("excludedRequestDomains"))).unwrap_or_default(),
            resource_types: types(c.get("resourceTypes")),
            excluded_resource_types: types(c.get("excludedResourceTypes")).unwrap_or_default(),
            request_methods: lower(strings(c.get("requestMethods"))),
            excluded_request_methods: lower(strings(c.get("excludedRequestMethods"))).unwrap_or_default(),
            third_party: domain_type.map(|t| t == "thirdParty"),
            tab_ids: numbers(c.get("tabIds")),
            excluded_tab_ids: numbers(c.get("excludedTabIds")).unwrap_or_default(),
        })
    }

    /// The resource types it is for: as written, or every one but the top frame's.
    fn wants_type(&self, t: ResourceType) -> bool {
        let listed = match &self.resource_types {
            Some(list) => list.contains(&t),
            None => t != ResourceType::MainFrame,
        };
        listed && !self.excluded_resource_types.contains(&t)
    }

    fn matches(&self, r: &Request, host: &str, initiator_host: Option<&str>) -> bool {
        if !self.wants_type(r.resource_type) {
            return false;
        }
        let method = r.method.to_ascii_lowercase();
        if self.request_methods.as_ref().is_some_and(|m| !m.contains(&method)) || self.excluded_request_methods.contains(&method) {
            return false;
        }
        if let Some(tab) = r.tab {
            if self.tab_ids.as_ref().is_some_and(|t| !t.contains(&tab)) || self.excluded_tab_ids.contains(&tab) {
                return false;
            }
        } else if self.tab_ids.is_some() {
            return false;
        }
        if self.request_domains.as_ref().is_some_and(|d| !d.iter().any(|d| within(host, d))) || self.excluded_request_domains.iter().any(|d| within(host, d)) {
            return false;
        }
        match initiator_host {
            Some(i) => {
                if self.initiator_domains.as_ref().is_some_and(|d| !d.iter().any(|d| within(i, d))) || self.excluded_initiator_domains.iter().any(|d| within(i, d)) {
                    return false;
                }
            }
            // The browser's own request: no page's domain to be among those named.
            None if self.initiator_domains.is_some() => return false,
            None => {}
        }
        if let Some(third) = self.third_party {
            let is_third = initiator_host.is_some_and(|i| !same_site(i, host));
            if third != is_third {
                return false;
            }
        }
        self.url.as_ref().map_or(true, |re| re.is_match(r.url))
    }
}

/// Whether `host` is `domain` or one of its subdomains.
fn within(host: &str, domain: &str) -> bool {
    host == domain || host.strip_suffix(domain).is_some_and(|rest| rest.ends_with('.'))
}

/// Whether two hosts are of one site: the same last two labels (three where the second-last
/// is short, as in co.uk), without the public suffix list.
fn same_site(a: &str, b: &str) -> bool {
    fn site(host: &str) -> String {
        let labels: Vec<&str> = host.trim_end_matches('.').split('.').collect();
        if host.parse::<std::net::IpAddr>().is_ok() || labels.len() <= 2 {
            return host.to_string();
        }
        let n = labels.len();
        let take = if labels[n - 2].len() <= 3 && labels[n - 1].len() == 2 { 3 } else { 2 };
        labels[n.saturating_sub(take)..].join(".")
    }
    site(a) == site(b)
}

fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(|h| h.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase()))
}

/// A request, as an engine sees it.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    pub url: &'a str,
    /// The page (its URL or origin) that makes it; None for the browser's own (a typed address,
    /// a background the window opens).
    pub initiator: Option<&'a str>,
    pub resource_type: ResourceType,
    pub method: &'a str,
    pub tab: Option<i64>,
}

/// What is to happen to a request.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Block,
    /// To this address instead (an upgraded scheme, or a redirect).
    Redirect(String),
}

/// One extension's rules, indexed for asking about every request.
#[derive(Debug, Clone, Default)]
pub struct Rules {
    /// The extension's id, and its origin for redirects to its own files.
    id: String,
    origin: String,
    rules: Vec<Rule>,
    /// Rules whose urlFilter begins `||domain`, by that domain: the only ones a request to a
    /// host can match among them are those of the host and its parents.
    by_domain: HashMap<String, Vec<usize>>,
    /// The rest, tried on every request.
    others: Vec<usize>,
}

/// The domain an `||`-anchored urlFilter starts with, if its anchor is followed by a whole
/// host name (what ad blockers' rules mostly are).
fn anchored_domain(filter: &str) -> Option<String> {
    let rest = filter.strip_prefix("||")?;
    let end = rest.find(|c: char| matches!(c, '^' | '/' | '*' | '|' | ':' | '?')).unwrap_or(rest.len());
    let domain = &rest[..end];
    // A domain cut short by a wildcard (`||ads*`) is no whole host name.
    if domain.is_empty() || rest[end..].starts_with('*') && !rest[end..].starts_with("*^") {
        return None;
    }
    domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.').then(|| domain.to_ascii_lowercase())
}

impl Rules {
    /// An extension's rules (its enabled static rule sets', then its dynamic and session ones),
    /// those Chrome would refuse left out (and said in the log).
    pub fn new(id: &str, origin: &str, written: &[Value]) -> Rules {
        let mut rules = Rules { id: id.to_string(), origin: origin.to_string(), ..Rules::default() };
        for v in written {
            match Rule::parse(v) {
                Ok(rule) => {
                    let at = rules.rules.len();
                    match rule.url_filter.as_deref().and_then(anchored_domain) {
                        Some(domain) => rules.by_domain.entry(domain).or_default().push(at),
                        None => rules.others.push(at),
                    }
                    rules.rules.push(rule);
                }
                Err(e) => log::warn!("extension {id}: {e}"),
            }
        }
        rules
    }

    /// The extension's id: what its content-blocker list is called in an engine.
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// The rule that decides a request, among this extension's: the highest priority, then
    /// Chrome's order of actions. Header changes are not deciding.
    fn winner(&self, r: &Request) -> Option<&Rule> {
        let host = host_of(r.url).unwrap_or_default();
        let initiator_host = r.initiator.and_then(host_of);
        let mut candidates: Vec<usize> = self.others.clone();
        let mut parent = host.as_str();
        loop {
            if let Some(list) = self.by_domain.get(parent) {
                candidates.extend(list);
            }
            match parent.find('.') {
                Some(dot) => parent = &parent[dot + 1..],
                None => break,
            }
        }
        let mut best: Option<&Rule> = None;
        for &i in &candidates {
            let rule = &self.rules[i];
            if matches!(rule.action, Action::ModifyHeaders(..)) || !rule.matches(r, &host, initiator_host.as_deref()) {
                continue;
            }
            let better = match best {
                None => true,
                Some(b) => rule.priority > b.priority || (rule.priority == b.priority && rule.action.rank() < b.action.rank()),
            };
            if better {
                best = Some(rule);
            }
        }
        // A frame an allowAllRequests rule took in lets all its requests through, below that
        // rule's priority: asked here of the frame's own address.
        if r.resource_type != ResourceType::MainFrame {
            if let Some(page) = r.initiator.filter(|p| p.contains("://")) {
                let frame = Request { url: page, initiator: None, resource_type: ResourceType::MainFrame, method: "get", tab: r.tab };
                let page_host = host_of(page).unwrap_or_default();
                for rule in self.rules.iter().filter(|rule| rule.action == Action::AllowAllRequests) {
                    if rule.matches(&frame, &page_host, None) && best.map_or(true, |b| rule.priority >= b.priority) {
                        return Some(rule);
                    }
                }
            }
        }
        best
    }

    /// What this extension's rules make of a request, if anything.
    pub fn decide(&self, r: &Request) -> Option<Decision> {
        let rule = self.winner(r)?;
        match &rule.action {
            Action::Block => Some(Decision::Block),
            Action::UpgradeScheme => upgraded(r.url).map(Decision::Redirect),
            Action::Redirect(to) => self.redirect(rule, to, r.url).filter(|u| u != r.url).map(Decision::Redirect),
            _ => None,
        }
    }

    fn redirect(&self, rule: &Rule, to: &Value, url: &str) -> Option<String> {
        if let Some(u) = to.get("url").and_then(Value::as_str) {
            return Some(u.to_string());
        }
        if let Some(p) = to.get("extensionPath").and_then(Value::as_str) {
            return Some(format!("{}/{}", self.origin, p.trim_start_matches('/')));
        }
        if let Some(sub) = to.get("regexSubstitution").and_then(Value::as_str) {
            let re = rule.url.as_ref()?;
            let caps = re.captures(url)?;
            // Chrome's \1 ... \9 and \0, as the matched groups.
            let mut out = String::new();
            let mut chars = sub.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(d) = chars.peek().and_then(|d| d.to_digit(10)) {
                        chars.next();
                        out.push_str(caps.get(d as usize).map_or("", |m| m.as_str()));
                        continue;
                    }
                }
                out.push(c);
            }
            return Some(out);
        }
        let transform = to.get("transform")?;
        transformed(url, transform)
    }
}

/// http:// as https://, ws:// as wss://.
fn upgraded(url: &str) -> Option<String> {
    let mut u = url::Url::parse(url).ok()?;
    let scheme = match u.scheme() {
        "http" => "https",
        "ws" => "wss",
        _ => return None,
    };
    u.set_scheme(scheme).ok()?;
    Some(u.to_string())
}

/// A URL changed as a redirect's `transform` says.
fn transformed(url: &str, t: &Value) -> Option<String> {
    let mut u = url::Url::parse(url).ok()?;
    let text = |k: &str| t.get(k).and_then(Value::as_str);
    if let Some(s) = text("scheme") {
        u.set_scheme(s).ok()?;
    }
    if let Some(h) = text("host") {
        u.set_host(Some(h)).ok()?;
    }
    if let Some(p) = text("port") {
        u.set_port(if p.is_empty() { None } else { p.parse().ok() }).ok()?;
    }
    if let Some(p) = text("path") {
        u.set_path(p);
    }
    if let Some(q) = text("query") {
        u.set_query(if q.is_empty() || q == "?" { None } else { Some(q.trim_start_matches('?')) });
    }
    if let Some(qt) = t.get("queryTransform") {
        let remove: Vec<String> = strings(qt.get("removeParams")).unwrap_or_default();
        let mut pairs: Vec<(String, String)> = u.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).filter(|(k, _)| !remove.contains(k)).collect();
        for add in qt.get("addOrReplaceParams").and_then(Value::as_array).cloned().unwrap_or_default() {
            let (Some(k), Some(v)) = (add.get("key").and_then(Value::as_str), add.get("value").and_then(Value::as_str)) else { continue };
            let replace_only = add.get("replaceOnly").and_then(Value::as_bool).unwrap_or(false);
            match pairs.iter_mut().find(|(pk, _)| pk == k) {
                Some(p) => p.1 = v.to_string(),
                None if !replace_only => pairs.push((k.to_string(), v.to_string())),
                None => {}
            }
        }
        if pairs.is_empty() {
            u.set_query(None);
        } else {
            u.query_pairs_mut().clear().extend_pairs(pairs);
        }
    }
    if let Some(f) = text("fragment") {
        u.set_fragment(if f.is_empty() || f == "#" { None } else { Some(f.trim_start_matches('#')) });
    }
    if let Some(user) = text("username") {
        u.set_username(user).ok()?;
    }
    if let Some(p) = text("password") {
        u.set_password(Some(p)).ok()?;
    }
    Some(u.to_string())
}

/// What all of a profile's extensions make of a request (Chrome's order across extensions: one
/// that blocks wins, then the first that redirects or upgrades; an allow decides nothing for
/// another extension).
pub fn decide(all: &[Rules], r: &Request) -> Option<Decision> {
    let decisions: Vec<Decision> = all.iter().filter_map(|rules| rules.decide(r)).collect();
    if decisions.contains(&Decision::Block) {
        return Some(Decision::Block);
    }
    decisions.into_iter().next()
}

// ---- for an engine that matches them itself ---------------------------------------------------

/// An extension's rules as an engine matching them itself is given them (Android's WebView did;
/// no engine now does): each rule's conditions already read, its urlFilter as a regular
/// expression, its resource types as the list it is for, a redirect to a fixed address as that
/// address. Left out, as that engine cannot apply them: header changes, rules about tabs, and
/// redirects by `transform`.
/// `{"id", "rules": [{"p" priority, "a" action (block, allow, allowAllRequests, upgradeScheme,
/// redirect), "r" expression, "cs" case-sensitive, "d" the domain an `||` filter is anchored at,
/// "t" types, "m"/"xm" methods, "i"/"xi" initiator domains, "q"/"xq" request domains, "tp"
/// third party, "to" a redirect's address, "sub" a redirect's regexSubstitution}]}`.
pub fn digest(rules: &Rules) -> Value {
    let mut out = Vec::new();
    for rule in &rules.rules {
        if rule.tab_ids.is_some() || !rule.excluded_tab_ids.is_empty() {
            continue;
        }
        let mut d = Map::new();
        let action = match &rule.action {
            Action::Block => "block",
            Action::Allow => "allow",
            Action::AllowAllRequests => "allowAllRequests",
            Action::UpgradeScheme => "upgradeScheme",
            Action::Redirect(to) => {
                if let Some(u) = to.get("url").and_then(Value::as_str) {
                    d.insert("to".into(), json!(u));
                } else if let Some(p) = to.get("extensionPath").and_then(Value::as_str) {
                    d.insert("to".into(), json!(format!("{}/{}", rules.origin, p.trim_start_matches('/'))));
                } else if let Some(sub) = to.get("regexSubstitution").and_then(Value::as_str) {
                    d.insert("sub".into(), json!(sub));
                } else {
                    continue;
                }
                "redirect"
            }
            Action::ModifyHeaders(..) => continue,
        };
        d.insert("p".into(), json!(rule.priority));
        d.insert("a".into(), json!(action));
        if let Some(re) = &rule.url {
            d.insert("r".into(), json!(re.as_str()));
        }
        if rule.case_sensitive {
            d.insert("cs".into(), json!(true));
        }
        if let Some(domain) = rule.url_filter.as_deref().and_then(anchored_domain) {
            d.insert("d".into(), json!(domain));
        }
        let types: Vec<&str> = ResourceType::ALL.iter().filter(|t| rule.wants_type(**t)).map(|t| t.name()).collect();
        d.insert("t".into(), json!(types));
        let mut list = |key: &str, v: &Option<Vec<String>>| {
            if let Some(v) = v {
                d.insert(key.into(), json!(v));
            }
        };
        list("m", &rule.request_methods);
        list("i", &rule.initiator_domains);
        list("q", &rule.request_domains);
        for (key, v) in [("xm", &rule.excluded_request_methods), ("xi", &rule.excluded_initiator_domains), ("xq", &rule.excluded_request_domains)] {
            if !v.is_empty() {
                d.insert(key.into(), json!(v));
            }
        }
        if let Some(third) = rule.third_party {
            d.insert("tp".into(), json!(third));
        }
        out.push(Value::Object(d));
    }
    json!({ "id": rules.id, "rules": out })
}

// ---- WebKit's content blockers ----------------------------------------------------------------

/// WebKit's names for what a rule of these types is for, and whether it is a frame's document
/// at the top (Some(true)) or in a frame (Some(false)), or not a document (None).
fn webkit_types(types: &[ResourceType]) -> Vec<(Vec<&'static str>, Option<bool>)> {
    let mut plain: Vec<&'static str> = Vec::new();
    for t in types {
        let name = match t {
            ResourceType::MainFrame | ResourceType::SubFrame => continue,
            ResourceType::Stylesheet => "style-sheet",
            ResourceType::Script => "script",
            ResourceType::Image => "image",
            ResourceType::Font => "font",
            ResourceType::XmlHttpRequest => "raw",
            ResourceType::Ping => "ping",
            ResourceType::Media => "media",
            ResourceType::Websocket => "websocket",
            ResourceType::Object | ResourceType::CspReport | ResourceType::Webtransport | ResourceType::Webbundle | ResourceType::Other => "other",
        };
        if !plain.contains(&name) {
            plain.push(name);
        }
    }
    let mut out = Vec::new();
    if !plain.is_empty() {
        out.push((plain, None));
    }
    match (types.contains(&ResourceType::MainFrame), types.contains(&ResourceType::SubFrame)) {
        (true, true) => out.push((vec!["document"], None)),
        (true, false) => out.push((vec!["document"], Some(true))),
        (false, true) => out.push((vec!["document"], Some(false))),
        (false, false) => {}
    }
    out
}

/// urlFilter in WebKit's narrower language (no alternatives, no non-capturing groups): its `^`
/// is any separator character, and does not match the end of the address.
fn webkit_url_filter(filter: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = filter;
    if let Some(r) = rest.strip_prefix("||") {
        out.push_str("^[^:]+://+([^/]+\\.)?");
        rest = r;
    } else if let Some(r) = rest.strip_prefix('|') {
        out.push('^');
        rest = r;
    }
    let (body, end) = match rest.strip_suffix('|') {
        Some(b) => (b, true),
        None => (rest, false),
    };
    for c in body.chars() {
        match c {
            '*' => out.push_str(".*"),
            '^' => out.push_str("[^a-zA-Z0-9_.%-]"),
            c if !c.is_ascii() => return None,
            c if "\\.?+()[]{}$|".contains(c) => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    if end {
        out.push('$');
    }
    Some(if out.is_empty() { ".*".to_string() } else { out })
}

/// A regexFilter WebKit can take as its url-filter as written, if it can.
fn webkit_regex(source: &str) -> Option<String> {
    let unsupported = ["|", "{", "(?", "\\d", "\\D", "\\w", "\\W", "\\s", "\\S", "\\b", "\\B"];
    (source.is_ascii() && !unsupported.iter().any(|u| source.contains(u))).then(|| source.to_string())
}

/// An extension's rules as one WebKit content-blocker list (JSON): block as "block", allow and
/// allowAllRequests as "ignore-previous-rules", upgradeScheme as "make-https", in order of
/// priority so that what wins in Chrome comes last. What WebKit cannot say is left out: redirects
/// and header changes, rules about request methods or tabs, rules about request domains together
/// with an anchored urlFilter or excluding request domains, and regexFilters past WebKit's
/// language.
pub fn webkit_rules(rules: &Rules) -> String {
    let mut ordered: Vec<&Rule> = rules.rules.iter().filter(|r| matches!(r.action, Action::Block | Action::Allow | Action::AllowAllRequests | Action::UpgradeScheme)).collect();
    // Later rules win in WebKit: lower priority first, and at one priority the action Chrome
    // prefers last.
    ordered.sort_by(|a, b| a.priority.cmp(&b.priority).then(b.action.rank().cmp(&a.action.rank())));
    let mut out: Vec<Value> = Vec::new();
    for rule in ordered {
        if !rule.excluded_request_domains.is_empty()
            || rule.tab_ids.is_some()
            || !rule.excluded_tab_ids.is_empty()
            || rule.request_methods.is_some()
            || !rule.excluded_request_methods.is_empty()
        {
            continue;
        }
        let filter = match (&rule.url_filter, &rule.regex_filter) {
            (Some(f), _) => webkit_url_filter(f),
            (None, Some(r)) => webkit_regex(r),
            (None, None) => Some(".*".to_string()),
        };
        let Some(filter) = filter else { continue };
        // Request domains, written into the url-filter: one rule for each.
        let filters: Vec<String> = match &rule.request_domains {
            None => vec![filter],
            Some(domains) => {
                if filter.starts_with('^') {
                    continue;
                }
                let tail = if filter == ".*" { String::new() } else { format!(".*{filter}") };
                domains.iter().map(|d| format!("^[^:]+://+([^/]+\\.)?{}[/:]{tail}", d.replace('.', "\\."))).collect()
            }
        };
        let types: Vec<ResourceType> = ResourceType::ALL.iter().copied().filter(|t| rule.wants_type(*t)).collect();
        let action = match rule.action {
            Action::Block => "block",
            Action::UpgradeScheme => "make-https",
            _ => "ignore-previous-rules",
        };
        for url_filter in &filters {
            for (names, top) in webkit_types(&types) {
                let mut trigger = Map::new();
                trigger.insert("url-filter".into(), json!(url_filter));
                if rule.case_sensitive {
                    trigger.insert("url-filter-is-case-sensitive".into(), json!(true));
                }
                trigger.insert("resource-type".into(), json!(names));
                if let Some(top) = top {
                    trigger.insert("load-context".into(), json!([if top { "top-frame" } else { "child-frame" }]));
                }
                if let Some(third) = rule.third_party {
                    trigger.insert("load-type".into(), json!([if third { "third-party" } else { "first-party" }]));
                }
                // WebKit takes one of if-domain and unless-domain: the first where both are said.
                match (&rule.initiator_domains, rule.excluded_initiator_domains.is_empty()) {
                    (Some(d), _) => {
                        trigger.insert("if-domain".into(), json!(d.iter().map(|d| format!("*{d}")).collect::<Vec<_>>()));
                    }
                    (None, false) => {
                        trigger.insert("unless-domain".into(), json!(rule.excluded_initiator_domains.iter().map(|d| format!("*{d}")).collect::<Vec<_>>()));
                    }
                    (None, true) => {}
                }
                out.push(json!({ "trigger": trigger, "action": { "type": action } }));
            }
        }
    }
    Value::Array(out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digested_for_an_engine_that_matches_itself() {
        let r = rules(json!([
            { "id": 1, "action": { "type": "block" }, "condition": { "urlFilter": "||ads.example^", "resourceTypes": ["script"], "domainType": "thirdParty" } },
            { "id": 2, "priority": 3, "action": { "type": "redirect", "redirect": { "extensionPath": "/stub.js" } }, "condition": { "urlFilter": "track.js" } },
            { "id": 3, "action": { "type": "redirect", "redirect": { "transform": { "scheme": "https" } } }, "condition": {} },
            { "id": 4, "action": { "type": "block" }, "condition": { "tabIds": [1] } },
            { "id": 5, "action": { "type": "allow" }, "condition": { "requestMethods": ["post"], "excludedInitiatorDomains": ["a.com"] } }
        ]));
        let d = digest(&r);
        let all = d["rules"].as_array().unwrap();
        assert_eq!(d["id"], "ext");
        assert_eq!(all.len(), 3, "{all:?}");
        assert_eq!(all[0]["a"], "block");
        assert_eq!(all[0]["d"], "ads.example");
        assert_eq!(all[0]["t"], json!(["script"]));
        assert_eq!(all[0]["tp"], true);
        assert!(Regex::new(&format!("(?i){}", all[0]["r"].as_str().unwrap())).unwrap().is_match("https://cdn.ADS.example/x.js"));
        assert_eq!(all[1]["to"], "chrome-extension://ext/stub.js");
        assert_eq!(all[1]["p"], 3);
        assert!(!all[1]["t"].as_array().unwrap().contains(&json!("main_frame")));
        assert_eq!(all[2]["m"], json!(["post"]));
        assert_eq!(all[2]["xi"], json!(["a.com"]));

        // What android/test/DnrTest.java takes the window to give for its urlFilters.
        assert_eq!(url_filter_regex("||ads.example^"), r"^[a-zA-Z][a-zA-Z0-9+.-]*://(?:[^/?#@]*@)?(?:[^/?#:@]*\.)?ads\.example(?:[^a-zA-Z0-9_.%-]|$)");
        assert_eq!(url_filter_regex("/blocked"), "/blocked");
        assert_eq!(url_filter_regex("track.js"), r"track\.js");
    }

    fn rules(written: Value) -> Rules {
        Rules::new("ext", "chrome-extension://ext", written.as_array().unwrap())
    }

    fn request<'a>(url: &'a str, initiator: Option<&'a str>, t: ResourceType) -> Request<'a> {
        Request { url, initiator, resource_type: t, method: "get", tab: Some(1) }
    }

    #[test]
    fn url_filters_match_as_chrome_says() {
        // Chrome's own examples.
        let cases: &[(&str, &str, bool)] = &[
            ("abc", "https://abcd.com", true),
            ("abc", "https://example.com/abcd", true),
            ("abc", "https://ab.com", false),
            ("abc*d", "https://abcd.com", true),
            ("abc*d", "https://example.com/abcxyzd", true),
            ("abc*d", "https://abc.com", false),
            ("||a.example.com", "https://a.example.com/", true),
            ("||a.example.com", "https://b.a.example.com/xyz", true),
            ("||a.example.com", "https://example.com/", false),
            ("||a.example.com", "https://nota.example.com/", false),
            ("|https*", "https://example.com", true),
            ("|https*", "http://example.com/", false),
            ("example*^123|", "https://example.com/123", true),
            ("example*^123|", "http://abc.com/example?123", true),
            ("example*^123|", "https://example.com/1234", false),
            ("example*^123|", "https://abc.com/example0123", false),
        ];
        for (filter, url, want) in cases {
            let re = Regex::new(&format!("(?i){}", url_filter_regex(filter))).unwrap();
            assert_eq!(re.is_match(url), *want, "{filter} on {url}");
        }
    }

    #[test]
    fn the_highest_priority_decides_then_chromes_order() {
        let r = rules(json!([
            { "id": 1, "priority": 1, "action": { "type": "block" }, "condition": { "urlFilter": "||ads.example.com" } },
            { "id": 2, "priority": 2, "action": { "type": "allow" }, "condition": { "urlFilter": "||ads.example.com/ok" } },
            { "id": 3, "priority": 1, "action": { "type": "allow" }, "condition": { "urlFilter": "same-priority" } },
            { "id": 4, "priority": 1, "action": { "type": "block" }, "condition": { "urlFilter": "same-priority" } },
            { "id": 5, "action": { "type": "upgradeScheme" }, "condition": { "urlFilter": "|http://upgrade.example.com/" } },
        ]));
        let page = Some("https://news.example.org/");
        assert_eq!(r.decide(&request("https://ads.example.com/x.js", page, ResourceType::Script)), Some(Decision::Block));
        assert_eq!(r.decide(&request("https://cdn.ads.example.com/x.js", page, ResourceType::Script)), Some(Decision::Block), "subdomains too");
        assert_eq!(r.decide(&request("https://ads.example.com/ok.js", page, ResourceType::Script)), None, "a higher allow");
        assert_eq!(r.decide(&request("https://x.com/same-priority", page, ResourceType::Image)), None, "allow before block at one priority");
        assert_eq!(
            r.decide(&request("http://upgrade.example.com/a?b", page, ResourceType::Image)),
            Some(Decision::Redirect("https://upgrade.example.com/a?b".into()))
        );
        // Without resourceTypes, every type but the top frame's.
        assert_eq!(r.decide(&request("https://ads.example.com/", None, ResourceType::MainFrame)), None);
        assert_eq!(r.decide(&request("https://ads.example.com/", page, ResourceType::SubFrame)), Some(Decision::Block));
    }

    #[test]
    fn conditions_narrow_a_rule() {
        let r = rules(json!([
            { "id": 1, "action": { "type": "block" }, "condition": {
                "requestDomains": ["tracker.com"], "excludedRequestDomains": ["ok.tracker.com"],
                "initiatorDomains": ["site.com"], "resourceTypes": ["script", "xmlhttprequest"], "requestMethods": ["get"] } },
            { "id": 2, "action": { "type": "block" }, "condition": { "urlFilter": "pixel", "domainType": "thirdParty" } },
        ]));
        let site = Some("https://www.site.com/page");
        assert_eq!(r.decide(&request("https://a.tracker.com/t.js", site, ResourceType::Script)), Some(Decision::Block));
        assert_eq!(r.decide(&request("https://ok.tracker.com/t.js", site, ResourceType::Script)), None, "an excluded domain");
        assert_eq!(r.decide(&request("https://a.tracker.com/t.js", Some("https://other.com/"), ResourceType::Script)), None, "another site's page");
        assert_eq!(r.decide(&request("https://a.tracker.com/t.png", site, ResourceType::Image)), None, "another type");
        let post = Request { method: "POST", ..request("https://a.tracker.com/t.js", site, ResourceType::Script) };
        assert_eq!(r.decide(&post), None, "another method");
        assert_eq!(r.decide(&request("https://cdn.other.com/pixel.gif", site, ResourceType::Image)), Some(Decision::Block), "third party");
        assert_eq!(r.decide(&request("https://img.site.com/pixel.gif", site, ResourceType::Image)), None, "first party");
    }

    #[test]
    fn redirects() {
        let r = rules(json!([
            { "id": 1, "action": { "type": "redirect", "redirect": { "extensionPath": "/blank.js" } }, "condition": { "urlFilter": "||ads.com/a.js" } },
            { "id": 2, "action": { "type": "redirect", "redirect": { "regexSubstitution": "https://\\1.example.org/\\2" } },
              "condition": { "regexFilter": "^https://www\\.(.+)\\.com/(.*)$" } },
            { "id": 3, "action": { "type": "redirect", "redirect": { "transform": {
                "scheme": "https", "queryTransform": { "removeParams": ["utm_source"], "addOrReplaceParams": [{ "key": "ref", "value": "sm" }] } } } },
              "condition": { "urlFilter": "||shop.net", "resourceTypes": ["main_frame"] } },
        ]));
        let page = Some("https://p.com/");
        assert_eq!(r.decide(&request("https://ads.com/a.js", page, ResourceType::Script)), Some(Decision::Redirect("chrome-extension://ext/blank.js".into())));
        assert_eq!(
            r.decide(&request("https://www.foo.com/x/y", page, ResourceType::Image)),
            Some(Decision::Redirect("https://foo.example.org/x/y".into()))
        );
        assert_eq!(
            r.decide(&request("http://shop.net/item?utm_source=a&id=4", None, ResourceType::MainFrame)),
            Some(Decision::Redirect("https://shop.net/item?id=4&ref=sm".into()))
        );
    }

    #[test]
    fn a_page_let_in_lets_its_requests_through() {
        let r = rules(json!([
            { "id": 1, "priority": 1, "action": { "type": "block" }, "condition": { "urlFilter": "||ads.com" } },
            { "id": 2, "priority": 2, "action": { "type": "allowAllRequests" }, "condition": { "urlFilter": "||trusted.org", "resourceTypes": ["main_frame"] } },
        ]));
        assert_eq!(r.decide(&request("https://ads.com/x.js", Some("https://trusted.org/a"), ResourceType::Script)), None);
        assert_eq!(r.decide(&request("https://ads.com/x.js", Some("https://other.org/a"), ResourceType::Script)), Some(Decision::Block));
    }

    #[test]
    fn across_extensions_a_block_wins() {
        let a = Rules::new("a", "chrome-extension://a", json!([{ "id": 1, "action": { "type": "allow" }, "condition": { "urlFilter": "x" } }]).as_array().unwrap());
        let b = Rules::new("b", "chrome-extension://b", json!([{ "id": 1, "action": { "type": "block" }, "condition": { "urlFilter": "x" } }]).as_array().unwrap());
        assert_eq!(decide(&[a, b], &request("https://x.com/", Some("https://p.com/"), ResourceType::Image)), Some(Decision::Block));
    }

    #[test]
    fn rules_chrome_refuses_are_left_out() {
        let r = rules(json!([
            { "id": 1, "action": { "type": "block" }, "condition": { "urlFilter": "a", "regexFilter": "b" } },
            { "id": 2, "action": { "type": "nothing" }, "condition": {} },
            { "action": { "type": "block" }, "condition": {} },
            { "id": 4, "action": { "type": "block" }, "condition": { "regexFilter": "(" } },
            { "id": 5, "action": { "type": "block" }, "condition": { "urlFilter": "kept" } },
        ]));
        assert_eq!(r.rules().iter().map(|r| r.id).collect::<Vec<_>>(), vec![5]);
    }

    #[test]
    fn as_webkits_content_blockers() {
        let r = rules(json!([
            { "id": 1, "priority": 2, "action": { "type": "allow" }, "condition": { "urlFilter": "||ads.com/ok^" } },
            { "id": 2, "priority": 1, "action": { "type": "block" }, "condition": { "urlFilter": "||ads.com^", "resourceTypes": ["script", "image", "sub_frame"] } },
            { "id": 3, "action": { "type": "block" }, "condition": { "requestDomains": ["t.net", "u.net"], "excludedInitiatorDomains": ["t.net"] } },
            { "id": 4, "action": { "type": "redirect", "redirect": { "url": "https://x/" } }, "condition": { "urlFilter": "r" } },
            { "id": 5, "action": { "type": "block" }, "condition": { "regexFilter": "^https://(a|b)\\.com" } },
        ]));
        let list: Value = serde_json::from_str(&webkit_rules(&r)).unwrap();
        let list = list.as_array().unwrap();
        // The allow (higher priority) comes after the blocks; the redirect and the alternatives
        // are left out.
        let last = list.last().unwrap();
        assert_eq!(last["action"]["type"], "ignore-previous-rules");
        assert_eq!(last["trigger"]["url-filter"], "^[^:]+://+([^/]+\\.)?ads\\.com/ok[^a-zA-Z0-9_.%-]");
        let blocks: Vec<&Value> = list.iter().filter(|r| r["action"]["type"] == "block").collect();
        // Rule 2: its scripts and images, and its frame documents apart; rule 3: a rule for each
        // request domain, for documents in frames and everything else.
        assert_eq!(blocks.len(), 2 + 4);
        assert!(blocks.iter().any(|b| b["trigger"]["resource-type"] == json!(["document"]) && b["trigger"]["load-context"] == json!(["child-frame"])));
        assert!(blocks.iter().any(|b| b["trigger"]["url-filter"] == "^[^:]+://+([^/]+\\.)?u\\.net[/:]" && b["trigger"]["unless-domain"] == json!(["*t.net"])));
        // WebKit's own parser takes every url-filter: checked in the engines' tests; here, that
        // none uses what WebKit has not got.
        for rule in list {
            let f = rule["trigger"]["url-filter"].as_str().unwrap();
            assert!(!f.contains("(?") && !f.contains('|'), "{f}");
        }
    }
}
