//! `scripting`, and Manifest V2's `tabs.executeScript`, `insertCSS` and `removeCSS`: code and
//! style sheets put into a tab's frames when an extension asks, and the content scripts it
//! registers while it runs.

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{err, ok, world_of, Answer, BrowserSide, Caller, Request, Runtime};
use crate::engine::TabId;
use crate::extensions::set::{self, Script};

/// How long the frames below the top one have to answer once the top one has.
const OTHER_FRAMES: Duration = Duration::from_millis(300);
/// How long code may run before its caller is told it did not finish.
pub(super) const SCRIPT_WAIT: Duration = Duration::from_secs(10);

/// Which frames a script is for.
#[derive(Debug, Clone, PartialEq)]
enum Wanted {
    Top,
    All,
    Frames(Vec<i64>),
}

/// Code run in the extension's world, waiting for its frames' results.
pub(in crate::extensions::runtime) struct Pending {
    pub caller: String,
    pub call: u64,
    profile: String,
    extension: String,
    tab: TabId,
    wanted: Wanted,
    results: Vec<(i64, Value)>,
    error: Option<String>,
    top_at: Option<Instant>,
    since: Instant,
    /// Manifest V2's: a plain list of results.
    legacy: bool,
}

impl Pending {
    /// One frame's result: whether every frame wanted has answered.
    pub fn add(&mut self, from: &Caller, frame: i64, value: Value, error: Option<String>) -> bool {
        if from.profile != self.profile || from.extension != self.extension || from.tab != self.tab || self.results.iter().any(|(f, _)| *f == frame) {
            return false;
        }
        match error {
            Some(e) => {
                if frame == 0 || self.error.is_none() {
                    self.error = Some(e);
                }
            }
            None => self.results.push((frame, value)),
        }
        if frame == 0 {
            self.top_at = Some(Instant::now());
        }
        let answered = |f: i64| f == frame || self.results.iter().any(|(r, _)| *r == f);
        match &self.wanted {
            Wanted::Top => frame == 0,
            Wanted::Frames(list) => list.iter().all(|f| answered(*f)),
            Wanted::All => false,
        }
    }

    pub fn deadline(&self) -> Instant {
        match (self.top_at, &self.wanted) {
            (Some(t), Wanted::All) => t + OTHER_FRAMES,
            _ => self.since + SCRIPT_WAIT,
        }
    }

    pub fn due(&self, now: Instant) -> bool {
        now >= self.deadline()
    }

    /// Who is answered, and with what.
    pub fn answer(mut self) -> (String, u64, Result<Value, String>) {
        self.results.sort_by_key(|(f, _)| *f);
        let result = if self.results.is_empty() {
            Err(self.error.unwrap_or_else(|| "The script did not finish in the frames it was for.".to_string()))
        } else if self.legacy {
            Ok(Value::Array(self.results.into_iter().map(|(_, v)| v).collect()))
        } else {
            let caller = self.caller.clone();
            Ok(Value::Array(self.results.into_iter().map(|(f, v)| json!({ "frameId": f, "result": v, "documentId": format!("{caller}-{f}") })).collect()))
        };
        (self.caller, self.call, result)
    }
}

/// A file of the extension's, read; none if it is not there or would be outside it.
fn read(root: &Path, file: &str) -> Result<String, String> {
    set::inside(root, file.trim_start_matches('/')).and_then(|p| std::fs::read_to_string(p).ok()).ok_or_else(|| format!("Could not load file: '{file}'."))
}

