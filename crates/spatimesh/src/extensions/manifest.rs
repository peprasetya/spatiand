//! An extension's manifest.json: Manifest V2 and V3, as Chrome and Firefox read them.
//!
//! Read leniently, as both browsers do: a key this does not know is kept (in [`Manifest::raw`],
//! which `runtime.getManifest()` returns) and otherwise ignored; a key of the wrong shape is
//! taken as absent. What is required is a name, a version and manifest_version 2 or 3.

use std::collections::BTreeMap;

use serde_json::Value;

/// When a content script goes into a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunAt {
    DocumentStart,
    DocumentEnd,
    DocumentIdle,
}

/// Which JavaScript world a content script runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum World {
    /// The extension's own, apart from the page's scripts (the default).
    Isolated,
    /// The page's own (Manifest V3's `"world": "MAIN"`).
    Main,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContentScript {
    pub matches: Vec<String>,
    pub exclude_matches: Vec<String>,
    pub include_globs: Vec<String>,
    pub exclude_globs: Vec<String>,
    pub js: Vec<String>,
    pub css: Vec<String>,
    pub run_at: RunAt,
    pub all_frames: bool,
    pub match_about_blank: bool,
    pub world: World,
}

/// What runs in the background.
#[derive(Debug, Clone, PartialEq)]
pub enum Background {
    /// An HTML page of the extension's (Manifest V2).
    Page(String),
    /// Scripts in a page made for them (Manifest V2; Firefox's V3 too).
    Scripts(Vec<String>),
    /// Chrome's Manifest V3: a service worker, as a module or not; run here as a page.
    ServiceWorker { script: String, module: bool },
}

/// Which of the toolbar's buttons the extension has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    /// Manifest V3's `action`.
    Action,
    /// Manifest V2's `browser_action`.
    BrowserAction,
    /// Manifest V2's `page_action`: a button for some pages only.
    PageAction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub kind: ActionKind,
    pub popup: Option<String>,
    pub title: Option<String>,
    /// Icons by size; a single path is size 0.
    pub icons: BTreeMap<u32, String>,
}

/// Resources pages may load (`web_accessible_resources`).
#[derive(Debug, Clone, PartialEq)]
pub struct WebAccessible {
    pub resources: Vec<String>,
    /// For whom (Manifest V3); every page in Manifest V2.
    pub matches: Vec<String>,
}

/// A declarativeNetRequest rule set of the manifest's.
#[derive(Debug, Clone, PartialEq)]
pub struct Ruleset {
    pub id: String,
    pub enabled: bool,
    pub path: String,
}

/// A keyboard command (`commands`).
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub name: String,
    pub key: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// The manifest as written, for `runtime.getManifest()`.
    pub raw: Value,
    pub manifest_version: u8,
    pub name: String,
    pub short_name: Option<String>,
    pub version: String,
    pub description: String,
    pub default_locale: Option<String>,
    pub icons: BTreeMap<u32, String>,
    /// API permissions (`storage`, `tabs`, ...).
    pub permissions: Vec<String>,
    /// Hosts the extension may reach: V3's `host_permissions`, and V2's from `permissions`.
    pub host_permissions: Vec<String>,
    pub optional_permissions: Vec<String>,
    pub optional_host_permissions: Vec<String>,
    pub content_scripts: Vec<ContentScript>,
    pub background: Option<Background>,
    pub action: Option<Action>,
    pub options_page: Option<String>,
    /// Whether the options open in a tab of their own rather than inside SpatiMesh's settings.
    pub options_in_tab: bool,
    pub web_accessible: Vec<WebAccessible>,
    pub rulesets: Vec<Ruleset>,
    pub commands: Vec<Command>,
    pub new_tab_page: Option<String>,
    /// Firefox's id for it (`browser_specific_settings.gecko.id`, or V2's `applications`).
    pub gecko_id: Option<String>,
    /// Chrome's public key, from which its id is made.
    pub key: Option<String>,
}

fn strings(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(items)) => items.iter().filter_map(|i| i.as_str().map(str::to_string)).collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

fn string(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_string).filter(|s| !s.is_empty())
}

fn icons(v: Option<&Value>) -> BTreeMap<u32, String> {
    match v {
        Some(Value::Object(map)) => map.iter().filter_map(|(size, path)| Some((size.parse().ok()?, path.as_str()?.to_string()))).collect(),
        Some(Value::String(path)) => BTreeMap::from([(0, path.clone())]),
        _ => BTreeMap::new(),
    }
}

