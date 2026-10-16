//! api.js -- `chrome` and `browser` in every extension context -- run in Node, as an engine would
//! run it: booted for each flavour (Chrome's, Firefox's) and each kind of context (a content
//! script, the extension's own page), its calls read from the bridge and answers and events
//! handed back as the window's runtime hands them (src/extensions/runtime). Where there is no
//! `node`, this says so and passes.

use std::process::Command;

use spatimesh::extensions::set;

fn node() -> Option<String> {
    for name in ["node", "nodejs"] {
        if Command::new(name).arg("--version").output().is_ok_and(|o| o.status.success()) {
            return Some(name.to_string());
        }
    }
    None
}

fn config(flavor: &str, kind: &str) -> String {
    serde_json::json!({
        "id": "abcdefghijklmnopabcdefghijklmnop",
        "runtimeId": if flavor == "firefox" { "test@example.org" } else { "abcdefghijklmnopabcdefghijklmnop" },
        "token": "secret",
        "kind": kind,
        "flavor": flavor,
        "manifest": { "manifest_version": 3, "name": "T", "version": "1", "action": {} },
        "messages": { "hello": { "message": "Hello $who$, you are $1", "placeholders": { "who": "$1" } } },
        "uiLanguage": "en-US",
        "origin": format!("{}://abcdefghijklmnopabcdefghijklmnop", if flavor == "firefox" { "moz-extension" } else { "chrome-extension" }),
        "api": { "storage": true, "webNavigation": true, "scripting": true },
        "version": "1",
        "incognito": false,
        "os": "linux"
    })
    .to_string()
}

