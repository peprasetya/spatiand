//! Extensions end to end on an engine program (included by each engine's tests: WebKitGTK's,
//! CEF's): the engine host as SpatiMesh runs it, the extension runtime as SpatiMesh's window runs
//! it (the test plays the window), and a Chrome Manifest V3 extension and a Firefox Manifest V2
//! one installed from folders -- their content scripts talking to their backgrounds (a service
//! worker run as a page; background scripts), storage kept, tabs queried, a file fetched that is
//! made web accessible, and code run in a tab when an extension asks.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

use spatimesh::engine::{Happened, ProfileSetup, TabId, View};
use spatimesh::extensions::runtime::{BrowserSide, Runtime};
use spatimesh::extensions::set::{Addresses, ExtensionSet};
use spatimesh::extensions::storage::{self, Area, Session};
use spatimesh::extensions::install;
use spatimesh::store::Store;
use spatimesh::tabs::{Place, Tabs, WindowKey};
use spatimesh::wire::{FromHost, Link, ToHost};

/// The test as SpatiMesh's window: one window of tabs, an engine host, and what pages said.
struct Window {
    child: Child,
    link: Link,
    tabs: Tabs,
    titles: Vec<(TabId, String)>,
    /// Pages that could not be loaded, and tabs whose page stopped: what a failure says.
    trouble: Vec<String>,
}

impl Window {
    fn send(&mut self, message: ToHost) {
        self.link.send(&message.encode(), &[]).expect("the engine is listening");
    }

