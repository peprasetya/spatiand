//! The Deck's menus in the Mac's room: the settings list, the launcher's bubbles, the window list, the
//! environment picker and the computers page.
//!
//! The behaviour is the Deck's own `spatiand-shell` state machine, unchanged: the same rows, the same words,
//! the same intents (up, down, accept, back) and the same events. This module is what the Deck's `scene.rs`
//! does with it, drawn on the CPU into pictures the room hangs in front of the wearer: a card for a list,
//! a glass bubble for each application. The numbers (card width, row heights, colours) are the Deck's.

use std::collections::HashMap;
use std::sync::Arc;

use spatiand_render::panel;
use spatiand_shell::{
    DesktopPanels, EnvironmentChoice, EnvironmentEntry, HostRow, HostTab, Intent, Mode, Shell,
    ShellEvent, WindowEntry,
};

use crate::chrome::{self, over};
use crate::menu_model::{self, MenuModel};

/// The card is this much of the horizontal field across, and may grow this much of the vertical before it
/// scrolls; it hangs this far away.
pub const CARD_FOV_FRACTION: f64 = 0.62;
pub const CARD_HEIGHT_FRACTION: f64 = 0.68;
pub const MENU_DISTANCE: f64 = 1.6;
/// How wide a launcher bubble is, metres.
pub const BUBBLE_DIAMETER_M: f64 = 0.16;
const TRAILING_EM: f32 = 23.0;

const CARD_GROUND: [f32; 4] = [0.043, 0.05, 0.072, 0.93];
const CARD_RIM: [f32; 4] = [0.55, 0.70, 1.0, 0.16];
const ROW_SELECTED: [f32; 4] = [0.42, 0.68, 1.0, 0.20];
const ROW_MARK: [f32; 4] = [0.45, 0.72, 1.0, 1.0];
const INK: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const INK_ROW: [f32; 4] = [0.78, 0.82, 0.90, 1.0];
const INK_TRAILING: [f32; 4] = [0.50, 0.70, 1.0, 1.0];
const INK_DETAIL: [f32; 4] = [0.70, 0.76, 0.87, 1.0];
const INK_HINT: [f32; 4] = [0.50, 0.55, 0.66, 1.0];
const SEPARATOR: [f32; 4] = [1.0, 1.0, 1.0, 0.10];
const SCROLL_TRACK: [f32; 4] = [1.0, 1.0, 1.0, 0.08];
const SCROLL_THUMB: [f32; 4] = [0.42, 0.68, 1.0, 0.70];

/// The panels the menus use, in the room's panel ids.
pub const CARD_ID: u32 = 0xFFF2;
/// A launcher bubble's glass is drawn by the renderer from its picture (the icon); the focused one has its own
/// range of ids so the renderer knows to light it. The names under them are plain panels.
pub const BUBBLE_FIRST: u32 = 0xFF10;
pub const BUBBLE_FOCUSED_FIRST: u32 = 0xFF20;
pub const DOT_FIRST: u32 = 0xFF30;
pub const LABEL_FIRST: u32 = 0xFF40;
/// The launcher's search field, and the line that says nothing was found.
pub const SEARCH_ID: u32 = 0xFFF4;
/// The status line (time, battery, windows), held to the head in the upper left as the Deck's is.
pub const STATUS_ID: u32 = 0xFFF6;
pub const SEARCH_NOTE_ID: u32 = 0xFFF5;
/// The field's picture and where it hangs: across the top of the launcher's view, with the bubbles a little
/// lower than the Deck puts them to make the room. Numbers set by the glasses' 23 degrees of height: the
/// field's top edge and the last row's name both stay inside it.
const SEARCH_W_PX: u32 = 1300;
const SEARCH_H_PX: u32 = 124;
const SEARCH_WIDTH_M: f64 = 0.54;
const SEARCH_PITCH_DEG: f64 = 9.1;
const SEARCH_DROP_DEG: f64 = 0.6;
/// The rows are a little closer than the Deck's, to leave the field's top for the search.
const SEARCH_ROW_SQUEEZE: f64 = 0.9;

/// The card's picture is this many pixels for each of the Deck's logical ones.
const CARD_SCALE: f32 = 1.5;
/// A bubble's picture, and the room under its disc for the label.
const BUBBLE_PX: u32 = 256;

/// What a pointer or a press found on a menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A row of the card or a bubble, by the shell's index.
    Row(usize),
    /// The card's title line, or the launcher's empty sky.
    Back,
}

/// One picture of a panel.
#[derive(Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

/// What the room has to hang for a menu: each panel, its size in metres and where it is, and its picture.
#[derive(Clone)]
pub struct PanelSpec {
    pub id: u32,
    pub image: Image,
    pub width_m: f64,
    /// Offsets from the menu's anchor: round, up, and how far out.
    pub yaw: f64,
    pub pitch: f64,
    pub radius: f64,
}

pub struct ShellUi {
    pub shell: Shell,
    /// Where the menu is: the way the wearer was facing when it opened.
    pub anchor: (f64, f64),
    /// Where the list was scrolled to last time, so it follows the cursor and does not slide under it.
    first: usize,
    /// Pictures of applications, by the id the launcher names them with.
    icons: HashMap<String, (u32, u32, Arc<Vec<u8>>)>,
    layout: Option<panel::Layout>,
    /// Which panel is which bubble.
    pub bubbles: HashMap<u32, usize>,
    /// Pictures of what was drawn last, and a number that changes with them.
    pub images: HashMap<u32, PanelSpec>,
    pub version: u64,
    pub dirty: bool,
    /// The status line's words: time, battery, how many windows.
    status: String,
}

