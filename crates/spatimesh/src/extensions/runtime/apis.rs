//! The rest of the APIs: alarms, notifications, permissions, management, context menus,
//! commands, idle, offscreen documents, bookmarks (SpatiMesh's own), declarativeNetRequest's
//! rules (kept here; engines apply them), and what SpatiMesh does not have yet, said so.

use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use super::{err, ok, Answer, BrowserSide, Caller, Ext, Kind, Runtime};
use crate::bookmarks::{self, At, Bookmark};
use crate::extensions::install;
use crate::extensions::pattern::{Pattern, Patterns};
use crate::profiles::PRIVATE;
use crate::store::now_ms;

/// An alarm an extension set.
#[derive(Debug, Clone)]
pub(super) struct Alarm {
    pub profile: String,
    pub extension: String,
    name: String,
    /// Milliseconds since 1970.
    scheduled: f64,
    /// Minutes.
    period: Option<f64>,
}

impl Alarm {
    fn json(&self) -> Value {
        let mut v = json!({ "name": self.name, "scheduledTime": self.scheduled });
        if let Some(p) = self.period {
            v["periodInMinutes"] = json!(p);
        }
        v
    }
}

/// When the next alarm goes off.
pub(super) fn next_alarm(rt: &Runtime) -> Option<Instant> {
    let now = now_ms() as f64;
    rt.alarms.iter().map(|a| a.scheduled).reduce(f64::min).map(|t| Instant::now() + Duration::from_millis((t - now).max(0.0) as u64))
}

/// Alarms whose time has come: `alarms.onAlarm`, and set again if they repeat.
pub(super) fn fire_alarms(rt: &mut Runtime, side: &mut dyn BrowserSide) {
    let now = now_ms() as f64;
    let mut due = Vec::new();
    rt.alarms.retain_mut(|a| {
        if a.scheduled > now {
            return true;
        }
        due.push(a.clone());
        match a.period {
            Some(p) => {
                a.scheduled = (a.scheduled + p * 60_000.0).max(now + p * 60_000.0 * 0.5);
                true
            }
            None => false,
        }
    });
    for a in due {
        let info = a.json();
        rt.emit(side, &a.profile, Some(&a.extension), "alarms.onAlarm", &|_| Some(vec![info.clone()]), &|c| c.kind != Kind::Content);
    }
}

/// Whether a `webNavigation` listener's filter (`{url: [UrlFilter]}`) takes an address.
pub(super) fn url_filter_takes(filter: &Value, url: &str) -> bool {
    let Some(filters) = filter.get("url").and_then(Value::as_array) else { return true };
    let Ok(u) = url::Url::parse(url) else { return false };
    let host = u.host_str().unwrap_or("");
    let path = u.path();
    let query = u.query().unwrap_or("");
    let text = |f: &Value, key: &str| f.get(key).and_then(Value::as_str).map(str::to_string);
    filters.iter().any(|f| {
        let mut ok = true;
        let mut check = |key: &str, test: &dyn Fn(&str) -> bool| {
            if let Some(v) = text(f, key) {
                ok &= test(&v);
            }
        };
        // A dot before the host, as Chrome has it: ".foo" is any part of it starting so.
        let dotted = format!(".{host}");
        check("hostContains", &|v| dotted.contains(v));
        check("hostEquals", &|v| host == v);
        check("hostPrefix", &|v| host.starts_with(v));
        check("hostSuffix", &|v| host.ends_with(v));
        check("pathContains", &|v| path.contains(v));
        check("pathEquals", &|v| path == v);
        check("pathPrefix", &|v| path.starts_with(v));
        check("pathSuffix", &|v| path.ends_with(v));
        check("queryContains", &|v| query.contains(v));
        check("queryEquals", &|v| query == v);
        check("queryPrefix", &|v| query.starts_with(v));
        check("querySuffix", &|v| query.ends_with(v));
        check("urlContains", &|v| url.contains(v));
        check("urlEquals", &|v| url == v);
        check("urlPrefix", &|v| url.starts_with(v));
        check("urlSuffix", &|v| url.ends_with(v));
        if let Some(schemes) = f.get("schemes").and_then(Value::as_array) {
            ok &= schemes.iter().any(|s| s.as_str() == Some(u.scheme()));
        }
        if let Some(ports) = f.get("ports").and_then(Value::as_array) {
            let port = u.port_or_known_default().map(i64::from);
            ok &= ports.iter().any(|p| match p {
                Value::Array(range) => {
                    let (lo, hi) = (range.first().and_then(Value::as_i64), range.get(1).and_then(Value::as_i64));
                    matches!((port, lo, hi), (Some(x), Some(l), Some(h)) if x >= l && x <= h)
                }
                v => v.as_i64() == port,
            });
        }
        ok
    })
}