    fn title_of(&self, tab: TabId) -> Option<&str> {
        self.titles.iter().rev().find(|(t, _)| *t == tab).map(|(_, title)| title.as_str())
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        let _ = self.link.send(&ToHost::Quit.encode(), &[]);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

const VIEW: View = View { width: 400, height: 300, zoom: 1.0 };

impl BrowserSide for Window {
    fn tabs(&self) -> &Tabs {
        &self.tabs
    }
    fn last_focused(&self) -> Option<WindowKey> {
        self.tabs.windows().first().map(|w| w.key)
    }
    fn open_tab(&mut self, _profile: &str, window: Option<WindowKey>, url: &str, _index: Option<usize>, active: bool, opener: Option<TabId>) -> Option<TabId> {
        let window = window.or_else(|| self.last_focused())?;
        let tab = self.tabs.new_tab(window, url, Place::End, active, opener)?;
        self.send(ToHost::Open { tab, url: url.to_string(), view: VIEW });
        Some(tab)
    }
    fn open_window(&mut self, _profile: &str, _urls: &[String], _tab: Option<TabId>, _focused: bool) -> Option<WindowKey> {
        None
    }
    fn load(&mut self, tab: TabId, url: &str) {
        self.send(ToHost::Load { tab, url: url.to_string() });
    }
    fn activate(&mut self, tab: TabId) {
        self.tabs.activate_tab(tab);
    }
    fn close_tab(&mut self, tab: TabId) {
        self.tabs.close_tab(tab);
        self.send(ToHost::Close { tab });
    }
    fn close_window(&mut self, _window: WindowKey) {}
    fn focus_window(&mut self, _window: WindowKey) {}
    fn reload(&mut self, tab: TabId) {
        self.send(ToHost::Reload { tab });
    }
    fn back(&mut self, tab: TabId) {
        self.send(ToHost::Back { tab });
    }
    fn forward(&mut self, tab: TabId) {
        self.send(ToHost::Forward { tab });
    }
    fn mute(&mut self, tab: TabId, muted: bool) {
        self.send(ToHost::Mute { tab, muted });
    }
    fn move_tab(&mut self, _tab: TabId, _window: WindowKey, _index: Option<usize>) {}
    fn zoom(&self) -> f32 {
        1.0
    }
    fn set_zoom(&mut self, _zoom: f32) {}
    fn page_size(&self, _window: WindowKey) -> (u32, u32) {
        (VIEW.width, VIEW.height)
    }
    fn set_extensions(&mut self, _profile: &str, set: &ExtensionSet) {
        let set = serde_json::to_string(set).unwrap();
        self.send(ToHost::Extensions { set });
    }
    fn run_script(&mut self, _profile: &str, tab: TabId, world: &str, all_frames: bool, code: &str, request: u64, boot: bool) {
        self.send(ToHost::RunScript { tab, world: world.to_string(), all_frames, code: code.to_string(), request, boot });
    }
    fn open_hidden(&mut self, _profile: &str, tab: TabId, url: &str) {
        self.send(ToHost::Open { tab, url: url.to_string(), view: VIEW });
    }
    fn close_hidden(&mut self, _profile: &str, tab: TabId) {
        self.send(ToHost::Close { tab });
    }
    fn open_popup(&mut self, _profile: &str, _window: WindowKey, _tab: TabId, _url: &str) {}
    fn close_popup(&mut self, _profile: &str, _tab: TabId) {}
    fn notify(&mut self, _title: &str, _message: &str) {}
    fn actions_changed(&mut self, _profile: &str) {}
    fn capture(&mut self, _window: WindowKey) -> Option<(u32, u32, Vec<u8>)> {
        None
    }
}

/// How the engine's program ended, if it has.
fn ended(w: &mut Window) -> String {
    use std::os::unix::process::ExitStatusExt;
    std::thread::sleep(Duration::from_millis(200));
    match w.child.try_wait() {
        Ok(Some(status)) => match (status.code(), status.signal()) {
            (Some(code), _) => format!("it ended with {code}"),
            (None, Some(signal)) => format!("it was stopped by signal {signal}"),
            _ => format!("it ended: {status}"),
        },
        _ => "it is still running".into(),
    }
}

/// Read what the engine says once, answering extensions' calls with the runtime.
fn step(rt: &mut Runtime, w: &mut Window) {
    if w.link.is_closed() {
        let how = ended(w);
        panic!("the engine went ({how}); the pages' titles were {:?}; trouble: {:?}", w.titles, w.trouble);
    }
    w.link.wait_readable(50);
    for (message, _) in w.link.receive_from_host() {
        match message {
            FromHost::Picture { tab, buffer, .. } => w.send(ToHost::Release { tab, buffer }),
            FromHost::Happened(Happened::Extension { tab, json }) => rt.call(w, "default", tab, &json),
            FromHost::Happened(Happened::ScriptResult { request, result, .. }) => rt.script_result(w, request, result),
            FromHost::Happened(Happened::Title { tab, title }) => w.titles.push((tab, title)),
            FromHost::Happened(Happened::Dialog { tab, .. }) => w.send(ToHost::Answer { tab, accepted: true, text: None }),
            FromHost::Happened(Happened::Failed { tab, url, why }) => w.trouble.push(format!("tab {tab} could not load {url}: {why}")),
            FromHost::Happened(Happened::Crashed { tab, why }) => w.trouble.push(format!("tab {tab} stopped: {why}")),
            _ => {}
        }
    }
    rt.tick(w);
}

/// Steps until `done`.
fn pump(rt: &mut Runtime, w: &mut Window, what: &str, seconds: u64, done: impl Fn(&Window) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !done(w) {
        assert!(Instant::now() < deadline, "waited {seconds} s for {what}; the pages' titles were {:?}; trouble: {:?}", w.titles, w.trouble);
        step(rt, w);
    }
}

/// A web server on this machine: every page is a plain one, titled by its path.
fn server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut request = [0u8; 4096];
            let n = stream.read(&mut request).unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..n]);
            let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
            let body = format!("<!doctype html><title>page {path}</title><p>{path}</p>");
            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        }
    });
    address
}

