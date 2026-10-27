//! The room's icons: line drawings on a 24-unit grid, drawn at the size they are shown (so
//! they are as sharp in the glasses as the text beside them) and tinted as text is.
//!
//! Drawn for Spatiand, in one hand: round ends and joins, one weight of line, and nothing
//! filled but a play mark -- so a row of them reads as one set.

use std::cell::RefCell;
use std::collections::HashMap;

use super::{Color, Painter, Rect, Texture};
use crate::Gl;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Back,
    Forward,
    Up,
    Down,
    Play,
    Pause,
    Options,
    Info,
    Close,
    Plus,
    Grid,
    List,
    Search,
    Home,
    Folder,
    Desktop,
    Document,
    Download,
    Picture,
    Film,
    Music,
    Drive,
    Cloud,
    Trash,
    Rotate,
    Cube,
    Terminal,
    Link,
    Import,
    Server,
    Refresh,
    Eye,
    /// Ten seconds back, and on: a film's skips.
    Back10,
    Forward10,
    /// Locked: a padlock.
    Lock,
    /// Lines of falling length: in the order of the names.
    SortName,
    /// A clock: what changed last.
    SortRecent,
    /// Two paths crossing: in no order.
    Shuffle,
    /// A speaker sounding, and one silenced.
    Speaker,
    Muted,
    /// Kept for later: a bookmark.
    Star,
    /// Held in place: a pinned tab.
    Pin,
    /// An add-on: an extension.
    Puzzle,
    /// Two sheets: copied.
    Copy,
    /// The web, and a site.
    Globe,
    /// Take care: a triangle with a mark.
    Warning,
}