impl Default for ShellUi {
    fn default() -> Self {
        ShellUi::new()
    }
}

impl ShellUi {
    pub fn new() -> Self {
        ShellUi {
            // Nothing local to launch: a Mac's applications are another computer's tab, and calibration is
            // the glasses' business, settled once. Wi-Fi and Bluetooth are System Settings' panes, floated in
            // the room as the Deck floats its own panels.
            shell: Shell::new(Vec::new(), DesktopPanels::ALL, true),
            anchor: (0.0, 0.0),
            status: String::new(),
            first: 0,
            icons: HashMap::new(),
            layout: None,
            bubbles: HashMap::new(),
            images: HashMap::new(),
            version: 0,
            dirty: true,
        }
    }

    pub fn mode(&self) -> Mode {
        self.shell.mode()
    }

    pub fn open(&self) -> bool {
        self.shell.menu_is_open()
    }

    /// An application's picture, by the id the launcher gives it.
    pub fn set_icon(&mut self, id: &str, width: u32, height: u32, rgba: Vec<u8>) {
        if width > 0 && height > 0 && rgba.len() >= (width * height * 4) as usize {
            self.icons.insert(id.to_string(), (width, height, Arc::new(rgba)));
            self.dirty = true;
        }
    }

    /// The computers and what each offers, for the launcher and the computers page.
    pub fn set_hosts(&mut self, rows: Vec<HostRow>, tabs: Vec<HostTab>) {
        self.shell.set_hosts(rows, tabs);
        self.dirty = true;
    }

    pub fn set_environment_entries(&mut self, entries: Vec<EnvironmentEntry>, current: EnvironmentChoice) {
        self.shell.set_environments(entries, current);
        self.dirty = true;
    }

    pub fn set_windows(&mut self, windows: Vec<WindowEntry>, open_list: bool) {
        if open_list {
            self.shell.set_windows(windows);
        } else {
            self.shell.refresh_windows(windows);
        }
        self.dirty = true;
    }

    /// Feed one intent in. Marks the pictures out of date.
    pub fn handle(&mut self, intent: Intent) -> Option<ShellEvent> {
        self.dirty = true;
        self.shell.handle(intent)
    }

    /// The pointer is over this row or bubble: the shell's cursor goes there, as D-pad would take it.
    pub fn point(&mut self, index: usize) -> bool {
        let moved = self.shell.point(index);
        if moved {
            self.dirty = true;
        }
        moved
    }

    /// What the pointer is on at a point of a menu panel, in that panel's own pixels.
    pub fn target(&self, panel_id: u32, x: f64, y: f64) -> Option<Target> {
        if panel_id == CARD_ID {
            let layout = self.layout.as_ref()?;
            let (lx, ly) = ((x as f32 / CARD_SCALE) - panel::RIM, (y as f32 / CARD_SCALE) - panel::RIM);
            if lx >= layout.title.x && lx <= layout.title.x + layout.title.w && ly >= layout.title.y && ly <= layout.title.y + layout.title.h {
                return Some(Target::Back);
            }
            for (offset, rect) in layout.rows.iter().enumerate() {
                if lx >= rect.x && lx <= rect.x + rect.w && ly >= rect.y && ly <= rect.y + rect.h {
                    return Some(Target::Row(layout.first + offset));
                }
            }
            return None;
        }
        let index = *self.bubbles.get(&panel_id)?;
        // A bubble is round: the corners of its square are sky.
        let size = BUBBLE_PX as f64;
        let (dx, dy) = (x - size * 0.5, y - size * 0.5);
        (dx * dx + dy * dy <= size * size * 0.25).then_some(Target::Row(index))
    }

    /// Draw every panel the open menu has, and say whether anything changed. Slow: it sets text, so it
    /// is called with the room unlocked, on a snapshot of what it needs.
    pub fn render(&mut self, fov: (f64, f64)) -> bool {
        if !self.dirty {
            return false;
        }
        self.dirty = false;
        self.images.clear();
        self.bubbles.clear();
        self.layout = None;
        let launcher = self.shell.launcher();
        if self.shell.mode() == Mode::Launcher && (!launcher.is_empty() || !launcher.query().is_empty()) {
            self.render_launcher();
        } else if let Some(model) = menu_model::model(&self.shell) {
            self.render_card(&model, fov);
        }
        if self.shell.mode() != Mode::Launcher {
            self.render_status();
        }
        self.version += 1;
        true
    }

    /// What the status line says; drawn again when it changes.
    pub fn set_status(&mut self, text: &str) {
        if self.status != text {
            self.status = text.to_string();
            self.dirty = true;
        }
    }

