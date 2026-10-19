//! The window's side of extensions (docs/spatimesh.md, "Extensions"): what the runtime asks of
//! the browser -- its tabs and windows, its engines -- and the extensions' pages that no window
//! shows as a tab: their backgrounds, offscreen documents and popups, and their toolbar buttons.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use spatiand_app::ui::Rect;
use spatimesh::engine::{self, DialogKind, Format, Happened, TabId, View};
use spatimesh::extensions::install;
use spatimesh::extensions::runtime::{ActionView, BrowserSide, Change, Navigation, Picture, Runtime};
use spatimesh::extensions::set::ExtensionSet;
use spatimesh::tabs::{Place, Tabs, WindowKey};
use spatimesh::wire::ToHost;

use super::{is_start, Browser};
use crate::page::PageTexture;

/// How big an extension's popup is drawn (Chrome lets a popup be at most 800 by 600).
pub(super) const POPUP: (f32, f32) = (440.0, 560.0);
/// The size an extension's page that nobody sees is laid out at.
const HIDDEN_VIEW: (u32, u32) = (800, 600);

/// What happened to tabs and windows, told to the extensions once the browser is done with it.
pub(super) enum Told {
    EngineStarted(String),
    TabCreated(TabId),
    TabChanged(TabId, Change),
    TabActivated(TabId),
    TabRemoved { profile: String, tab: TabId, window: WindowKey, closing: bool },
    TabMoved { tab: TabId, from: (WindowKey, usize), to: (WindowKey, usize) },
    WindowCreated(WindowKey),
    WindowRemoved { profile: String, window: WindowKey },
    WindowFocused(Option<WindowKey>),
    Navigation { tab: TabId, url: String, what: Navigation },
}

/// An extension's popup over a window.
pub(super) struct ExtPopup {
    pub tab: TabId,
    pub profile: String,
    pub rect: Rect,
}

impl Browser {
    /// The runtime, out of its place while it works: what it does asks things of the browser.
    /// None if it is already at work (what it caused is told after, from [`Browser::told`]).
    pub(super) fn with_extensions<T>(&mut self, f: impl FnOnce(&mut Runtime, &mut Browser) -> T) -> Option<T> {
        let mut rt = self.extensions.take()?;
        let out = f(&mut rt, self);
        self.extensions = Some(rt);
        Some(out)
    }

    /// Tell the extensions what happened since they were last told.
    pub(super) fn tell_extensions(&mut self) {
        for _ in 0..4 {
            let told = std::mem::take(&mut self.told);
            if told.is_empty() || self.extensions.is_none() {
                self.told.extend(told);
                return;
            }
            self.with_extensions(|rt, b| {
                for t in told {
                    match t {
                        Told::EngineStarted(profile) => {
                            if b.hosts.contains_key(&profile) {
                                rt.engine_started(b, &profile);
                                rt.startup_done(&profile);
                            }
                        }
                        Told::TabCreated(tab) => rt.tab_created(b, tab),
                        Told::TabChanged(tab, change) => rt.tab_changed(b, tab, &change),
                        Told::TabActivated(tab) => rt.tab_activated(b, tab),
                        Told::TabRemoved { profile, tab, window, closing } => rt.tab_removed(b, &profile, tab, window, closing),
                        Told::TabMoved { tab, from, to } => rt.tab_moved(b, tab, from, to),
                        Told::WindowCreated(window) => rt.window_created(b, window),
                        Told::WindowRemoved { profile, window } => rt.window_removed(b, &profile, window),
                        Told::WindowFocused(window) => rt.window_focused(b, window),
                        Told::Navigation { tab, url, what } => rt.navigation(b, tab, &url, &what),
                    }
                }
            });
        }
    }

    /// A profile's engine has just started: its extensions go to it before any page does, so
    /// that the first page has its content scripts.
    pub(super) fn extensions_to_new_engine(&mut self, profile: &str) {
        let profile = profile.to_string();
        if self.with_extensions(|rt, b| {
            rt.engine_started(b, &profile);
            rt.startup_done(&profile);
        })
        .is_none()
        {
            self.told.push(Told::EngineStarted(profile));
        }
    }