/// Whether a permission names hosts rather than an API: a match pattern.
pub fn is_host_permission(p: &str) -> bool {
    p == "<all_urls>" || p.contains("://")
}

fn content_script(v: &Value) -> Option<ContentScript> {
    let matches = strings(v.get("matches"));
    if matches.is_empty() {
        return None;
    }
    Some(ContentScript {
        matches,
        exclude_matches: strings(v.get("exclude_matches")),
        include_globs: strings(v.get("include_globs")),
        exclude_globs: strings(v.get("exclude_globs")),
        js: strings(v.get("js")),
        css: strings(v.get("css")),
        run_at: match v.get("run_at").and_then(Value::as_str) {
            Some("document_start") => RunAt::DocumentStart,
            Some("document_end") => RunAt::DocumentEnd,
            _ => RunAt::DocumentIdle,
        },
        all_frames: v.get("all_frames").and_then(Value::as_bool).unwrap_or(false),
        match_about_blank: v.get("match_about_blank").and_then(Value::as_bool).unwrap_or(false)
            || v.get("match_origin_as_fallback").and_then(Value::as_bool).unwrap_or(false),
        world: match v.get("world").and_then(Value::as_str) {
            Some("MAIN") => World::Main,
            _ => World::Isolated,
        },
    })
}

