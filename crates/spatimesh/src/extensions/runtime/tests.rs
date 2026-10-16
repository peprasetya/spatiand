//! The runtime against a stand-in browser: extensions installed from folders, their contexts
//! played by hand as api.js would say things, and what the runtime has delivered read back
//! from the code it asked the engine to run.

use serde_json::{json, Value};

use super::*;
use crate::store::Scratch;
use crate::tabs::Place;

/// A browser of one window, with what the runtime asked of it written down.
struct Side {
    tabs: Tabs,
    focused: Option<WindowKey>,
    zoom: f32,
    sets: Vec<(String, ExtensionSet)>,
    /// (profile, tab, world, all frames, code, request)
    scripts: Vec<(String, TabId, String, bool, String, u64)>,
    hidden: Vec<(String, TabId, String)>,
    closed_hidden: Vec<TabId>,
    popups: Vec<(WindowKey, TabId, String)>,
    notes: Vec<(String, String)>,
    loads: Vec<(TabId, String)>,
    closed: Vec<TabId>,
}

impl Side {
    fn new(urls: &[&str]) -> Side {
        let mut tabs = Tabs::new();
        let window = tabs.new_window("default");
        for u in urls {
            tabs.new_tab(window, u, Place::End, true, None);
        }
        Side {
            tabs,
            focused: Some(window),
            zoom: 1.0,
            sets: Vec::new(),
            scripts: Vec::new(),
            hidden: Vec::new(),
            closed_hidden: Vec::new(),
            popups: Vec::new(),
            notes: Vec::new(),
            loads: Vec::new(),
            closed: Vec::new(),
        }
    }

    /// Everything delivered to extension contexts since last asked, as api.js would get it.
    fn delivered(&mut self) -> Vec<Value> {
        let prefix = "globalThis.__spatimeshExt&&globalThis.__spatimeshExt.deliver(";
        let out = self
            .scripts
            .iter()
            .filter_map(|(_, _, _, _, code, _)| code.strip_prefix(prefix)?.strip_suffix(')').map(str::to_string))
            .map(|s| serde_json::from_str::<Value>(&serde_json::from_str::<String>(&s).unwrap()).unwrap())
            .collect();
        self.scripts.retain(|(_, _, _, _, code, _)| !code.starts_with(prefix));
        out
    }
}

impl BrowserSide for Side {
    fn tabs(&self) -> &Tabs {
        &self.tabs
    }
    fn last_focused(&self) -> Option<WindowKey> {
        self.focused
    }
    fn open_tab(&mut self, profile: &str, window: Option<WindowKey>, url: &str, index: Option<usize>, active: bool, opener: Option<TabId>) -> Option<TabId> {
        let window = window.unwrap_or_else(|| self.tabs.new_window(profile));
        let tab = self.tabs.new_tab(window, url, Place::End, active, opener)?;
        if let Some(i) = index {
            let from = self.tabs.window(window)?.tabs.len() - 1;
            self.tabs.reorder(window, from, i);
        }
        Some(tab)
    }
    fn open_window(&mut self, profile: &str, urls: &[String], tab: Option<TabId>, _focused: bool) -> Option<WindowKey> {
        let window = self.tabs.new_window(profile);
        for u in urls {
            self.tabs.new_tab(window, u, Place::End, true, None);
        }
        if let Some(t) = tab {
            self.tabs.move_tab(t, window, None).ok()?;
        }
        Some(window)
    }
    fn load(&mut self, tab: TabId, url: &str) {
        self.loads.push((tab, url.to_string()));
        if let Some(t) = self.tabs.tab_mut(tab) {
            t.url = url.to_string();
        }
    }
    fn activate(&mut self, tab: TabId) {
        self.tabs.activate_tab(tab);
    }
    fn close_tab(&mut self, tab: TabId) {
        self.closed.push(tab);
        self.tabs.close_tab(tab);
    }
    fn close_window(&mut self, window: WindowKey) {
        self.tabs.close_window(window);
    }
    fn focus_window(&mut self, window: WindowKey) {
        self.focused = Some(window);
    }
    fn reload(&mut self, _tab: TabId) {}
    fn back(&mut self, _tab: TabId) {}
    fn forward(&mut self, _tab: TabId) {}
    fn mute(&mut self, tab: TabId, muted: bool) {
        if let Some(t) = self.tabs.tab_mut(tab) {
            t.muted = muted;
        }
    }
    fn move_tab(&mut self, tab: TabId, window: WindowKey, index: Option<usize>) {
        let _ = self.tabs.move_tab(tab, window, index);
    }
    fn zoom(&self) -> f32 {
        self.zoom
    }
    fn set_zoom(&mut self, zoom: f32) {
        self.zoom = zoom;
    }
    fn page_size(&self, _window: WindowKey) -> (u32, u32) {
        (1580, 868)
    }
    fn set_extensions(&mut self, profile: &str, set: &ExtensionSet) {
        self.sets.push((profile.to_string(), set.clone()));
    }
    fn run_script(&mut self, profile: &str, tab: TabId, world: &str, all_frames: bool, code: &str, request: u64) {
        self.scripts.push((profile.to_string(), tab, world.to_string(), all_frames, code.to_string(), request));
    }
    fn open_hidden(&mut self, profile: &str, tab: TabId, url: &str) {
        self.hidden.push((profile.to_string(), tab, url.to_string()));
    }
    fn close_hidden(&mut self, _profile: &str, tab: TabId) {
        self.closed_hidden.push(tab);
    }
    fn open_popup(&mut self, _profile: &str, window: WindowKey, tab: TabId, url: &str) {
        self.popups.push((window, tab, url.to_string()));
    }
    fn close_popup(&mut self, _profile: &str, _tab: TabId) {}
    fn notify(&mut self, title: &str, message: &str) {
        self.notes.push((title.to_string(), message.to_string()));
    }
    fn actions_changed(&mut self, _profile: &str) {}
    fn capture(&mut self, _window: WindowKey) -> Option<(u32, u32, Vec<u8>)> {
        Some((2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]))
    }
}