    /// The Deck's status bar: a small dark plate with the line on it, in the upper left of the view, held to the head.
    /// Sized by its width, as the Deck does -- 15 degrees across at 1.5 metres -- so a long line is a smaller one.
    fn render_status(&mut self) {
        if self.status.is_empty() {
            return;
        }
        let h = 64u32;
        let text = chrome::text_image(&self.status, h as f32 * 0.5, 1400, [214, 226, 248, 255]);
        if text.is_empty() {
            return;
        }
        let w = text.width + 40;
        let mut canvas = vec![0u8; (w * h * 4) as usize];
        round_rect(&mut canvas, w, h, 0.0, 0.0, w as f32, h as f32, h as f32 * 0.3, [0.02, 0.03, 0.06, 0.55]);
        chrome::blit(&mut canvas, w, h, &text.rgba, text.width, text.height, 20.0, (h as f32 - text.height as f32) * 0.5, text.width as f32, text.height as f32, [1.0; 4]);
        self.images.insert(STATUS_ID, PanelSpec {
            id: STATUS_ID,
            image: Image { width: w, height: h, rgba: Arc::new(premultiply(canvas)) },
            width_m: 2.0 * 1.5 * (7.5f64).to_radians().tan(),
            yaw: 8.5f64.to_radians(),
            pitch: 10.2f64.to_radians(),
            radius: 1.5,
        });
    }

    fn render_card(&mut self, model: &MenuModel, fov: (f64, f64)) {
        let scale = CARD_SCALE;
        let device = |logical: f32| (logical * scale).max(1.0);
        let content_px = (panel::TEXT_WIDTH * scale) as u32;
        let ink = |c: [f32; 4]| [(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, (c[3] * 255.0) as u8];
        // One line, cropped to its ink, and given far more width than it can use so it never wraps.
        let line = |s: &str, em: f32, c: [f32; 4]| chrome::text_image(s, device(em), content_px * 4, ink(c));

        let title = line(&model.title, panel::TITLE_EM, INK);
        let rows: Vec<(spatiand_render::TextImage, Option<spatiand_render::TextImage>, spatiand_render::TextImage)> = model
            .rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let selected = i == model.cursor;
                (
                    line(&row.label, panel::ROW_EM, if selected { INK } else { INK_ROW }),
                    row.trailing.as_deref().map(|t| line(t, TRAILING_EM, INK_TRAILING)),
                    // Selected rows are lit; the other colour is not drawn until the cursor moves there.
                    line(&row.label, panel::ROW_EM, INK),
                )
            })
            .collect();
        let detail = (!model.detail.is_empty()).then(|| chrome::text_block(&model.detail, device(panel::DETAIL_EM), content_px, ink(INK_DETAIL)));
        let footer = (!model.footer.is_empty()).then(|| line(&model.footer, panel::FOOTER_EM, INK_HINT));
        let detail_height = detail.as_ref().map(|d| d.height as f32 / scale).unwrap_or(0.0);

        let card_deg = (fov.0 * CARD_FOV_FRACTION) as f32;
        let budget = (fov.1 * CARD_HEIGHT_FRACTION) as f32 * (panel::WIDTH / card_deg);
        let layout = panel::Layout::new(
            &panel::Menu { rows: model.rows.len(), cursor: model.cursor, detail_height, footer: footer.is_some(), budget_height: budget },
            self.first,
        );
        self.first = layout.first;

        let rim = panel::RIM;
        let (cw, ch) = (((panel::WIDTH + rim * 2.0) * scale).ceil() as u32, ((layout.height + rim * 2.0) * scale).ceil() as u32);
        let mut canvas = vec![0u8; (cw * ch * 4) as usize];
        // A rectangle in logical pixels of the card.
        let fill = |canvas: &mut Vec<u8>, r: panel::Rect, c: [f32; 4], radius: f32| {
            round_rect(canvas, cw, ch, (r.x + rim) * scale, (r.y + rim) * scale, r.w * scale, r.h * scale, radius * scale, c);
        };
        round_rect(&mut canvas, cw, ch, 0.0, 0.0, cw as f32, ch as f32, (panel::CARD_RADIUS + rim) * scale, CARD_RIM);
        fill(&mut canvas, panel::Rect { x: 0.0, y: 0.0, w: panel::WIDTH, h: layout.height }, CARD_GROUND, panel::CARD_RADIUS);

        // One piece of text inside a band, at its left or right edge, vertically centred; a label wider
        // than its band is scaled down rather than cut, because the end of a filename says what it is.
        let text_in = |canvas: &mut Vec<u8>, band: panel::Rect, image: &spatiand_render::TextImage, right_edge: bool| {
            if image.is_empty() {
                return;
            }
            let (bw, bh) = (band.w * scale, band.h * scale);
            let (mut w, mut h) = (image.width as f32, image.height as f32);
            if w > bw {
                h *= bw / w;
                w = bw;
            }
            let x = if right_edge { band.x * scale + bw - w } else { band.x * scale };
            let y = band.y * scale + (bh - h) * 0.5;
            chrome::blit(canvas, cw, ch, &image.rgba, image.width, image.height, x + rim * scale, y + rim * scale, w, h, [1.0; 4]);
        };
        text_in(&mut canvas, layout.title, &title, false);
        if let (Some(band), Some(image)) = (layout.footer, footer.as_ref()) {
            text_in(&mut canvas, band, image, true);
        }
        for (offset, rect) in layout.rows.iter().enumerate() {
            let index = layout.first + offset;
            let Some((label, trailing, _)) = rows.get(index) else { continue };
            if index == model.cursor {
                fill(&mut canvas, *rect, ROW_SELECTED, panel::ROW_RADIUS);
                fill(&mut canvas, panel::Rect { x: rect.x + 9.0, y: rect.y + rect.h * 0.26, w: 5.0, h: rect.h * 0.48 }, ROW_MARK, 2.5);
            }
            let mut label_width = rect.w - panel::ROW_INSET * 2.0;
            if let Some(trail) = trailing {
                let band = panel::Rect { x: rect.x + panel::ROW_INSET, w: rect.w - panel::ROW_INSET * 2.0, ..*rect };
                text_in(&mut canvas, band, trail, true);
                label_width = (label_width - trail.width as f32 / scale - panel::ROW_INSET).max(panel::ROW_INSET);
            }
            text_in(&mut canvas, panel::Rect { x: rect.x + panel::ROW_INSET, w: label_width, ..*rect }, label, false);
        }
        if let Some(track) = layout.scroll_track {
            fill(&mut canvas, track, SCROLL_TRACK, track.w * 0.5);
        }
        if let Some(thumb) = layout.scroll_thumb {
            fill(&mut canvas, thumb, SCROLL_THUMB, thumb.w * 0.5);
        }
        if let Some(sep) = layout.separator {
            fill(&mut canvas, sep, SEPARATOR, sep.h * 0.5);
        }
        if let (Some(rect), Some(image)) = (layout.detail, detail.as_ref()) {
            chrome::blit(&mut canvas, cw, ch, &image.rgba, image.width, image.height, (rect.x + rim) * scale, (rect.y + rim) * scale, image.width as f32, image.height as f32, [1.0; 4]);
        }

