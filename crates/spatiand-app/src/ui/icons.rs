//! The room's icons: line drawings on a 24-unit grid, drawn at the size they are shown (so
//! they are as sharp in the glasses as the text beside them) and tinted as text is.
//!
//! Drawn for Spatiand, in one hand: round ends and joins, one weight of line, and nothing
//! filled but a play mark and a list's disclosure triangles -- so a row of them reads as one
//! set.

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
    /// Listening: a microphone.
    Microphone,
    /// Settings: a cog.
    Cog,
    /// Take care: a triangle with a mark.
    Warning,
    /// Cutting, to move what is cut.
    Cut,
    /// Writing: renaming.
    Pencil,
    /// What was copied, to put somewhere.
    Paste,
    /// A key: a password, a passkey (SpatiKey).
    Key,
    /// A list's folder, closed and open: a triangle pointing at it, and down into what it
    /// holds (Finder's disclosure triangles).
    Disclose,
    Disclosed,
    /// Drawing over a picture or a page: a pen's nib in a ring (Preview's Markup).
    Markup,
    /// Choosing what was drawn: the pointer's arrow.
    Pointer,
    /// Cutting a picture down: two corners.
    Crop,
    /// Words put on a picture.
    Text,
    /// Shapes drawn: a box, an oval, an arrow.
    Square,
    Oval,
    Arrow,
    /// A change taken back, and put back again.
    Undo,
    Redo,
    /// Written to the file.
    Save,
    /// How heavy a line is drawn.
    LineThin,
    LineMedium,
    LineThick,
    /// Text put into another language: a character and a letter.
    Translate,
    /// All of it chosen: a frame of dashes.
    SelectAll,
    /// Drawing by hand: a pen and the line it leaves.
    Pen,
    /// A highlighter: a marker's flat tip over the line it marks.
    Highlighter,
    /// What is drawn taken away: an eraser on its line.
    Eraser,
    /// Words' typeface: a capital and a small letter.
    Typeface,
    /// Words heavy, and slanted.
    Bold,
    Italic,
    /// Lines set from the left, about the middle, from the right.
    AlignLeft,
    AlignCenter,
    AlignRight,
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
            Icon::Microphone => r#"<rect x="9" y="3.5" width="6" height="11" rx="3"/><path d="M5.5 11.5a6.5 6.5 0 0 0 13 0M12 18v2.5M9 20.5h6"/>"#,
            Icon::Cog => r#"<circle cx="12" cy="12" r="3.2"/><path d="M12 3.5v2.3M12 18.2v2.3M3.5 12h2.3M18.2 12h2.3M6 6l1.6 1.6M16.4 16.4L18 18M6 18l1.6-1.6M16.4 7.6L18 6"/>"#,
            Icon::Warning => r#"<path d="M12 4l9 15.5H3z"/><path d="M12 10v4.5M12 17.2v.2"/>"#,
            Icon::Cut => r#"<circle cx="6.5" cy="17.5" r="2.8"/><circle cx="17.5" cy="17.5" r="2.8"/><path d="M8.6 15.6L18 4M15.4 15.6L6 4"/>"#,
            Icon::Pencil => r#"<path d="M15 5l4 4L8.5 19.5H4.5v-4z"/><path d="M12.5 7.5l4 4"/>"#,
            Icon::Paste => r#"<rect x="5" y="5" width="14" height="16" rx="2"/><path d="M9 5V3.5h6V5M9 11h6M9 15h4"/>"#,
            Icon::Key => r#"<circle cx="7.5" cy="12" r="4"/><path d="M11.5 12h9M17.5 12v3.5M20.5 12v2.5"/>"#,
            Icon::Disclose => r##"<path d="M9.5 6.8v10.4a.7.7 0 0 0 1.1.6l7.1-5.2a.7.7 0 0 0 0-1.2l-7.1-5.2a.7.7 0 0 0-1.1.6z" fill="#fff" stroke-width="1.2"/>"##,
            Icon::Disclosed => r##"<path d="M6.8 9.5h10.4a.7.7 0 0 1 .6 1.1l-5.2 7.1a.7.7 0 0 1-1.2 0l-5.2-7.1a.7.7 0 0 1 .6-1.1z" fill="#fff" stroke-width="1.2"/>"##,
            Icon::Back10 => r#"<path d="M4.5 12a7.5 7.5 0 1 0 2.2-5.3M4.5 4.5v3.8h3.8"/><path d="M9.3 10.4l1.3-1v6" stroke-width="1.6"/><ellipse cx="14.4" cy="12.4" rx="1.7" ry="2.9" stroke-width="1.6"/>"#,
            Icon::Forward10 => r#"<path d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3M19.5 4.5v3.8h-3.8"/><path d="M8.8 10.4l1.3-1v6" stroke-width="1.6"/><ellipse cx="13.9" cy="12.4" rx="1.7" ry="2.9" stroke-width="1.6"/>"#,
            Icon::Markup => r#"<circle cx="12" cy="12" r="9.2"/><path d="M8.2 15.8l1.1-3.9 5.2-5.2 2.8 2.8-5.2 5.2zM9.3 11.9l2.8 2.8"/>"#,
            Icon::Pointer => r#"<path d="M7 3.8v14.4l3.6-3.4 2.6 5.6 2.4-1.1-2.6-5.5h5z"/>"#,
            Icon::Crop => r#"<path d="M7 3.5V17h13.5M3.5 7H17v13.5"/>"#,
            Icon::Text => r#"<path d="M5.5 6.5V5h13v1.5M12 5v14M9 19h6"/>"#,
            Icon::Square => r#"<rect x="4.5" y="5.5" width="15" height="13" rx="1.2"/>"#,
            Icon::Oval => r#"<ellipse cx="12" cy="12" rx="8.6" ry="6.6"/>"#,
            Icon::Arrow => r#"<path d="M5 19L18.5 5.5M10.5 5.5h8v8"/>"#,
            Icon::Undo => r#"<path d="M9 13.5L4.5 9 9 4.5"/><path d="M4.5 9h10a5 5 0 0 1 0 10H11"/>"#,
            Icon::Redo => r#"<path d="M15 13.5L19.5 9 15 4.5"/><path d="M19.5 9h-10a5 5 0 0 0 0 10H13"/>"#,
            Icon::Save => r#"<path d="M5 4.5h11.2l2.8 2.8v12.2H5z"/><path d="M8.5 4.5v4.5h6.5V4.5M8 19.5v-5.5h8v5.5"/>"#,
            Icon::LineThin => r#"<path d="M4.5 12h15" stroke-width="1.2"/>"#,
            Icon::LineMedium => r#"<path d="M4.5 12h15" stroke-width="2.8"/>"#,
            Icon::LineThick => r#"<path d="M5.5 12h13" stroke-width="5"/>"#,
            Icon::Translate => r#"<path d="M3.5 5.5h9M8 3.5v2M10.8 5.5c-.7 4.2-3.2 7.6-6.8 9.6M5.8 9.4c1.4 2.4 3.4 4.2 5.6 5.2M12.6 20.5l3.9-9 3.9 9M14 17.4h5"/>"#,
            Icon::SelectAll => r#"<rect x="4.5" y="4.5" width="15" height="15" rx="2" stroke-dasharray="3 2.6"/><path d="M8.5 9.5h7M8.5 12.5h7M8.5 15.5h4"/>"#,
            Icon::Pen => r#"<path d="M17.6 3.6l2.8 2.8-10.5 10.5-3.9 1.1 1.1-3.9z"/><path d="M15.4 5.8l2.8 2.8M3.5 21c1.6-1.1 3.1-1.3 4.6-.6s3 .5 4.4-.5"/>"#,
            Icon::Highlighter => r#"<path d="M8.6 11.8l6.6-8.6 5.1 3.9-6.6 8.6z"/><path d="M8.6 11.8l-1.2 4.6 2.4 1.8 3.9-2.5"/><path d="M3.5 21.2h17" stroke-width="2.6"/>"#,
            Icon::Eraser => r#"<path d="M4 15l9-9 6 6-7 7H8z"/><path d="M8.5 10.5l6 6M12 19h8"/>"#,
            Icon::Typeface => r#"<path d="M3 19L8 5l5 14M4.8 14h6.4"/><circle cx="17.3" cy="15.9" r="3.1"/><path d="M20.4 12.6V19"/>"#,
            Icon::Bold => r#"<path d="M7.5 4.8h5.6a3.4 3.4 0 0 1 0 6.8H7.5zM7.5 11.6h6.4a3.8 3.8 0 0 1 0 7.6H7.5z" stroke-width="2.5"/>"#,
            Icon::Italic => r#"<path d="M10.5 4.8h7M6.5 19.2h7M14.4 4.8l-4 14.4"/>"#,
            Icon::AlignLeft => r#"<path d="M4 6h16M4 10h10M4 14h16M4 18h10"/>"#,
            Icon::AlignCenter => r#"<path d="M4 6h16M7 10h10M4 14h16M7 18h10"/>"#,
            Icon::AlignRight => r#"<path d="M4 6h16M10 10h10M4 14h16M10 18h10"/>"#,
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