/// The checks, in the context the API was booted in: `posted` has what it said, `deliver` hands
/// it something, `flavor` and `kind` are what it was booted as.
const CHECKS: &str = r#"
const assert = (ok, what) => { if (!ok) throw new Error('failed: ' + what); };
const last = () => posted[posted.length - 1];
const settle = () => new Promise(r => setTimeout(r, 0));
(async () => {
  assert(posted.length >= 1 && posted[0].c === 'ready' && posted[0].t === 'secret', 'it says it is ready, with its token');
  assert(typeof chrome === 'object' && typeof chrome.runtime.sendMessage === 'function', 'chrome is there');
  assert((typeof browser === 'object') === (flavor === 'firefox'), 'browser only for Firefox');
  assert((typeof chrome.tabs === 'object') === (kind !== 'content'), 'tabs only for its own pages');
  assert(typeof chrome.storage.local.get === 'function', 'storage, asked for');
  assert(chrome.runtime.getURL('/a.html') === origin + '/a.html', 'getURL');
  assert(chrome.i18n.getMessage('hello', 'Ann') === 'Hello Ann, you are Ann', 'getMessage with placeholders');
  const x = posted[0].x;

  // A message and its answer.
  const p = chrome.runtime.sendMessage({ a: 1 });
  const sent = last();
  assert(sent.c === 'runtime.sendMessage' && JSON.stringify(sent.a) === '[null,{"a":1}]', 'sendMessage says ' + JSON.stringify(sent));
  deliver({ to: 'someone else', r: sent.i, v: 'not mine' });
  deliver({ to: x, r: sent.i, v: 'ok' });
  assert(await p === 'ok', 'the answer');

  // Listened, none answered: Chrome's callback has lastError, a promise gets nothing.
  const q = chrome.runtime.sendMessage('x');
  deliver({ to: x, r: last().i, v: null, closed: true });
  assert(await q === undefined, 'nothing, as a promise');
  let seen = 'no callback';
  chrome.runtime.sendMessage('y', function (v) { seen = chrome.runtime.lastError ? chrome.runtime.lastError.message : 'no error:' + v; });
  deliver({ to: x, r: last().i, v: null, closed: true });
  await settle(); await settle();
  assert(flavor === 'firefox' ? seen === 'no error:undefined' : seen === 'The message port closed before a response was received.', 'callback: ' + seen);

  // An error is the promise's, and the callback's lastError.
  const failing = chrome.runtime.sendMessage('z');
  deliver({ to: x, r: last().i, x: 'Could not establish connection. Receiving end does not exist.' });
  let error = null;
  try { await failing; } catch (e) { error = e.message; }
  assert(error && error.startsWith('Could not establish'), 'rejected: ' + error);

  // A message for this context: the first sendResponse answers it.
  chrome.runtime.onMessage.addListener((m, sender, sendResponse) => { sendResponse('pong:' + m + ':' + sender.id); });
  assert(last().c === 'listen' && last().a[0] === 'runtime.onMessage', 'it says it listens');
  deliver({ to: x, ev: 'runtime.onMessage', a: ['ping', { id: 'me' }], q: 5 });
  assert(last().c === 'message.respond' && JSON.stringify(last().a) === '[5,"pong:ping:me"]', 'answered: ' + JSON.stringify(last()));
  // A promise answers it too (Firefox's way, Chrome's from 2024).
  chrome.runtime.onMessage.addListener(() => undefined);
  if (flavor === 'firefox') {
    const b = browser.runtime.onMessage;
    b.addListener(m => Promise.resolve('later:' + m));
  }

  // A port.
  const port = chrome.runtime.connect({ name: 'p' });
  const opened = last();
  assert(opened.c === 'runtime.connect' && opened.a[1] === 'p', 'connect');
  let got = null;
  port.onMessage.addListener(m => { got = m; });
  deliver({ to: x, ev: 'port.message', a: [opened.a[0], 'over the port'] });
  assert(got === 'over the port', 'a port message');
  port.postMessage({ back: true });
  assert(last().c === 'port.post' && last().a[0] === opened.a[0], 'posted on the port');

  // Events filtered per listener (webNavigation's url filters), for the extension's pages.
  if (kind !== 'content') {
    const calls = [];
    chrome.webNavigation.onCompleted.addListener(d => calls.push('a:' + d.url), { url: [{ hostEquals: 'a.example' }] });
    chrome.webNavigation.onCompleted.addListener(d => calls.push('any:' + d.url));
    deliver({ to: x, ev: 'webNavigation.onCompleted', a: [{ url: 'https://b.example/' }] });
    deliver({ to: x, ev: 'webNavigation.onCompleted', a: [{ url: 'https://a.example/x' }] });
    assert(JSON.stringify(calls) === '["any:https://b.example/","a:https://a.example/x","any:https://a.example/x"]', 'filters: ' + JSON.stringify(calls));
    // Storage's onChanged for an area, and for all.
    const changes = [];
    chrome.storage.onChanged.addListener((c, area) => changes.push(area + ':' + Object.keys(c)));
    chrome.storage.local.onChanged.addListener(c => changes.push('local-only:' + Object.keys(c)));
    deliver({ to: x, ev: 'storage.onChanged', a: [{ k: { newValue: 1 } }, 'local'] });
    assert(JSON.stringify(changes) === '["local:k","local-only:k"]', 'storage events: ' + JSON.stringify(changes));
  }

  // Code the extension runs here, and its result.
  __spatimeshExt.run(7, () => 1 + 1);
  await settle();
  assert(last().c === 'scripting.done' && JSON.stringify(last().a) === '[7,0,2,null]', 'a script result: ' + JSON.stringify(last()));
  __spatimeshExt.run(8, () => { throw new Error('nope'); });
  assert(last().c === 'scripting.done' && last().a[3] === 'nope', 'a script error');
  __spatimeshExt.run(9, () => document);
  await settle();
  assert(last().a[2] !== undefined, 'a result that cannot cross is sent as null, not dropped');

  // Firefox's browser: promises, whatever the call.
  if (flavor === 'firefox') {
    const r = browser.storage.local.get('k');
    deliver({ to: x, r: last().i, v: { k: 2 } });
    assert((await r).k === 2, 'browser.storage');
    assert(browser.runtime.id === 'test@example.org', 'runtime.id is the add-on id');
  }
  console.log('OK');
})().catch(e => { console.error(e.stack || String(e)); process.exit(1); });
"#;