    /// A profile's engine has gone, and its extensions' pages with it.
    pub(super) fn extensions_engine_gone(&mut self, profile: &str) {
        if let Some(rt) = self.extensions.as_mut() {
            rt.engine_gone(profile);
        }
        let gone: Vec<TabId> = self.hidden.iter().filter(|(_, p)| p.as_str() == profile).map(|(t, _)| *t).collect();
        for tab in gone {
            self.hidden.remove(&tab);
            self.forget_view(tab);
        }
        self.ext_popups.retain(|_, p| p.profile != profile);
    }

    /// A tab's picture and what waited on it, gone.
    pub(super) fn forget_view(&mut self, tab: TabId) {
        if let Some(view) = self.views.remove(&tab) {
            if let (Some(texture), Ok(gl)) = (view.texture, self.app.gl()) {
                texture.destroy(gl);
            }
        }
        for host in self.hosts.values_mut() {
            host.pictures.remove(&tab);
        }
        self.latest_buffer.remove(&tab);
    }

    /// Something a page of an extension's that no window shows did.
    pub(super) fn hidden_happened(&mut self, profile: &str, happened: Happened) {
        match happened {
            Happened::OpenTab { url, background, .. } => {
                let window = self.last_focused.filter(|w| self.tabs.window(*w).is_some_and(|x| x.profile == profile));
                if BrowserSide::open_tab(self, profile, window, &url, None, !background, None).is_none() {
                    log::info!("an extension's page asked for {url}, with no window of {profile} to open it in");
                }
            }
            Happened::CloseTab { tab } => {
                // window.close() in a popup closes it; a background's is ignored, as in Chrome.
                if let Some(window) = self.ext_popups.iter().find(|(_, p)| p.tab == tab).map(|(w, _)| *w) {
                    self.close_ext_popup(window);
                }
            }
            Happened::Crashed { tab, why } => {
                log::warn!("an extension's page (tab {tab}) stopped: {why}");
                let profile = profile.to_string();
                self.with_extensions(|rt, b| rt.hidden_gone(b, &profile, tab));
            }
            Happened::Failed { tab, url, why } => log::warn!("an extension's page (tab {tab}) could not load {url}: {why}"),
            Happened::Dialog { tab, kind, message, .. } => {
                // Nobody sees a background's alert(): it is said in the log and answered.
                log::info!("an extension's page (tab {tab}) says: {message}");
                let accepted = kind == DialogKind::Alert;
                self.tell_profile(Some(profile), ToHost::Answer { tab, accepted, text: None });
            }
            Happened::Choose { tab, .. } => self.tell_profile(Some(profile), ToHost::Choose { tab, index: None }),
            Happened::Picture { tab } => self.dirty_popup(tab),
            _ => {}
        }
    }

    pub(super) fn dirty_popup(&mut self, tab: TabId) {
        let windows: Vec<WindowKey> = self.ext_popups.iter().filter(|(_, p)| p.tab == tab).map(|(w, _)| *w).collect();
        for key in windows {
            if let Some(w) = self.win_of_key(key) {
                self.wins[w].dirty = true;
            }
        }
    }

    /// The popup over a window, closed.
    pub(super) fn close_ext_popup(&mut self, window: WindowKey) {
        let Some(popup) = self.ext_popups.remove(&window) else { return };
        self.with_extensions(|rt, b| rt.popup_closed(b, popup.tab));
        self.hidden.remove(&popup.tab);
        self.tell_profile(Some(&popup.profile), ToHost::Close { tab: popup.tab });
        self.forget_view(popup.tab);
        if let Some(w) = self.win_of_key(window) {
            self.wins[w].dirty = true;
        }
        self.focus_pages();
    }

    /// Where a popup goes over window `w`: below its button, inside the page.
    pub(super) fn popup_rect(&self, w: usize, extension: &str) -> Rect {
        let win = &self.wins[w];
        let page = win.page;
        let (width, height) = (POPUP.0.min(page.w), POPUP.1.min(page.h));
        let right = win.action_buttons.iter().find(|(e, _)| e == extension).map_or(page.right() - 8.0, |(_, r)| r.right());
        let x = (right - width).clamp(page.x, page.right() - width);
        Rect::new(x, page.y + 4.0, width, height)
    }

