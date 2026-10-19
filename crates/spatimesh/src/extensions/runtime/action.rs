//! The toolbar button an extension has (`action`, Manifest V2's `browserAction` and
//! `pageAction`): its title, picture, badge and popup -- for every tab, or for one -- as the
//! extension sets them, and what happens when the wearer presses it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use serde_json::{json, Value};

use super::{err, ok, Answer, BrowserSide, Caller, Kind, Runtime};
use crate::engine::TabId;
use crate::extensions::manifest::ActionKind;
use crate::tabs::WindowKey;

/// A button's picture.
#[derive(Debug, Clone, PartialEq)]
pub enum Picture {
    /// Files of the extension's (paths in it), by size; size 0 is one of no stated size.
    Files(BTreeMap<u32, String>),
    /// Pixels the extension made (`imageData`): width, height, and red, green, blue, alpha.
    Pixels(u32, u32, Vec<u8>),
}

#[derive(Debug, Clone, Default)]
pub(super) struct Values {
    title: Option<String>,
    picture: Option<Picture>,
    popup: Option<String>,
    badge: Option<String>,
    badge_background: Option<[u8; 4]>,
    badge_color: Option<[u8; 4]>,
    enabled: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub(in crate::extensions::runtime) struct ActionState {
    every: Values,
    tabs: HashMap<TabId, Values>,
    /// `pageAction.show`'s tabs.
    shown: HashSet<TabId>,
}

impl ActionState {
    /// What was set for a tab, gone with it.
    pub fn forget(&mut self, tab: TabId) {
        self.tabs.remove(&tab);
        self.shown.remove(&tab);
    }

    fn get<T: Clone>(&self, tab: Option<TabId>, field: impl Fn(&Values) -> &Option<T>) -> Option<T> {
        tab.and_then(|t| self.tabs.get(&t)).and_then(|v| field(v).clone()).or_else(|| field(&self.every).clone())
    }

    fn values(&mut self, tab: Option<TabId>) -> &mut Values {
        match tab {
            Some(t) => self.tabs.entry(t).or_default(),
            None => &mut self.every,
        }
    }
}

/// What the toolbar shows for an extension, for the tab in front.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionView {
    pub extension: String,
    pub name: String,
    pub title: String,
    /// Where its files are, for a picture of files.
    pub root: PathBuf,
    pub picture: Option<Picture>,
    pub badge: String,
    pub badge_background: [u8; 4],
    pub badge_color: [u8; 4],
    pub enabled: bool,
    /// The popup's address, if pressing it opens one.
    pub popup: Option<String>,
}

/// Chrome's badge, unless the extension says otherwise.
const BADGE_BACKGROUND: [u8; 4] = [217, 48, 37, 255];
const BADGE_COLOR: [u8; 4] = [255, 255, 255, 255];

/// A colour as an extension gives one: `[r, g, b, a]`, or CSS's `#rgb`, `#rrggbb`, `#rrggbbaa`,
/// `rgb()`, `rgba()` or a name.
pub(super) fn color(v: &Value) -> Option<[u8; 4]> {
    if let Some(a) = v.as_array() {
        let n: Vec<u8> = a.iter().map(|x| x.as_f64().unwrap_or(0.0).clamp(0.0, 255.0) as u8).collect();
        return (n.len() >= 3).then(|| [n[0], n[1], n[2], n.get(3).copied().unwrap_or(255)]);
    }
    let s = v.as_str()?.trim().to_ascii_lowercase();
    if let Some(hex) = s.strip_prefix('#') {
        let digits: Vec<u8> = hex.chars().map(|c| c.to_digit(16).map(|d| d as u8)).collect::<Option<_>>()?;
        return match digits.len() {
            3 | 4 => Some([digits[0] * 17, digits[1] * 17, digits[2] * 17, digits.get(3).map_or(255, |a| a * 17)]),
            6 | 8 => {
                let byte = |i: usize| digits[i] * 16 + digits[i + 1];
                Some([byte(0), byte(2), byte(4), if digits.len() == 8 { byte(6) } else { 255 }])
            }
            _ => None,
        };
    }
    if let Some(inner) = s.strip_prefix("rgba(").or_else(|| s.strip_prefix("rgb(")).and_then(|r| r.strip_suffix(')')) {
        let parts: Vec<f64> = inner.split(',').map(|p| p.trim().parse::<f64>()).collect::<Result<_, _>>().ok()?;
        if parts.len() < 3 {
            return None;
        }
        let alpha = parts.get(3).map_or(255.0, |a| if *a <= 1.0 { a * 255.0 } else { *a });
        return Some([parts[0] as u8, parts[1] as u8, parts[2] as u8, alpha.clamp(0.0, 255.0) as u8]);
    }
    Some(match s.as_str() {
        "black" => [0, 0, 0, 255],
        "white" => [255, 255, 255, 255],
        "red" => [255, 0, 0, 255],
        "green" => [0, 128, 0, 255],
        "blue" => [0, 0, 255, 255],
        "yellow" => [255, 255, 0, 255],
        "orange" => [255, 165, 0, 255],
        "purple" => [128, 0, 128, 255],
        "gray" | "grey" => [128, 128, 128, 255],
        "transparent" => [0, 0, 0, 0],
        _ => return None,
    })
}

