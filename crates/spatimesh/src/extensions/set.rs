//! What an engine host is given to run a profile's extensions, and what it serves them from.
//!
//! The window sends a profile's [`ExtensionSet`] to that profile's engine host whenever it
//! changes. The engine puts each content script into the frames it is for, in the extension's
//! own world where the engine has worlds, each preceded by [`boot`] (the API, set up for that
//! extension and that kind of context); it puts [`boot`] into the extension's own pages; and it
//! answers the extension's own addresses (`chrome-extension://<id>/...`, `moz-extension://<id>/...`,
//! or the engine's own: [`Addresses`]) with [`serve`].

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::install::Flavor;

/// The code every extension context starts with: `chrome` and `browser`.
pub const API_JS: &str = include_str!("api.js");

/// The page a background made of scripts (or a service worker) runs in.
pub const BACKGROUND_PAGE: &str = "_generated_background_page.html";

/// What a Manifest V3 service worker's code expects of `self`, in a page.
const WORKER_SHIM: &str = "_spatimesh_worker.js";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtensionSet {
    pub extensions: Vec<Spec>,
    /// For an engine with no hooks of its own for extensions (Android's WebView,
    /// [`Addresses::Https`]): what it puts at the start of every page, and serves by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_start: Option<PageStart>,
}

/// What an engine with no hooks of its own for extensions is given besides the set: Android's
/// WebView, which runs scripts it is given at the start of every frame's every page and serves
/// what it intercepts, and nothing more (docs/spatimesh.md, "Extensions").
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PageStart {
    /// For each extension, its id and the script for the start of every frame's every page
    /// ([`page_start`]): one each, so that one that cannot run takes no other with it.
    pub scripts: Vec<(String, String)>,
    /// What the top frame's way in for an extension is called: this, then its id.
    pub entry: String,
    /// What a call through it says.
    pub secret: String,
    /// The page made for a background of scripts (or a service worker), at this path of the
    /// extension's: its HTML is the spec's `background_html`.
    pub background_page: String,
    /// The worker shim that page loads first, at this path, and its code.
    pub worker_path: String,
    pub worker: String,
    /// How the code that hands api.js something ([`deliver`]) begins, before the JSON it hands:
    /// what such code is, for an engine that can reach a frame only with that JSON.
    pub deliver_prefix: String,
    /// Each extension's web-accessible resources as regular expressions' sources, by its id:
    /// [(its paths, the pages they are for: "" for any)].
    pub web_accessible: Vec<(String, Vec<(String, String)>)>,
}

/// How an engine gives extensions' pages and files their addresses: each extension's origin,
/// which its pages run at and `runtime.getURL` names (docs/spatimesh.md, "Extensions").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Addresses {
    /// chrome-extension:// for Chrome's, moz-extension:// for Firefox's (WebKit's engines).
    #[default]
    Browsers,
    /// Chromium's own extension system holds chrome-extension://, and lets no page reach an
    /// extension there that it does not know itself: Chrome's are at spatimesh-extension://
    /// instead, Firefox's at moz-extension:// (Steam's Chromium).
    Chromium,
    /// Only the web's own schemes give a page an origin: every extension is at
    /// https://<id>.spatimesh-extension.invalid, answered by the engine and never by the network
    /// (Android's WebView).
    Https,
}

/// The scheme of [`Addresses::Chromium`]'s Chrome extensions.
pub const OWN_SCHEME: &str = "spatimesh-extension";

/// The domain [`Addresses::Https`] puts each extension under.
pub const HTTPS_DOMAIN: &str = "spatimesh-extension.invalid";

impl Addresses {
    /// An extension's origin.
    pub fn origin(self, flavor: Flavor, id: &str) -> String {
        match (self, flavor) {
            (Addresses::Https, _) => format!("https://{id}.{HTTPS_DOMAIN}"),
            (Addresses::Chromium, Flavor::Chrome) => format!("{OWN_SCHEME}://{id}"),
            (_, Flavor::Chrome) => format!("chrome-extension://{id}"),
            (_, Flavor::Firefox) => format!("moz-extension://{id}"),
        }
    }
}