        // Metres: the card is this much of the field across at its distance.
        let card_width = 2.0 * MENU_DISTANCE * (fov.0 * CARD_FOV_FRACTION / 2.0).to_radians().tan();
        let width_m = card_width * (panel::WIDTH + rim * 2.0) as f64 / panel::WIDTH as f64;
        self.layout = Some(layout);
        self.images.insert(
            CARD_ID,
            PanelSpec { id: CARD_ID, image: Image { width: cw, height: ch, rgba: Arc::new(premultiply(canvas)) }, width_m, yaw: 0.0, pitch: 0.0, radius: MENU_DISTANCE },
        );
    }

    fn render_launcher(&mut self) {
        let launcher = self.shell.launcher();
        let bubbles = launcher.bubbles();
        let placements = launcher.placements();
        let cursor = launcher.cursor();
        let pages = launcher.pages();
        let page = launcher.page();
        let (query, searched) = (launcher.query().to_string(), launcher.searched());
        let drop = SEARCH_DROP_DEG.to_radians();
        let mut specs = Vec::new();
        for (index, placement) in &placements {
            let Some((label, icon_name)) = bubbles.get(*index) else { continue };
            // The application's own picture, or the computer or group's drawn one, or its first letter.
            let icon = self
                .icons
                .get(label)
                .or_else(|| icon_name.as_ref().and_then(|n| self.icons.get(n)))
                .cloned()
                .or_else(|| icon_name.as_deref().and_then(crate::theme_icons::themed).map(|(w, h, px)| (w, h, Arc::new(px))));
            let focused = *index == cursor;
            let diameter = BUBBLE_DIAMETER_M * placement.scale as f64;
            let id = if focused { BUBBLE_FOCUSED_FIRST } else { BUBBLE_FIRST } + *index as u32 % 16;
            specs.push((id, *index, PanelSpec {
                id,
                image: Image { width: BUBBLE_PX, height: BUBBLE_PX, rgba: Arc::new(icon_image(label, icon)) },
                width_m: diameter,
                yaw: placement.yaw as f64,
                pitch: placement.pitch as f64 * SEARCH_ROW_SQUEEZE - drop,
                radius: placement.radius as f64,
            }));
            // The name, clear of the glass even when it is the focused one and larger.
            let (lw, lh, label_rgba) = label_image(label, focused);
            let label_height_m = LABEL_HEIGHT_M;
            let below = diameter * 0.5 + 0.010 + label_height_m * 0.5;
            let lid = LABEL_FIRST + *index as u32 % 16;
            self.images.insert(lid, PanelSpec {
                id: lid,
                image: Image { width: lw, height: lh, rgba: Arc::new(label_rgba) },
                width_m: label_height_m * lw as f64 / lh as f64,
                yaw: placement.yaw as f64,
                pitch: placement.pitch as f64 * SEARCH_ROW_SQUEEZE - drop - below / placement.radius as f64,
                radius: placement.radius as f64,
            });
        }
        for (id, index, spec) in specs {
            self.bubbles.insert(id, index);
            self.images.insert(id, spec);
        }
        // The search field, always there: it says that typing does something, and what it has caught.
        let (sw, sh, search) = search_image(&query, bubbles.len(), searched);
        self.images.insert(SEARCH_ID, PanelSpec {
            id: SEARCH_ID,
            image: Image { width: sw, height: sh, rgba: Arc::new(search) },
            width_m: SEARCH_WIDTH_M,
            yaw: 0.0,
            pitch: SEARCH_PITCH_DEG.to_radians(),
            radius: spatiand_shell::launcher::ARC_RADIUS_M as f64,
        });
        if bubbles.is_empty() && !query.is_empty() {
            let (nw, nh, note) = label_image(&format!("No application called \u{201c}{query}\u{201d}"), true);
            self.images.insert(SEARCH_NOTE_ID, PanelSpec {
                id: SEARCH_NOTE_ID,
                image: Image { width: nw, height: nh, rgba: Arc::new(note) },
                width_m: LABEL_HEIGHT_M * 1.3 * nw as f64 / nh as f64,
                yaw: 0.0,
                pitch: -drop,
                radius: spatiand_shell::launcher::ARC_RADIUS_M as f64,
            });
        }
        // Page dots, so a paged grid is not mistaken for a short one. A long list has too many for the
        // field's height -- two hundred applications are seventeen pages -- and says where it is in words.
        if pages > 6 {
            let (pw, ph, note) = label_image(&format!("{} / {}", page + 1, pages), false);
            self.images.insert(DOT_FIRST, PanelSpec {
                id: DOT_FIRST,
                image: Image { width: pw, height: ph, rgba: Arc::new(note) },
                width_m: LABEL_HEIGHT_M * pw as f64 / ph as f64,
                yaw: -(9.0f64 * 1.9).to_radians(),
                pitch: -drop,
                radius: spatiand_shell::launcher::ARC_RADIUS_M as f64,
            });
        } else if pages > 1 {
            for p in 0..pages.min(8) {
                let size = if p == page { 40 } else { 24 };
                let mut canvas = vec![0u8; (size * size * 4) as usize];
                chrome::disc(&mut canvas, size, size, size as f32 * 0.5, size as f32 * 0.5, size as f32 * 0.5 - 1.0, [0.78, 0.86, 1.0, if p == page { 0.95 } else { 0.40 }]);
                let offset = p as f64 - (pages as f64 - 1.0) * 0.5;
                let id = DOT_FIRST + p as u32;
                self.images.insert(id, PanelSpec {
                    id,
                    image: Image { width: size, height: size, rgba: Arc::new(premultiply(canvas)) },
                    width_m: if p == page { 0.020 } else { 0.012 },
                    yaw: -(9.0f64 * 2.2).to_radians(),
                    pitch: offset * 3.2f64.to_radians(),
                    radius: spatiand_shell::launcher::ARC_RADIUS_M as f64,
                });
            }
        }
    }
}