/// An extension's folder, made of `files`.
fn folder(scratch: &Scratch, name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = scratch.0.join(name);
    for (path, text) in files {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    dir
}

const CHROME_V3: &str = r#"{
  "manifest_version": 3, "name": "__MSG_name__", "version": "1.0", "default_locale": "en",
  "permissions": ["storage", "tabs", "alarms", "scripting", "activeTab", "notifications", "contextMenus"],
  "optional_permissions": ["bookmarks"],
  "host_permissions": ["https://example.com/*"],
  "background": { "service_worker": "sw.js" },
  "content_scripts": [{ "matches": ["https://example.com/*"], "exclude_matches": ["https://example.com/private/*"], "js": ["content.js"], "css": ["content.css"], "run_at": "document_start" }],
  "action": { "default_popup": "popup.html", "default_title": "Test" },
  "options_page": "options.html"
}"#;

const FIREFOX_V2: &str = r#"{
  "manifest_version": 2, "name": "Fox", "version": "2.0",
  "permissions": ["storage", "<all_urls>"],
  "background": { "scripts": ["bg.js"] },
  "browser_action": { "default_title": "Fox" },
  "browser_specific_settings": { "gecko": { "id": "fox@example.org" } }
}"#;

/// A context of an extension's, saying things as api.js does.
#[derive(Clone)]
struct Ctx {
    tab: TabId,
    token: String,
    extension: String,
    id: String,
    frame: i64,
    url: String,
    next: u64,
}

impl Ctx {
    fn say(&mut self, rt: &mut Runtime, side: &mut Side, call: &str, args: Value) -> u64 {
        self.next += 1;
        let message = json!({ "t": self.token, "e": self.extension, "k": "x", "f": self.frame, "x": self.id, "u": self.url, "c": call, "a": args, "i": self.next });
        rt.call(side, "default", self.tab, &message.to_string());
        self.next
    }

    /// A call that wants no answer.
    fn tell(&self, rt: &mut Runtime, side: &mut Side, call: &str, args: Value) {
        let message = json!({ "t": self.token, "e": self.extension, "k": "x", "f": self.frame, "x": self.id, "u": self.url, "c": call, "a": args });
        rt.call(side, "default", self.tab, &message.to_string());
    }
}

fn config(spec: &Spec, content: bool) -> Value {
    serde_json::from_str(if content { &spec.content_config } else { &spec.page_config }).unwrap()
}

/// A profile running the Chrome extension above, its background loaded and listening for
/// messages, and a content script in the window's tab at example.com.
struct World {
    _scratch: Scratch,
    rt: Runtime,
    side: Side,
    id: String,
    bg: Ctx,
    content: Ctx,
}