/// A number standing for some text, to find a style sheet by again.
fn key_of(text: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in text.bytes() {
        hash = (hash ^ b as u64).wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// Where code goes: a tab of the caller's profile whose page the extension may touch.
fn target(rt: &Runtime, side: &dyn BrowserSide, caller: &Caller, tab: Option<TabId>) -> Result<TabId, String> {
    let ext = rt.ext(&caller.profile, &caller.extension).ok_or("extension gone")?;
    let tab = tab.ok_or("No tab given.")?;
    let t = side.tabs().tab(tab).filter(|t| t.profile == caller.profile).ok_or_else(|| format!("No tab with id: {tab}."))?;
    let url = t.url.clone();
    let foreign_extension = set::parse_url(&url).is_some() && !url.starts_with(&ext.origin());
    if foreign_extension || !ext.may_access(&url, Some(tab)) {
        return Err(format!("Cannot access contents of url \"{url}\". Extension manifest must request permission to access this host."));
    }
    Ok(tab)
}

/// Code run in the extension's world in a tab: each frame's result comes back through api.js.
#[allow(clippy::too_many_arguments)]
fn run_isolated(rt: &mut Runtime, side: &mut dyn BrowserSide, caller: &Caller, tab: TabId, wanted: Wanted, body: &str, id: u64, legacy: bool) -> Answer {
    let q = rt.question();
    let code = format!("globalThis.__spatimeshExt&&globalThis.__spatimeshExt.run({q},function(){{{body}}});");
    let all = wanted != Wanted::Top;
    rt.requests.insert(
        q,
        Request::Script(Pending {
            caller: caller.context.clone(),
            call: id,
            profile: caller.profile.clone(),
            extension: caller.extension.clone(),
            tab,
            wanted,
            results: Vec::new(),
            error: None,
            top_at: None,
            since: Instant::now(),
            legacy,
        }),
    );
    side.run_script(&caller.profile, tab, &world_of(&caller.extension), all, &code, 0, true);
    Answer::Later
}

/// What evaluates files one after another, the last one's value being the result.
fn eval_files(codes: &[(String, String)]) -> String {
    let mut body = String::from("var r;");
    for (file, code) in codes {
        let source = format!("{code}\n//# sourceURL={file}");
        body.push_str(&format!("r=(0,eval)({});", serde_json::to_string(&source).unwrap_or_default()));
    }
    body.push_str("return r;");
    body
}

fn style_code(css: &str, insert: bool) -> String {
    let key = serde_json::to_string(&key_of(css)).unwrap_or_default();
    if insert {
        let text = serde_json::to_string(css).unwrap_or_default();
        format!("(function(){{var s=document.createElement('style');s.setAttribute('data-spatimesh-css',{key});s.textContent={text};(document.head||document.documentElement).appendChild(s);}})();")
    } else {
        format!("(function(){{var s=document.querySelector('style[data-spatimesh-css=\"'+{key}+'\"]');if(s)s.remove();}})();")
    }
}

pub(super) fn call(rt: &mut Runtime, side: &mut dyn BrowserSide, caller: &Caller, call: &str, args: &[Value], id: Option<u64>) -> Answer {
    let method = &call["scripting.".len()..];
    let Some(root) = rt.ext(&caller.profile, &caller.extension).map(|e| e.installed.root.clone()) else { return err("extension gone") };
    let details = args.first().cloned().unwrap_or(Value::Null);
    match method {
        "executeScript" => {
            let Some(id) = id else { return Answer::Later };
            let t = &details["target"];
            let tab = match target(rt, side, caller, t.get("tabId").and_then(Value::as_u64)) {
                Ok(tab) => tab,
                Err(e) => return err(e),
            };
            let wanted = if t.get("allFrames").and_then(Value::as_bool) == Some(true) {
                Wanted::All
            } else {
                match t.get("frameIds").and_then(Value::as_array) {
                    Some(ids) if ids.iter().any(|f| f.as_i64() != Some(0)) => Wanted::Frames(ids.iter().filter_map(Value::as_i64).collect()),
                    _ => Wanted::Top,
                }
            };
            let main = details.get("world").and_then(Value::as_str) == Some("MAIN");
            let body = match (details.get("func").and_then(Value::as_str), details.get("files").and_then(Value::as_array)) {
                (Some(func), _) => {
                    let args = details.get("args").cloned().unwrap_or(json!([]));
                    format!("return ({func}).apply(null,{args});")
                }
                (None, Some(files)) => {
                    let mut codes = Vec::new();
                    for f in files.iter().filter_map(Value::as_str) {
                        match read(&root, f) {
                            Ok(code) => codes.push((f.to_string(), code)),
                            Err(e) => return err(e),
                        }
                    }
                    eval_files(&codes)
                }
                _ => return err("Exactly one of 'func' and 'files' must be specified."),
            };
            if main {
                // The page's own world: the engine runs it, and the top frame's result comes back
                // (other frames run it too, unanswered).
                let q = rt.question();
                rt.requests.insert(q, Request::MainWorld { caller: caller.context.clone(), call: id, legacy: false, since: Instant::now() });
                let code = format!("(function(){{{body}}})()");
                side.run_script(&caller.profile, tab, "", wanted != Wanted::Top, &code, q, false);
                return Answer::Later;
            }
            run_isolated(rt, side, caller, tab, wanted, &body, id, false)
        }
        "insertCSS" | "removeCSS" => {
            let t = &details["target"];
            let tab = match target(rt, side, caller, t.get("tabId").and_then(Value::as_u64)) {
                Ok(tab) => tab,
                Err(e) => return err(e),
            };
            let css = match (details.get("css").and_then(Value::as_str), details.get("files").and_then(Value::as_array)) {
                (Some(css), _) => css.to_string(),
                (None, Some(files)) => {
                    let mut all = String::new();
                    for f in files.iter().filter_map(Value::as_str) {
                        match read(&root, f) {
                            Ok(code) => all.push_str(&code),
                            Err(e) => return err(e),
                        }
                    }
                    all
                }
                _ => return err("Exactly one of 'css' and 'files' must be specified."),
            };
            let all = t.get("allFrames").and_then(Value::as_bool) == Some(true);
            side.run_script(&caller.profile, tab, &world_of(&caller.extension), all, &style_code(&css, method == "insertCSS"), 0, false);
            ok(Value::Null)
        }
        "registerContentScripts" => {
            let Some(list) = details.as_array().cloned() else { return err("registerContentScripts wants a list") };
            let Some(ext) = rt.ext_mut(&caller.profile, &caller.extension) else { return err("extension gone") };
            for s in &list {
                let Some(id) = s.get("id").and_then(Value::as_str) else { return err("A script's id is required.") };
                if id.starts_with('_') {
                    return err(format!("Script's ID '{id}' must not start with '_'."));
                }
                if ext.state.registered.iter().any(|r| r["id"] == id) || list.iter().filter(|o| o["id"] == id).count() > 1 {
                    return err(format!("Duplicate script ID '{id}'"));
                }
                if s.get("matches").and_then(Value::as_array).map_or(true, |m| m.is_empty()) {
                    return err(format!("Script with ID '{id}' must specify 'matches'."));
                }
            }
            ext.state.registered.extend(list);
            let (profile, id) = (caller.profile.clone(), caller.extension.clone());
            rt.save_state(&profile, &id);
            rt.send_set(side, &profile);
            ok(Value::Null)
        }
        "getRegisteredContentScripts" => {
            let Some(ext) = rt.ext(&caller.profile, &caller.extension) else { return err("extension gone") };
            let ids: Option<Vec<Value>> = details.get("ids").and_then(Value::as_array).cloned();
            ok(Value::Array(ext.state.registered.iter().filter(|r| ids.as_ref().map_or(true, |i| i.contains(&r["id"]))).cloned().collect()))
        }
        "unregisterContentScripts" => {
            let Some(ext) = rt.ext_mut(&caller.profile, &caller.extension) else { return err("extension gone") };
            match details.get("ids").and_then(Value::as_array) {
                Some(ids) => {
                    if let Some(missing) = ids.iter().find(|i| !ext.state.registered.iter().any(|r| r["id"] == **i)) {
                        return err(format!("Nonexistent script ID '{}'", missing.as_str().unwrap_or("")));
                    }
                    ext.state.registered.retain(|r| !ids.contains(&r["id"]));
                }
                None => ext.state.registered.clear(),
            }
            let (profile, id) = (caller.profile.clone(), caller.extension.clone());
            rt.save_state(&profile, &id);
            rt.send_set(side, &profile);
            ok(Value::Null)
        }
        "updateContentScripts" => {
            let Some(list) = details.as_array().cloned() else { return err("updateContentScripts wants a list") };
            let Some(ext) = rt.ext_mut(&caller.profile, &caller.extension) else { return err("extension gone") };
            for s in &list {
                let id = s.get("id").cloned().unwrap_or(Value::Null);
                let Some(existing) = ext.state.registered.iter_mut().find(|r| r["id"] == id) else {
                    return err(format!("Nonexistent script ID '{}'", id.as_str().unwrap_or("")));
                };
                if let (Some(old), Some(new)) = (existing.as_object_mut(), s.as_object()) {
                    for (k, v) in new {
                        old.insert(k.clone(), v.clone());
                    }
                }
            }
            let (profile, id) = (caller.profile.clone(), caller.extension.clone());
            rt.save_state(&profile, &id);
            rt.send_set(side, &profile);
            ok(Value::Null)
        }
        _ => err(format!("no such call: {call}")),
    }
}

/// Manifest V2's `tabs.executeScript(tabId?, details)`, `tabs.insertCSS` and `tabs.removeCSS`.
pub(super) fn legacy(rt: &mut Runtime, side: &mut dyn BrowserSide, caller: &Caller, method: &str, args: &[Value], id: Option<u64>) -> Answer {
    let Some(root) = rt.ext(&caller.profile, &caller.extension).map(|e| e.installed.root.clone()) else { return err("extension gone") };
    let (tab, details) = match args.first() {
        Some(Value::Number(n)) => (n.as_u64(), args.get(1).cloned().unwrap_or(Value::Null)),
        Some(Value::Null) => (None, args.get(1).cloned().unwrap_or(Value::Null)),
        _ => (None, args.first().cloned().unwrap_or(Value::Null)),
    };
    let tab = tab.or_else(|| side.tabs().tab(caller.tab).map(|t| t.id)).or_else(|| side.tabs().active(rt.current_window(side, caller)?));
    let tab = match target(rt, side, caller, tab) {
        Ok(t) => t,
        Err(e) => return err(e),
    };
    let all = details.get("allFrames").and_then(Value::as_bool) == Some(true);
    let frame = details.get("frameId").and_then(Value::as_i64).filter(|f| *f != 0);
    let (code, file) = (details.get("code").and_then(Value::as_str), details.get("file").and_then(Value::as_str));
    let text = match (code, file) {
        (Some(c), _) => (String::from("(code)"), c.to_string()),
        (None, Some(f)) => match read(&root, f) {
            Ok(c) => (f.to_string(), c),
            Err(e) => return err(e),
        },
        _ => return err("Exactly one of 'code' and 'file' must be specified."),
    };
    if method == "executeScript" {
        let Some(id) = id else { return Answer::Later };
        let wanted = match (all, frame) {
            (true, _) => Wanted::All,
            (false, Some(f)) => Wanted::Frames(vec![f]),
            (false, None) => Wanted::Top,
        };
        return run_isolated(rt, side, caller, tab, wanted, &eval_files(&[text]), id, true);
    }
    side.run_script(&caller.profile, tab, &world_of(&caller.extension), all || frame.is_some(), &style_code(&text.1, method == "insertCSS"), 0, false);
    ok(Value::Null)
}

/// A content script `scripting.registerContentScripts` was given, as an engine runs it.
pub(super) fn registered_script(root: &Path, r: &Value) -> Option<Script> {
    let list = |key: &str| -> Vec<String> { r.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default() };
    let files = |key: &str| -> Vec<(String, String)> { list(key).into_iter().filter_map(|f| read(root, &f).ok().map(|code| (f, code))).collect() };
    let matches = list("matches");
    if matches.is_empty() {
        return None;
    }
    Some(
        Script {
            matches,
            exclude_matches: list("excludeMatches"),
            include_globs: list("includeGlobs"),
            exclude_globs: list("excludeGlobs"),
            include: String::new(),
            globs: String::new(),
            exclude: String::new(),
            js: files("js"),
            css: files("css"),
            run_at: r.get("runAt").and_then(Value::as_str).unwrap_or("document_idle").to_string(),
            all_frames: r.get("allFrames").and_then(Value::as_bool).unwrap_or(false),
            match_about_blank: r.get("matchOriginAsFallback").and_then(Value::as_bool).unwrap_or(false),
            main_world: r.get("world").and_then(Value::as_str) == Some("MAIN"),
        }
        .with_regexes(),
    )
}
