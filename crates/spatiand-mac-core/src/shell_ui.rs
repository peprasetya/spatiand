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
            // the glasses' business, settled once.
            shell: Shell::new(Vec::new(), DesktopPanels::NONE, true),
            anchor: (0.0, 0.0),
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

    pub fn set_environments(&mut self, choices: Vec<(String, EnvironmentChoice)>, current: EnvironmentChoice) {
        let entries = choices.into_iter().map(|(label, choice)| EnvironmentEntry { label, choice }).collect();
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
        if self.shell.mode() == Mode::Launcher && !self.shell.launcher().is_empty() {
            self.render_launcher();
        } else if let Some(model) = menu_model::model(&self.shell) {
            self.render_card(&model, fov);
        }
        self.version += 1;
        true
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
                pitch: placement.pitch as f64,
                radius: placement.radius as f64,
            }));
            // The name, clear of the glass even when it is the focused one and larger.
            let (lw, lh, label_rgba) = label_image(label, focused);
            let label_height_m = LABEL_HEIGHT_M;
            let drop = diameter * 0.5 + 0.022 + label_height_m * 0.5;
            let lid = LABEL_FIRST + *index as u32 % 16;
            self.images.insert(lid, PanelSpec {
                id: lid,
                image: Image { width: lw, height: lh, rgba: Arc::new(label_rgba) },
                width_m: label_height_m * lw as f64 / lh as f64,
                yaw: placement.yaw as f64,
                pitch: placement.pitch as f64 - drop / placement.radius as f64,
                radius: placement.radius as f64,
            });
        }
        for (id, index, spec) in specs {
            self.bubbles.insert(id, index);
            self.images.insert(id, spec);
        }
        // Page dots, so a paged grid is not mistaken for a short one.
        if pages > 1 {
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
}