impl Icon {
    /// The drawing: SVG on a 24-unit grid, in `currentColor`.
    fn paths(self) -> &'static str {
        match self {
            Icon::Back => r#"<path d="M15 18l-6-6 6-6"/>"#,
            Icon::Forward => r#"<path d="M9 18l6-6-6-6"/>"#,
            Icon::Up => r#"<path d="M12 19V5M5.5 11.5L12 5l6.5 6.5"/>"#,
            Icon::Down => r#"<path d="M12 5v14M5.5 12.5L12 19l6.5-6.5"/>"#,
            Icon::Play => r##"<path d="M7.5 4.8v14.4a.8.8 0 0 0 1.2.7l11.4-7.2a.8.8 0 0 0 0-1.4L8.7 4.1a.8.8 0 0 0-1.2.7z" fill="#fff"/>"##,
            Icon::Pause => r#"<path d="M8 5v14M16 5v14" stroke-width="3"/>"#,
            Icon::Options => r#"<path d="M4 7h9M18 7h2M4 17h3M12 17h8"/><circle cx="15.5" cy="7" r="2.5"/><circle cx="9.5" cy="17" r="2.5"/>"#,
            Icon::Info => r#"<circle cx="12" cy="12" r="9"/><path d="M12 11v6M12 7.6v.2"/>"#,
            Icon::Close => r#"<path d="M6.5 6.5l11 11M17.5 6.5l-11 11"/>"#,
            Icon::Plus => r#"<path d="M12 5v14M5 12h14"/>"#,
            Icon::Grid => r#"<rect x="4" y="4" width="6.5" height="6.5" rx="1.5"/><rect x="13.5" y="4" width="6.5" height="6.5" rx="1.5"/><rect x="4" y="13.5" width="6.5" height="6.5" rx="1.5"/><rect x="13.5" y="13.5" width="6.5" height="6.5" rx="1.5"/>"#,
            Icon::List => r#"<path d="M9 6h11M9 12h11M9 18h11M4.5 6h.1M4.5 12h.1M4.5 18h.1"/>"#,
            Icon::Search => r#"<circle cx="10.5" cy="10.5" r="6.5"/><path d="M20 20l-4.8-4.8"/>"#,
            Icon::Home => r#"<path d="M3.5 11L12 3.8l8.5 7.2M6 9.3V20h12V9.3M10 20v-5.5h4V20"/>"#,
            Icon::Folder => r#"<path d="M3.5 7.5A2 2 0 0 1 5.5 5.5h3.8l2 2.2h7.2a2 2 0 0 1 2 2v7.8a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z"/>"#,
            Icon::Desktop => r#"<rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8.5 20h7M12 16v4"/>"#,
            Icon::Document => r#"<path d="M14 3.5H7.5a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2h9a2 2 0 0 0 2-2V8z"/><path d="M14 3.5V8h4.5M9 13h6M9 16.5h4"/>"#,
            Icon::Download => r#"<path d="M12 4v11M7 10.5l5 5 5-5M5 20h14"/>"#,
            Icon::Picture => r#"<rect x="3" y="5" width="18" height="14" rx="2"/><circle cx="9" cy="10" r="1.7"/><path d="M21 15.5l-5-5L6.5 19"/>"#,
            Icon::Film => r#"<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M7.5 5v14M16.5 5v14M3 9.7h4.5M3 14.3h4.5M16.5 9.7H21M16.5 14.3H21"/>"#,
            Icon::Music => r#"<path d="M9 17.5V6.5l11-2v11"/><circle cx="6.5" cy="17.5" r="2.5"/><circle cx="17.5" cy="15.5" r="2.5"/>"#,
            Icon::Drive => r#"<rect x="3" y="12.5" width="18" height="7" rx="2"/><path d="M5.5 12.5l2.2-7h8.6l2.2 7M7 16h.1M10 16h.1"/>"#,
            Icon::Cloud => r#"<path d="M7 18.5h10.5a4 4 0 0 0 .4-8 6 6 0 0 0-11.5 1.6A3.3 3.3 0 0 0 7 18.5z"/>"#,
            Icon::Trash => r#"<path d="M4 7h16M9.5 7V4.5h5V7M6 7l1 13h10l1-13M10 11v5.5M14 11v5.5"/>"#,
            Icon::Rotate => r#"<path d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3M19.5 4.5v3.8h-3.8"/>"#,
            Icon::Cube => r#"<path d="M12 3l8 4.5v9L12 21l-8-4.5v-9zM12 12l8-4.5M12 12v9M12 12L4 7.5"/>"#,
            Icon::Terminal => r#"<rect x="3" y="4.5" width="18" height="15" rx="2.5"/><path d="M7 9.5l3 2.5-3 2.5M12.5 15h4.5"/>"#,
            Icon::Link => r#"<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>"#,
            Icon::Import => r#"<path d="M12 3.5v11M7.5 10l4.5 4.5 4.5-4.5M4 16.5v3h16v-3"/>"#,
            Icon::Server => r#"<rect x="4" y="4" width="16" height="7" rx="1.8"/><rect x="4" y="13" width="16" height="7" rx="1.8"/><path d="M8 7.5h.1M8 16.5h.1"/>"#,
            Icon::Refresh => r#"<path d="M20 11a8 8 0 0 0-14.6-3.5M4 4v4h4M4 13a8 8 0 0 0 14.6 3.5M20 20v-4h-4"/>"#,
            Icon::Lock => r#"<rect x="5" y="10.5" width="14" height="10" rx="2"/><path d="M8 10.5V7.5a4 4 0 0 1 8 0v3M12 14.5v2.5"/>"#,
            Icon::SortName => r#"<path d="M4 6.5h16M4 12h11M4 17.5h6"/>"#,
            Icon::SortRecent => r#"<circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 2"/>"#,
            Icon::Shuffle => r#"<path d="M3.5 7h3.2c4 0 5.6 10 10.6 10H20M3.5 17h3.2c1.6 0 2.8-1 3.8-2.4M13.5 9.4c1-1.4 2.2-2.4 3.8-2.4H20M17.5 4.5L20 7l-2.5 2.5M17.5 14.5L20 17l-2.5 2.5"/>"#,
            Icon::Eye => r#"<path d="M2.5 12s3.5-6.5 9.5-6.5S21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="3"/>"#,
            Icon::Speaker => r#"<path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z"/><path d="M15.5 9a4 4 0 0 1 0 6M18 6.5a7.5 7.5 0 0 1 0 11"/>"#,
            Icon::Muted => r#"<path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z"/><path d="M16 9.5l5 5M21 9.5l-5 5"/>"#,
            Icon::Star => r#"<path d="M12 3.8l2.5 5.2 5.7.8-4.1 4 1 5.7L12 16.8l-5.1 2.7 1-5.7-4.1-4 5.7-.8z"/>"#,
            Icon::Pin => r#"<path d="M9 4h6M10 4v5.5L7 13h10l-3-3.5V4M12 13v7"/>"#,
            Icon::Puzzle => r#"<path d="M5 8h3.5a2 2 0 1 1 4 0H16v3.5a2 2 0 1 1 0 4V19H5v-3.5a2 2 0 1 0 0-4z"/>"#,
            Icon::Copy => r#"<rect x="8.5" y="8.5" width="11" height="11" rx="2"/><path d="M15.5 8.5V6.5a2 2 0 0 0-2-2h-7a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h2"/>"#,
            Icon::Globe => r#"<circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17M12 3.5c2.4 2.3 3.6 5.1 3.6 8.5s-1.2 6.2-3.6 8.5M12 3.5C9.6 5.8 8.4 8.6 8.4 12s1.2 6.2 3.6 8.5"/>"#,
            Icon::Warning => r#"<path d="M12 4l9 15.5H3z"/><path d="M12 10v4.5M12 17.2v.2"/>"#,
            Icon::Back10 => r#"<path d="M4.5 12a7.5 7.5 0 1 0 2.2-5.3M4.5 4.5v3.8h3.8"/><path d="M9.3 10.4l1.3-1v6" stroke-width="1.6"/><ellipse cx="14.4" cy="12.4" rx="1.7" ry="2.9" stroke-width="1.6"/>"#,
            Icon::Forward10 => r#"<path d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3M19.5 4.5v3.8h-3.8"/><path d="M8.8 10.4l1.3-1v6" stroke-width="1.6"/><ellipse cx="13.9" cy="12.4" rx="1.7" ry="2.9" stroke-width="1.6"/>"#,
        }
    }
}

