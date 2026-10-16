//! `tabs` and `windows`: SpatiMesh's tabs and windows as Chrome's and Firefox's APIs show them.

use serde_json::{json, Value};

use super::{err, ok, resolve, Answer, BrowserSide, Caller, Ext, Runtime};
use crate::engine::TabId;
use crate::profiles::PRIVATE;
use crate::tabs::WindowKey;

/// `windows.WINDOW_ID_CURRENT`.
const CURRENT: i64 = -2;

/// A tab as `tabs.Tab`, everything in it: what an extension may not know is taken out by
/// [`redact`].
pub(super) fn tab_json(rt: &Runtime, side: &dyn BrowserSide, tab: TabId, viewer: Option<&Ext>) -> Option<Value> {
    let _ = rt;
    let tabs = side.tabs();
    let t = tabs.tab(tab)?;
    let window = tabs.window_of(tab)?;
    let w = tabs.window(window)?;
    let index = w.tabs.iter().position(|&x| x == tab)?;
    let active = w.active_tab() == Some(tab);
    let (width, height) = side.page_size(window);
    let title = if t.title.trim().is_empty() { t.url.clone() } else { t.title.clone() };
    let mut v = json!({
        "id": tab,
        "index": index,
        "windowId": window,
        "highlighted": active,
        "active": active,
        "selected": active,
        "pinned": false,
        "audible": t.playing,
        "discarded": false,
        "autoDiscardable": true,
        "frozen": false,
        "hidden": false,
        "mutedInfo": { "muted": t.muted },
        "status": if t.loading { "loading" } else { "complete" },
        "incognito": t.profile == PRIVATE,
        "cookieStoreId": if t.profile == PRIVATE { "firefox-private" } else { "firefox-default" },
        "width": width,
        "height": height,
        "groupId": -1,
        "url": t.url,
        "title": title,
    });
    if let Some(o) = t.opener {
        v["openerTabId"] = json!(o);
    }
    Some(match viewer {
        Some(ext) => redact(&v, ext, tab),
        None => v,
    })
}

/// A tab as an extension may see it: its address and title only with the `tabs` permission or
/// access to its page, as in Chrome.
pub(super) fn redact(full: &Value, ext: &Ext, tab: TabId) -> Value {
    let url = full.get("url").and_then(Value::as_str).unwrap_or("");
    if ext.may_see(url, tab) {
        return full.clone();
    }
    let mut v = full.clone();
    if let Some(o) = v.as_object_mut() {
        for key in ["url", "title", "pendingUrl", "favIconUrl"] {
            o.remove(key);
        }
    }
    v
}

/// A window as `windows.Window`, with its tabs if `populate`.
pub(super) fn window_json(rt: &Runtime, side: &dyn BrowserSide, window: WindowKey, populate: bool, viewer: Option<&Ext>) -> Option<Value> {
    let w = side.tabs().window(window)?;
    let (width, height) = side.page_size(window);
    let mut v = json!({
        "id": window,
        "focused": side.last_focused() == Some(window),
        "top": 0,
        "left": 0,
        "width": width,
        "height": height,
        "incognito": w.profile == PRIVATE,
        "type": "normal",
        "state": "normal",
        "alwaysOnTop": false,
    });
    if populate {
        v["tabs"] = Value::Array(w.tabs.iter().filter_map(|&t| tab_json(rt, side, t, viewer)).collect());
    }
    Some(v)
}

fn no_tab(tab: impl std::fmt::Display) -> Answer {
    err(format!("No tab with id: {tab}."))
}

fn no_window(window: impl std::fmt::Display) -> Answer {
    err(format!("No window with id: {window}."))
}

/// A tab of the caller's profile, by the number an extension gave.
fn own_tab(side: &dyn BrowserSide, caller: &Caller, tab: Option<&Value>) -> Option<TabId> {
    let id = tab?.as_u64()?;
    side.tabs().tab(id).filter(|t| t.profile == caller.profile).map(|t| t.id)
}

/// A window of the caller's profile, by the number an extension gave (`WINDOW_ID_CURRENT`
/// for the current one).
fn own_window(rt: &Runtime, side: &dyn BrowserSide, caller: &Caller, window: Option<&Value>) -> Option<WindowKey> {
    let id = window?.as_i64()?;
    let key = if id == CURRENT { rt.current_window(side, caller)? } else { WindowKey::try_from(id).ok()? };
    side.tabs().window(key).filter(|w| w.profile == caller.profile).map(|w| w.key)
}