fn world() -> World {
    let scratch = Scratch::new("ext-runtime");
    let store = scratch.store();
    let dir = folder(
        &scratch,
        "chrome-ext",
        &[
            ("manifest.json", CHROME_V3),
            ("_locales/en/messages.json", r#"{"name": {"message": "Tester"}}"#),
            ("sw.js", "self.addEventListener('install', () => {});"),
            ("content.js", "console.log('content');"),
            ("content.css", "body { outline: 1px solid red; }"),
            ("popup.html", "<p>popup</p>"),
            ("options.html", "<p>options</p>"),
            ("page.html", "<p>page</p>"),
            ("inject.js", "document.title"),
        ],
    );
    let (installed, _) = install::install(&store, "default", &dir).unwrap();
    let mut rt = Runtime::new(store);
    let mut side = Side::new(&["https://example.com/a"]);
    rt.engine_started(&mut side, "default");
    let spec = side.sets.last().unwrap().1.extensions[0].clone();
    let (_, bg_tab, _) = side.hidden[0].clone();
    let origin = spec.origin();
    let mut bg = Ctx { tab: bg_tab, token: config(&spec, false)["token"].as_str().unwrap().into(), extension: installed.id.clone(), id: "bg".into(), frame: 0, url: format!("{origin}/_generated_background_page.html"), next: 0 };
    let content_tab = side.tabs.tabs().next().unwrap().id;
    let mut content = Ctx { tab: content_tab, token: config(&spec, true)["token"].as_str().unwrap().into(), extension: installed.id.clone(), id: "content".into(), frame: 0, url: "https://example.com/a".into(), next: 0 };
    bg.tell(&mut rt, &mut side, "ready", json!(["page"]));
    bg.tell(&mut rt, &mut side, "listen", json!(["runtime.onMessage", true]));
    bg.tell(&mut rt, &mut side, "listen", json!(["runtime.onInstalled", true]));
    bg.tell(&mut rt, &mut side, "loaded", json!([]));
    content.tell(&mut rt, &mut side, "ready", json!(["content"]));
    let _ = (&mut bg, &mut content);
    World { _scratch: scratch, rt, side, id: installed.id, bg, content }
}

#[test]
fn a_profiles_extensions_are_given_to_its_engine() {
    let w = world();
    let (profile, set) = w.side.sets.last().unwrap();
    assert_eq!(profile, "default");
    let spec = &set.extensions[0];
    assert_eq!(spec.id, w.id);
    assert_eq!(spec.flavor, Flavor::Chrome);
    // Its content script, its code read, and how an engine decides which frames take it.
    let script = &spec.scripts[0];
    assert_eq!(script.js, vec![("content.js".to_string(), "console.log('content');".to_string())]);
    assert_eq!(script.css[0].0, "content.css");
    assert_eq!(script.run_at, "document_start");
    assert!(script.wants("https://example.com/a") && !script.wants("https://example.com/private/x") && !script.wants("https://other.org/"));
    assert!(!script.include.is_empty() && !script.exclude.is_empty());
    // Its service worker runs as a page, in a tab no window shows.
    assert!(spec.background_html.as_deref().unwrap().contains("src=\"/sw.js\""));
    assert_eq!(w.side.hidden[0].2, format!("chrome-extension://{}/_generated_background_page.html", w.id));
    assert!(Runtime::is_hidden(w.side.hidden[0].1));
    // Each kind of context has its own secret, its manifest localized, and the APIs it asked for.
    let (content, page) = (config(spec, true), config(spec, false));
    assert_ne!(content["token"], page["token"]);
    assert_eq!(content["kind"], "content");
    assert_eq!(page["manifest"]["name"], "Tester");
    assert_eq!(page["api"]["storage"], true);
    assert!(page["api"].get("bookmarks").is_none(), "optional, not granted");
    assert_eq!(page["messages"]["name"]["message"], "Tester");
}

#[test]
fn installing_tells_the_background_why() {
    let mut w = world();
    let got = w.side.delivered();
    assert!(got.iter().any(|m| m["to"] == "bg" && m["ev"] == "runtime.onInstalled" && m["a"][0]["reason"] == "install"), "{got:?}");
    // The next start of the profile's engine: no longer new.
    w.rt.engine_started(&mut w.side, "default");
    let (_, bg_tab, _) = w.side.hidden.last().unwrap().clone();
    let bg = Ctx { tab: bg_tab, next: 0, ..w.bg.clone() };
    bg.tell(&mut w.rt, &mut w.side, "ready", json!(["page"]));
    bg.tell(&mut w.rt, &mut w.side, "listen", json!(["runtime.onInstalled", true]));
    bg.tell(&mut w.rt, &mut w.side, "loaded", json!([]));
    assert!(!w.side.delivered().iter().any(|m| m["ev"] == "runtime.onInstalled"));
}

#[test]
fn a_content_script_and_its_background_talk() {
    let mut w = world();
    w.side.delivered();
    let call = w.content.say(&mut w.rt, &mut w.side, "runtime.sendMessage", json!([null, { "hi": 1 }]));
    let got = w.side.delivered();
    assert_eq!(got.len(), 1, "{got:?}");
    let event = &got[0];
    assert_eq!((event["to"].as_str(), event["ev"].as_str()), (Some("bg"), Some("runtime.onMessage")));
    assert_eq!(event["a"][0], json!({ "hi": 1 }));
    let sender = &event["a"][1];
    assert_eq!(sender["id"], json!(w.id));
    assert_eq!(sender["url"], "https://example.com/a");
    assert_eq!(sender["origin"], "https://example.com");
    assert_eq!(sender["tab"]["id"], json!(w.content.tab));
    assert_eq!(sender["frameId"], 0);
    // The background answers; the content script gets it, in its own world.
    let q = event["q"].as_u64().unwrap();
    w.bg.tell(&mut w.rt, &mut w.side, "message.respond", json!([q, "pong"]));
    let (_, tab, world, all, _, _) = w.side.scripts.last().unwrap().clone();
    assert_eq!((tab, world.as_str(), all), (w.content.tab, format!("spatimesh-{}", w.id).as_str(), false));
    assert_eq!(w.side.delivered(), vec![json!({ "to": "content", "r": call, "v": "pong" })]);
    // A second answer is too late; only a receiver may answer.
    w.bg.tell(&mut w.rt, &mut w.side, "message.respond", json!([q, "again"]));
    assert!(w.side.delivered().is_empty());
    // Nobody answers: the port closed before a response.
    let call = w.content.say(&mut w.rt, &mut w.side, "runtime.sendMessage", json!([null, "x"]));
    let q = w.side.delivered()[0]["q"].as_u64().unwrap();
    w.bg.tell(&mut w.rt, &mut w.side, "message.none", json!([q, true]));
    assert_eq!(w.side.delivered(), vec![json!({ "to": "content", "r": call, "v": null, "closed": true })]);
    // Nobody listening at all: an error.
    w.bg.tell(&mut w.rt, &mut w.side, "listen", json!(["runtime.onMessage", false]));
    let call = w.content.say(&mut w.rt, &mut w.side, "runtime.sendMessage", json!([null, "x"]));
    assert_eq!(w.side.delivered(), vec![json!({ "to": "content", "r": call, "x": NO_RECEIVER })]);
    // An extension that is not there.
    let call = w.content.say(&mut w.rt, &mut w.side, "runtime.sendMessage", json!(["nobody", "x"]));
    assert_eq!(w.side.delivered()[0]["r"], json!(call));
}

#[test]
fn the_background_messages_a_tab() {
    let mut w = world();
    w.content.tell(&mut w.rt, &mut w.side, "listen", json!(["runtime.onMessage", true]));
    w.side.delivered();
    let call = w.bg.say(&mut w.rt, &mut w.side, "tabs.sendMessage", json!([w.content.tab, { "do": "it" }, {}]));
    let got = w.side.delivered();
    assert_eq!(got[0]["to"], "content");
    assert_eq!(got[0]["a"][1]["id"], json!(w.id), "from the extension itself: no tab");
    assert!(got[0]["a"][1].get("tab").is_none());
    let q = got[0]["q"].as_u64().unwrap();
    w.content.tell(&mut w.rt, &mut w.side, "message.respond", json!([q, { "done": true }]));
    assert_eq!(w.side.delivered(), vec![json!({ "to": "bg", "r": call, "v": { "done": true } })]);
    // A frame that is not there.
    let call = w.bg.say(&mut w.rt, &mut w.side, "tabs.sendMessage", json!([w.content.tab, 1, { "frameId": 7 }]));
    assert_eq!(w.side.delivered()[0]["x"], NO_RECEIVER);
    let _ = call;
}

#[test]
fn what_is_sent_before_the_background_loads_waits_for_it() {
    let mut w = world();
    // The background starts again (its page reloaded): it is not there until it has loaded.
    w.bg.tell(&mut w.rt, &mut w.side, "gone", json!([]));
    w.rt.ext_mut("default", &w.id).unwrap().loaded = false;
    w.side.delivered();
    let call = w.content.say(&mut w.rt, &mut w.side, "runtime.sendMessage", json!([null, "early"]));
    assert!(w.side.delivered().is_empty());
    let bg = Ctx { id: "bg2".into(), next: 0, ..w.bg.clone() };
    bg.tell(&mut w.rt, &mut w.side, "ready", json!(["page"]));
    bg.tell(&mut w.rt, &mut w.side, "listen", json!(["runtime.onMessage", true]));
    bg.tell(&mut w.rt, &mut w.side, "loaded", json!([]));
    let got = w.side.delivered();
    let event = got.iter().find(|m| m["ev"] == "runtime.onMessage").expect("the message, once loaded");
    assert_eq!(event["to"], "bg2");
    bg.tell(&mut w.rt, &mut w.side, "message.respond", json!([event["q"], "late"]));
    assert_eq!(w.side.delivered(), vec![json!({ "to": "content", "r": call, "v": "late" })]);
}

#[test]
fn ports_go_both_ways_until_one_end_goes() {
    let mut w = world();
    w.bg.tell(&mut w.rt, &mut w.side, "listen", json!(["runtime.onConnect", true]));
    w.side.delivered();
    w.content.tell(&mut w.rt, &mut w.side, "runtime.connect", json!(["content.1", "chat", null, { "name": "chat" }]));
    let got = w.side.delivered();
    assert_eq!(got[0]["ev"], "runtime.onConnect");
    assert_eq!(got[0]["a"][0], "content.1");
    assert_eq!(got[0]["a"][1], "chat");
    w.content.tell(&mut w.rt, &mut w.side, "port.post", json!(["content.1", "ping"]));
    assert_eq!(w.side.delivered(), vec![json!({ "to": "bg", "ev": "port.message", "a": ["content.1", "ping"] })]);
    w.bg.tell(&mut w.rt, &mut w.side, "port.post", json!(["content.1", "pong"]));
    assert_eq!(w.side.delivered(), vec![json!({ "to": "content", "ev": "port.message", "a": ["content.1", "pong"] })]);
    // A port id that is not the caller's own is refused.
    w.bg.tell(&mut w.rt, &mut w.side, "runtime.connect", json!(["content.2", "", null, {}]));
    assert!(w.side.delivered().is_empty());
    // The content script's page goes: the background's end is told.
    w.content.tell(&mut w.rt, &mut w.side, "gone", json!([]));
    assert_eq!(w.side.delivered(), vec![json!({ "to": "bg", "ev": "port.disconnect", "a": ["content.1"] })]);
    // Nobody to connect to.
    let other = Ctx { id: "content-b".into(), next: 0, ..w.content.clone() };
    other.tell(&mut w.rt, &mut w.side, "ready", json!(["content"]));
    w.bg.tell(&mut w.rt, &mut w.side, "listen", json!(["runtime.onConnect", false]));
    other.tell(&mut w.rt, &mut w.side, "runtime.connect", json!(["content-b.1", "", null, {}]));
    assert_eq!(w.side.delivered(), vec![json!({ "to": "content-b", "ev": "port.disconnect", "a": ["content-b.1", NO_RECEIVER] })]);
}

#[test]
fn storage_is_kept_and_its_changes_told() {
    let mut w = world();
    w.content.tell(&mut w.rt, &mut w.side, "listen", json!(["storage.onChanged", true]));
    w.side.delivered();
    let call = w.bg.say(&mut w.rt, &mut w.side, "storage.local.set", json!([{ "count": 1 }]));
    let got = w.side.delivered();
    assert!(got.contains(&json!({ "to": "bg", "r": call, "v": null })));
    assert!(got.contains(&json!({ "to": "content", "ev": "storage.onChanged", "a": [{ "count": { "newValue": 1 } }, "local"] })));
    let call = w.content.say(&mut w.rt, &mut w.side, "storage.local.get", json!(["count"]));
    assert_eq!(w.side.delivered(), vec![json!({ "to": "content", "r": call, "v": { "count": 1 } })]);
    // Session storage is the extension's own pages' unless they open it to content scripts.
    let call = w.content.say(&mut w.rt, &mut w.side, "storage.session.get", json!([null]));
    assert!(w.side.delivered()[0]["x"].as_str().unwrap().contains("not allowed"));
    w.bg.say(&mut w.rt, &mut w.side, "storage.session.setAccessLevel", json!([{ "accessLevel": "TRUSTED_AND_UNTRUSTED_CONTEXTS" }]));
    w.side.delivered();
    let call2 = w.content.say(&mut w.rt, &mut w.side, "storage.session.get", json!([null]));
    assert_eq!(w.side.delivered(), vec![json!({ "to": "content", "r": call2, "v": {} })]);
    let _ = call;
}

#[test]
fn tokens_and_origins_are_checked() {
    let mut w = world();
    w.side.delivered();
    // A page that guessed nothing gets nothing.
    let mut stranger = Ctx { token: "guess".into(), id: "s".into(), next: 0, ..w.content.clone() };
    stranger.say(&mut w.rt, &mut w.side, "storage.local.get", json!([null]));
    assert!(w.side.delivered().is_empty());
    // The pages' token from a web page's address is refused.
    let mut fake = Ctx { token: w.bg.token.clone(), id: "f".into(), url: "https://evil.example/".into(), next: 0, ..w.content.clone() };
    fake.say(&mut w.rt, &mut w.side, "tabs.query", json!([{}]));
    assert!(w.side.delivered().is_empty());
    // A content script asking for what only the extension's pages may: refused.
    let call = w.content.say(&mut w.rt, &mut w.side, "tabs.query", json!([{}]));
    assert!(w.side.delivered()[0]["x"].as_str().unwrap().contains("not available to content scripts"), "{call}");
    // A context claimed from another tab: refused.
    let mut moved = Ctx { tab: 999, next: 0, ..w.content.clone() };
    moved.say(&mut w.rt, &mut w.side, "storage.local.get", json!([null]));
    assert!(w.side.delivered().is_empty());
}

#[test]
fn tabs_and_windows() {
    let mut w = world();
    w.side.delivered();
    let call = w.bg.say(&mut w.rt, &mut w.side, "tabs.query", json!([{ "active": true, "currentWindow": true }]));
    let got = w.side.delivered();
    assert_eq!(got[0]["r"], json!(call));
    let tabs = got[0]["v"].as_array().unwrap();
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0]["url"], "https://example.com/a");
    assert_eq!(tabs[0]["active"], true);
    assert_eq!(tabs[0]["index"], 0);
    // A relative address is the extension's own page.
    let call = w.bg.say(&mut w.rt, &mut w.side, "tabs.create", json!([{ "url": "page.html", "active": false }]));
    let created = w.side.delivered()[0]["v"].clone();
    assert_eq!(created["url"], format!("chrome-extension://{}/page.html", w.id), "{call}");
    assert_eq!(created["active"], false);
    let new_tab = created["id"].as_u64().unwrap();
    w.bg.say(&mut w.rt, &mut w.side, "tabs.update", json!([new_tab, { "url": "https://example.com/b", "active": true }]));
    assert_eq!(w.side.loads.last().unwrap(), &(new_tab, "https://example.com/b".to_string()));
    assert_eq!(w.side.delivered()[0]["v"]["active"], true);
    w.bg.say(&mut w.rt, &mut w.side, "tabs.update", json!([new_tab, { "url": "javascript:alert(1)" }]));
    assert!(w.side.delivered()[0]["x"].is_string());
    w.bg.say(&mut w.rt, &mut w.side, "tabs.query", json!([{ "url": "https://example.com/*" }]));
    assert_eq!(w.side.delivered()[0]["v"].as_array().unwrap().len(), 2);
    w.bg.say(&mut w.rt, &mut w.side, "windows.getAll", json!([{ "populate": true }]));
    let windows = w.side.delivered()[0]["v"].clone();
    assert_eq!(windows.as_array().unwrap().len(), 1);
    assert_eq!(windows[0]["tabs"].as_array().unwrap().len(), 2);
    w.bg.say(&mut w.rt, &mut w.side, "tabs.remove", json!([new_tab]));
    assert_eq!(w.side.closed, vec![new_tab]);
    w.bg.say(&mut w.rt, &mut w.side, "tabs.get", json!([12345]));
    assert_eq!(w.side.delivered().last().unwrap()["x"], "No tab with id: 12345.");
    w.bg.say(&mut w.rt, &mut w.side, "tabs.captureVisibleTab", json!([null, {}]));
    let picture = w.side.delivered().last().unwrap()["v"].as_str().unwrap().to_string();
    assert!(picture.starts_with("data:image/png;base64,iVBORw0KGgo"), "{picture}");
}