thread_local! {
    /// Each icon drawn so far, at each size: on the GPU of the one context this thread draws with.
    static DRAWN: RefCell<HashMap<(Icon, u32), Texture>> = RefCell::new(HashMap::new());
}

fn render(icon: Icon, px: u32) -> Option<(u32, u32, Vec<u8>)> {
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="{px}" height="{px}" fill="none" stroke="#fff" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">{}</svg>"##,
        icon.paths()
    );
    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(px, px)?;
    // The drawing is already `px` across: its own width and height say so.
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    // White throughout, its shape in the alpha: tinted when drawn, like text.
    let rgba = pixmap.take().chunks_exact(4).flat_map(|p| [255, 255, 255, p[3]]).collect();
    Some((px, px, rgba))
}

/// Draw `icon` filling the square `rect` (centred in it if it is not square), in `color`.
pub fn draw(p: &mut Painter, gl: &Gl, icon: Icon, rect: Rect, color: Color) {
    let side = rect.w.min(rect.h);
    let px = (side * p.scale()).round().max(8.0) as u32;
    let texture = DRAWN.with(|drawn| {
        if let Some(t) = drawn.borrow().get(&(icon, px)) {
            return Some(*t);
        }
        let (w, h, rgba) = render(icon, px)?;
        let t = p.texture(gl, &rgba, w, h).ok()?;
        drawn.borrow_mut().insert((icon, px), t);
        Some(t)
    });
    if let Some(texture) = texture {
        let at = Rect::new(rect.x + (rect.w - side) * 0.5, rect.y + (rect.h - side) * 0.5, side, side);
        p.image(gl, texture, at, [0.0, 0.0, 1.0, 1.0], 0.0, color, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_draws_something() {
        use Icon::*;
        let all = [
            Back, Forward, Up, Down, Play, Pause, Options, Info, Close, Plus, Grid, List, Search, Home, Folder, Desktop, Document, Download, Picture, Film, Music, Drive,
            Cloud, Trash, Rotate, Cube, Terminal, Link, Import, Server, Refresh, Eye, Back10, Forward10, Lock, SortName, SortRecent, Shuffle, Speaker, Muted,
            Star, Pin, Puzzle, Copy, Globe, Warning,
        ];
        for icon in all {
            let (_, _, rgba) = render(icon, 48).unwrap_or_else(|| panic!("{icon:?} does not render"));
            assert!(rgba.chunks_exact(4).filter(|p| p[3] > 128).count() > 40, "{icon:?} is nearly empty");
        }
    }
}