/// The tab a call means when it names none: the caller's own, if it is in a tab, or the one
/// in front of the current window.
fn default_tab(rt: &Runtime, side: &dyn BrowserSide, caller: &Caller) -> Option<TabId> {
    if side.tabs().tab(caller.tab).is_some() {
        return Some(caller.tab);
    }
    side.tabs().active(rt.current_window(side, caller)?)
}

/// An optional tab id first, then the rest: `(tabId?, ...)`.
fn split_tab<'a>(rt: &Runtime, side: &dyn BrowserSide, caller: &Caller, args: &'a [Value]) -> (Result<TabId, String>, &'a [Value]) {
    match args.first() {
        Some(Value::Number(_)) => match own_tab(side, caller, args.first()) {
            Some(t) => (Ok(t), &args[1..]),
            None => (Err(format!("No tab with id: {}.", args[0])), &args[1..]),
        },
        Some(Value::Null) => (default_tab(rt, side, caller).ok_or_else(|| "No current tab.".to_string()), &args[1..]),
        _ => (default_tab(rt, side, caller).ok_or_else(|| "No current tab.".to_string()), args),
    }
}

/// An address an extension asked a tab to load: relative ones against the extension, and
/// `javascript:` refused, as Chrome refuses it.
fn address(ext: &Ext, caller: &Caller, url: &str) -> Result<String, String> {
    let base = if caller.url.starts_with(&ext.origin()) { caller.url.clone() } else { ext.origin() };
    let url = resolve(&base, url);
    if url.trim_start().to_ascii_lowercase().starts_with("javascript:") {
        return Err("JavaScript URLs are not allowed in tabs.create or tabs.update.".into());
    }
    Ok(url)
}