/// One extension, as an engine runs it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Spec {
    pub id: String,
    pub root: PathBuf,
    pub flavor: Flavor,
    /// Its pages' origin, as the engine is to serve them ([`Addresses::origin`]).
    pub origin: String,
    pub scripts: Vec<Script>,
    /// `web_accessible_resources`: (resources, for pages matching).
    pub web_accessible: Vec<(Vec<String>, Vec<String>)>,
    /// The background page's own path, or the page made for its scripts.
    pub background: Option<String>,
    /// That page, if it is made here.
    pub background_html: Option<String>,
    /// The API's configuration for a content script, and for the extension's own pages: JSON,
    /// each with its own token, which the window checks every call by.
    pub content_config: String,
    pub page_config: String,
    /// `__MSG_name__` in its style sheets: the messages by name, in lower case.
    pub messages: std::collections::BTreeMap<String, String>,
}

/// One `content_scripts` entry (or one registered by `scripting.registerContentScripts`), its
/// files read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Script {
    pub matches: Vec<String>,
    pub exclude_matches: Vec<String>,
    pub include_globs: Vec<String>,
    pub exclude_globs: Vec<String>,
    /// The same as regular expressions' sources (JavaScript's `new RegExp`), for an engine that
    /// decides in the frame: a frame at `url` takes the script if `include` matches it, `globs`
    /// is "" or matches it, and `exclude` is "" or does not.
    #[serde(default)]
    pub include: String,
    #[serde(default)]
    pub globs: String,
    #[serde(default)]
    pub exclude: String,
    /// Each file: its name and its code, in order.
    pub js: Vec<(String, String)>,
    pub css: Vec<(String, String)>,
    /// "document_start", "document_end" or "document_idle".
    pub run_at: String,
    pub all_frames: bool,
    pub match_about_blank: bool,
    /// The page's own world (`"world": "MAIN"`) rather than the extension's.
    pub main_world: bool,
}

impl Script {
    /// Its regular expressions, from its patterns and globs.
    pub fn with_regexes(mut self) -> Script {
        use super::pattern::{any_regex, glob_regex, Patterns};
        self.include = Patterns::new(&self.matches).regex();
        self.globs = any_regex(&self.include_globs.iter().map(|g| glob_regex(g)).collect::<Vec<_>>());
        let mut excluded: Vec<String> = Vec::new();
        let exclude = Patterns::new(&self.exclude_matches).regex();
        if !exclude.is_empty() {
            excluded.push(exclude);
        }
        excluded.extend(self.exclude_globs.iter().map(|g| glob_regex(g)));
        self.exclude = any_regex(&excluded);
        self
    }

    /// Whether a frame at `url` takes it (what the regular expressions say, in Rust).
    pub fn wants(&self, url: &str) -> bool {
        use super::pattern::{script_wants, Patterns};
        script_wants(&Patterns::new(&self.matches), &Patterns::new(&self.exclude_matches), &self.include_globs, &self.exclude_globs, url)
    }
}

impl Spec {
    /// The world its content scripts run in.
    pub fn world(&self) -> String {
        format!("spatimesh-{}", self.id)
    }

    /// Its pages' origin, as the engine is to serve them.
    pub fn origin(&self) -> String {
        self.origin.clone()
    }

    /// Its origin's scheme: what the engine answers for it.
    pub fn scheme(&self) -> &str {
        self.origin.split("://").next().unwrap_or("")
    }

    /// The background page's address, if it has one.
    pub fn background_url(&self) -> Option<String> {
        self.background.as_ref().map(|p| format!("{}/{}", self.origin(), p.trim_start_matches('/')))
    }
}

/// android.js: one extension in Android's WebView.
const ANDROID_JS: &str = include_str!("android.js");