#[test]
fn tabs_are_told_without_what_an_extension_may_not_know() {
    let scratch = Scratch::new("ext-redact");
    let store = scratch.store();
    let dir = folder(&scratch, "quiet", &[("manifest.json", r#"{"manifest_version": 3, "name": "Q", "version": "1", "background": {"service_worker": "sw.js"}}"#), ("sw.js", "")]);
    let (installed, _) = install::install(&store, "default", &dir).unwrap();
    let mut rt = Runtime::new(store);
    let mut side = Side::new(&["https://secret.example/"]);
    rt.engine_started(&mut side, "default");
    let spec = side.sets.last().unwrap().1.extensions[0].clone();
    let mut bg = Ctx { tab: side.hidden[0].1, token: config(&spec, false)["token"].as_str().unwrap().into(), extension: installed.id.clone(), id: "bg".into(), frame: 0, url: format!("chrome-extension://{}/x", installed.id), next: 0 };
    bg.tell(&mut rt, &mut side, "listen", json!(["tabs.onUpdated", true]));
    bg.say(&mut rt, &mut side, "tabs.query", json!([{}]));
    let got = side.delivered();
    let tab = &got.last().unwrap()["v"][0];
    assert!(tab.get("url").is_none() && tab.get("title").is_none(), "{tab}");
    assert!(tab.get("id").is_some());
    let id = tab["id"].as_u64().unwrap();
    rt.tab_changed(&mut side, id, &Change { url: Some("https://secret.example/2".into()), loading: Some(true), ..Default::default() });
    let got = side.delivered();
    assert_eq!(got[0]["a"][1], json!({ "status": "loading" }), "the status, not where");
}

#[test]
fn scripts_run_where_an_extension_asks_and_answer() {
    let mut w = world();
    w.side.delivered();
    let call = w.bg.say(&mut w.rt, &mut w.side, "scripting.executeScript", json!([{ "target": { "tabId": w.content.tab }, "func": "function (a) { return a + 1; }", "args": [1] }]));
    let (_, tab, world, all, code, _) = w.side.scripts.last().unwrap().clone();
    assert_eq!((tab, world, all), (w.content.tab, format!("spatimesh-{}", w.id), false));
    assert!(code.contains("return (function (a) { return a + 1; }).apply(null,[1]);"), "{code}");
    let q: u64 = code.split("run(").nth(1).unwrap().split(',').next().unwrap().parse().unwrap();
    w.content.tell(&mut w.rt, &mut w.side, "scripting.done", json!([q, 0, 2, null]));
    let got = w.side.delivered();
    assert_eq!(got[0]["r"], json!(call));
    assert_eq!(got[0]["v"][0]["result"], 2);
    assert_eq!(got[0]["v"][0]["frameId"], 0);
    // Files, as Manifest V2's tabs.executeScript gives them: a list of results.
    let call = w.bg.say(&mut w.rt, &mut w.side, "tabs.executeScript", json!([w.content.tab, { "file": "inject.js" }]));
    let code = w.side.scripts.last().unwrap().4.clone();
    assert!(code.contains("(0,eval)(\"document.title"), "{code}");
    let q: u64 = code.split("run(").nth(1).unwrap().split(',').next().unwrap().parse().unwrap();
    w.content.tell(&mut w.rt, &mut w.side, "scripting.done", json!([q, 0, "Title", null]));
    assert_eq!(w.side.delivered()[0], json!({ "to": "bg", "r": call, "v": ["Title"] }));
    // The page's own world: the engine's answer.
    w.bg.say(&mut w.rt, &mut w.side, "scripting.executeScript", json!([{ "target": { "tabId": w.content.tab }, "func": "() => 3", "world": "MAIN" }]));
    let (_, _, world, _, _, request) = w.side.scripts.last().unwrap().clone();
    assert_eq!(world, "");
    w.rt.script_result(&mut w.side, request, Ok("3".into()));
    assert_eq!(w.side.delivered()[0]["v"][0]["result"], 3);
    // A page it may not touch.
    let other = w.side.open_tab("default", w.side.focused, "https://other.org/", None, false, None).unwrap();
    w.bg.say(&mut w.rt, &mut w.side, "scripting.executeScript", json!([{ "target": { "tabId": other }, "func": "() => 1" }]));
    assert!(w.side.delivered()[0]["x"].as_str().unwrap().starts_with("Cannot access contents of url"));
    // Content scripts registered while it runs: kept, and given to the engine.
    let before = w.side.sets.len();
    w.bg.say(&mut w.rt, &mut w.side, "scripting.registerContentScripts", json!([[{ "id": "late", "matches": ["https://other.org/*"], "js": ["inject.js"] }]]));
    assert_eq!(w.side.sets.len(), before + 1);
    let spec = &w.side.sets.last().unwrap().1.extensions[0];
    assert!(spec.scripts.iter().any(|s| s.wants("https://other.org/x") && s.js[0].0 == "inject.js"));
    w.bg.say(&mut w.rt, &mut w.side, "scripting.registerContentScripts", json!([[{ "id": "late", "matches": ["https://x/*"] }]]));
    assert!(w.side.delivered().last().unwrap()["x"].as_str().unwrap().contains("Duplicate"));
}

#[test]
fn alarms_go_off() {
    let mut w = world();
    w.bg.tell(&mut w.rt, &mut w.side, "listen", json!(["alarms.onAlarm", true]));
    w.bg.say(&mut w.rt, &mut w.side, "alarms.create", json!(["soon", { "when": 1.0 }]));
    w.bg.say(&mut w.rt, &mut w.side, "alarms.create", json!(["later", { "delayInMinutes": 60 }]));
    w.side.delivered();
    w.rt.tick(&mut w.side);
    let got = w.side.delivered();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["ev"], "alarms.onAlarm");
    assert_eq!(got[0]["a"][0]["name"], "soon");
    w.bg.say(&mut w.rt, &mut w.side, "alarms.getAll", json!([]));
    let all = w.side.delivered()[0]["v"].clone();
    assert_eq!(all.as_array().unwrap().len(), 1);
    assert_eq!(all[0]["name"], "later");
    assert!(w.rt.next_tick().is_some());
}

#[test]
fn the_toolbar_button() {
    let mut w = world();
    let tab = w.content.tab;
    let window = w.side.focused.unwrap();
    let views = w.rt.actions("default", Some(tab));
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].title, "Test");
    assert_eq!(views[0].popup.as_deref(), Some(format!("chrome-extension://{}/popup.html", w.id).as_str()));
    w.bg.say(&mut w.rt, &mut w.side, "action.setBadgeText", json!([{ "text": "3", "tabId": tab }]));
    w.bg.say(&mut w.rt, &mut w.side, "action.setBadgeBackgroundColor", json!([{ "color": "#00ff00" }]));
    assert_eq!(w.rt.actions("default", Some(tab))[0].badge, "3");
    assert_eq!(w.rt.actions("default", None)[0].badge, "", "only for that tab");
    assert_eq!(w.rt.actions("default", Some(tab))[0].badge_background, [0, 255, 0, 255]);
    w.side.delivered();
    w.bg.say(&mut w.rt, &mut w.side, "action.getBadgeBackgroundColor", json!([{}]));
    assert_eq!(w.side.delivered()[0]["v"], json!([0, 255, 0, 255]));
    // Pressed: its popup, over the window, and the tab in front is the extension's to touch.
    w.rt.action_clicked(&mut w.side, "default", window, &w.id);
    let (popup_window, popup_tab, url) = w.side.popups[0].clone();
    assert_eq!(popup_window, window);
    assert!(url.ends_with("/popup.html"));
    assert_eq!(w.rt.popup_of(popup_tab).map(|p| p.1.to_string()), Some(w.id.clone()));
    assert!(w.rt.ext("default", &w.id).unwrap().active_tabs.contains(&tab));
    // The popup's "current window" is the one it is over.
    let mut popup = Ctx { tab: popup_tab, id: "popup".into(), url: url.clone(), next: 0, ..w.bg.clone() };
    popup.say(&mut w.rt, &mut w.side, "windows.getCurrent", json!([{}]));
    assert_eq!(w.side.delivered()[0]["v"]["id"], json!(window));
    w.rt.popup_closed(&mut w.side, popup_tab);
    assert!(w.rt.popup_of(popup_tab).is_none());
    // Without a popup: onClicked, to the background.
    w.bg.say(&mut w.rt, &mut w.side, "action.setPopup", json!([{ "popup": "" }]));
    w.bg.tell(&mut w.rt, &mut w.side, "listen", json!(["action.onClicked", true]));
    w.side.delivered();
    w.rt.action_clicked(&mut w.side, "default", window, &w.id);
    let got = w.side.delivered();
    assert_eq!(got[0]["ev"], "action.onClicked");
    assert_eq!(got[0]["a"][0]["id"], json!(tab));
}