impl Manifest {
    /// A manifest.json, read.
    pub fn parse(text: &str) -> Result<Manifest, String> {
        // Chrome reads manifests with comments in them; a few published ones have them.
        let raw: Value = serde_json::from_str(text)
            .or_else(|_| serde_json::from_str(&strip_comments(text)))
            .map_err(|e| format!("manifest.json is not JSON: {e}"))?;
        let manifest_version = match raw.get("manifest_version").and_then(Value::as_u64) {
            Some(2) => 2,
            Some(3) => 3,
            Some(other) => return Err(format!("manifest_version {other} is not one this reads (2 or 3)")),
            None => return Err("manifest.json has no manifest_version".into()),
        };
        let name = string(raw.get("name")).ok_or("manifest.json has no name")?;
        let version = string(raw.get("version")).ok_or("manifest.json has no version")?;

        let mut permissions = Vec::new();
        let mut host_permissions = strings(raw.get("host_permissions"));
        for p in strings(raw.get("permissions")) {
            if is_host_permission(&p) {
                host_permissions.push(p);
            } else {
                permissions.push(p);
            }
        }
        let mut optional_permissions = Vec::new();
        let mut optional_host_permissions = strings(raw.get("optional_host_permissions"));
        for p in strings(raw.get("optional_permissions")) {
            if is_host_permission(&p) {
                optional_host_permissions.push(p);
            } else {
                optional_permissions.push(p);
            }
        }

        let background = raw.get("background").and_then(|b| {
            if let Some(worker) = string(b.get("service_worker")) {
                // Firefox's V3 has `scripts` beside it, for the browsers that have no worker.
                let scripts = strings(b.get("scripts"));
                if !scripts.is_empty() && b.get("type").and_then(Value::as_str) != Some("module") {
                    return Some(Background::Scripts(scripts));
                }
                return Some(Background::ServiceWorker { script: worker, module: b.get("type").and_then(Value::as_str) == Some("module") });
            }
            if let Some(page) = string(b.get("page")) {
                return Some(Background::Page(page));
            }
            let scripts = strings(b.get("scripts"));
            (!scripts.is_empty()).then_some(Background::Scripts(scripts))
        });

        let action = [("action", ActionKind::Action), ("browser_action", ActionKind::BrowserAction), ("page_action", ActionKind::PageAction)]
            .into_iter()
            .find_map(|(key, kind)| {
                let a = raw.get(key)?;
                let mut icon_set = icons(a.get("default_icon"));
                if icon_set.is_empty() {
                    icon_set = icons(raw.get("icons"));
                }
                Some(Action { kind, popup: string(a.get("default_popup")), title: string(a.get("default_title")), icons: icon_set })
            });

        let (options_page, options_in_tab) = match raw.get("options_ui") {
            Some(ui) if string(ui.get("page")).is_some() => {
                (string(ui.get("page")), ui.get("open_in_tab").and_then(Value::as_bool).unwrap_or(false))
            }
            _ => (string(raw.get("options_page")), true),
        };

        let web_accessible = match raw.get("web_accessible_resources") {
            Some(Value::Array(items)) if items.iter().all(Value::is_string) => {
                vec![WebAccessible { resources: strings(Some(&Value::Array(items.clone()))), matches: vec!["<all_urls>".into()] }]
            }
            Some(Value::Array(items)) => items
                .iter()
                .filter(|i| i.is_object())
                .map(|i| WebAccessible { resources: strings(i.get("resources")), matches: strings(i.get("matches")) })
                .collect(),
            _ => Vec::new(),
        };

        let rulesets = raw
            .get("declarative_net_request")
            .and_then(|d| d.get("rule_resources"))
            .and_then(Value::as_array)
            .map(|rs| {
                rs.iter()
                    .filter_map(|r| {
                        Some(Ruleset {
                            id: string(r.get("id"))?,
                            enabled: r.get("enabled").and_then(Value::as_bool).unwrap_or(false),
                            path: string(r.get("path"))?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let commands = raw
            .get("commands")
            .and_then(Value::as_object)
            .map(|cs| {
                cs.iter()
                    .map(|(name, c)| Command {
                        name: name.clone(),
                        key: c.get("suggested_key").and_then(|k| string(k.get("linux")).or_else(|| string(k.get("default")))),
                        description: string(c.get("description")),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let gecko_id = ["browser_specific_settings", "applications"]
            .iter()
            .find_map(|k| string(raw.get(*k)?.get("gecko")?.get("id")));

        Ok(Manifest {
            manifest_version,
            short_name: string(raw.get("short_name")),
            description: string(raw.get("description")).unwrap_or_default(),
            default_locale: string(raw.get("default_locale")),
            icons: icons(raw.get("icons")),
            permissions,
            host_permissions,
            optional_permissions,
            optional_host_permissions,
            content_scripts: raw.get("content_scripts").and_then(Value::as_array).map(|cs| cs.iter().filter_map(content_script).collect()).unwrap_or_default(),
            background,
            action,
            options_page,
            options_in_tab,
            web_accessible,
            rulesets,
            commands,
            new_tab_page: raw.get("chrome_url_overrides").and_then(|o| string(o.get("newtab"))),
            gecko_id,
            key: string(raw.get("key")),
            name,
            version,
            raw,
        })
    }

    /// Whether it asked for an API permission (`storage`, `tabs`, ...).
    pub fn has_permission(&self, permission: &str) -> bool {
        self.permissions.iter().any(|p| p == permission)
    }

    /// The manifest with every `__MSG_name__` in its text given in the language chosen.
    pub fn localized(&self, message: &dyn Fn(&str) -> Option<String>) -> Manifest {
        fn walk(v: &Value, message: &dyn Fn(&str) -> Option<String>) -> Value {
            match v {
                Value::String(s) => Value::String(super::locale::substitute(s, message)),
                Value::Array(items) => Value::Array(items.iter().map(|i| walk(i, message)).collect()),
                Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), walk(v, message))).collect()),
                other => other.clone(),
            }
        }
        let raw = walk(&self.raw, message);
        Manifest::parse(&raw.to_string()).unwrap_or_else(|_| self.clone())
    }
}

/// `//` and `/* */` comments taken out of JSON, strings left alone.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut last = ' ';
                for n in chars.by_ref() {
                    if last == '*' && n == '/' {
                        break;
                    }
                    last = n;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHROME_V3: &str = r#"{
        // Chrome reads comments.
        "manifest_version": 3,
        "name": "__MSG_appName__",
        "version": "1.2.3",
        "default_locale": "en",
        "permissions": ["storage", "tabs", "scripting", "declarativeNetRequest"],
        "host_permissions": ["https://*.example.com/*"],
        "background": {"service_worker": "sw.js", "type": "module"},
        "action": {"default_popup": "popup.html", "default_title": "Do it", "default_icon": {"16": "i16.png", "32": "i32.png"}},
        "content_scripts": [{"matches": ["<all_urls>"], "js": ["a.js"], "css": ["a.css"], "run_at": "document_start", "all_frames": true},
                            {"matches": ["https://x.org/*"], "js": ["main.js"], "world": "MAIN"}],
        "options_ui": {"page": "options.html"},
        "web_accessible_resources": [{"resources": ["img/*.png"], "matches": ["https://*/*"]}],
        "declarative_net_request": {"rule_resources": [{"id": "ads", "enabled": true, "path": "rules.json"}]},
        "commands": {"_execute_action": {"suggested_key": {"default": "Ctrl+Shift+Y"}}}
    }"#;

    const FIREFOX_V2: &str = r#"{
        "manifest_version": 2,
        "name": "Old friend",
        "version": "4.0",
        "permissions": ["storage", "<all_urls>", "webRequest", "webRequestBlocking"],
        "background": {"scripts": ["lib.js", "bg.js"], "persistent": false},
        "browser_action": {"default_popup": "p.html", "default_icon": "icon.svg"},
        "applications": {"gecko": {"id": "old@friend.example"}},
        "web_accessible_resources": ["page.html"],
        "options_page": "opts.html"
    }"#;

    #[test]
    fn chrome_manifest_v3() {
        let m = Manifest::parse(CHROME_V3).unwrap();
        assert_eq!(m.manifest_version, 3);
        assert_eq!(m.background, Some(Background::ServiceWorker { script: "sw.js".into(), module: true }));
        assert_eq!(m.host_permissions, vec!["https://*.example.com/*"]);
        assert!(m.has_permission("scripting"));
        let action = m.action.clone().unwrap();
        assert_eq!(action.kind, ActionKind::Action);
        assert_eq!(action.popup.as_deref(), Some("popup.html"));
        assert_eq!(action.icons.get(&32).map(String::as_str), Some("i32.png"));
        assert_eq!(m.content_scripts.len(), 2);
        assert_eq!(m.content_scripts[0].run_at, RunAt::DocumentStart);
        assert!(m.content_scripts[0].all_frames);
        assert_eq!(m.content_scripts[1].world, World::Main);
        assert_eq!(m.content_scripts[1].run_at, RunAt::DocumentIdle);
        assert_eq!((m.options_page.as_deref(), m.options_in_tab), (Some("options.html"), false));
        assert_eq!(m.web_accessible[0].matches, vec!["https://*/*"]);
        assert_eq!(m.rulesets, vec![Ruleset { id: "ads".into(), enabled: true, path: "rules.json".into() }]);
        assert_eq!(m.commands[0].key.as_deref(), Some("Ctrl+Shift+Y"));
        let localized = m.localized(&|name| (name == "appName").then(|| "Tidy".to_string()));
        assert_eq!(localized.name, "Tidy");
    }

    #[test]
    fn firefox_manifest_v2() {
        let m = Manifest::parse(FIREFOX_V2).unwrap();
        assert_eq!(m.background, Some(Background::Scripts(vec!["lib.js".into(), "bg.js".into()])));
        assert_eq!(m.host_permissions, vec!["<all_urls>"], "V2's hosts are in its permissions");
        assert_eq!(m.permissions, vec!["storage", "webRequest", "webRequestBlocking"]);
        assert_eq!(m.gecko_id.as_deref(), Some("old@friend.example"));
        assert_eq!(m.action.as_ref().unwrap().kind, ActionKind::BrowserAction);
        assert_eq!(m.action.as_ref().unwrap().icons.get(&0).map(String::as_str), Some("icon.svg"));
        assert_eq!(m.web_accessible[0].resources, vec!["page.html"]);
        assert_eq!((m.options_page.as_deref(), m.options_in_tab), (Some("opts.html"), true));
    }

    #[test]
    fn firefox_v3_keeps_its_scripts_where_both_are_given() {
        let m = Manifest::parse(r#"{"manifest_version":3,"name":"n","version":"1",
            "background":{"service_worker":"sw.js","scripts":["bg.js"]},
            "browser_specific_settings":{"gecko":{"id":"{1234}"}}}"#)
        .unwrap();
        assert_eq!(m.background, Some(Background::Scripts(vec!["bg.js".into()])));
        assert_eq!(m.gecko_id.as_deref(), Some("{1234}"));
    }

    #[test]
    fn what_is_required() {
        assert!(Manifest::parse(r#"{"name":"n","version":"1"}"#).is_err());
        assert!(Manifest::parse(r#"{"manifest_version":4,"name":"n","version":"1"}"#).is_err());
        assert!(Manifest::parse(r#"{"manifest_version":3,"version":"1"}"#).is_err());
        assert!(Manifest::parse("not json").is_err());
        // Unknown keys are kept, not refused.
        let m = Manifest::parse(r#"{"manifest_version":3,"name":"n","version":"1","side_panel":{"x":1}}"#).unwrap();
        assert!(m.raw.get("side_panel").is_some());
    }
}