/// `icon` drawn `px` across in memory, as how much of each pixel it covers (0 to 255, top row
/// first): for a picture made without a GPU, such as one handed to the room with a drag.
pub fn coverage(icon: Icon, px: u32) -> Option<Vec<u8>> {
    let (_, _, rgba) = render(icon, px)?;
    Some(rgba.chunks_exact(4).map(|p| p[3]).collect())
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
            Star, Pin, Puzzle, Copy, Globe, Warning, Cut, Pencil, Paste, Disclose, Disclosed, Markup, Pointer, Crop, Text, Square, Oval, Arrow, Undo, Redo, Save,
            LineThin, LineMedium, LineThick, Translate, SelectAll, Pen, Highlighter, Eraser, Typeface, Bold, Italic, AlignLeft, AlignCenter, AlignRight,
        ];
        for icon in all {
            let (_, _, rgba) = render(icon, 48).unwrap_or_else(|| panic!("{icon:?} does not render"));
            assert!(rgba.chunks_exact(4).filter(|p| p[3] > 128).count() > 40, "{icon:?} is nearly empty");
        }
        // The three weights of line are drawn heavier one after the other.
        let ink = |icon| render(icon, 48).unwrap().2.chunks_exact(4).filter(|p| p[3] > 128).count();
        assert!(ink(LineThin) < ink(LineMedium) && ink(LineMedium) < ink(LineThick));
    }
}