/// A rounded rectangle, anti-aliased, over the canvas. Pixel units.
#[allow(clippy::too_many_arguments)]
fn round_rect(canvas: &mut [u8], cw: u32, ch: u32, x: f32, y: f32, w: f32, h: f32, radius: f32, colour: [f32; 4]) {
    let radius = radius.min(w * 0.5).min(h * 0.5).max(0.0);
    let (x0, y0) = (x.floor().max(0.0) as i32, y.floor().max(0.0) as i32);
    let (x1, y1) = ((x + w).ceil().min(cw as f32) as i32, (y + h).ceil().min(ch as f32) as i32);
    let (cx, cy) = (x + w * 0.5, y + h * 0.5);
    let (hx, hy) = (w * 0.5 - radius, h * 0.5 - radius);
    for py in y0..y1 {
        for px in x0..x1 {
            let (dx, dy) = ((px as f32 + 0.5 - cx).abs() - hx, (py as f32 + 0.5 - cy).abs() - hy);
            let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() + dx.max(dy).min(0.0) - radius;
            let cover = (0.5 - outside).clamp(0.0, 1.0);
            if cover > 0.0 {
                let i = ((py as u32 * cw + px as u32) * 4) as usize;
                over(&mut canvas[i..i + 4], [colour[0], colour[1], colour[2], colour[3] * cover]);
            }
        }
    }
}

/// Straight alpha to premultiplied, in the order Metal's BGRA wants.
pub fn premultiply(mut rgba: Vec<u8>) -> Vec<u8> {
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        let (r, g, b) = (px[0] as u32 * a / 255, px[1] as u32 * a / 255, px[2] as u32 * a / 255);
        px[0] = b as u8;
        px[1] = g as u8;
        px[2] = r as u8;
    }
    rgba
}

/// A shape's coverage of a pixel, from its signed distance: the edge is half a pixel soft.
fn cover(distance: f32) -> f32 {
    (0.5 - distance).clamp(0.0, 1.0)
}

/// A rounded rectangle's outline, `stroke` thick inside its edge, brighter at the top than the bottom.
#[allow(clippy::too_many_arguments)]
fn round_rect_ring(canvas: &mut [u8], cw: u32, ch: u32, x: f32, y: f32, w: f32, h: f32, radius: f32, stroke: f32, colour: [f32; 4]) {
    let (cx, cy) = (x + w * 0.5, y + h * 0.5);
    let (hx, hy) = (w * 0.5 - radius, h * 0.5 - radius);
    for py in (y.floor().max(0.0) as u32)..((y + h).ceil().min(ch as f32) as u32) {
        for px in (x.floor().max(0.0) as u32)..((x + w).ceil().min(cw as f32) as u32) {
            let (dx, dy) = ((px as f32 + 0.5 - cx).abs() - hx, (py as f32 + 0.5 - cy).abs() - hy);
            let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() + dx.max(dy).min(0.0) - radius;
            let c = cover((outside + stroke * 0.5).abs() - stroke * 0.5);
            if c > 0.0 {
                let light = 1.0 - 0.6 * ((py as f32 - y) / h).clamp(0.0, 1.0);
                let i = ((py * cw + px) * 4) as usize;
                over(&mut canvas[i..i + 4], [colour[0], colour[1], colour[2], colour[3] * c * light]);
            }
        }
    }
}