/// The script Android's WebView runs at the start of every frame's every page for one
/// extension: the API, the extension's configurations and its content scripts (each entry's
/// files compiled in, as a function), and what puts them where they go (android.js). The top
/// frame is called in by `window[entry]`, saying `secret`.
pub fn page_start(spec: &Spec, secret: &str, entry: &str) -> String {
    let quoted = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    let mut scripts = Vec::new();
    for script in &spec.scripts {
        let run_at = match script.run_at.as_str() {
            "document_start" => 0,
            "document_end" => 1,
            _ => 2,
        };
        // The files in one function, after an empty statement: a file's "use strict" is no
        // directive for the others.
        let files: Vec<&str> = script.js.iter().map(|(_, code)| code.as_str()).collect();
        let params = if script.main_world { "" } else { "chrome,browser,globalThis" };
        let js = if files.is_empty() { "null".to_string() } else { format!("function({params}){{;\n{}\n}}", files.join("\n;\n")) };
        // Written with __MSG_name__, as in Chrome.
        let css: Vec<String> =
            script.css.iter().map(|(_, css)| super::locale::substitute(css, &|name| spec.messages.get(&name.to_lowercase()).cloned())).collect();
        scripts.push(format!(
            "{{runAt:{run_at},allFrames:{},mainWorld:{},include:{},globs:{},exclude:{},css:{},js:{js}}}",
            script.all_frames,
            script.main_world,
            quoted(&script.include),
            quoted(&script.globs),
            quoted(&script.exclude),
            quoted(&css.join("\n")),
        ));
    }
    let ext = format!(
        "{{id:{},origin:{},content:{},page:{},scripts:[{}]}}",
        quoted(&spec.id),
        quoted(&spec.origin),
        spec.content_config,
        spec.page_config,
        scripts.join(",")
    );
    format!("({ANDROID_JS})((function(){{\n{API_JS}\nreturn __spatimeshExtBoot;}})(),{ext},{},{});", quoted(secret), quoted(entry))
}

/// What Android's WebView is given besides the set ([`PageStart`]).
pub fn page_starts(set: &ExtensionSet, secret: &str, entry: &str) -> PageStart {
    use super::pattern::{path_regex, Patterns};
    PageStart {
        scripts: set.extensions.iter().map(|spec| (spec.id.clone(), page_start(spec, secret, &format!("{entry}{}", spec.id)))).collect(),
        entry: entry.to_string(),
        secret: secret.to_string(),
        background_page: BACKGROUND_PAGE.to_string(),
        worker_path: WORKER_SHIM.to_string(),
        worker: WORKER_JS.to_string(),
        deliver_prefix: DELIVER_PREFIX.to_string(),
        web_accessible: set
            .extensions
            .iter()
            .map(|spec| {
                let each = spec.web_accessible.iter().map(|(paths, pages)| {
                    let paths: Vec<String> = paths.iter().map(|p| path_regex(p.trim_start_matches('/'))).collect();
                    (super::pattern::any_regex(&paths), Patterns::new(pages).regex())
                });
                (spec.id.clone(), each.collect())
            })
            .collect(),
    }
}

/// `bridge` is the engine's own: an expression that is a function taking a string and handing
/// it to the window (see each engine). `config` is a [`Spec`]'s `content_config` or
/// `page_config`.
pub fn boot(bridge: &str, config: &str) -> String {
    format!(
        "(function(){{if(globalThis.__spatimeshExt)return;var __spatimeshPost=({bridge});\n{API_JS}\n__spatimeshExtBoot({config},__spatimeshPost);}})();"
    )
}

/// The API as a function of the bridge, for an engine with no world of the extension's own
/// (CEF, Android's WebView): `(function(post){...})`, which the engine calls with a function of
/// its own that the page cannot reach. `closed` (a content script's): nothing is put on the
/// page's globals, and the function returns the API -- `{chrome, browser, deliver, run, global}`
/// -- for the engine to keep and hand to the content scripts' closure. Otherwise (the
/// extension's own pages) it sets `chrome` and `browser` up as globals, as [`boot`] does.
pub fn boot_function(config: &str, closed: bool) -> String {
    format!("(function(__spatimeshPost){{\n{API_JS}\nreturn __spatimeshExtBoot({config},__spatimeshPost,{closed});}})")
}