#[test]
fn a_firefox_extension_is_firefoxs() {
    let scratch = Scratch::new("ext-firefox");
    let store = scratch.store();
    let dir = folder(&scratch, "fox", &[("manifest.json", FIREFOX_V2), ("bg.js", "browser.runtime.onMessage.addListener(() => {});")]);
    let (installed, _) = install::install(&store, "default", &dir).unwrap();
    let mut rt = Runtime::new(store);
    let mut side = Side::new(&["https://anywhere.org/"]);
    rt.engine_started(&mut side, "default");
    let spec = side.sets.last().unwrap().1.extensions[0].clone();
    assert_eq!(spec.flavor, Flavor::Firefox);
    assert_eq!(spec.origin(), format!("moz-extension://{}", installed.id));
    assert_eq!(config(&spec, false)["runtimeId"], "fox@example.org");
    assert!(spec.background_html.as_deref().unwrap().contains("src=\"/bg.js\""));
    let mut bg = Ctx { tab: side.hidden[0].1, token: config(&spec, false)["token"].as_str().unwrap().into(), extension: installed.id.clone(), id: "bg".into(), frame: 0, url: format!("moz-extension://{}/_generated_background_page.html", installed.id), next: 0 };
    bg.tell(&mut rt, &mut side, "loaded", json!([]));
    // <all_urls>: every page's address is its to know, and its button is there.
    bg.say(&mut rt, &mut side, "tabs.query", json!([{}]));
    assert_eq!(side.delivered().last().unwrap()["v"][0]["url"], "https://anywhere.org/");
    assert_eq!(rt.actions("default", None)[0].title, "Fox");
    // Messages to it by its add-on id, from another context of its own.
    let mut page = Ctx { id: "page".into(), url: format!("moz-extension://{}/page.html", installed.id), next: 0, ..bg.clone() };
    page.tell(&mut rt, &mut side, "ready", json!(["page"]));
    let mut bg2 = Ctx { id: "bg".into(), next: 10, ..page.clone() };
    bg2.url = format!("moz-extension://{}/_generated_background_page.html", installed.id);
    bg2.tab = side.hidden[0].1;
    bg2.tell(&mut rt, &mut side, "listen", json!(["runtime.onMessage", true]));
    side.delivered();
    page.say(&mut rt, &mut side, "runtime.sendMessage", json!(["fox@example.org", "hello"]));
    let got = side.delivered();
    assert_eq!(got[0]["ev"], "runtime.onMessage");
    assert_eq!(got[0]["a"][1]["id"], "fox@example.org");
    assert_eq!(got[0]["a"][1]["envType"], "addon_child");
}