/// `management.ExtensionInfo`.
fn info(ext: &Ext) -> Value {
    let icons: Vec<Value> = ext.manifest.icons.iter().map(|(size, path)| json!({ "size": size, "url": ext.url(path) })).collect();
    let mut permissions: Vec<String> = ext.manifest.permissions.iter().filter(|p| !crate::extensions::manifest::is_host_permission(p)).cloned().collect();
    permissions.extend(ext.state.permissions.iter().cloned());
    json!({
        "id": ext.installed.id,
        "name": ext.manifest.name,
        "shortName": ext.manifest.short_name.clone().unwrap_or_else(|| ext.manifest.name.clone()),
        "description": ext.manifest.description,
        "version": ext.manifest.version,
        "mayDisable": true,
        "mayEnable": true,
        "enabled": ext.installed.enabled,
        "isApp": false,
        "type": "extension",
        "installType": "development",
        "offlineEnabled": false,
        "optionsUrl": ext.manifest.options_page.as_ref().map(|p| ext.url(p)).unwrap_or_default(),
        "homepageUrl": ext.manifest.raw.get("homepage_url").and_then(Value::as_str).unwrap_or(""),
        "permissions": permissions,
        "hostPermissions": ext.hosts(),
        "icons": icons,
    })
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

pub(super) fn call(rt: &mut Runtime, side: &mut dyn BrowserSide, caller: &Caller, call: &str, args: &[Value], id: Option<u64>) -> Answer {
    let (namespace, method) = call.split_once('.').unwrap_or((call, ""));
    let Some(ext) = rt.ext(&caller.profile, &caller.extension) else { return err("extension gone") };
    let needs = match namespace {
        "alarms" | "notifications" | "idle" | "offscreen" | "bookmarks" | "history" | "cookies" | "downloads" | "webNavigation" | "webRequest" | "declarativeNetRequest" => Some(namespace),
        "contextMenus" | "menus" => Some("contextMenus"),
        "management" if !matches!(method, "getSelf" | "uninstallSelf" | "getPermissionWarningsByManifest") => Some("management"),
        _ => None,
    };
    if let Some(permission) = needs {
        if !ext.has_api(permission) {
            return err(format!("The '{permission}' permission is required for {call}."));
        }
    }
    let arg = |n: usize| args.get(n).cloned().unwrap_or(Value::Null);
    let profile = caller.profile.clone();
    let extension = caller.extension.clone();
    match namespace {
        "alarms" => alarms(rt, caller, method, args),
        "notifications" => {
            let Some(ext) = rt.ext_mut(&profile, &extension) else { return err("extension gone") };
            match method {
                "create" => {
                    let given = arg(0).as_str().map(str::to_string).filter(|s| !s.is_empty());
                    let name = given.unwrap_or_else(crate::store::new_id);
                    let options = arg(1);
                    ext.notifications.insert(name.clone());
                    let title = options.get("title").and_then(Value::as_str).unwrap_or(&ext.manifest.name).to_string();
                    let message = options.get("message").and_then(Value::as_str).unwrap_or("").to_string();
                    side.notify(&title, &message);
                    ok(json!(name))
                }
                "update" => ok(json!(ext.notifications.contains(arg(0).as_str().unwrap_or("")))),
                "clear" => {
                    let name = arg(0).as_str().unwrap_or("").to_string();
                    let was = ext.notifications.remove(&name);
                    if was {
                        rt.emit(side, &profile, Some(&extension), "notifications.onClosed", &|_| Some(vec![json!(name), json!(false)]), &|c| c.kind != Kind::Content);
                    }
                    ok(json!(was))
                }
                "getAll" => ok(Value::Object(ext.notifications.iter().map(|n| (n.clone(), json!(true))).collect())),
                "getPermissionLevel" => ok(json!("granted")),
                _ => err(format!("no such call: {call}")),
            }
        }
        "permissions" => permissions(rt, side, caller, method, &arg(0)),
        "management" => match method {
            "getSelf" => ok(info(ext)),
            "getAll" => ok(Value::Array(rt.profiles.get(&profile).map(|l| l.exts.iter().map(info).collect()).unwrap_or_default())),
            "get" => match rt.find(&profile, arg(0).as_str().unwrap_or("")).and_then(|id| rt.ext(&profile, &id)) {
                Some(other) => ok(info(other)),
                None => err(format!("Failed to find extension with id {}.", arg(0))),
            },
            "getPermissionWarningsById" | "getPermissionWarningsByManifest" => ok(json!([])),
            "setEnabled" | "uninstall" | "uninstallSelf" => {
                if profile == PRIVATE {
                    return err("Extensions cannot be changed from a private window.");
                }
                let target = if method == "uninstallSelf" { Some(extension.clone()) } else { rt.find(&profile, arg(0).as_str().unwrap_or("")) };
                let Some(target) = target else { return err(format!("Failed to find extension with id {}.", arg(0))) };
                let home = rt.ext(&profile, &target).map(|e| e.home.clone()).unwrap_or(profile.clone());
                let done = if method == "setEnabled" {
                    install::set(&rt.store, &home, &target, Some(arg(1).as_bool().unwrap_or(true)), None)
                } else {
                    install::uninstall(&rt.store, &home, &target)
                };
                if let Err(e) = done {
                    return err(e);
                }
                rt.refresh(side, &profile, &[]);
                if method == "uninstallSelf" {
                    Answer::Later
                } else {
                    ok(Value::Null)
                }
            }
            _ => err(format!("{call} is not supported in SpatiMesh")),
        },
        "contextMenus" | "menus" => menus(rt, caller, method, args),
        "commands" => match method {
            "getAll" => ok(Value::Array(ext.manifest.commands.iter().map(|c| json!({ "name": c.name, "description": c.description.clone().unwrap_or_default(), "shortcut": c.key.clone().unwrap_or_default() })).collect())),
            _ => err(format!("no such call: {call}")),
        },
        "idle" => match method {
            "queryState" => ok(json!("active")),
            "getAutoLockDelay" => ok(json!(0)),
            _ => ok(Value::Null),
        },
        "offscreen" => {
            let Some(ext) = rt.ext(&profile, &extension) else { return err("extension gone") };
            match method {
                "createDocument" => {
                    let Some(id) = id else { return Answer::Later };
                    if ext.offscreen.is_some() {
                        return err("Only a single offscreen document may be created.");
                    }
                    let Some(path) = arg(0).get("url").and_then(Value::as_str).map(str::to_string) else { return err("An offscreen document needs a url.") };
                    let url = super::resolve(&ext.origin(), &path);
                    if !url.starts_with(&ext.origin()) {
                        return err("An offscreen document must be the extension's own page.");
                    }
                    let tab = rt.hidden_tab();
                    if let Some(ext) = rt.ext_mut(&profile, &extension) {
                        ext.offscreen = Some(tab);
                        ext.offscreen_waiting.push((caller.context.clone(), id));
                    }
                    side.open_hidden(&profile, tab, &url);
                    Answer::Later
                }
                "closeDocument" => {
                    let Some(tab) = ext.offscreen else { return err("No current offscreen document.") };
                    side.close_hidden(&profile, tab);
                    rt.hidden_gone(side, &profile, tab);
                    ok(Value::Null)
                }
                "hasDocument" => ok(json!(ext.offscreen.is_some())),
                _ => err(format!("no such call: {call}")),
            }
        }
        "bookmarks" => bookmarks_call(rt, side, caller, method, args),
        "history" => match method {
            // SpatiMesh keeps no history yet.
            "search" | "getVisits" => ok(json!([])),
            _ => ok(Value::Null),
        },
        "cookies" => match method {
            "getAllCookieStores" => {
                let tabs: Vec<Value> = side.tabs().tabs().filter(|t| t.profile == profile).map(|t| json!(t.id)).collect();
                ok(json!([{ "id": if profile == PRIVATE { "1" } else { "0" }, "tabIds": tabs }]))
            }
            _ => err("cookies are not reachable yet in SpatiMesh"),
        },
        "declarativeNetRequest" => dnr(rt, caller, method, &arg(0)),
        "webNavigation" => {
            let Some(tab) = arg(0).get("tabId").and_then(Value::as_u64) else { return err("No tab given.") };
            let Some(t) = side.tabs().tab(tab).filter(|t| t.profile == profile) else { return err(format!("No tab with id: {tab}.")) };
            let frame = json!({ "errorOccurred": false, "url": t.url, "parentFrameId": -1, "frameId": 0, "processId": -1, "documentLifecycle": "active", "frameType": "outermost_frame" });
            match method {
                "getFrame" if arg(0).get("frameId").and_then(Value::as_i64).unwrap_or(0) == 0 => ok(frame),
                "getFrame" => ok(Value::Null),
                "getAllFrames" => ok(json!([frame])),
                _ => err(format!("no such call: {call}")),
            }
        }
        "webRequest" => ok(Value::Null),
        "downloads" => err("downloads are not supported yet in SpatiMesh"),
        _ => err(format!("{call} is not supported in SpatiMesh")),
    }
}

fn alarms(rt: &mut Runtime, caller: &Caller, method: &str, args: &[Value]) -> Answer {
    let name_arg = |n: usize| args.get(n).and_then(Value::as_str).unwrap_or("").to_string();
    let mine = |a: &super::apis::Alarm| a.profile == caller.profile && a.extension == caller.extension;
    match method {
        "create" => {
            let name = name_arg(0);
            let info = args.get(1).cloned().unwrap_or(Value::Null);
            let now = now_ms() as f64;
            let period = info.get("periodInMinutes").and_then(Value::as_f64);
            let scheduled = match (info.get("when").and_then(Value::as_f64), info.get("delayInMinutes").and_then(Value::as_f64), period) {
                (Some(when), _, _) => when,
                (None, Some(delay), _) => now + delay * 60_000.0,
                (None, None, Some(p)) => now + p * 60_000.0,
                _ => return err("An alarm needs when, delayInMinutes or periodInMinutes."),
            };
            rt.alarms.retain(|a| !(mine(a) && a.name == name));
            rt.alarms.push(Alarm { profile: caller.profile.clone(), extension: caller.extension.clone(), name, scheduled, period });
            ok(Value::Null)
        }
        "get" => {
            let name = name_arg(0);
            ok(rt.alarms.iter().find(|a| mine(a) && a.name == name).map_or(Value::Null, Alarm::json))
        }
        "getAll" => ok(Value::Array(rt.alarms.iter().filter(|a| mine(a)).map(Alarm::json).collect())),
        "clear" => {
            let name = name_arg(0);
            let before = rt.alarms.len();
            rt.alarms.retain(|a| !(mine(a) && a.name == name));
            ok(json!(rt.alarms.len() != before))
        }
        "clearAll" => {
            let before = rt.alarms.len();
            rt.alarms.retain(|a| !mine(a));
            ok(json!(rt.alarms.len() != before))
        }
        _ => err(format!("no such call: alarms.{method}")),
    }
}

fn permissions(rt: &mut Runtime, side: &mut dyn BrowserSide, caller: &Caller, method: &str, asked: &Value) -> Answer {
    let Some(ext) = rt.ext(&caller.profile, &caller.extension) else { return err("extension gone") };
    let perms = strings(asked.get("permissions"));
    let origins = strings(asked.get("origins"));
    let hosts = Patterns::new(&ext.hosts());
    let covered = |o: &String| Pattern::parse(o).is_ok_and(|p| hosts.cover(&p));
    match method {
        "contains" => ok(json!(perms.iter().all(|p| ext.has(p)) && origins.iter().all(covered))),
        "getAll" => {
            let mut api: Vec<String> = ext.manifest.permissions.iter().filter(|p| !crate::extensions::manifest::is_host_permission(p)).cloned().collect();
            api.extend(ext.state.permissions.iter().cloned());
            ok(json!({ "permissions": api, "origins": ext.hosts() }))
        }
        "request" => {
            let optional = &ext.manifest.optional_permissions;
            let mut optional_hosts: Vec<String> = ext.manifest.optional_host_permissions.clone();
            optional_hosts.extend(optional.iter().filter(|p| crate::extensions::manifest::is_host_permission(p)).cloned());
            let optional_hosts = Patterns::new(&optional_hosts);
            if let Some(p) = perms.iter().find(|p| !ext.has(p) && !optional.contains(p)) {
                return err(format!("Only permissions specified in the manifest may be requested: {p}"));
            }
            if let Some(o) = origins.iter().find(|o| !covered(o) && !Pattern::parse(o).is_ok_and(|p| optional_hosts.cover(&p))) {
                return err(format!("Only permissions specified in the manifest may be requested: {o}"));
            }
            let new_perms: Vec<String> = perms.iter().filter(|p| !ext.has(p)).cloned().collect();
            let new_origins: Vec<String> = origins.iter().filter(|o| !covered(o)).cloned().collect();
            if new_perms.is_empty() && new_origins.is_empty() {
                return ok(json!(true));
            }
            if let Some(ext) = rt.ext_mut(&caller.profile, &caller.extension) {
                ext.state.permissions.extend(new_perms.iter().cloned());
                ext.state.origins.extend(new_origins.iter().cloned());
            }
            changed(rt, side, caller, "permissions.onAdded", new_perms, new_origins);
            ok(json!(true))
        }
        "remove" => {
            if let Some(p) = perms.iter().find(|p| ext.manifest.has_permission(p)) {
                return err(format!("You cannot remove required permissions: {p}"));
            }
            let gone_perms: Vec<String> = perms.iter().filter(|p| ext.state.permissions.contains(p)).cloned().collect();
            let gone_origins: Vec<String> = origins.iter().filter(|o| ext.state.origins.contains(o)).cloned().collect();
            if let Some(ext) = rt.ext_mut(&caller.profile, &caller.extension) {
                ext.state.permissions.retain(|p| !gone_perms.contains(p));
                ext.state.origins.retain(|o| !gone_origins.contains(o));
            }
            let removed = !gone_perms.is_empty() || !gone_origins.is_empty();
            if removed {
                changed(rt, side, caller, "permissions.onRemoved", gone_perms, gone_origins);
            }
            ok(json!(removed))
        }
        "addHostAccessRequest" | "removeHostAccessRequest" => ok(Value::Null),
        _ => err(format!("no such call: permissions.{method}")),
    }
}

/// Permissions granted or taken back: kept, given to the engine (new contexts get the APIs),
/// and told.
fn changed(rt: &mut Runtime, side: &mut dyn BrowserSide, caller: &Caller, event: &str, perms: Vec<String>, origins: Vec<String>) {
    rt.save_state(&caller.profile, &caller.extension);
    rt.send_set(side, &caller.profile);
    let details = json!({ "permissions": perms, "origins": origins });
    rt.emit(side, &caller.profile, Some(&caller.extension), event, &|_| Some(vec![details.clone()]), &|c| c.kind != Kind::Content);
}

fn menus(rt: &mut Runtime, caller: &Caller, method: &str, args: &[Value]) -> Answer {
    let Some(ext) = rt.ext_mut(&caller.profile, &caller.extension) else { return err("extension gone") };
    let id = args.first().cloned().unwrap_or(Value::Null);
    let same = |m: &Map<String, Value>, id: &Value| m.get("id") == Some(id);
    match method {
        "create" => {
            let Some(props) = id.as_object().cloned() else { return err("contextMenus.create wants its properties") };
            let item_id = props.get("id").cloned().unwrap_or(Value::Null);
            if ext.menus.iter().any(|m| same(m, &item_id)) {
                return err(format!("Cannot create item with duplicate id {}", item_id.as_str().map_or(item_id.to_string(), str::to_string)));
            }
            if let Some(parent) = props.get("parentId") {
                if !ext.menus.iter().any(|m| same(m, parent)) {
                    return err(format!("Cannot find menu item with id {parent}"));
                }
            }
            ext.menus.push(props);
            ok(item_id)
        }
        "update" => {
            let props = args.get(1).and_then(Value::as_object).cloned().unwrap_or_default();
            let Some(item) = ext.menus.iter_mut().find(|m| same(m, &id)) else { return err(format!("Cannot find menu item with id {id}")) };
            for (k, v) in props {
                if k != "id" {
                    item.insert(k, v);
                }
            }
            ok(Value::Null)
        }
        "remove" => {
            if !ext.menus.iter().any(|m| same(m, &id)) {
                return err(format!("Cannot find menu item with id {id}"));
            }
            // It, and everything under it.
            let mut gone = vec![id];
            let mut i = 0;
            while i < gone.len() {
                let parent = gone[i].clone();
                gone.extend(ext.menus.iter().filter(|m| m.get("parentId") == Some(&parent)).filter_map(|m| m.get("id").cloned()));
                i += 1;
            }
            ext.menus.retain(|m| !gone.iter().any(|g| same(m, g)));
            ok(Value::Null)
        }
        "removeAll" => {
            ext.menus.clear();
            ok(Value::Null)
        }
        _ => ok(Value::Null),
    }
}

fn dnr(rt: &mut Runtime, caller: &Caller, method: &str, options: &Value) -> Answer {
    let Some(ext) = rt.ext_mut(&caller.profile, &caller.extension) else { return err("extension gone") };
    let ids = |key: &str| -> Vec<i64> { options.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default() };
    let update = |rules: &mut Vec<Value>| -> Result<(), String> {
        let remove = ids("removeRuleIds");
        rules.retain(|r| !r.get("id").and_then(Value::as_i64).is_some_and(|i| remove.contains(&i)));
        for rule in options.get("addRules").and_then(Value::as_array).cloned().unwrap_or_default() {
            let id = rule.get("id").and_then(Value::as_i64).ok_or("A rule needs an id.")?;
            if rules.iter().any(|r| r.get("id").and_then(Value::as_i64) == Some(id)) {
                return Err(format!("Rule with id {id} does not have a unique ID."));
            }
            rules.push(rule);
        }
        Ok(())
    };
    let pick = |rules: &[Value]| -> Value {
        let wanted = ids("ruleIds");
        Value::Array(rules.iter().filter(|r| wanted.is_empty() || r.get("id").and_then(Value::as_i64).is_some_and(|i| wanted.contains(&i))).cloned().collect())
    };
    let answer = match method {
        "updateDynamicRules" => match update(&mut ext.state.dynamic_rules) {
            Ok(()) => ok(Value::Null),
            Err(e) => return err(e),
        },
        "getDynamicRules" => return ok(pick(&ext.state.dynamic_rules)),
        "updateSessionRules" => {
            return match update(&mut ext.session_rules) {
                Ok(()) => ok(Value::Null),
                Err(e) => err(e),
            }
        }
        "getSessionRules" => return ok(pick(&ext.session_rules)),
        "updateEnabledRulesets" => {
            let mut enabled = ext.state.enabled_rulesets.clone().unwrap_or_else(|| ext.manifest.rulesets.iter().filter(|r| r.enabled).map(|r| r.id.clone()).collect());
            let disable = strings(options.get("disableRulesetIds"));
            let enable = strings(options.get("enableRulesetIds"));
            if let Some(unknown) = enable.iter().find(|e| !ext.manifest.rulesets.iter().any(|r| r.id == **e)) {
                return err(format!("Invalid ruleset id: {unknown}."));
            }
            enabled.retain(|e| !disable.contains(e));
            for e in enable {
                if !enabled.contains(&e) {
                    enabled.push(e);
                }
            }
            ext.state.enabled_rulesets = Some(enabled);
            ok(Value::Null)
        }
        "getEnabledRulesets" => {
            return ok(json!(ext.state.enabled_rulesets.clone().unwrap_or_else(|| ext.manifest.rulesets.iter().filter(|r| r.enabled).map(|r| r.id.clone()).collect())));
        }
        "updateStaticRules" => {
            let set = options.get("rulesetId").and_then(Value::as_str).unwrap_or("").to_string();
            let entry = ext.state.disabled_rules.entry(set).or_default();
            let enable = ids("enableRuleIds");
            entry.retain(|i| !enable.contains(i));
            for d in ids("disableRuleIds") {
                if !entry.contains(&d) {
                    entry.push(d);
                }
            }
            ok(Value::Null)
        }
        "getDisabledRuleIds" => {
            let set = options.get("rulesetId").and_then(Value::as_str).unwrap_or("");
            return ok(json!(ext.state.disabled_rules.get(set).cloned().unwrap_or_default()));
        }
        "getAvailableStaticRuleCount" => return ok(json!(30000)),
        "isRegexSupported" => return ok(json!({ "isSupported": true })),
        "getMatchedRules" => return ok(json!({ "rulesMatchedInfo": [] })),
        "testMatchOutcome" => return ok(json!({ "matchedRules": [] })),
        "setExtensionActionOptions" => return ok(Value::Null),
        _ => return err(format!("no such call: declarativeNetRequest.{method}")),
    };
    rt.save_state(&caller.profile, &caller.extension);
    answer
}

// ---- bookmarks ------------------------------------------------------------------------------

/// The tree's root, and the one folder under it that SpatiMesh's bookmarks are in.
const ROOT: &str = "0";
const TOP: &str = "1";

fn to_api(id: &str) -> String {
    if id.is_empty() {
        TOP.to_string()
    } else {
        id.to_string()
    }
}

fn from_api(id: &str) -> String {
    if id == TOP {
        String::new()
    } else {
        id.to_string()
    }
}

fn node(b: &Bookmark, all: &[Bookmark], depth: Option<usize>) -> Value {
    let siblings = bookmarks::children(all, &b.parent);
    let index = siblings.iter().position(|s| s.id == b.id).unwrap_or(0);
    let mut v = json!({ "id": b.id, "parentId": to_api(&b.parent), "index": index, "title": b.title, "dateAdded": b.added });
    match &b.url {
        Some(u) => v["url"] = json!(u),
        None => {
            v["dateGroupModified"] = json!(b.added);
            if let Some(d) = depth {
                v["children"] = Value::Array(bookmarks::children(all, &b.id).into_iter().map(|c| node(c, all, (d > 0).then(|| d - 1))).collect());
            }
        }
    }
    v
}

fn top(all: &[Bookmark], depth: Option<usize>) -> Value {
    let mut v = json!({ "id": TOP, "parentId": ROOT, "index": 0, "title": "Bookmarks", "dateAdded": 0, "folderType": "bookmarks-bar" });
    if let Some(d) = depth {
        v["children"] = Value::Array(bookmarks::children(all, "").into_iter().map(|c| node(c, all, (d > 0).then(|| d - 1))).collect());
    }
    v
}

fn bookmarks_call(rt: &mut Runtime, side: &mut dyn BrowserSide, caller: &Caller, method: &str, args: &[Value]) -> Answer {
    let profile = caller.profile.clone();
    let all = if rt.is_private(&profile) { Vec::new() } else { bookmarks::all(&rt.store, &profile) };
    let find = |id: &str| all.iter().find(|b| b.id == id);
    let deep = Some(usize::MAX);
    let arg = |n: usize| args.get(n).cloned().unwrap_or(Value::Null);
    let id = arg(0).as_str().unwrap_or("").to_string();
    let missing = || err("Can't find bookmark for id.");
    let writes = matches!(method, "create" | "update" | "move" | "remove" | "removeTree");
    if writes && rt.is_private(&profile) {
        return err("Bookmarks are not kept in private windows.");
    }
    if writes && (id == ROOT || (id == TOP && method != "create")) {
        return err("Can't modify the root bookmark folders.");
    }
    let told = |rt: &Runtime, side: &mut dyn BrowserSide, event: &str, args: Vec<Value>| {
        rt.emit(side, &profile, None, event, &|ext| ext.has("bookmarks").then(|| args.clone()), &|c| c.kind != Kind::Content);
    };
    match method {
        "getTree" => ok(json!([{ "id": ROOT, "title": "", "dateAdded": 0, "children": [top(&all, deep)] }])),
        "getSubTree" => match id.as_str() {
            ROOT => ok(json!([{ "id": ROOT, "title": "", "dateAdded": 0, "children": [top(&all, deep)] }])),
            TOP => ok(json!([top(&all, deep)])),
            _ => find(&id).map_or_else(missing, |b| ok(json!([node(b, &all, deep)]))),
        },
        "get" => {
            let ids: Vec<String> = match arg(0) {
                Value::Array(a) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
                Value::String(s) => vec![s],
                _ => Vec::new(),
            };
            let mut out = Vec::new();
            for i in ids {
                match i.as_str() {
                    ROOT => out.push(json!({ "id": ROOT, "title": "", "dateAdded": 0 })),
                    TOP => out.push(top(&all, None)),
                    other => match find(other) {
                        Some(b) => out.push(node(b, &all, None)),
                        None => return missing(),
                    },
                }
            }
            ok(Value::Array(out))
        }
        "getChildren" => match id.as_str() {
            ROOT => ok(json!([top(&all, None)])),
            other => {
                let parent = from_api(other);
                if !parent.is_empty() && find(&parent).is_none() {
                    return missing();
                }
                ok(Value::Array(bookmarks::children(&all, &parent).into_iter().map(|b| node(b, &all, None)).collect()))
            }
        },
        "getRecent" => {
            let n = arg(0).as_u64().unwrap_or(10) as usize;
            let mut items: Vec<&Bookmark> = all.iter().filter(|b| !b.is_folder()).collect();
            items.sort_by_key(|b| std::cmp::Reverse(b.added));
            ok(Value::Array(items.into_iter().take(n).map(|b| node(b, &all, None)).collect()))
        }
        "search" => {
            let q = arg(0);
            let words: Vec<String> = match &q {
                Value::String(s) => s.split_whitespace().map(str::to_lowercase).collect(),
                v => v.get("query").and_then(Value::as_str).unwrap_or("").split_whitespace().map(str::to_lowercase).collect(),
            };
            let url = q.get("url").and_then(Value::as_str);
            let title = q.get("title").and_then(Value::as_str);
            let found: Vec<Value> = all
                .iter()
                .filter(|b| {
                    let text = format!("{} {}", b.title, b.url.clone().unwrap_or_default()).to_lowercase();
                    words.iter().all(|w| text.contains(w)) && url.map_or(true, |u| b.url.as_deref() == Some(u)) && title.map_or(true, |t| b.title == t)
                })
                .map(|b| node(b, &all, None))
                .collect();
            ok(Value::Array(found))
        }
        "create" => {
            let props = arg(0);
            let parent = from_api(props.get("parentId").and_then(Value::as_str).unwrap_or(TOP));
            if parent == ROOT || (!parent.is_empty() && !find(&parent).is_some_and(Bookmark::is_folder)) {
                return err("Can't find parent bookmark for id.");
            }
            let siblings = bookmarks::children(&all, &parent);
            let before = props.get("index").and_then(Value::as_u64).and_then(|i| siblings.get(i as usize)).map(|b| b.id.clone());
            let at = match &before {
                Some(b) => At::Before(b),
                None => At::End,
            };
            let title = props.get("title").and_then(Value::as_str).unwrap_or("");
            let url = props.get("url").and_then(Value::as_str);
            match bookmarks::add(&rt.store, &profile, &parent, title, url, at) {
                Ok(b) => {
                    let now = bookmarks::all(&rt.store, &profile);
                    let made = node(&b, &now, Some(0));
                    told(rt, side, "bookmarks.onCreated", vec![json!(b.id), made.clone()]);
                    ok(made)
                }
                Err(e) => err(e),
            }
        }
        "update" => {
            let Some(mut b) = find(&id).cloned() else { return missing() };
            let changes = arg(1);
            if let Some(t) = changes.get("title").and_then(Value::as_str) {
                b.title = t.to_string();
            }
            if let (Some(u), false) = (changes.get("url").and_then(Value::as_str), b.is_folder()) {
                b.url = Some(u.to_string());
            }
            if let Err(e) = bookmarks::update(&rt.store, &profile, &b) {
                return err(e);
            }
            let mut info = json!({ "title": b.title });
            if let Some(u) = &b.url {
                info["url"] = json!(u);
            }
            told(rt, side, "bookmarks.onChanged", vec![json!(b.id), info]);
            let now = bookmarks::all(&rt.store, &profile);
            ok(node(&b, &now, None))
        }
        "move" => {
            let Some(b) = find(&id).cloned() else { return missing() };
            let dest = arg(1);
            let old_index = bookmarks::children(&all, &b.parent).iter().position(|x| x.id == b.id).unwrap_or(0);
            let parent = dest.get("parentId").and_then(Value::as_str).map(from_api).unwrap_or(b.parent.clone());
            let others: Vec<&Bookmark> = bookmarks::children(&all, &parent).into_iter().filter(|x| x.id != b.id).collect();
            let before = dest.get("index").and_then(Value::as_u64).and_then(|i| others.get(i as usize)).map(|x| x.id.clone());
            let at = match &before {
                Some(x) => At::Before(x),
                None => At::End,
            };
            if let Err(e) = bookmarks::move_to(&rt.store, &profile, &b.id, &parent, at) {
                return err(e);
            }
            let now = bookmarks::all(&rt.store, &profile);
            let moved = now.iter().find(|x| x.id == b.id).cloned().unwrap_or(b.clone());
            let info = node(&moved, &now, None);
            told(rt, side, "bookmarks.onMoved", vec![json!(b.id), json!({ "parentId": to_api(&parent), "index": info["index"], "oldParentId": to_api(&b.parent), "oldIndex": old_index })]);
            ok(info)
        }
        "remove" | "removeTree" => {
            let Some(b) = find(&id).cloned() else { return missing() };
            if method == "remove" && b.is_folder() && !bookmarks::children(&all, &b.id).is_empty() {
                return err("Can't remove non-empty folder (use recursive to force).");
            }
            let gone = node(&b, &all, deep);
            bookmarks::remove(&rt.store, &profile, &b.id);
            told(rt, side, "bookmarks.onRemoved", vec![json!(b.id), json!({ "parentId": to_api(&b.parent), "index": gone["index"], "node": gone })]);
            ok(Value::Null)
        }
        _ => err(format!("no such call: bookmarks.{method}")),
    }
}