/// How [`deliver`]'s code begins.
const DELIVER_PREFIX: &str = "globalThis.__spatimeshExt&&globalThis.__spatimeshExt.deliver(";

/// The code that hands `json` to the extension context it is run in.
pub fn deliver(json: &str) -> String {
    format!("{DELIVER_PREFIX}{})", serde_json::to_string(json).unwrap_or_default())
}

/// The page made for a background of scripts, or of a service worker (run as a page, as
/// Firefox runs its Manifest V3 backgrounds): its scripts in order, as the manifest has them.
pub fn background_html(scripts: &[String], worker: Option<(&str, bool)>) -> String {
    let mut html = String::from("<!doctype html><html><head><meta charset=\"utf-8\">");
    if let Some((script, module)) = worker {
        html.push_str(&format!("<script src=\"/{WORKER_SHIM}\"></script>"));
        let kind = if module { " type=\"module\"" } else { "" };
        html.push_str(&format!("<script{kind} src=\"/{}\"></script>", script.trim_start_matches('/')));
        html.push_str("<script>__spatimeshWorkerStarted();</script>");
    }
    for s in scripts {
        html.push_str(&format!("<script src=\"/{}\"></script>", s.trim_start_matches('/')));
    }
    html.push_str("</head><body></body></html>");
    html
}

/// What a service worker's code expects to find: `importScripts`, `registration`, `clients`,
/// `skipWaiting`, and its install and activate events.
const WORKER_JS: &str = r#"(function(){
  var loaded = {};
  self.importScripts = function () {
    for (var i = 0; i < arguments.length; i++) {
      var url = new URL(arguments[i], location.href).href;
      var request = new XMLHttpRequest();
      request.open('GET', url, false);
      request.send();
      if (request.status && request.status !== 200) throw new Error('importScripts: ' + url + ' could not be loaded');
      (0, eval)(request.responseText + '\n//# sourceURL=' + url);
      loaded[url] = true;
    }
  };
  self.skipWaiting = function () { return Promise.resolve(); };
  self.clients = {
    matchAll: function () { return Promise.resolve([]); },
    claim: function () { return Promise.resolve(); },
    get: function () { return Promise.resolve(undefined); },
    openWindow: function (url) { return chrome.tabs.create({ url: url }).then(function () { return null; }); }
  };
  self.registration = {
    scope: location.origin + '/',
    active: { state: 'activated', postMessage: function () {} },
    showNotification: function (title, options) {
      options = options || {};
      return chrome.notifications.create({ type: 'basic', title: title, message: options.body || '', iconUrl: options.icon || '' }).then(function () {});
    },
    getNotifications: function () { return Promise.resolve([]); },
    unregister: function () { return Promise.resolve(false); },
    update: function () { return Promise.resolve(); }
  };
  self.__spatimeshWorkerStarted = function () {
    ['install', 'activate'].forEach(function (name) {
      var event = new Event(name);
      event.waitUntil = function () {};
      self.dispatchEvent(event);
    });
  };
})();"#;