#[test]
fn bookmarks_are_spatimeshs_own() {
    let mut w = world();
    w.side.delivered();
    w.bg.say(&mut w.rt, &mut w.side, "bookmarks.getTree", json!([]));
    assert!(w.side.delivered()[0]["x"].as_str().unwrap().contains("'bookmarks' permission"), "optional, not granted");
    w.bg.say(&mut w.rt, &mut w.side, "permissions.request", json!([{ "permissions": ["bookmarks"] }]));
    assert_eq!(w.side.delivered().last().unwrap()["v"], true);
    w.bg.say(&mut w.rt, &mut w.side, "bookmarks.create", json!([{ "title": "Example", "url": "https://example.com/" }]));
    let made = w.side.delivered().last().unwrap()["v"].clone();
    assert_eq!(made["parentId"], "1");
    assert_eq!(made["url"], "https://example.com/");
    let all = crate::bookmarks::all(&w.rt.store, "default");
    assert_eq!(all.len(), 1, "a bookmark of SpatiMesh's, synced as the others are");
    w.bg.say(&mut w.rt, &mut w.side, "bookmarks.search", json!(["example"]));
    assert_eq!(w.side.delivered().last().unwrap()["v"].as_array().unwrap().len(), 1);
    w.bg.say(&mut w.rt, &mut w.side, "permissions.request", json!([{ "permissions": ["history"] }]));
    assert!(w.side.delivered().last().unwrap()["x"].is_string(), "not in the manifest");
}