pub(super) fn tab_call(rt: &mut Runtime, side: &mut dyn BrowserSide, caller: &Caller, call: &str, args: &[Value], id: Option<u64>) -> Answer {
    let method = &call["tabs.".len()..];
    if matches!(method, "executeScript" | "insertCSS" | "removeCSS") {
        return super::scripting::legacy(rt, side, caller, method, args, id);
    }
    let rt: &Runtime = rt;
    let Some(ext) = rt.ext(&caller.profile, &caller.extension) else { return err("extension gone") };
    let arg = |n: usize| args.get(n).cloned().unwrap_or(Value::Null);
    match method {
        "query" => ok(query(rt, side, caller, ext, &arg(0))),
        "get" => match own_tab(side, caller, args.first()) {
            Some(t) => ok(tab_json(rt, side, t, Some(ext)).unwrap_or(Value::Null)),
            None => no_tab(arg(0)),
        },
        "getCurrent" => ok(match side.tabs().tab(caller.tab) {
            Some(_) => tab_json(rt, side, caller.tab, Some(ext)).unwrap_or(Value::Null),
            None => Value::Null,
        }),
        "getSelected" => {
            let window = own_window(rt, side, caller, args.first()).or_else(|| rt.current_window(side, caller));
            match window.and_then(|w| side.tabs().active(w)) {
                Some(t) => ok(tab_json(rt, side, t, Some(ext)).unwrap_or(Value::Null)),
                None => err("No current tab."),
            }
        }
        "getAllInWindow" => {
            let window = own_window(rt, side, caller, args.first()).or_else(|| rt.current_window(side, caller));
            match window.and_then(|w| window_json(rt, side, w, true, Some(ext))) {
                Some(w) => ok(w["tabs"].clone()),
                None => no_window(arg(0)),
            }
        }
        "create" => create(rt, side, caller, ext, &arg(0)),
        "update" => {
            let (tab, rest) = split_tab(rt, side, caller, args);
            let tab = match tab {
                Ok(t) => t,
                Err(e) => return err(e),
            };
            let props = rest.first().cloned().unwrap_or(Value::Null);
            if let Some(url) = props.get("url").and_then(Value::as_str) {
                match address(ext, caller, url) {
                    Ok(url) => side.load(tab, &url),
                    Err(e) => return err(e),
                }
            }
            if props.get("active").and_then(Value::as_bool) == Some(true) || props.get("highlighted").and_then(Value::as_bool) == Some(true) || props.get("selected").and_then(Value::as_bool) == Some(true) {
                side.activate(tab);
            }
            if let Some(muted) = props.get("muted").and_then(Value::as_bool) {
                side.mute(tab, muted);
            }
            let mut v = tab_json(rt, side, tab, Some(ext)).unwrap_or(Value::Null);
            if let (Some(url), Some(o)) = (props.get("url").and_then(Value::as_str), v.as_object_mut()) {
                if o.contains_key("url") {
                    o.insert("pendingUrl".into(), json!(resolve(&ext.origin(), url)));
                }
            }
            ok(v)
        }
        "remove" => {
            let ids: Vec<Value> = match arg(0) {
                Value::Array(a) => a,
                one => vec![one],
            };
            let mut found = Vec::new();
            for v in &ids {
                match own_tab(side, caller, Some(v)) {
                    Some(t) => found.push(t),
                    None => return no_tab(v),
                }
            }
            for t in found {
                side.close_tab(t);
            }
            ok(Value::Null)
        }
        "reload" | "goBack" | "goForward" | "discard" => {
            let (tab, _) = split_tab(rt, side, caller, args);
            let tab = match tab {
                Ok(t) => t,
                Err(e) => return err(e),
            };
            match method {
                "reload" => side.reload(tab),
                "goBack" => side.back(tab),
                "goForward" => side.forward(tab),
                _ => {}
            }
            ok(if method == "discard" { tab_json(rt, side, tab, Some(ext)).unwrap_or(Value::Null) } else { Value::Null })
        }
        "duplicate" => {
            let Some(tab) = own_tab(side, caller, args.first()) else { return no_tab(arg(0)) };
            let (Some(window), Some(t)) = (side.tabs().window_of(tab), side.tabs().tab(tab).cloned()) else { return no_tab(tab) };
            let index = side.tabs().window(window).and_then(|w| w.tabs.iter().position(|&x| x == tab)).map(|i| i + 1);
            match side.open_tab(&caller.profile, Some(window), &t.url, index, true, Some(tab)) {
                Some(new) => ok(tab_json(rt, side, new, Some(ext)).unwrap_or(Value::Null)),
                None => err("The tab could not be duplicated."),
            }
        }
        "highlight" => {
            let info = arg(0);
            let Some(window) = own_window(rt, side, caller, info.get("windowId")).or_else(|| rt.current_window(side, caller)) else { return no_window(info["windowId"].clone()) };
            let first = match &info["tabs"] {
                Value::Array(a) => a.first().and_then(Value::as_u64),
                v => v.as_u64(),
            };
            let tab = first.and_then(|i| side.tabs().window(window)?.tabs.get(i as usize).copied());
            match tab {
                Some(t) => side.activate(t),
                None => return err("No tab at that index."),
            }
            ok(window_json(rt, side, window, true, Some(ext)).unwrap_or(Value::Null))
        }
        "move" => {
            let props = arg(1);
            let many = arg(0).is_array();
            let ids: Vec<Value> = match arg(0) {
                Value::Array(a) => a,
                one => vec![one],
            };
            let mut moved = Vec::new();
            for (n, v) in ids.iter().enumerate() {
                let Some(tab) = own_tab(side, caller, Some(v)) else { return no_tab(v) };
                let window = match props.get("windowId") {
                    Some(w) => match own_window(rt, side, caller, Some(w)) {
                        Some(w) => w,
                        None => return no_window(w),
                    },
                    None => match side.tabs().window_of(tab) {
                        Some(w) => w,
                        None => return no_tab(tab),
                    },
                };
                let index = props.get("index").and_then(Value::as_i64).filter(|i| *i >= 0).map(|i| i as usize + n);
                side.move_tab(tab, window, index);
                moved.push(tab_json(rt, side, tab, Some(ext)).unwrap_or(Value::Null));
            }
            ok(if many { Value::Array(moved) } else { moved.pop().unwrap_or(Value::Null) })
        }
        "captureVisibleTab" => {
            let window = match args.first() {
                Some(Value::Number(_)) => own_window(rt, side, caller, args.first()),
                _ => rt.current_window(side, caller),
            };
            let Some(window) = window else { return no_window(arg(0)) };
            let tab = side.tabs().active(window);
            let url = tab.and_then(|t| side.tabs().tab(t)).map(|t| t.url.clone()).unwrap_or_default();
            if !ext.may_access(&url, tab) {
                return err("Either the '<all_urls>' or 'activeTab' permission is required.");
            }
            match side.capture(window) {
                Some((w, h, rgba)) => {
                    use base64::Engine as _;
                    ok(json!(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png(w, h, &rgba)))))
                }
                None => err("Failed to capture tab: nothing is shown there."),
            }
        }
        "detectLanguage" => ok(json!("und")),
        "getZoom" => ok(json!(side.zoom())),
        "setZoom" => {
            let factor = args.iter().find_map(Value::as_f64).unwrap_or(0.0);
            if factor > 0.0 {
                side.set_zoom(factor as f32);
            }
            ok(Value::Null)
        }
        "getZoomSettings" => ok(json!({ "mode": "automatic", "scope": "per-origin", "defaultZoomFactor": 1.0 })),
        "setZoomSettings" => ok(Value::Null),
        "group" | "ungroup" => err("Tab groups are not supported in SpatiMesh."),
        _ => err(format!("no such call: {call}")),
    }
}