/// A circle's outline.
fn ring(canvas: &mut [u8], cw: u32, ch: u32, cx: f32, cy: f32, radius: f32, stroke: f32, colour: [f32; 4]) {
    let reach = radius + stroke;
    for py in ((cy - reach).floor().max(0.0) as u32)..((cy + reach).ceil().min(ch as f32) as u32) {
        for px in ((cx - reach).floor().max(0.0) as u32)..((cx + reach).ceil().min(cw as f32) as u32) {
            let d = ((px as f32 + 0.5 - cx).powi(2) + (py as f32 + 0.5 - cy).powi(2)).sqrt();
            let c = cover((d - radius).abs() - stroke * 0.5);
            if c > 0.0 {
                let i = ((py * cw + px) * 4) as usize;
                over(&mut canvas[i..i + 4], [colour[0], colour[1], colour[2], colour[3] * c]);
            }
        }
    }
}

/// A line with round ends.
#[allow(clippy::too_many_arguments)]
fn stroke_line(canvas: &mut [u8], cw: u32, ch: u32, a: (f32, f32), b: (f32, f32), width: f32, colour: [f32; 4]) {
    let (lo, hi) = ((a.0.min(b.0) - width) as i32, (a.0.max(b.0) + width) as i32);
    let (top, bottom) = ((a.1.min(b.1) - width) as i32, (a.1.max(b.1) + width) as i32);
    let (vx, vy) = (b.0 - a.0, b.1 - a.1);
    let length2 = (vx * vx + vy * vy).max(1e-6);
    for py in top.max(0)..bottom.min(ch as i32) {
        for px in lo.max(0)..hi.min(cw as i32) {
            let (qx, qy) = (px as f32 + 0.5 - a.0, py as f32 + 0.5 - a.1);
            let t = ((qx * vx + qy * vy) / length2).clamp(0.0, 1.0);
            let d = ((qx - vx * t).powi(2) + (qy - vy * t).powi(2)).sqrt();
            let c = cover(d - width * 0.5);
            if c > 0.0 {
                let i = ((py as u32 * cw + px as u32) * 4) as usize;
                over(&mut canvas[i..i + 4], [colour[0], colour[1], colour[2], colour[3] * c]);
            }
        }
    }
}

/// The launcher's search field: a pane of glass with a magnifying glass, what has been typed with its
/// caret (or a hint that typing is what to do), and on the right how much it has caught.
fn search_image(query: &str, found: usize, searched: usize) -> (u32, u32, Vec<u8>) {
    let (w, h) = (SEARCH_W_PX, SEARCH_H_PX);
    let (wf, hf) = (w as f32, h as f32);
    let mut canvas = vec![0u8; (w * h * 4) as usize];
    let radius = hf * 0.5;
    // Glass: dark ground, a lighter top where the room's light falls, a rim that is lit from above.
    round_rect(&mut canvas, w, h, 0.0, 0.0, wf, hf, radius, [0.035, 0.05, 0.085, 0.74]);
    round_rect(&mut canvas, w, h, hf * 0.5, 4.0, wf - hf, hf * 0.36, hf * 0.18, [0.60, 0.76, 1.0, 0.09]);
    let focus = if query.is_empty() { 0.30 } else { 0.62 };
    round_rect_ring(&mut canvas, w, h, 1.0, 1.0, wf - 2.0, hf - 2.0, radius - 1.0, 2.5, [0.62, 0.78, 1.0, focus]);
    // The magnifying glass.
    let ink = [0.62, 0.78, 1.0, if query.is_empty() { 0.65 } else { 0.95 }];
    let (lx, ly, lr) = (hf * 0.72, hf * 0.45, hf * 0.17);
    ring(&mut canvas, w, h, lx, ly, lr, hf * 0.065, ink);
    let handle = std::f32::consts::FRAC_1_SQRT_2;
    stroke_line(&mut canvas, w, h, (lx + lr * handle, ly + lr * handle), (lx + lr * handle + hf * 0.17, ly + lr * handle + hf * 0.17), hf * 0.075, ink);
    // How much it caught, at the right.
    let count = match (query.is_empty(), searched) {
        (_, 0) => String::new(),
        (true, n) => format!("{n} apps"),
        (false, n) => format!("{found} of {n}"),
    };
    let mut right = wf - hf * 0.6;
    if !count.is_empty() {
        let tint = if !query.is_empty() && found == 0 { [255, 150, 130, 235] } else { [140, 178, 255, if query.is_empty() { 150 } else { 235 }] };
        let t = chrome::text_image(&count, hf * 0.40, 360, tint);
        if !t.is_empty() {
            right -= t.width as f32;
            chrome::blit(&mut canvas, w, h, &t.rgba, t.width, t.height, right, (hf - t.height as f32) * 0.5, t.width as f32, t.height as f32, [1.0; 4]);
            right -= hf * 0.35;
        }
    }
    let left = hf * 1.28;
    let room = (right - left).max(40.0) as u32;
    if query.is_empty() {
        let t = chrome::text_image("Type to search", hf * 0.46, room, [150, 164, 190, 200]);
        if !t.is_empty() {
            chrome::blit(&mut canvas, w, h, &t.rgba, t.width, t.height, left + 6.0, (hf - t.height as f32) * 0.5, t.width as f32, t.height as f32, [1.0; 4]);
        }
        // The caret waits at the start of it.
        round_rect(&mut canvas, w, h, left - 6.0, hf * 0.22, 3.5, hf * 0.56, 1.75, [0.50, 0.74, 1.0, 0.95]);
    } else {
        // A long search shows its end, where the typing is.
        let mut shown: String = query.to_string();
        let mut t = chrome::text_image(&shown, hf * 0.50, 4096, [245, 248, 255, 255]);
        while t.width > room && shown.chars().count() > 1 {
            shown.remove(0);
            t = chrome::text_image(&format!("\u{2026}{shown}"), hf * 0.50, 4096, [245, 248, 255, 255]);
        }
        if !t.is_empty() {
            chrome::blit(&mut canvas, w, h, &t.rgba, t.width, t.height, left, (hf - t.height as f32) * 0.5, t.width as f32, t.height as f32, [1.0; 4]);
            round_rect(&mut canvas, w, h, left + t.width as f32 + 5.0, hf * 0.22, 3.5, hf * 0.56, 1.75, [0.50, 0.74, 1.0, 0.95]);
        }
    }
    (w, h, premultiply(canvas))
}