#[test]
fn an_offscreen_document_answers_once_loaded() {
    let scratch = Scratch::new("ext-offscreen");
    let store = scratch.store();
    let dir = folder(&scratch, "off", &[("manifest.json", r#"{"manifest_version": 3, "name": "O", "version": "1", "permissions": ["offscreen"], "background": {"service_worker": "sw.js"}}"#), ("sw.js", ""), ("off.html", "")]);
    let (installed, _) = install::install(&store, "default", &dir).unwrap();
    let mut rt = Runtime::new(store);
    let mut side = Side::new(&[]);
    rt.engine_started(&mut side, "default");
    let spec = side.sets.last().unwrap().1.extensions[0].clone();
    let token: String = config(&spec, false)["token"].as_str().unwrap().into();
    let origin = spec.origin();
    let mut bg = Ctx { tab: side.hidden[0].1, token: token.clone(), extension: installed.id.clone(), id: "bg".into(), frame: 0, url: format!("{origin}/_generated_background_page.html"), next: 0 };
    bg.tell(&mut rt, &mut side, "loaded", json!([]));
    side.delivered();
    let call = bg.say(&mut rt, &mut side, "offscreen.createDocument", json!([{ "url": "off.html", "reasons": ["DOM_PARSER"], "justification": "x" }]));
    let (_, off_tab, url) = side.hidden.last().unwrap().clone();
    assert_eq!(url, format!("{origin}/off.html"));
    assert!(side.delivered().is_empty(), "not until it has loaded");
    let off = Ctx { tab: off_tab, id: "off".into(), url, next: 0, ..bg.clone() };
    off.tell(&mut rt, &mut side, "loaded", json!([]));
    assert_eq!(side.delivered(), vec![json!({ "to": "bg", "r": call, "v": null })]);
    let mut bg = Ctx { next: 5, ..off.clone() };
    bg.tab = side.hidden[0].1;
    bg.id = "bg".into();
    bg.url = format!("{origin}/_generated_background_page.html");
    bg.say(&mut rt, &mut side, "runtime.getContexts", json!([{ "contextTypes": ["OFFSCREEN_DOCUMENT"] }]));
    let contexts = side.delivered()[0]["v"].clone();
    assert_eq!(contexts.as_array().unwrap().len(), 1);
    assert_eq!(contexts[0]["documentUrl"], format!("{origin}/off.html"));
}

#[test]
fn private_windows_run_only_what_was_let_in_and_keep_nothing() {
    let scratch = Scratch::new("ext-private");
    let store = scratch.store();
    crate::profiles::list(&store);
    let dir = folder(&scratch, "p", &[("manifest.json", r#"{"manifest_version": 3, "name": "P", "version": "1", "permissions": ["storage"], "background": {"service_worker": "sw.js"}}"#), ("sw.js", "")]);
    let (installed, _) = install::install(&store, "default", &dir).unwrap();
    let mut rt = Runtime::new(store.clone());
    let mut side = Side::new(&[]);
    rt.engine_started(&mut side, PRIVATE);
    assert!(side.sets.last().unwrap().1.extensions.is_empty(), "not let into private windows");
    install::set(&store, "default", &installed.id, None, Some(true)).unwrap();
    rt.refresh(&mut side, PRIVATE, None);
    let spec = side.sets.last().unwrap().1.extensions[0].clone();
    assert_eq!(config(&spec, false)["incognito"], true);
    let mut bg = Ctx { tab: side.hidden.last().unwrap().1, token: config(&spec, false)["token"].as_str().unwrap().into(), extension: installed.id.clone(), id: "bg".into(), frame: 0, url: format!("{}/x", spec.origin()), next: 0 };
    let message = json!({ "t": bg.token, "e": bg.extension, "f": 0, "x": "bg", "u": bg.url, "c": "storage.local.set", "a": [{ "secret": 1 }], "i": 1 });
    rt.call(&mut side, PRIVATE, bg.tab, &message.to_string());
    assert!(storage::all(&store, &Session::default(), "default", &installed.id, Area::Local).is_empty(), "nothing written");
    let _ = &mut bg;
}
