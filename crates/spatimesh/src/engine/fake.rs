//! An engine with no web in it: each page is a flat colour chosen by its address, and history
//! is kept as a browser keeps it. What the window and the wire are tested against, and what
//! SpatiMesh shows where no engine could be started (`SPATIMESH_ENGINE=fake`).

use std::collections::HashMap;
use std::time::Duration;

use super::{Button, Canvas, Engine, Format, Happened, KeyPress, Modifiers, Rect, TabId, View};

struct Page {
    history: Vec<String>,
    at: usize,
    view: View,
    /// Something to draw.
    dirty: bool,
    muted: bool,
    window: u32,
    /// What was typed on the page since it loaded, for tests to read back.
    typed: String,
    clicks: u32,
}

impl Page {
    fn url(&self) -> &str {
        &self.history[self.at]
    }
}

#[derive(Default)]
pub struct Fake {
    pages: HashMap<TabId, Page>,
    happened: Vec<Happened>,
}

/// The colour a page is drawn in: the same address, the same colour.
pub fn colour_of(url: &str) -> [u8; 3] {
    let mut h: u32 = 0x811c_9dc5;
    for b in url.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    [(h >> 16) as u8 | 0x20, (h >> 8) as u8 | 0x20, h as u8 | 0x20]
}

/// A page's title: its address without the scheme.
fn title_of(url: &str) -> String {
    url.split_once("://").map(|(_, rest)| rest).unwrap_or(url).trim_end_matches('/').to_string()
}

impl Fake {
    pub fn new() -> Fake {
        Fake::default()
    }

    /// What has been typed into a tab's page.
    pub fn typed(&self, tab: TabId) -> Option<&str> {
        self.pages.get(&tab).map(|p| p.typed.as_str())
    }

    pub fn clicks(&self, tab: TabId) -> u32 {
        self.pages.get(&tab).map_or(0, |p| p.clicks)
    }

    pub fn window_of(&self, tab: TabId) -> Option<u32> {
        self.pages.get(&tab).map(|p| p.window)
    }

    pub fn is_muted(&self, tab: TabId) -> Option<bool> {
        self.pages.get(&tab).map(|p| p.muted)
    }

    fn arrived(&mut self, tab: TabId) {
        let Some(page) = self.pages.get_mut(&tab) else { return };
        page.dirty = true;
        page.typed.clear();
        let url = page.url().to_string();
        let (back, forward) = (page.at > 0, page.at + 1 < page.history.len());
        self.happened.push(Happened::Url { tab, url: url.clone() });
        self.happened.push(Happened::Title { tab, title: title_of(&url) });
        self.happened.push(Happened::Progress { tab, progress: 1.0, loading: false });
        self.happened.push(Happened::History { tab, back, forward });
    }
}

impl Engine for Fake {
    fn name(&self) -> String {
        "no engine (flat pages)".to_string()
    }

    fn open(&mut self, tab: TabId, url: &str, view: View) -> Result<(), String> {
        self.pages.insert(tab, Page { history: vec![url.to_string()], at: 0, view, dirty: true, muted: false, window: 0, typed: String::new(), clicks: 0 });
        self.arrived(tab);
        Ok(())
    }

    fn close(&mut self, tab: TabId) {
        self.pages.remove(&tab);
    }

    fn load(&mut self, tab: TabId, url: &str) {
        if let Some(page) = self.pages.get_mut(&tab) {
            page.history.truncate(page.at + 1);
            page.history.push(url.to_string());
            page.at += 1;
            self.arrived(tab);
        }
    }

    fn back(&mut self, tab: TabId) {
        if let Some(page) = self.pages.get_mut(&tab).filter(|p| p.at > 0) {
            page.at -= 1;
            self.arrived(tab);
        }
    }

    fn forward(&mut self, tab: TabId) {
        if let Some(page) = self.pages.get_mut(&tab).filter(|p| p.at + 1 < p.history.len()) {
            page.at += 1;
            self.arrived(tab);
        }
    }

    fn reload(&mut self, tab: TabId) {
        if self.pages.contains_key(&tab) {
            self.arrived(tab);
        }
    }

    fn stop(&mut self, _tab: TabId) {}

    fn resize(&mut self, tab: TabId, view: View) {
        if let Some(page) = self.pages.get_mut(&tab) {
            page.view = view;
            page.dirty = true;
        }
    }

    fn pointer(&mut self, tab: TabId, _x: f32, _y: f32, button: Option<(Button, bool)>, _modifiers: Modifiers) {
        if let (Some(page), Some((Button::Left, true))) = (self.pages.get_mut(&tab), button) {
            page.clicks += 1;
        }
    }

    fn pointer_left(&mut self, _tab: TabId) {}

    fn scroll(&mut self, _tab: TabId, _x: f32, _y: f32, _dx: f32, _dy: f32, _modifiers: Modifiers) {}