/// How tall a bubble's name is, metres.
const LABEL_HEIGHT_M: f64 = 0.034;

/// What goes inside a bubble's glass: the application's picture over the whole square, or its first letter.
/// The glass itself is the renderer's, as the Deck's is: it refracts the room behind it.
fn icon_image(label: &str, icon: Option<(u32, u32, Arc<Vec<u8>>)>) -> Vec<u8> {
    let mut canvas = vec![0u8; (BUBBLE_PX * BUBBLE_PX * 4) as usize];
    let side = BUBBLE_PX as f32;
    match icon {
        Some((iw, ih, pixels)) => chrome::blit(&mut canvas, BUBBLE_PX, BUBBLE_PX, &pixels, iw, ih, 0.0, 0.0, side, side, [1.0; 4]),
        None => {
            let initial: String = label.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
            let image = chrome::text_image(&initial, side * 0.6, 256, [255, 255, 255, 235]);
            if !image.is_empty() {
                let (iw, ih) = (image.width as f32, image.height as f32);
                chrome::blit(&mut canvas, BUBBLE_PX, BUBBLE_PX, &image.rgba, image.width, image.height, (side - iw) * 0.5, (side - ih) * 0.5, iw, ih, [1.0; 4]);
            }
        }
    }
    premultiply(canvas)
}

/// A bubble's name on a plate of dark glass, so it reads over a page as well as over the sky.
fn label_image(label: &str, focused: bool) -> (u32, u32, Vec<u8>) {
    let h = 64u32;
    let name = chrome::text_image(label, h as f32 * 0.52, 1024, [226, 234, 250, if focused { 255 } else { 215 }]);
    if name.is_empty() {
        return (2, 2, vec![0; 16]);
    }
    let w = name.width + 40;
    let mut canvas = vec![0u8; (w * h * 4) as usize];
    round_rect(&mut canvas, w, h, 0.0, 0.0, w as f32, h as f32, h as f32 * 0.5, [0.04, 0.05, 0.08, 0.72]);
    let y = (h as f32 - name.height as f32) * 0.5;
    chrome::blit(&mut canvas, w, h, &name.rgba, name.width, name.height, 20.0, y, name.width as f32, name.height as f32, [1.0; 4]);
    (w, h, premultiply(canvas))
}

#[cfg(test)]
mod tests {
    use super::*;
    use spatiand_shell::NavDirection;

    #[test]
    fn the_settings_list_is_the_decks_and_has_a_card_with_rows() {
        let mut ui = ShellUi::new();
        ui.handle(Intent::ToggleHud);
        assert_eq!(ui.mode(), Mode::Hud);
        assert!(ui.render((40.0, 22.5)));
        let card = ui.images.get(&CARD_ID).expect("a card");
        assert!(card.image.width > 1000 && card.image.height > 300);
        let opaque = card.image.rgba.chunks_exact(4).filter(|p| p[3] > 200).count();
        assert!(opaque > card.image.rgba.len() / 8, "most of a card is ground");
        // Pointing at the first row, and at the title.
        let layout = ui.layout.as_ref().unwrap();
        let row = layout.rows[0];
        let at = |lx: f32, ly: f32| ui.target(CARD_ID, ((lx + panel::RIM) * CARD_SCALE) as f64, ((ly + panel::RIM) * CARD_SCALE) as f64);
        assert_eq!(at(row.x + row.w * 0.5, row.y + row.h * 0.5), Some(Target::Row(0)));
        assert_eq!(at(layout.title.x + 10.0, layout.title.y + 10.0), Some(Target::Back));
    }

    #[test]
    fn a_launcher_with_computers_is_bubbles_and_pointing_at_one_moves_the_cursor() {
        let mut ui = ShellUi::new();
        let tabs = vec![
            HostTab { label: "deepmagpie".into(), address: "deepmagpie".into(), online: true, apps: vec![] },
            HostTab { label: "This Mac".into(), address: "mac".into(), online: true, apps: vec![] },
        ];
        ui.set_hosts(Vec::new(), tabs);
        ui.handle(Intent::ToggleLauncher);
        assert!(ui.render((40.0, 22.5)));
        assert_eq!(ui.bubbles.len(), 2, "a bubble for each computer");
        let (&id, &index) = ui.bubbles.iter().find(|(_, i)| **i == 1).unwrap();
        assert_eq!(ui.target(id, BUBBLE_PX as f64 / 2.0, BUBBLE_PX as f64 / 2.0), Some(Target::Row(1)));
        assert_eq!(ui.target(id, 2.0, 2.0), None, "the corner of a bubble's square is sky");
        assert!(ui.point(index));
        assert_eq!(ui.shell.launcher().cursor(), 1);
        // Down into the focused one by accepting; back out by B.
        ui.handle(Intent::Accept);
        assert!(matches!(ui.shell.launcher().level(), spatiand_shell::Level::Host(1)));
        ui.handle(Intent::Back);
        ui.handle(Intent::Navigate(NavDirection::Left));
    }

