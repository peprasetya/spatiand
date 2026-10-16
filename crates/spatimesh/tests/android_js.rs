//! What Android's WebView runs at the start of every page for an extension (set::page_start,
//! src/extensions/android.js) run in Node, as the WebView would: in a page's own world, with
//! the engine's message listener (`spatimeshExt`) as the WebView gives it. Its content scripts
//! go where and when they are for, with their API; its own pages get the API as globals; the
//! engine reaches the top frame through its way in, and other frames by messages. Where there
//! is no `node`, this says so and passes.

use std::process::Command;

use spatimesh::extensions::install::Flavor;
use spatimesh::extensions::set::{self, Addresses, Script, Spec};

fn node() -> Option<String> {
    for name in ["node", "nodejs"] {
        if Command::new(name).arg("--version").output().is_ok_and(|o| o.status.success()) {
            return Some(name.to_string());
        }
    }
    None
}

const ID: &str = "abcdefghijklmnopabcdefghijklmnop";

fn config(kind: &str, token: &str) -> String {
    serde_json::json!({
        "id": ID,
        "runtimeId": ID,
        "token": token,
        "kind": kind,
        "flavor": "chrome",
        "manifest": { "manifest_version": 3, "name": "T", "version": "1" },
        "messages": {},
        "uiLanguage": "en-US",
        "origin": Addresses::Https.origin(Flavor::Chrome, ID),
        "api": { "storage": true, "tabs": true },
        "version": "1",
        "incognito": false,
        "os": "android"
    })
    .to_string()
}

fn script(matches: &str, run_at: &str, js: &[&str], css: &[&str], main_world: bool) -> Script {
    Script {
        matches: vec![matches.to_string()],
        exclude_matches: Vec::new(),
        include_globs: Vec::new(),
        exclude_globs: Vec::new(),
        include: String::new(),
        globs: String::new(),
        exclude: String::new(),
        js: js.iter().enumerate().map(|(i, code)| (format!("f{i}.js"), code.to_string())).collect(),
        css: css.iter().enumerate().map(|(i, code)| (format!("f{i}.css"), code.to_string())).collect(),
        run_at: run_at.to_string(),
        all_frames: false,
        match_about_blank: false,
        main_world,
    }
    .with_regexes()
}

fn spec() -> Spec {
    Spec {
        id: ID.into(),
        root: std::env::temp_dir(),
        flavor: Flavor::Chrome,
        origin: Addresses::Https.origin(Flavor::Chrome, ID),
        scripts: vec![
            // At the start: its API is there (not `tabs`, as for a content script), and a message
            // it sends is answered.
            script(
                "https://example.com/*",
                "document_start",
                &["marks.push('start:' + typeof chrome.runtime.sendMessage + ':' + typeof chrome.tabs); chrome.runtime.sendMessage('hi').then(function (v) { marks.push('answer:' + v); });"],
                &[],
                false,
            ),
            // The page's own world: no API.
            script("https://example.com/*", "document_start", &["marks.push('main:' + typeof chrome);"], &[], true),
            // Once the page is parsed: two files sharing what the first declares, and a style
            // sheet, written with __MSG_name__, from the start.
            script("https://example.com/*", "document_end", &["const SHARED = 'shared';", "marks.push('end:' + SHARED);"], &["a{color:__MSG_color__}"], false),
            // Another site's.
            script("https://example.org/*", "document_start", &["marks.push('wrong page');"], &[], false),
        ],
        web_accessible: vec![(vec!["img/*".into()], vec!["https://example.com/*".into()])],
        background: None,
        background_html: None,
        content_config: config("content", "content-token"),
        page_config: config("page", "page-token"),
        messages: [("color".to_string(), "red".to_string())].into_iter().collect(),
        rules: Vec::new(),
    }
}

/// A page at `href` (of `origin`), the extension's script put at its start: what the checks
/// have to hand.
const HARNESS: &str = r#"
const vm = require('vm');
function page(href, origin) {
  const sandbox = { console, setTimeout, URL, process, EventTarget, Event, marks: [] };
  sandbox.window = sandbox;
  sandbox.top = sandbox;
  sandbox.addEventListener = function () {};
  sandbox.location = { href: href, origin: origin };
  const document = new EventTarget();
  document.readyState = 'loading';
  document.adoptedStyleSheets = [];
  sandbox.document = document;
  sandbox.CSSStyleSheet = class { replaceSync(css) { this.css = css; } };
  const listeners = [];
  const posted = [];
  sandbox.spatimeshExt = {
    postMessage: function (s) { posted.push(JSON.parse(s)); },
    addEventListener: function (type, fn) { if (type === 'message') listeners.push(fn); }
  };
  vm.createContext(sandbox);
  vm.runInContext(SCRIPT, sandbox);
  return {
    sandbox, posted, document,
    parsed: () => { document.readyState = 'interactive'; document.dispatchEvent(new Event('DOMContentLoaded')); },
    message: (trusted, e, d) => listeners.forEach(l => l({ isTrusted: trusted, data: JSON.stringify({ e: e, d: JSON.stringify(d) }) }))
  };
}
const assert = (ok, what) => { if (!ok) throw new Error('failed: ' + what); };
const settle = () => new Promise(r => setTimeout(r, 0));
"#;