    fn key(&mut self, tab: TabId, key: &KeyPress) {
        if let (Some(page), true, Some(text)) = (self.pages.get_mut(&tab), key.pressed, key.text.as_ref()) {
            page.typed.push_str(text);
        }
    }

    fn focus(&mut self, _tab: TabId, _focused: bool) {}

    fn set_window(&mut self, tab: TabId, window: u32) {
        if let Some(page) = self.pages.get_mut(&tab) {
            page.window = window;
        }
    }

    fn set_muted(&mut self, tab: TabId, muted: bool) {
        if let Some(page) = self.pages.get_mut(&tab) {
            page.muted = muted;
        }
    }

    fn choose(&mut self, _tab: TabId, _index: Option<usize>) {}

    fn answer(&mut self, _tab: TabId, _accepted: bool, _text: Option<String>) {}

    fn run_script(&mut self, tab: TabId, _world: &str, _all_frames: bool, _code: &str, request: u64, _boot: bool) {
        if request != 0 && self.pages.contains_key(&tab) {
            self.happened.push(Happened::ScriptResult { tab, request, result: Err("pages here run no scripts".into()) });
        }
    }

    fn pump(&mut self, wait: Duration, canvas: &mut dyn Canvas) -> Vec<Happened> {
        let mut drawn = Vec::new();
        for (tab, page) in self.pages.iter_mut().filter(|(_, p)| p.dirty) {
            let View { width, height, .. } = page.view;
            let [r, g, b] = colour_of(page.url());
            if let Some(pixels) = canvas.begin(*tab, width, height, Format::Bgra) {
                for px in pixels.chunks_exact_mut(4) {
                    px.copy_from_slice(&[b, g, r, 255]);
                }
                canvas.finish(*tab, &[Rect::whole(width, height)]);
                page.dirty = false;
                drawn.push(*tab);
            }
        }
        let mut out = std::mem::take(&mut self.happened);
        out.extend(drawn.into_iter().map(|tab| Happened::Picture { tab }));
        if out.is_empty() && !wait.is_zero() {
            std::thread::sleep(wait.min(Duration::from_millis(5)));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::KeyPress;

    /// A canvas in ordinary memory.
    #[derive(Default)]
    struct Memory {
        pictures: HashMap<TabId, (u32, u32, Vec<u8>)>,
        finished: Vec<TabId>,
    }

    impl Canvas for Memory {
        fn begin(&mut self, tab: TabId, width: u32, height: u32, _format: Format) -> Option<&mut [u8]> {
            let entry = self.pictures.entry(tab).or_insert((0, 0, Vec::new()));
            if (entry.0, entry.1) != (width, height) {
                *entry = (width, height, vec![0; (width * height * 4) as usize]);
            }
            Some(&mut entry.2)
        }
        fn finish(&mut self, tab: TabId, _changed: &[Rect]) {
            self.finished.push(tab);
        }
    }

    #[test]
    fn the_interface_end_to_end() {
        let mut engine = Fake::new();
        let mut canvas = Memory::default();
        engine.open(7, "https://a.example/", View::new(64, 32, 1.0)).unwrap();
        let happened = engine.pump(Duration::ZERO, &mut canvas);
        assert!(happened.contains(&Happened::Title { tab: 7, title: "a.example".into() }));
        assert!(happened.contains(&Happened::Picture { tab: 7 }));
        let [r, g, b] = colour_of("https://a.example/");
        assert_eq!(&canvas.pictures[&7].2[..4], &[b, g, r, 255]);

        engine.load(7, "https://b.example/");
        let happened = engine.pump(Duration::ZERO, &mut canvas);
        assert!(happened.contains(&Happened::History { tab: 7, back: true, forward: false }));
        engine.back(7);
        let happened = engine.pump(Duration::ZERO, &mut canvas);
        assert!(happened.contains(&Happened::Url { tab: 7, url: "https://a.example/".into() }));
        assert!(happened.contains(&Happened::History { tab: 7, back: false, forward: true }));

        let key = KeyPress { code: 30, key: spatiand_app::Key::Char('a'), text: Some("a".into()), pressed: true, repeat: false, modifiers: Modifiers::default() };
        engine.key(7, &key);
        assert_eq!(engine.typed(7), Some("a"));
        engine.pointer(7, 3.0, 4.0, Some((Button::Left, true)), Modifiers::default());
        assert_eq!(engine.clicks(7), 1);
        engine.set_window(7, 3);
        assert_eq!(engine.window_of(7), Some(3));
        engine.resize(7, View::new(10, 10, 1.0));
        engine.pump(Duration::ZERO, &mut canvas);
        assert_eq!((canvas.pictures[&7].0, canvas.pictures[&7].1), (10, 10));
        // A tab it does not have is no harm.
        engine.load(99, "https://x/");
        engine.close(7);
        assert!(engine.pump(Duration::ZERO, &mut canvas).is_empty());
    }
}