    fn mac_tab(n: usize) -> HostTab {
        let names = ["Activity Monitor", "App Store", "Calculator", "Calendar", "Google Chrome", "Google Sheets", "Safari", "Slack", "Terminal", "TextEdit", "Visual Studio Code", "Xcode", "Claude", "Finder", "Mail", "Maps"];
        HostTab {
            label: "This Mac".into(),
            address: "mac".into(),
            online: true,
            apps: (0..n).map(|i| spatiand_shell::RemoteEntry { id: format!("a{i}"), name: if i < names.len() { names[i].into() } else { format!("Application {i}") }, icon: None }).collect(),
        }
    }

    /// What the wearer sees, as one picture of the glasses' field: every panel at its angle. With
    /// `SPATIAND_PREVIEW=dir` set the pictures are written there; otherwise this only checks the fit.
    fn view_of(ui: &ShellUi, name: &str) {
        const PPD: f64 = 40.0;
        let (w, h) = ((40.0 * PPD) as usize, (23.0 * PPD) as usize);
        let mut out = vec![0u8; w * h * 4];
        for px in out.chunks_exact_mut(4) {
            px.copy_from_slice(&[26, 22, 20, 255]);
        }
        let mut ids: Vec<_> = ui.images.keys().copied().collect();
        ids.sort();
        for id in ids {
            let spec = &ui.images[&id];
            let (aw, ah) = (spec.width_m / spec.radius * 57.2958 * PPD, spec.width_m / spec.radius * 57.2958 * PPD * spec.image.height as f64 / spec.image.width as f64);
            let (cx, cy) = (w as f64 * 0.5 - spec.yaw * 57.2958 * PPD, h as f64 * 0.5 - spec.pitch * 57.2958 * PPD);
            let (x0, y0) = (cx - aw * 0.5, cy - ah * 0.5);
            assert!(x0 >= -1.0 && x0 + aw <= w as f64 + 1.0 && y0 >= -1.0 && y0 + ah <= h as f64 + 1.0, "{name}: panel {id:#x} leaves the field: {x0:.0},{y0:.0} {aw:.0}x{ah:.0} of {w}x{h}");
            for y in (y0.max(0.0) as usize)..((y0 + ah).min(h as f64) as usize) {
                for x in (x0.max(0.0) as usize)..((x0 + aw).min(w as f64) as usize) {
                    let (sx, sy) = (((x as f64 - x0) / aw * spec.image.width as f64) as usize, ((y as f64 - y0) / ah * spec.image.height as f64) as usize);
                    let s = &spec.image.rgba[(sy.min(spec.image.height as usize - 1) * spec.image.width as usize + sx.min(spec.image.width as usize - 1)) * 4..][..4];
                    let d = &mut out[(y * w + x) * 4..][..4];
                    let a = s[3] as u32;
                    // Premultiplied BGRA over the room.
                    d[0] = (s[2] as u32 + d[0] as u32 * (255 - a) / 255).min(255) as u8;
                    d[1] = (s[1] as u32 + d[1] as u32 * (255 - a) / 255).min(255) as u8;
                    d[2] = (s[0] as u32 + d[2] as u32 * (255 - a) / 255).min(255) as u8;
                }
            }
        }
        if let Ok(dir) = std::env::var("SPATIAND_PREVIEW") {
            let mut raw = format!("{w} {h}\n").into_bytes();
            raw.extend_from_slice(&out);
            std::fs::write(format!("{dir}/{name}.rgba"), raw).unwrap();
        }
    }

    #[test]
    fn the_launchers_search_field_and_bubbles_fit_the_glasses_view() {
        let mut ui = ShellUi::new();
        ui.set_hosts(vec![], vec![mac_tab(200)]);
        ui.handle(Intent::ToggleLauncher);
        ui.handle(Intent::Accept); // into This Mac: 200 applications
        assert!(ui.render((40.0, 23.0)));
        assert!(ui.images.contains_key(&SEARCH_ID), "the field is there before anything is typed");
        view_of(&ui, "launcher-empty");
        for c in "g".chars() {
            ui.shell.type_text(&c.to_string());
        }
        ui.dirty = true;
        assert!(ui.render((40.0, 23.0)));
        view_of(&ui, "launcher-g");
        ui.shell.type_text("oogle");
        ui.dirty = true;
        ui.render((40.0, 23.0));
        view_of(&ui, "launcher-google");
        assert_eq!(ui.shell.launcher().len(), 2);
        ui.shell.type_text("zzz");
        ui.dirty = true;
        assert!(ui.render((40.0, 23.0)), "an empty result still draws the launcher, not a settings card");
        assert!(ui.images.contains_key(&SEARCH_NOTE_ID));
        assert!(ui.bubbles.is_empty());
        view_of(&ui, "launcher-none");
    }

    #[test]
    fn the_search_field_changes_with_what_is_typed() {
        let (w, h, empty) = search_image("", 200, 200);
        let (w2, h2, typed) = search_image("chrome", 1, 200);
        assert_eq!((w, h), (w2, h2));
        assert_ne!(empty, typed);
        assert!(empty.chunks_exact(4).filter(|p| p[3] > 100).count() > (w * h) as usize / 2, "a pane of glass");
    }
}