const CHECKS: &str = r#"
(async () => {
  // A page its content scripts are for.
  const p = page('https://example.com/a', 'https://example.com');
  const m = p.sandbox.marks;
  assert(p.posted.length >= 1 && p.posted[0].c === 'ready' && p.posted[0].t === 'content-token', 'its API says it is ready: ' + JSON.stringify(p.posted[0]));
  assert(m[0] === 'start:function:undefined', 'at the start, with its API: ' + JSON.stringify(m));
  assert(m[1] === 'main:undefined', 'the page world\'s, without: ' + JSON.stringify(m));
  assert(typeof p.sandbox.chrome === 'undefined', 'nothing on the page\'s globals');
  assert(p.document.adoptedStyleSheets.length === 1 && p.document.adoptedStyleSheets[0].css === 'a{color:red}', 'its style sheet, from the start');
  assert(m.indexOf('end:shared') < 0, 'not yet at the end');
  p.parsed();
  assert(m.indexOf('end:shared') >= 0, 'once parsed, the files sharing a name: ' + JSON.stringify(m));
  assert(m.indexOf('wrong page') < 0, 'another site\'s not here');

  // Its message answered by a delivery from the engine; a page's own events, and another
  // extension's deliveries, not taken.
  const sent = p.posted.find(c => c.c === 'runtime.sendMessage');
  assert(sent && sent.t === 'content-token', 'the message went out: ' + JSON.stringify(p.posted));
  p.message(false, ID, { to: sent.x, r: sent.i, v: 'forged' });
  p.message(true, 'another', { to: sent.x, r: sent.i, v: 'not mine' });
  await settle();
  assert(m.indexOf('answer:forged') < 0 && m.indexOf('answer:not mine') < 0, 'only the engine\'s own, for this extension: ' + JSON.stringify(m));
  p.message(true, ID, { to: sent.x, r: sent.i, v: 'yes' });
  await settle(); await settle();
  assert(m.indexOf('answer:yes') >= 0, 'the answer: ' + JSON.stringify(m));

  // The engine's way into the top frame: given the API as globalThis, saying the secret.
  const way = p.sandbox[ENTRY];
  assert(typeof way === 'function' && Object.keys(p.sandbox).indexOf(ENTRY) < 0, 'a way in, not listed');
  assert(way('not the secret', g => 'in', false) === undefined, 'not without the secret');
  assert(way(SECRET, g => g.__spatimeshExt.id, false) === ID, 'in, with its API');
  let replaced = true;
  try { p.sandbox[ENTRY] = null; } catch (e) {}
  replaced = p.sandbox[ENTRY] !== way;
  assert(!replaced, 'the page cannot replace it');

  // A page none of them is for: nothing runs, nothing is said, and only a script asked for by
  // the engine makes the API.
  const q = page('https://example.net/', 'https://example.net');
  q.parsed();
  assert(q.posted.length === 0 && q.sandbox.marks.length === 0, 'nothing on another site');
  assert(q.sandbox[ENTRY](SECRET, g => 'in', false) === undefined, 'no API made unasked');
  assert(q.sandbox[ENTRY](SECRET, g => typeof g.__spatimeshExt.deliver, true) === 'function', 'made when the engine asks');

  // The extension's own page: its API as the page's globals, with its page token.
  const own = page(ORIGIN + '/popup.html', ORIGIN);
  assert(typeof own.sandbox.chrome === 'object' && typeof own.sandbox.chrome.tabs === 'object', 'its own page has the API, tabs and all');
  assert(own.posted[0].c === 'ready' && own.posted[0].t === 'page-token', 'with the page token');
  assert(own.sandbox[ENTRY] === undefined, 'no way in needed there');
  console.log('OK');
})().catch(e => { console.error(e.stack || String(e)); process.exit(1); });
"#;

#[test]
fn an_extension_in_androids_webview() {
    let Some(node) = node() else {
        eprintln!("no node: Android's page script is not run here");
        return;
    };
    let spec = spec();
    let starts = set::page_starts(&set::ExtensionSet { extensions: vec![spec.clone()], page_start: None }, "the-secret", "_way");
    let (id, script) = &starts.scripts[0];
    assert_eq!(id, ID);
    let entry = format!("_way{ID}");
    let program = format!(
        "const SCRIPT = {script};\nconst ID = {id};\nconst ENTRY = {entry};\nconst SECRET = {secret};\nconst ORIGIN = {origin};\n{HARNESS}\n{CHECKS}",
        script = serde_json::to_string(script).unwrap(),
        id = serde_json::to_string(ID).unwrap(),
        entry = serde_json::to_string(&entry).unwrap(),
        secret = serde_json::to_string("the-secret").unwrap(),
        origin = serde_json::to_string(&spec.origin).unwrap(),
    );
    let dir = std::env::temp_dir().join(format!("spatimesh-android-js-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("android.js");
    std::fs::write(&file, program).unwrap();
    let out = Command::new(&node).arg(&file).output().unwrap();
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success() && stdout.contains("OK"), "{stdout}\n{stderr}");
    // What the engine serves by: the worker shim, and the web-accessible paths as expressions.
    assert!(starts.worker.contains("importScripts"));
    let pages = spatimesh::extensions::pattern::Patterns::new(&["https://example.com/*".to_string()]).regex();
    assert_eq!(starts.web_accessible, vec![(ID.to_string(), vec![("^img/.*$".to_string(), pages)])]);
}