    /// The toolbar's extension buttons for a window, as the runtime has them for its tab in front.
    pub(super) fn actions_for(&self, key: WindowKey) -> Vec<ActionView> {
        let Some(window) = self.tabs.window(key) else { return Vec::new() };
        let tab = window.active_tab();
        self.extensions.as_ref().map_or(Vec::new(), |rt| rt.actions(&window.profile, tab))
    }

    /// The picture of an extension's button, made once.
    pub(super) fn icon(&mut self, view: &ActionView) -> Option<spatiand_app::ui::Texture> {
        let key = (view.extension.clone(), format!("{:?}", picture_key(view.picture.as_ref()?)));
        if !self.icons.contains_key(&key) {
            let texture = match view.picture.as_ref()? {
                Picture::Pixels(w, h, rgba) => self.app.gl().ok().and_then(|gl| texture_of(gl, *w, *h, rgba)),
                Picture::Files(files) => {
                    // The size nearest to a button's, or the biggest.
                    let path = files.iter().find(|(size, _)| **size >= 32).or_else(|| files.iter().next_back()).map(|(_, p)| p.clone())?;
                    let file = spatimesh::extensions::set::inside(&view.root, path.trim_start_matches('/'))?;
                    let (w, h, rgba) = decode_png(&file)?;
                    self.app.gl().ok().and_then(|gl| texture_of(gl, w, h, &rgba))
                }
            };
            self.icons.insert(key.clone(), texture);
        }
        self.icons.get(&key).and_then(|t| t.as_ref()).map(PageTexture::texture)
    }

    /// Install what `--load-extension` named, into the profile, at start.
    pub fn load_extensions(&mut self, profile: &str, paths: &[String]) {
        for path in paths {
            match install::install(&self.store, profile, Path::new(path)) {
                Ok((installed, manifest)) => log::info!("extension {} ({}) loaded from {path}", manifest.name, installed.id),
                Err(e) => log::warn!("the extension at {path} could not be loaded: {e}"),
            }
        }
        if !paths.is_empty() {
            let profile = profile.to_string();
            self.with_extensions(|rt, b| rt.refresh(b, &profile, &[]));
        }
    }

    /// The extension at a folder or package, installed in a window's profile (or installed again).
    pub(super) fn install_extension(&mut self, profile: &str, path: &str) -> Result<String, String> {
        if profile == spatimesh::profiles::PRIVATE {
            return Err("Extensions are installed in a profile, not in a private window".into());
        }
        let (installed, manifest) = install::install(&self.store, profile, Path::new(path.trim()))?;
        let profile = profile.to_string();
        let id = installed.id.clone();
        self.with_extensions(|rt, b| rt.refresh(b, &profile, &[&id]));
        Ok(manifest.name)
    }

    /// A toast: something said to the wearer for a few seconds.
    pub(super) fn say(&mut self, text: String) {
        self.toast = Some((text, Instant::now()));
        for win in &mut self.wins {
            win.dirty = true;
        }
    }
}

fn picture_key(p: &Picture) -> String {
    match p {
        Picture::Files(files) => files.values().cloned().collect::<Vec<_>>().join(","),
        Picture::Pixels(w, h, rgba) => {
            let mut hash: u64 = 0xcbf29ce484222325;
            for b in rgba {
                hash = (hash ^ *b as u64).wrapping_mul(0x100000001b3);
            }
            format!("{w}x{h}:{hash:x}")
        }
    }
}