#[test]
fn the_api_in_every_kind_of_context() {
    let Some(node) = node() else {
        eprintln!("no node: api.js is not run here");
        return;
    };
    let dir = std::env::temp_dir().join(format!("spatimesh-api-js-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for flavor in ["chrome", "firefox"] {
        for kind in ["content", "page"] {
            let config = config(flavor, kind);
            let origin = serde_json::from_str::<serde_json::Value>(&config).unwrap()["origin"].as_str().unwrap().to_string();
            let boot = set::boot("function(s){__posted.push(JSON.parse(s));}", &config);
            // A window of its own for the API: the context's global, as a page's is.
            let script = format!(
                r#"
const vm = require('vm');
const sandbox = {{ __posted: [], console, setTimeout, URL, Promise, JSON, process }};
sandbox.window = sandbox;
sandbox.top = sandbox;
sandbox.addEventListener = function () {{}};
sandbox.location = {{ href: {href} }};
sandbox.document = {{ readyState: 'complete' }};
vm.createContext(sandbox);
vm.runInContext({boot}, sandbox);
sandbox.posted = sandbox.__posted;
sandbox.deliver = m => sandbox.__spatimeshExt.deliver(JSON.stringify(m));
sandbox.flavor = {flavor};
sandbox.kind = {kind};
sandbox.origin = {origin};
vm.runInContext({checks}, sandbox);
"#,
                href = serde_json::to_string(&if kind == "page" { format!("{origin}/page.html") } else { "https://example.com/".to_string() }).unwrap(),
                boot = serde_json::to_string(&boot).unwrap(),
                flavor = serde_json::to_string(flavor).unwrap(),
                kind = serde_json::to_string(kind).unwrap(),
                origin = serde_json::to_string(&origin).unwrap(),
                checks = serde_json::to_string(CHECKS).unwrap(),
            );
            let file = dir.join(format!("{flavor}-{kind}.js"));
            std::fs::write(&file, script).unwrap();
            let out = Command::new(&node).arg(&file).output().unwrap();
            let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            assert!(out.status.success() && stdout.contains("OK"), "{flavor} {kind}:\n{stdout}\n{stderr}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Where an engine has no world of the extension's own (CEF, Android's WebView): the API kept
/// out of the page's globals, handed to the content scripts' closure, and the token not taken by
/// a page that sets traps on what every object has.
const CLOSED_CHECKS: &str = r#"
const assert = (ok, what) => { if (!ok) throw new Error('failed: ' + what); };
(async () => {
  // The page, before any of it runs, is the extension's: then the page's own scripts set traps.
  let stolen = [];
  Object.prototype.toJSON = function () { stolen.push(this); return this; };
  assert(typeof chrome === 'undefined' && typeof browser === 'undefined' && typeof __spatimeshExt === 'undefined', 'nothing on the page globals');
  assert(api && typeof api.deliver === 'function' && typeof api.run === 'function', 'what the engine keeps');
  assert(api.global.chrome === api.chrome, 'globalThis.chrome, for webextension-polyfill and the like');
  assert(api.global.location === location, 'the rest of globalThis is the page\'s');
  // A content script, as the engine runs it.
  const content = new Function('chrome', 'browser', 'globalThis', 'return chrome.runtime.sendMessage({ hi: 1 });');
  const p = content(api.chrome, api.browser || undefined, api.global);
  const sent = posted[posted.length - 1];
  assert(sent.c === 'runtime.sendMessage' && sent.t === 'secret', 'a call, with its token: ' + JSON.stringify(sent));
  assert(stolen.every(o => JSON.stringify(Object.keys(o)).indexOf('"t"') < 0), 'the token never passed a toJSON of the page\'s');
  api.deliver(JSON.stringify({ to: sent.x, r: sent.i, v: 'ok' }));
  assert(await p === 'ok', 'the answer, through the kept deliver');
  console.log('OK');
})().catch(e => { console.error(e.stack || String(e)); process.exit(1); });
"#;

#[test]
fn the_api_kept_from_the_page_where_there_is_no_world_of_its_own() {
    let Some(node) = node() else {
        eprintln!("no node: api.js is not run here");
        return;
    };
    let dir = std::env::temp_dir().join(format!("spatimesh-api-js-closed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for flavor in ["chrome", "firefox"] {
        let boot = set::boot_function(&config(flavor, "content"), true);
        let script = format!(
            r#"
const vm = require('vm');
const sandbox = {{ console, setTimeout, URL, Promise, JSON, process, Proxy, Reflect, Function }};
sandbox.window = sandbox;
sandbox.top = sandbox;
sandbox.addEventListener = function () {{}};
sandbox.location = {{ href: 'https://example.com/' }};
sandbox.document = {{ readyState: 'loading' }};
vm.createContext(sandbox);
sandbox.posted = [];
const post = s => sandbox.posted.push(JSON.parse(s));
sandbox.api = vm.runInContext({boot}, sandbox)(post);
vm.runInContext({checks}, sandbox);
"#,
            boot = serde_json::to_string(&boot).unwrap(),
            checks = serde_json::to_string(CLOSED_CHECKS).unwrap(),
        );
        let file = dir.join(format!("closed-{flavor}.js"));
        std::fs::write(&file, script).unwrap();
        let out = Command::new(&node).arg(&file).output().unwrap();
        let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success() && stdout.contains("OK"), "{flavor}:\n{stdout}\n{stderr}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