fn folder(root: &Path, name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = root.join(name);
    for (path, text) in files {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    dir
}

const CHROME_MANIFEST: &str = r#"{
  "manifest_version": 3,
  "name": "Chrome's kind",
  "version": "1.0",
  "permissions": ["storage", "tabs", "scripting"],
  "host_permissions": ["http://127.0.0.1/*"],
  "background": { "service_worker": "sw.js" },
  "content_scripts": [{ "matches": ["http://127.0.0.1/chrome*"], "js": ["shared.js", "content.js"], "run_at": "document_end" }],
  "web_accessible_resources": [{ "resources": ["data.txt"], "matches": ["http://127.0.0.1/*"] }]
}"#;

/// The service worker: answers the content script once it has kept what it was told and found
/// the content script's tab among the tabs; runs code in that tab when asked, keeping its result.
const CHROME_WORKER: &str = r#"
chrome.runtime.onMessage.addListener(function (message, sender, sendResponse) {
  if (message.greeting) {
    chrome.storage.local.set({ seen: message.greeting })
      .then(function () { return chrome.tabs.query({ url: 'http://127.0.0.1/*' }); })
      .then(function (tabs) {
        var found = tabs.some(function (t) { return t.id === sender.tab.id; });
        sendResponse({ answer: message.greeting + '-' + (found ? 'tab' : 'notab') });
      });
    return true;
  }
  if (message.script) {
    chrome.scripting.executeScript({
      target: { tabId: sender.tab.id },
      func: function (x) { document.title = 'scripted:' + x; return x + '!'; },
      args: ['yes']
    }).then(function (results) { return chrome.storage.local.set({ result: results[0].result }); });
  }
});
"#;

/// Two files of one content script, sharing what the first declares, as in Chrome.
const CHROME_SHARED: &str = "const GREETING = 'hello';";
const CHROME_CONTENT: &str = r#"
chrome.runtime.sendMessage({ greeting: GREETING }, function (reply) {
  fetch(chrome.runtime.getURL('data.txt')).then(function (r) { return r.text(); }).then(function (text) {
    document.title = 'chrome:' + reply.answer + ':' + text.trim() + ':' + typeof chrome.tabs;
    chrome.runtime.sendMessage({ script: true });
  }, function (e) { document.title = 'chrome-fetch-failed:' + e; });
});
"#;

const FIREFOX_MANIFEST: &str = r#"{
  "manifest_version": 2,
  "name": "Firefox's kind",
  "version": "2.0",
  "permissions": ["storage", "http://127.0.0.1/*"],
  "background": { "scripts": ["bg.js"] },
  "content_scripts": [{ "matches": ["http://127.0.0.1/firefox*"], "js": ["content.js"] }],
  "browser_specific_settings": { "gecko": { "id": "test@spatimesh" } }
}"#;

/// Firefox's way: a listener's promise is the answer.
const FIREFOX_BACKGROUND: &str = r#"
browser.runtime.onMessage.addListener(function (message) {
  return browser.storage.local.get('n').then(function (kept) {
    return browser.storage.local.set({ n: (kept.n || 0) + message.n });
  }).then(function () {
    return { doubled: message.n * 2, id: browser.runtime.id };
  });
});
"#;

const FIREFOX_CONTENT: &str = r#"
browser.runtime.sendMessage({ n: 21 }).then(function (r) { document.title = 'firefox:' + r.doubled + ':' + r.id; });
"#;

/// Not tested here, and why: a failure where the engine is `required`.
fn skip(required: bool, why: &str) {
    assert!(!required, "{why}");
    eprintln!("{why}; not tested here");
}