/// A picture as `setIcon` gives it: paths, or pixels made in a canvas (api.js made them plain).
fn picture(details: &Value) -> Option<Picture> {
    if let Some(data) = details.get("imageData") {
        let one = if data.get("data").is_some() { Some(data) } else { data.as_object().and_then(|m| m.values().next_back()) };
        let img = one?;
        let (w, h) = (img.get("width")?.as_u64()? as u32, img.get("height")?.as_u64()? as u32);
        let bytes: Vec<u8> = img.get("data")?.as_array()?.iter().map(|b| b.as_u64().unwrap_or(0) as u8).collect();
        return (bytes.len() == (w * h * 4) as usize).then_some(Picture::Pixels(w, h, bytes));
    }
    match details.get("path")? {
        Value::String(p) => Some(Picture::Files(BTreeMap::from([(0, p.clone())]))),
        Value::Object(m) => Some(Picture::Files(m.iter().filter_map(|(k, v)| Some((k.parse().ok()?, v.as_str()?.to_string()))).collect())),
        _ => None,
    }
}

pub(super) fn call(rt: &mut Runtime, side: &mut dyn BrowserSide, caller: &Caller, call: &str, args: &[Value]) -> Answer {
    let (namespace, method) = call.split_once('.').unwrap_or((call, ""));
    let current = rt.current_window(side, caller);
    let Some(ext) = rt.ext_mut(&caller.profile, &caller.extension) else { return err("extension gone") };
    let Some(action) = ext.manifest.action.clone() else { return err(format!("{namespace} is not in the manifest")) };
    let details = args.first().cloned().unwrap_or(Value::Null);
    // A tab: `{tabId}` in the details, or the number itself (enable, disable, show, hide).
    let tab: Option<TabId> = match &details {
        Value::Number(n) => n.as_u64(),
        v => v.get("tabId").and_then(Value::as_u64),
    };
    let state = &mut ext.action;
    let changed = |side: &mut dyn BrowserSide| side.actions_changed(&caller.profile);
    let answer = match method {
        "setTitle" => {
            state.values(tab).title = details.get("title").and_then(Value::as_str).map(str::to_string);
            changed(side);
            ok(Value::Null)
        }
        "getTitle" => ok(json!(state.get(tab, |v| &v.title).or(action.title.clone()).unwrap_or_else(|| ext.manifest.name.clone()))),
        "setIcon" => {
            let Some(p) = picture(&details) else { return err("setIcon wants imageData or path") };
            state.values(tab).picture = Some(p);
            changed(side);
            ok(Value::Null)
        }
        "setPopup" => {
            state.values(tab).popup = details.get("popup").and_then(Value::as_str).map(str::to_string);
            changed(side);
            ok(Value::Null)
        }
        "getPopup" => {
            let popup = state.get(tab, |v| &v.popup).or(action.popup.clone()).filter(|p| !p.is_empty());
            ok(json!(popup.map(|p| ext.url(&p)).unwrap_or_default()))
        }
        "setBadgeText" => {
            state.values(tab).badge = details.get("text").and_then(Value::as_str).map(str::to_string);
            changed(side);
            ok(Value::Null)
        }
        "getBadgeText" => ok(json!(state.get(tab, |v| &v.badge).unwrap_or_default())),
        "setBadgeBackgroundColor" | "setBadgeTextColor" => {
            let Some(c) = details.get("color").and_then(color) else { return err("not a colour") };
            let values = state.values(tab);
            if method == "setBadgeBackgroundColor" {
                values.badge_background = Some(c);
            } else {
                values.badge_color = Some(c);
            }
            changed(side);
            ok(Value::Null)
        }
        "getBadgeBackgroundColor" => ok(json!(state.get(tab, |v| &v.badge_background).unwrap_or(BADGE_BACKGROUND))),
        "getBadgeTextColor" => ok(json!(state.get(tab, |v| &v.badge_color).unwrap_or(BADGE_COLOR))),
        "enable" | "disable" | "show" | "hide" => {
            let on = matches!(method, "enable" | "show");
            if action.kind == ActionKind::PageAction {
                match tab {
                    Some(t) if on => {
                        state.shown.insert(t);
                    }
                    Some(t) => {
                        state.shown.remove(&t);
                    }
                    None => return err("pageAction needs a tab"),
                }
            } else {
                state.values(tab).enabled = Some(on);
            }
            changed(side);
            ok(Value::Null)
        }
        "isEnabled" | "isShown" => {
            let on = if action.kind == ActionKind::PageAction { tab.is_some_and(|t| state.shown.contains(&t)) } else { state.get(tab, |v| &v.enabled).unwrap_or(true) };
            ok(json!(on))
        }
        "getUserSettings" => ok(json!({ "isOnToolbar": true })),
        "openPopup" => {
            let window = details.get("windowId").and_then(Value::as_u64).map(|w| w as WindowKey).or(current);
            let Some(window) = window else { return err("No window to open the popup in.") };
            let opened = rt.open_popup(side, &caller.profile, window, &caller.extension);
            return if opened { ok(Value::Null) } else { err("The extension has no popup to open.") };
        }
        _ => err(format!("no such call: {call}")),
    };
    answer
}