/// A PNG file as width, height and red, green, blue, alpha.
fn decode_png(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::fs::File::open(path).ok()?);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let pixels = (info.width * info.height) as usize;
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf[..pixels * 4].to_vec(),
        png::ColorType::Rgb => buf[..pixels * 3].chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf[..pixels * 2].chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf[..pixels].iter().flat_map(|g| [*g, *g, *g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((info.width, info.height, rgba))
}

/// A texture of a picture: its colour multiplied by its alpha, as pages' are.
fn texture_of(gl: &spatiand_app::Gl, width: u32, height: u32, rgba: &[u8]) -> Option<PageTexture> {
    let premultiplied: Vec<u8> = rgba
        .chunks_exact(4)
        .flat_map(|p| {
            let a = p[3] as u32;
            [(p[0] as u32 * a / 255) as u8, (p[1] as u32 * a / 255) as u8, (p[2] as u32 * a / 255) as u8, p[3]]
        })
        .collect();
    let texture = PageTexture::new(gl, width, height, Format::Rgba).ok()?;
    texture.upload(gl, &premultiplied, &[engine::Rect::whole(width, height)]);
    Some(texture)
}

impl BrowserSide for Browser {
    fn tabs(&self) -> &Tabs {
        &self.tabs
    }

    fn last_focused(&self) -> Option<WindowKey> {
        self.last_focused.filter(|w| self.tabs.window(*w).is_some()).or_else(|| self.wins.first().map(|w| w.key))
    }

    fn open_tab(&mut self, profile: &str, window: Option<WindowKey>, url: &str, index: Option<usize>, active: bool, opener: Option<TabId>) -> Option<TabId> {
        let url = if url.is_empty() { "about:blank" } else { url };
        let window = window
            .filter(|w| self.tabs.window(*w).is_some_and(|x| x.profile == profile))
            .or_else(|| self.tabs.windows().iter().rev().find(|w| w.profile == profile).map(|w| w.key));
        let Some(key) = window else {
            let key = self.new_window(profile, Some(url));
            return self.tabs.window(key).and_then(|w| w.active_tab());
        };
        let tab = self.tabs.new_tab(key, url, Place::End, active, opener)?;
        if let Some(i) = index {
            let from = self.tabs.window(key).map_or(0, |w| w.tabs.len().saturating_sub(1));
            self.tabs.reorder(key, from, i);
        }
        self.open_in_engine(tab);
        self.told.push(Told::TabCreated(tab));
        if active {
            self.after_tab_change(key);
        } else if let Some(w) = self.win_of_key(key) {
            self.wins[w].dirty = true;
        }
        Some(tab)
    }

    fn open_window(&mut self, profile: &str, urls: &[String], tab: Option<TabId>, _focused: bool) -> Option<WindowKey> {
        if let Some(tab) = tab {
            self.tear_off(tab);
            return self.tabs.window_of(tab);
        }
        let key = self.new_window(profile, urls.first().map(String::as_str));
        for url in urls.iter().skip(1) {
            BrowserSide::open_tab(self, profile, Some(key), url, None, false, None);
        }
        Some(key)
    }

    fn load(&mut self, tab: TabId, url: &str) {
        if let Some(t) = self.tabs.tab_mut(tab) {
            t.url = url.to_string();
            t.loading = true;
            t.crashed = None;
        }
        let told = self.views.get(&tab).is_some_and(|v| v.told.is_some());
        if told {
            self.tell(tab, ToHost::Load { tab, url: url.to_string() });
        } else {
            self.reopen(tab);
        }
        if let Some(key) = self.tabs.window_of(tab) {
            if self.tabs.active(key) == Some(tab) {
                self.after_tab_change(key);
            }
        }
    }

    fn activate(&mut self, tab: TabId) {
        if let Some(key) = self.tabs.activate_tab(tab) {
            self.after_tab_change(key);
        }
    }

    fn close_tab(&mut self, tab: TabId) {
        Browser::close_tab(self, tab);
    }

    fn close_window(&mut self, window: WindowKey) {
        if let Some(w) = self.win_of_key(window) {
            Browser::close_window(self, w);
        }
    }

    fn focus_window(&mut self, window: WindowKey) {
        self.last_focused = Some(window);
    }

    fn reload(&mut self, tab: TabId) {
        self.reopen(tab);
    }

    fn back(&mut self, tab: TabId) {
        self.tell(tab, ToHost::Back { tab });
    }

    fn forward(&mut self, tab: TabId) {
        self.tell(tab, ToHost::Forward { tab });
    }

    fn mute(&mut self, tab: TabId, muted: bool) {
        if let Some(t) = self.tabs.tab_mut(tab) {
            t.muted = muted;
        }
        self.tell(tab, ToHost::Mute { tab, muted });
        self.dirty_bar(tab);
    }

    fn move_tab(&mut self, tab: TabId, window: WindowKey, index: Option<usize>) {
        Browser::move_tab(self, tab, window, index);
    }

    fn zoom(&self) -> f32 {
        self.settings.zoom()
    }

    fn set_zoom(&mut self, zoom: f32) {
        self.settings.zoom = zoom.clamp(0.5, 3.0);
        let _ = self.settings.save(&self.store);
        for w in 0..self.wins.len() {
            self.size_pages(w);
        }
    }

    fn page_size(&self, window: WindowKey) -> (u32, u32) {
        self.win_of_key(window).map_or((0, 0), |w| {
            let page = self.wins[w].page;
            (page.w as u32, page.h as u32)
        })
    }

    fn set_extensions(&mut self, profile: &str, set: &ExtensionSet) {
        let json = serde_json::to_string(set).unwrap_or_default();
        self.tell_profile(Some(profile), ToHost::Extensions { set: json });
    }

    fn run_script(&mut self, profile: &str, tab: TabId, world: &str, all_frames: bool, code: &str, request: u64, boot: bool) {
        self.tell_profile(Some(profile), ToHost::RunScript { tab, world: world.to_string(), all_frames, code: code.to_string(), request, boot });
    }

    fn open_hidden(&mut self, profile: &str, tab: TabId, url: &str) {
        self.hidden.insert(tab, profile.to_string());
        let view = View::new(HIDDEN_VIEW.0, HIDDEN_VIEW.1, 1.0);
        self.tell_profile(Some(profile), ToHost::Open { tab, url: url.to_string(), view });
    }

    fn close_hidden(&mut self, profile: &str, tab: TabId) {
        if self.hidden.remove(&tab).is_some() {
            self.tell_profile(Some(profile), ToHost::Close { tab });
            self.forget_view(tab);
        }
    }

    fn open_popup(&mut self, profile: &str, window: WindowKey, tab: TabId, url: &str) {
        let Some(w) = self.win_of_key(window) else { return };
        if self.ext_popups.contains_key(&window) {
            self.close_ext_popup(window);
        }
        let extension = spatimesh::extensions::set::parse_url(url).map(|(id, _)| id).unwrap_or_default();
        let rect = self.popup_rect(w, &extension);
        self.hidden.insert(tab, profile.to_string());
        let view = View::new(rect.w as u32, rect.h as u32, self.settings.zoom());
        self.tell_profile(Some(profile), ToHost::Open { tab, url: url.to_string(), view });
        self.tell_profile(Some(profile), ToHost::Window { tab, window });
        self.ext_popups.insert(window, super::ExtPopup { tab, profile: profile.to_string(), rect });
        self.wins[w].popup = None;
        self.wins[w].dirty = true;
        self.focus_pages();
    }

    fn close_popup(&mut self, _profile: &str, tab: TabId) {
        if let Some(window) = self.ext_popups.iter().find(|(_, p)| p.tab == tab).map(|(w, _)| *w) {
            let Some(popup) = self.ext_popups.remove(&window) else { return };
            self.hidden.remove(&popup.tab);
            self.tell_profile(Some(&popup.profile), ToHost::Close { tab: popup.tab });
            self.forget_view(popup.tab);
            if let Some(w) = self.win_of_key(window) {
                self.wins[w].dirty = true;
            }
        }
    }

    fn notify(&mut self, title: &str, message: &str) {
        log::info!("notification: {title}: {message}");
        self.say(if message.is_empty() { title.to_string() } else { format!("{title} \u{2014} {message}") });
    }

    fn actions_changed(&mut self, profile: &str) {
        for win in &mut self.wins {
            if self.tabs.window(win.key).is_some_and(|w| w.profile == profile) {
                win.dirty = true;
            }
        }
    }

    fn capture(&mut self, window: WindowKey) -> Option<(u32, u32, Vec<u8>)> {
        let tab = self.tabs.active(window)?;
        let profile = self.tabs.tab(tab)?.profile.clone();
        let buffer = *self.latest_buffer.get(&tab)?;
        let pictures = self.hosts.get(&profile)?.pictures.get(&tab)?;
        let bytes = pictures.buffer(buffer)?;
        let rgba = match pictures.format {
            Format::Rgba => bytes.to_vec(),
            Format::Bgra => bytes.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect(),
        };
        Some((pictures.width, pictures.height, rgba))
    }
}

/// What a page's change is to the extensions: a tab's `Change`, and webNavigation's moments.
pub(super) fn told_of(happened: &Happened, was_loading: bool, url: &str) -> Vec<Told> {
    match happened {
        Happened::Url { tab, url } => vec![
            Told::TabChanged(*tab, Change { url: Some(url.clone()), ..Default::default() }),
            Told::Navigation { tab: *tab, url: url.clone(), what: Navigation::Committed },
        ],
        Happened::Title { tab, title } => vec![Told::TabChanged(*tab, Change { title: Some(title.clone()), ..Default::default() })],
        Happened::Progress { tab, loading, .. } if *loading != was_loading => {
            let mut told = vec![Told::TabChanged(*tab, Change { loading: Some(*loading), ..Default::default() })];
            if *loading {
                told.push(Told::Navigation { tab: *tab, url: url.to_string(), what: Navigation::Before });
            } else if !is_start(url) {
                told.push(Told::Navigation { tab: *tab, url: url.to_string(), what: Navigation::DomContentLoaded });
                told.push(Told::Navigation { tab: *tab, url: url.to_string(), what: Navigation::Completed });
            }
            told
        }
        Happened::Sound { tab, playing } => vec![Told::TabChanged(*tab, Change { audible: Some(*playing), ..Default::default() })],
        Happened::Failed { tab, url, why } => vec![Told::Navigation { tab: *tab, url: url.clone(), what: Navigation::Error(why.clone()) }],
        _ => Vec::new(),
    }
}

/// An installed extension's name, in the wearer's language where it has one.
pub(super) fn extension_name(installed: &install::Installed) -> String {
    std::fs::read_to_string(installed.root.join("manifest.json"))
        .ok()
        .and_then(|text| spatimesh::extensions::manifest::Manifest::parse(&text).ok())
        .map(|m| {
            let language = std::env::var("LANG").ok().and_then(|l| l.split('.').next().map(str::to_string)).unwrap_or_else(|| "en-US".into());
            let messages = spatimesh::extensions::locale::Messages::load(&installed.root, &language, m.default_locale.as_deref());
            m.localized(&|n| Some(messages.get(n, &[], &installed.id)).filter(|s| !s.is_empty())).name
        })
        .unwrap_or_else(|| installed.id.clone())
}

/// The extensions' rows in the menu: each one (on or off), and installing one.
pub(super) fn menu_rows(store: &spatimesh::store::Store, profile: &str) -> Vec<(String, String, super::Action)> {
    use super::Action;
    let mut rows = Vec::new();
    if profile == spatimesh::profiles::PRIVATE {
        return rows;
    }
    let mut names: HashMap<String, String> = HashMap::new();
    for installed in install::list(store, profile) {
        let name = extension_name(&installed);
        names.insert(installed.id.clone(), name.clone());
        let mark = if installed.enabled { "on" } else { "off" };
        rows.push((name, mark.to_string(), Action::ToggleExtension(installed.id.clone())));
    }
    rows.push(("Load an extension\u{2026}".into(), "folder, .zip, .crx, .xpi".into(), Action::LoadExtension));
    if !names.is_empty() {
        rows.push(("Reload extensions".into(), "from their files".into(), Action::ReloadExtensions));
        rows.push(("Remove an extension\u{2026}".into(), String::new(), Action::RemoveExtension));
    }
    rows
}