/// Both extensions on the engine program at `program`, which gives them their `addresses`.
/// Where there is no display, or the engine cannot start, this says so and passes -- unless the
/// engine is `required` (as CI requires CEF's and the Mac's). A Mac's session has its window
/// server, and no DISPLAY.
pub fn run(program: &str, addresses: Addresses, required: bool) {
    if cfg!(not(target_os = "macos")) && std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        skip(required, "no display: extensions are not tested here (run under xvfb-run)");
        return;
    }
    let name = Path::new(program).file_name().unwrap().to_string_lossy().to_string();
    let root = std::env::temp_dir().join(format!("{name}-extensions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let chrome = folder(
        &root,
        "chrome",
        &[("manifest.json", CHROME_MANIFEST), ("sw.js", CHROME_WORKER), ("shared.js", CHROME_SHARED), ("content.js", CHROME_CONTENT), ("data.txt", "payload\n")],
    );
    let firefox = folder(&root, "firefox", &[("manifest.json", FIREFOX_MANIFEST), ("bg.js", FIREFOX_BACKGROUND), ("content.js", FIREFOX_CONTENT)]);
    let store = Store::at(&root.join("store"));
    let (chrome_ext, _) = install::install(&store, "default", &chrome).unwrap();
    let (firefox_ext, _) = install::install(&store, "default", &firefox).unwrap();

    let setup = ProfileSetup { id: "private".into(), private: true, data: PathBuf::new(), cache: PathBuf::new() };
    // What the pages say on their consoles, in the test's output when one fails.
    std::env::set_var("SPATIMESH_CONSOLE", "1");
    let (child, link) = spatimesh::host::start(Path::new(program), &setup).expect("the engine program starts");
    let mut w = Window { child, link, tabs: Tabs::new(), titles: Vec::new(), trouble: Vec::new() };
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(Instant::now() < deadline, "the engine said nothing");
        w.link.wait_readable(100);
        let hello = w.link.receive_from_host().into_iter().find_map(|(m, _)| match m {
            FromHost::Hello { engine } => Some(engine),
            _ => None,
        });
        if let Some(engine) = hello {
            eprintln!("engine: {engine}");
            if engine.starts_with("failed") {
                skip(required, &format!("the engine could not start: {engine}"));
                return;
            }
            break;
        }
        if w.link.is_closed() {
            skip(required, "the engine program ended at once (built without its engine?)");
            return;
        }
    }
    w.tabs.new_window("default");
    let mut rt = Runtime::new(store.clone());
    rt.set_addresses(addresses);
    rt.engine_started(&mut w, "default");
    rt.startup_done("default");

    let site = server();
    let chrome_tab = w.open_tab("default", None, &format!("{site}/chrome"), None, true, None).unwrap();
    let firefox_tab = w.open_tab("default", None, &format!("{site}/firefox"), None, false, None).unwrap();

    // Chrome's: its two files share a world, its worker answers after keeping and querying,
    // its web-accessible file is fetched, and a content script has no `tabs`.
    pump(&mut rt, &mut w, "Chrome's content script to hear back", 40, |w| w.title_of(chrome_tab).is_some_and(|t| t.starts_with("chrome")));
    assert_eq!(w.title_of(chrome_tab), Some("chrome:hello-tab:payload:undefined"));
    // Firefox's: its background's promise is the answer, and its id is its add-on id.
    pump(&mut rt, &mut w, "Firefox's content script to hear back", 40, |w| w.title_of(firefox_tab).is_some_and(|t| t.starts_with("firefox")));
    assert_eq!(w.title_of(firefox_tab), Some("firefox:42:test@spatimesh"));
    // Code run in the tab by Chrome's worker, and its result back to the worker.
    pump(&mut rt, &mut w, "the worker's script to run in the tab", 40, |w| w.title_of(chrome_tab) == Some("scripted:yes"));
    let session = Session::default();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let kept = storage::all(&store, &session, "default", &chrome_ext.id, Area::Local);
        if kept.get("result").is_some() {
            assert_eq!(kept.get("seen"), Some(&serde_json::json!("hello")));
            assert_eq!(kept.get("result"), Some(&serde_json::json!("yes!")));
            break;
        }
        assert!(Instant::now() < deadline, "the script's result was never kept: {kept:?}");
        step(&mut rt, &mut w);
    }
    let kept = storage::all(&store, &session, "default", &firefox_ext.id, Area::Local);
    assert_eq!(kept.get("n"), Some(&serde_json::json!(21)));
    drop(w);
    let _ = std::fs::remove_dir_all(&root);
}