fn query(rt: &Runtime, side: &dyn BrowserSide, caller: &Caller, ext: &Ext, info: &Value) -> Value {
    let current = rt.current_window(side, caller);
    let last = side.last_focused();
    let flag = |key: &str| info.get(key).and_then(Value::as_bool);
    let urls: Option<super::super::pattern::Patterns> = match info.get("url") {
        Some(Value::String(s)) => Some(super::super::pattern::Patterns::new(std::slice::from_ref(s))),
        Some(Value::Array(a)) => Some(super::super::pattern::Patterns::new(&a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect::<Vec<_>>())),
        _ => None,
    };
    let mut out = Vec::new();
    for w in side.tabs().windows() {
        if w.profile != caller.profile {
            continue;
        }
        if let Some(id) = info.get("windowId").and_then(Value::as_i64) {
            let wanted = if id == CURRENT { current.map(i64::from) } else { Some(id) };
            if wanted != Some(i64::from(w.key)) {
                continue;
            }
        }
        if let Some(c) = flag("currentWindow") {
            if c != (Some(w.key) == current) {
                continue;
            }
        }
        if let Some(l) = flag("lastFocusedWindow") {
            if l != (Some(w.key) == last) {
                continue;
            }
        }
        if info.get("windowType").and_then(Value::as_str).is_some_and(|t| t != "normal") {
            continue;
        }
        for &tab in &w.tabs {
            let Some(t) = tab_json(rt, side, tab, Some(ext)) else { continue };
            let field = |key: &str| t.get(key).and_then(Value::as_bool);
            let same = |key: &str, have: Option<bool>| flag(key).map_or(true, |want| Some(want) == have);
            let keep = same("active", field("active"))
                && same("highlighted", field("highlighted"))
                && same("pinned", Some(false))
                && same("audible", field("audible"))
                && same("discarded", Some(false))
                && same("autoDiscardable", Some(true))
                && same("frozen", Some(false))
                && same("muted", t["mutedInfo"]["muted"].as_bool())
                && info.get("status").and_then(Value::as_str).map_or(true, |s| t["status"] == s)
                && info.get("index").and_then(Value::as_u64).map_or(true, |i| t["index"].as_u64() == Some(i))
                && info.get("groupId").and_then(Value::as_i64).map_or(true, |g| g == -1)
                && info.get("title").and_then(Value::as_str).map_or(true, |p| t.get("title").and_then(Value::as_str).is_some_and(|title| super::super::pattern::wildcard(p, title, false)))
                && urls.as_ref().map_or(true, |u| t.get("url").and_then(Value::as_str).is_some_and(|url| u.matches(url)));
            if keep {
                out.push(t);
            }
        }
    }
    Value::Array(out)
}

fn create(rt: &Runtime, side: &mut dyn BrowserSide, caller: &Caller, ext: &Ext, props: &Value) -> Answer {
    let url = match props.get("url").and_then(Value::as_str) {
        // None: the browser's own new tab page.
        None => String::new(),
        Some(u) => match address(ext, caller, u) {
            Ok(u) => u,
            Err(e) => return err(e),
        },
    };
    let window = match props.get("windowId") {
        Some(w) => match own_window(rt, side, caller, Some(w)) {
            Some(w) => Some(w),
            None => return no_window(w),
        },
        None => rt.current_window(side, caller),
    };
    let index = props.get("index").and_then(Value::as_u64).map(|i| i as usize);
    let active = props.get("active").or_else(|| props.get("selected")).and_then(Value::as_bool).unwrap_or(true);
    let opener = own_tab(side, caller, props.get("openerTabId"));
    match side.open_tab(&caller.profile, window, &url, index, active, opener) {
        Some(tab) => {
            let mut v = tab_json(rt, side, tab, Some(ext)).unwrap_or(Value::Null);
            if !url.is_empty() {
                if let Some(o) = v.as_object_mut().filter(|o| o.contains_key("url")) {
                    o.insert("pendingUrl".into(), json!(url));
                }
            }
            ok(v)
        }
        None => err("The tab could not be created."),
    }
}