/// What answers `chrome-extension://...` and `moz-extension://...`.
#[derive(Debug, Clone, PartialEq)]
pub struct Resource {
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

fn mime_of(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "txt" => "text/plain",
        "xml" => "application/xml",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" => "audio/ogg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

/// An extension URL's id and path (percent-escapes undone; the query and fragment left out),
/// whichever of the [`Addresses`] it is at.
pub fn parse_url(url: &str) -> Option<(String, String)> {
    let rest = match ["chrome-extension://", "moz-extension://", "spatimesh-extension://"].iter().find_map(|p| url.strip_prefix(p)) {
        Some(rest) => rest.to_string(),
        None => {
            let rest = url.strip_prefix("https://")?;
            let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
            let id = host.to_ascii_lowercase().strip_suffix(&format!(".{HTTPS_DOMAIN}"))?.to_string();
            format!("{id}/{path}")
        }
    };
    let (id, path) = rest.split_once('/').unwrap_or((&rest, ""));
    let path = path.split(['?', '#']).next().unwrap_or("");
    let path = percent_encoding::percent_decode_str(path).decode_utf8().ok()?.into_owned();
    Some((id.to_ascii_lowercase(), path))
}

/// A path inside `root`, or nothing if it would leave it.
pub fn inside(root: &Path, path: &str) -> Option<PathBuf> {
    let relative = Path::new(path);
    if !relative.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let full = root.join(relative);
    let real = std::fs::canonicalize(&full).ok()?;
    let real_root = std::fs::canonicalize(root).ok()?;
    real.starts_with(&real_root).then_some(real)
}

/// Whether a page at `page_url` may load `path` of `spec` (its `web_accessible_resources`).
pub fn web_accessible(spec: &Spec, path: &str, page_url: &str) -> bool {
    spec.web_accessible.iter().any(|(resources, matches)| {
        resources.iter().any(|r| super::pattern::wildcard(r.trim_start_matches('/'), path, false))
            && (matches.is_empty() || super::pattern::Patterns::new(matches).matches(page_url))
    })
}

/// The file an extension URL names. `initiator` is the page asking for it, where the engine
/// knows it: a page that is not the extension's own may have only what the extension made
/// web accessible.
pub fn serve(set: &ExtensionSet, url: &str, initiator: Option<&str>) -> Result<Resource, u16> {
    let (id, path) = parse_url(url).ok_or(400u16)?;
    let spec = set.extensions.iter().find(|s| s.id == id).ok_or(404u16)?;
    let path = if path.is_empty() { "index.html".to_string() } else { path };
    if let Some(page) = initiator.filter(|p| !p.is_empty() && !p.starts_with(&spec.origin()) && !p.starts_with("about:")) {
        // An Origin header is the page's origin alone: its patterns are for a path.
        let page = if page.matches('/').count() == 2 { format!("{page}/") } else { page.to_string() };
        if !web_accessible(spec, &path, &page) {
            return Err(403);
        }
    }
    if path == BACKGROUND_PAGE {
        if let Some(html) = &spec.background_html {
            return Ok(Resource { mime: "text/html", bytes: html.as_bytes().to_vec() });
        }
    }
    if path == WORKER_SHIM {
        return Ok(Resource { mime: "text/javascript", bytes: WORKER_JS.as_bytes().to_vec() });
    }
    let file = inside(&spec.root, &path).ok_or(404u16)?;
    let bytes = std::fs::read(&file).map_err(|_| 404u16)?;
    let mime = mime_of(&path);
    // Style sheets are written with __MSG_name__, as in Chrome.
    if mime == "text/css" && !spec.messages.is_empty() {
        let text = String::from_utf8_lossy(&bytes);
        let localized = super::locale::substitute(&text, &|name| spec.messages.get(&name.to_lowercase()).cloned());
        return Ok(Resource { mime, bytes: localized.into_bytes() });
    }
    Ok(Resource { mime, bytes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(root: &Path) -> Spec {
        Spec {
            id: "abcdefghijklmnopabcdefghijklmnop".into(),
            root: root.to_path_buf(),
            flavor: Flavor::Chrome,
            origin: "chrome-extension://abcdefghijklmnopabcdefghijklmnop".into(),
            scripts: Vec::new(),
            web_accessible: vec![(vec!["img/*".into()], vec!["https://*.example.com/*".into()])],
            background: Some(BACKGROUND_PAGE.into()),
            background_html: Some(background_html(&["bg.js".into()], None)),
            content_config: "{}".into(),
            page_config: "{}".into(),
            messages: [("color".to_string(), "red".to_string())].into_iter().collect(),
        }
    }

    #[test]
    fn each_engines_addresses_are_read_back() {
        let id = "abcdefghijklmnopabcdefghijklmnop";
        for addresses in [Addresses::Browsers, Addresses::Chromium, Addresses::Https] {
            for flavor in [Flavor::Chrome, Flavor::Firefox] {
                let origin = addresses.origin(flavor, id);
                assert_eq!(parse_url(&format!("{origin}/a%20b/c.js?x#y")), Some((id.to_string(), "a b/c.js".to_string())), "{origin}");
                assert_eq!(parse_url(&origin), Some((id.to_string(), String::new())), "{origin}");
            }
        }
        assert_eq!(Addresses::Chromium.origin(Flavor::Chrome, id), format!("spatimesh-extension://{id}"));
        assert_eq!(Addresses::Chromium.origin(Flavor::Firefox, id), format!("moz-extension://{id}"));
        assert_eq!(Addresses::Https.origin(Flavor::Firefox, id), format!("https://{id}.spatimesh-extension.invalid"));
        // The web's own addresses are no extension's.
        assert_eq!(parse_url("https://example.com/x"), None);
        assert_eq!(parse_url("https://spatimesh-extension.invalid.example.com/x"), None);
    }

    #[test]
    fn an_extensions_files_are_served_from_its_folder_only() {
        let root = std::env::temp_dir().join(format!("spatimesh-serve-{}", crate::store::new_id()));
        std::fs::create_dir_all(root.join("img")).unwrap();
        std::fs::write(root.join("popup.html"), "<p>hi</p>").unwrap();
        std::fs::write(root.join("img/a.png"), [1, 2, 3]).unwrap();
        std::fs::write(root.join("s.css"), "a{color:__MSG_color__}").unwrap();
        std::fs::write(root.parent().unwrap().join("secret.txt"), "no").unwrap();
        let set = ExtensionSet { extensions: vec![spec(&root)], page_start: None };
        let base = "chrome-extension://abcdefghijklmnopabcdefghijklmnop";
        let r = serve(&set, &format!("{base}/popup.html?x=1#y"), None).unwrap();
        assert_eq!((r.mime, r.bytes.as_slice()), ("text/html", b"<p>hi</p>".as_slice()));
        assert_eq!(serve(&set, &format!("{base}/s.css"), None).unwrap().bytes, b"a{color:red}");
        assert!(String::from_utf8(serve(&set, &format!("{base}/{BACKGROUND_PAGE}"), None).unwrap().bytes).unwrap().contains("src=\"/bg.js\""));
        assert_eq!(serve(&set, &format!("{base}/../secret.txt"), None), Err(404));
        assert_eq!(serve(&set, &format!("{base}/%2E%2E/secret.txt"), None), Err(404));
        assert_eq!(serve(&set, &format!("{base}/missing.js"), None), Err(404));
        assert_eq!(serve(&set, "chrome-extension://nobody/popup.html", None), Err(404));
        // A web page has only what was made web accessible to it.
        assert!(serve(&set, &format!("{base}/img/a.png"), Some("https://www.example.com/")).is_ok());
        assert_eq!(serve(&set, &format!("{base}/img/a.png"), Some("https://other.org/")), Err(403));
        assert_eq!(serve(&set, &format!("{base}/popup.html"), Some("https://www.example.com/")), Err(403));
        // An Origin header, the page's origin alone.
        assert!(serve(&set, &format!("{base}/img/a.png"), Some("https://www.example.com")).is_ok());
        assert!(serve(&set, &format!("{base}/popup.html"), Some(&format!("{base}/options.html"))).is_ok(), "its own pages have everything");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_service_worker_runs_as_a_page() {
        let html = background_html(&[], Some(("sw.js", true)));
        assert!(html.contains("type=\"module\" src=\"/sw.js\""));
        assert!(html.contains(WORKER_SHIM));
        let boot_code = boot("function(s){}", "{\"id\":\"x\"}");
        assert!(boot_code.contains("__spatimeshExtBoot({\"id\":\"x\"}"));
        assert!(deliver("{\"r\":1}").contains("\"{\\\"r\\\":1}\""));
    }
}