impl Runtime {
    /// What the toolbar shows for a profile's extensions, for the tab in front.
    pub fn actions(&self, profile: &str, tab: Option<TabId>) -> Vec<ActionView> {
        let Some(loaded) = self.profiles.get(profile) else { return Vec::new() };
        loaded
            .exts
            .iter()
            .filter_map(|ext| {
                let action = ext.manifest.action.as_ref()?;
                let state = &ext.action;
                let enabled = if action.kind == ActionKind::PageAction { tab.is_some_and(|t| state.shown.contains(&t)) } else { state.get(tab, |v| &v.enabled).unwrap_or(true) };
                let picture = state.get(tab, |v| &v.picture).or_else(|| {
                    let icons = if action.icons.is_empty() { &ext.manifest.icons } else { &action.icons };
                    (!icons.is_empty()).then(|| Picture::Files(icons.clone()))
                });
                Some(ActionView {
                    extension: ext.installed.id.clone(),
                    name: ext.manifest.name.clone(),
                    title: state.get(tab, |v| &v.title).or(action.title.clone()).unwrap_or_else(|| ext.manifest.name.clone()),
                    root: ext.installed.root.clone(),
                    picture,
                    badge: state.get(tab, |v| &v.badge).unwrap_or_default(),
                    badge_background: state.get(tab, |v| &v.badge_background).unwrap_or(BADGE_BACKGROUND),
                    badge_color: state.get(tab, |v| &v.badge_color).unwrap_or(BADGE_COLOR),
                    enabled,
                    popup: state.get(tab, |v| &v.popup).or(action.popup.clone()).filter(|p| !p.is_empty()).map(|p| ext.url(&p)),
                })
            })
            .collect()
    }

    /// The popup of an extension's button, over `window`, if it has one.
    fn open_popup(&mut self, side: &mut dyn BrowserSide, profile: &str, window: WindowKey, extension: &str) -> bool {
        let tab = side.tabs().active(window);
        let Some(view) = self.actions(profile, tab).into_iter().find(|a| a.extension == extension) else { return false };
        let Some(url) = view.popup else { return false };
        // One popup at a time over a window.
        let open: Vec<TabId> = self.popups.iter().filter(|(_, (_, _, w))| *w == window).map(|(t, _)| *t).collect();
        for t in open {
            self.popup_closed(side, t);
            side.close_popup(profile, t);
        }
        let popup = self.hidden_tab();
        self.popups.insert(popup, (profile.to_string(), extension.to_string(), window));
        side.open_popup(profile, window, popup, &url);
        true
    }

    /// The wearer pressed an extension's button in `window`: its popup opens, or its
    /// `onClicked` is told, and it may touch the tab in front (`activeTab`).
    pub fn action_clicked(&mut self, side: &mut dyn BrowserSide, profile: &str, window: WindowKey, extension: &str) {
        let tab = side.tabs().active(window);
        let Some(view) = self.actions(profile, tab).into_iter().find(|a| a.extension == extension) else { return };
        if !view.enabled {
            return;
        }
        if let (Some(t), Some(ext)) = (tab, self.ext_mut(profile, extension)) {
            if ext.has("activeTab") {
                ext.active_tabs.insert(t);
            }
        }
        if self.open_popup(side, profile, window, extension) {
            return;
        }
        let Some(ext) = self.ext(profile, extension) else { return };
        let event = match ext.manifest.action.as_ref().map(|a| a.kind) {
            Some(ActionKind::Action) => "action.onClicked",
            Some(ActionKind::BrowserAction) => "browserAction.onClicked",
            Some(ActionKind::PageAction) => "pageAction.onClicked",
            None => return,
        };
        let Some(t) = tab else { return };
        let Some(full) = super::tabs::tab_json(self, side, t, None) else { return };
        self.emit(side, profile, Some(extension), event, &|ext| Some(vec![super::tabs::redact(&full, ext, t)]), &|c| c.kind != Kind::Content);
    }
}