pub(super) fn window_call(rt: &mut Runtime, side: &mut dyn BrowserSide, caller: &Caller, call: &str, args: &[Value]) -> Answer {
    let rt: &Runtime = rt;
    let Some(ext) = rt.ext(&caller.profile, &caller.extension) else { return err("extension gone") };
    let method = &call["windows.".len()..];
    let arg = |n: usize| args.get(n).cloned().unwrap_or(Value::Null);
    let populate = |v: &Value| v.get("populate").and_then(Value::as_bool).unwrap_or(false);
    match method {
        "get" => match own_window(rt, side, caller, args.first()) {
            Some(w) => ok(window_json(rt, side, w, populate(&arg(1)), Some(ext)).unwrap_or(Value::Null)),
            None => no_window(arg(0)),
        },
        "getCurrent" | "getLastFocused" => {
            let window = if method == "getCurrent" { rt.current_window(side, caller) } else { side.last_focused() };
            let window = window.filter(|w| side.tabs().window(*w).is_some_and(|x| x.profile == caller.profile)).or_else(|| side.tabs().windows().iter().find(|w| w.profile == caller.profile).map(|w| w.key));
            match window {
                Some(w) => ok(window_json(rt, side, w, populate(&arg(0)), Some(ext)).unwrap_or(Value::Null)),
                None => err("No current window."),
            }
        }
        "getAll" => {
            let all: Vec<Value> = side.tabs().windows().iter().filter(|w| w.profile == caller.profile).filter_map(|w| window_json(rt, side, w.key, populate(&arg(0)), Some(ext))).collect();
            ok(Value::Array(all))
        }
        "create" => {
            let props = arg(0);
            let incognito = props.get("incognito").and_then(Value::as_bool).unwrap_or(false);
            let profile = if incognito {
                if !ext.installed.in_private && caller.profile != PRIVATE {
                    return err("Incognito mode is not allowed for this extension.");
                }
                PRIVATE.to_string()
            } else {
                caller.profile.clone()
            };
            let mut urls = Vec::new();
            match props.get("url") {
                Some(Value::String(u)) => urls.push(u.clone()),
                Some(Value::Array(a)) => urls.extend(a.iter().filter_map(|v| v.as_str().map(str::to_string))),
                _ => {}
            }
            let mut resolved = Vec::new();
            for u in urls {
                match address(ext, caller, &u) {
                    Ok(u) => resolved.push(u),
                    Err(e) => return err(e),
                }
            }
            let tab = match props.get("tabId") {
                Some(t) => match own_tab(side, caller, Some(t)) {
                    Some(t) => Some(t),
                    None => return no_tab(t),
                },
                None => None,
            };
            let focused = props.get("focused").and_then(Value::as_bool).unwrap_or(true);
            match side.open_window(&profile, &resolved, tab, focused) {
                Some(w) => ok(window_json(rt, side, w, true, Some(ext)).unwrap_or(Value::Null)),
                None => err("The window could not be created."),
            }
        }
        "update" => {
            let Some(window) = own_window(rt, side, caller, args.first()) else { return no_window(arg(0)) };
            let props = arg(1);
            if props.get("focused").and_then(Value::as_bool) == Some(true) || props.get("drawAttention").and_then(Value::as_bool) == Some(true) {
                side.focus_window(window);
            }
            ok(window_json(rt, side, window, false, Some(ext)).unwrap_or(Value::Null))
        }
        "remove" => match own_window(rt, side, caller, args.first()) {
            Some(w) => {
                side.close_window(w);
                ok(Value::Null)
            }
            None => no_window(arg(0)),
        },
        _ => err(format!("no such call: {call}")),
    }
}

/// A picture as a PNG file: eight bits a channel, red, green, blue and alpha, rows unfiltered.
pub(super) fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(data);
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&crc.sum().to_be_bytes());
    }
    let row = width as usize * 4;
    let mut raw = Vec::with_capacity((row + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0);
        let start = y * row;
        raw.extend_from_slice(rgba.get(start..start + row).unwrap_or(&[]));
        raw.resize((y + 1) * (row + 1), 0);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    let _ = z.write_all(&raw);
    let idat = z.finish().unwrap_or_default();
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &idat);
    chunk(&mut out, b"IEND", &[]);
    out
}
