//! Finding in the page: a bar over the page's top right, the engine marking what it finds
//! (`Engine::find`), and which of them is in view counted here.

use spatiand_app::ui::{draw_icon, look, theme, Field, Icon, Input, Painter, Rect};
use spatiand_app::{Key, Modifiers};
use spatimesh::engine::TabId;
use spatimesh::wire::ToHost;

use super::Browser;

pub(super) struct FindBar {
    pub field: Field,
    /// The tab it finds in.
    pub tab: TabId,
    /// What the engine last said it found, and which of them is in view (from 1).
    pub matches: Option<u32>,
    pub at: u32,
    /// Where its parts were drawn, for the pointer.
    pub rect: Rect,
    pub up: Rect,
    pub down: Rect,
    pub close: Rect,
}

impl FindBar {
    fn new(tab: TabId) -> FindBar {
        let mut field = Field::sized(theme::ROW_EM * 0.8);
        field.hint = "Find in this page".into();
        field.focused = true;
        FindBar { field, tab, matches: None, at: 0, rect: Rect::default(), up: Rect::default(), down: Rect::default(), close: Rect::default() }
    }

    /// What it says of what was found: "3 of 12", "Not found".
    pub fn said(&self) -> String {
        match self.matches {
            _ if self.field.text.is_empty() => String::new(),
            None => String::new(),
            Some(0) => "Not found".into(),
            Some(n) => format!("{} of {n}", self.at.clamp(1, n)),
        }
    }
}

impl Browser {
    /// Control-F: the bar, its text chosen to be typed over; again, the same.
    pub(super) fn open_find(&mut self, w: usize) {
        let Some(tab) = self.tabs.active(self.wins[w].key) else { return };
        let bar = self.wins[w].find.get_or_insert_with(|| FindBar::new(tab));
        bar.tab = tab;
        bar.field.focused = true;
        bar.field.select_all();
        self.wins[w].editing = false;
        self.wins[w].address.focused = false;
        self.wins[w].dirty = true;
        self.focus_pages();
    }

    pub(super) fn close_find(&mut self, w: usize) {
        if let Some(bar) = self.wins[w].find.take() {
            self.tell(bar.tab, ToHost::Find { tab: bar.tab, text: String::new(), forward: true });
        }
        self.wins[w].dirty = true;
        self.focus_pages();
    }

    /// The next match (or the one before), the bar's text found again.
    pub(super) fn find_next(&mut self, w: usize, forward: bool) {
        let Some(bar) = self.wins[w].find.as_mut() else {
            self.open_find(w);
            return;
        };
        if bar.field.text.is_empty() {
            return;
        }
        if let Some(n) = bar.matches.filter(|n| *n > 0) {
            bar.at = if forward { bar.at % n + 1 } else { (bar.at + n - 2) % n + 1 };
        }
        let (tab, text) = (bar.tab, bar.field.text.clone());
        self.tell(tab, ToHost::Find { tab, text, forward });
        self.wins[w].dirty = true;
    }

    /// What the engine found.
    pub(super) fn found(&mut self, tab: TabId, matches: u32) {
        for win in &mut self.wins {
            if let Some(bar) = win.find.as_mut().filter(|b| b.tab == tab) {
                if bar.matches.is_none() || matches == 0 {
                    bar.at = if matches > 0 { 1 } else { 0 };
                }
                bar.matches = Some(matches);
                win.dirty = true;
            }
        }
    }

    /// A key while the bar has the keyboard. Whether it took it.
    pub(super) fn find_key(&mut self, w: usize, key: Key, text: Option<String>, m: Modifiers) -> bool {
        let Some(bar) = self.wins[w].find.as_mut().filter(|b| b.field.focused) else { return false };
        match key {
            Key::Escape => self.close_find(w),
            Key::Enter => self.find_next(w, !m.shift),
            _ => {
                let before = bar.field.text.clone();
                bar.field.input(&Input::Key { key, text, modifiers: m });
                if bar.field.text != before {
                    // Found anew from the top with each change.
                    bar.matches = None;
                    bar.at = 0;
                    let (tab, text) = (bar.tab, bar.field.text.clone());
                    self.tell(tab, ToHost::Find { tab, text, forward: true });
                }
            }
        }
        self.wins[w].dirty = true;
        true
    }

    /// A press over the bar. Whether it was.
    pub(super) fn find_press(&mut self, w: usize, x: f32, y: f32) -> bool {
        let Some(bar) = self.wins[w].find.as_mut() else { return false };
        if !bar.rect.contains(x, y) {
            if bar.field.focused {
                bar.field.focused = false;
                self.focus_pages();
            }
            return false;
        }
        if bar.close.contains(x, y) {
            self.close_find(w);
        } else if bar.up.contains(x, y) {
            self.find_next(w, false);
        } else if bar.down.contains(x, y) {
            self.find_next(w, true);
        } else {
            bar.field.focused = true;
            self.wins[w].editing = false;
            self.wins[w].address.focused = false;
            self.focus_pages();
        }
        self.wins[w].dirty = true;
        true
    }
}

/// The bar, at the top right of the page.
pub(super) fn draw_find(p: &mut Painter, gl: &spatiand_app::Gl, page: Rect, bar: &mut FindBar, hover: (f32, f32)) {
    let rect = Rect::new(page.right() - 640.0 - 16.0, page.y + 12.0, 640.0, 64.0);
    bar.rect = rect;
    p.fill(gl, rect, 18.0, theme::ground());
    look::glass(p, gl, rect, 18.0, 1.0);
    let mut inner = rect.inset(8.0);
    bar.close = inner.take_right(48.0);
    bar.down = inner.take_right(48.0);
    bar.up = inner.take_right(48.0);
    let said = bar.said();
    let said_w = if said.is_empty() { 0.0 } else { p.measure(gl, &said, theme::DETAIL_EM, 400.0).0 + 24.0 };
    let said_rect = inner.take_right(said_w);
    for (r, icon) in [(bar.up, Icon::Up), (bar.down, Icon::Down), (bar.close, Icon::Close)] {
        if r.contains(hover.0, hover.1) {
            p.fill(gl, r, 24.0, theme::wash(0.11));
        }
        draw_icon(p, gl, icon, r.inset(13.0), theme::ink_detail());
    }
    if !said.is_empty() {
        let ink = if bar.matches == Some(0) { look::trouble() } else { theme::ink_hint() };
        p.label(gl, &said, said_rect, theme::DETAIL_EM, ink, 0.5);
    }
    bar.field.draw(p, gl, inner);
}
