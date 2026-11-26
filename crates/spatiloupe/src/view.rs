//! Looking at one thing: a picture, a PDF, an object.
//!
//! What is looked at fills the window; the controls float over it on glass and fade when the
//! wearer has been still for a while, coming back at a touch. Everything is drawn by
//! [`View::paint`] for one eye at a time, so that in the room a stereo photograph or an object
//! gets two pictures and the controls are the same in both.
//!
//! The folder's other files are beside it, a page each, as a phone's photographs are: a picture
//! at its fitted size goes sideways under the hand to the next or the one before
//! ([`crate::swipe`]), and the arrow keys and the strip along the foot slide there too. Until a
//! file is read, its tile from the folder stands in for it -- beside this one as it comes in, and
//! in its own view while its picture is read.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use spatiand_app::glow::{self, HasContext};
use spatiand_app::ui::{draw_icon, look, theme, AutoHide, Button, Color, Confirm, Field, Icon, Input, List, Painter, Rect, Row, Shape, Strip, Texture, Tile, Typed, DOUBLE_CLICK, TILE_TINT};
use spatiand_app::{Gl, Key};

use crate::folder::{Entry, Kind};
use crate::markup::Area;
use crate::model::{self, Eye, Orbit, Renderer};
use crate::pdf::{self, Ask, Pdf};
use crate::picture::Picture;
use crate::stereo::Eyes;
use crate::swipe::{Carousel, Glide, Reach, Scrolled};
use crate::tools::{Desk, Placed, Said, BAR};

/// Live Text: the text in a picture or a page, chosen, copied and translated.
mod lens;

/// The gap between the file looked at and the one beside it on the carousel, beyond the window's
/// own width: with the room a fitted picture leaves around itself, about what a phone leaves
/// between its photographs.
/// How long what went wrong stays on show.
const TROUBLE_SHOWN: Duration = Duration::from_secs(8);

const BESIDE_GAP: f32 = 24.0;

/// How long a picture takes to come in over the tile that stood in for it while it was read: a
/// moment's blending, not a sudden sharpening.
const FADES_IN: Duration = Duration::from_millis(160);

/// How long a tile stands in for its picture before the dots say that it is still being read.
const STILL_READING: Duration = Duration::from_millis(600);

/// What a file's tile said of the picture it was made from: what the tile is drawn as when it
/// stands in for it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seen {
    /// The width over the height of what one eye sees.
    pub aspect: f32,
    /// How tall what one eye sees is at the picture's own size, in its own pixels: a picture
    /// smaller than the window is shown at that, not stretched to fill it.
    pub height: f32,
    /// Two eyes' worth.
    pub stereo: bool,
}

/// The width over the height of what one eye sees, in a picture `width` by `height` whose eyes
/// are packed as `eyes` says.
pub fn eye_aspect(width: f32, height: f32, eyes: Eyes) -> f32 {
    // A pair squeezed into a frame of a film's shape -- sixteen by nine -- is each eye at half
    // its width (or height); any other shape is two views at their own.
    eye_aspect_as(width, height, eyes, squeezed(width / height.max(1.0)))
}

/// The same, told whether each eye's view is squeezed to half along its packing -- half its
/// width side by side, half its height one over the other -- rather than judging it by the
/// frame's shape: a squeezed pair turned a quarter is no longer a film's shape, but its eyes are
/// still squeezed, now along the packing it was turned to.
fn eye_aspect_as(width: f32, height: f32, eyes: Eyes, squeeze: bool) -> f32 {
    let (w, h) = (width, height.max(1.0));
    match eyes {
        Eyes::Mono | Eyes::Pair => w / h,
        Eyes::SideBySide | Eyes::Crossed => {
            let a = w * 0.5 / h;
            if squeeze { a * 2.0 } else { a }
        }
        Eyes::TopBottom => {
            let a = w / (h * 0.5);
            if squeeze { a * 0.5 } else { a }
        }
    }
}

/// How tall what one eye sees is at a picture's own size, in its pixels: a squeezed pair's eye
/// at the height it is unsqueezed to.
pub fn eye_height(width: f32, height: f32, eyes: Eyes) -> f32 {
    eye_height_as(width, height, eyes, squeezed(width / height.max(1.0)))
}

/// The same, told whether the pair is squeezed, as eye_aspect_as is.
fn eye_height_as(width: f32, height: f32, eyes: Eyes, squeeze: bool) -> f32 {
    let (w, h) = match eyes {
        Eyes::Mono | Eyes::Pair => (width, height),
        Eyes::SideBySide | Eyes::Crossed => (width * 0.5, height),
        Eyes::TopBottom => (width, height * 0.5),
    };
    h.max(w / eye_aspect_as(width, height, eyes, squeeze).max(1e-3))
}

/// How wide a picture of this shape is when it is filling `area`: as large as it goes with a
/// little room left around it.
fn fit_width(area: Rect, aspect: f32) -> f32 {
    (area.w * 0.94).min(area.h * 0.92 * aspect)
}

/// How wide it is shown fitted: filling `area`, or at its own size -- `own_height` tall -- if
/// that is smaller. A small picture is shown as it is, crisp, not blown up to the window.
fn shown_width(area: Rect, aspect: f32, own_height: f32) -> f32 {
    fit_width(area, aspect).min(own_height.max(1.0) * aspect)
}

/// Where a picture of this shape, `own_height` tall at its own size, is drawn fitted to the
/// window `all`: in the middle of the stage.
fn fitted(all: Rect, aspect: f32, own_height: f32) -> Rect {
    let area = stage(all);
    let w = shown_width(area, aspect, own_height);
    let h = w / aspect.max(1e-3);
    Rect::new(area.x + (area.w - w) * 0.5, area.y + (area.h - h) * 0.5, w, h)
}

/// What two rectangles have in common; nothing, if they do not meet.
fn overlap(a: Rect, b: Rect) -> Rect {
    let (x, y) = (a.x.max(b.x), a.y.max(b.y));
    Rect::new(x, y, (a.right().min(b.right()) - x).max(0.0), (a.bottom().min(b.bottom()) - y).max(0.0))
}

/// Put a picture on the GPU with smaller copies of itself, so that a large photograph shown
/// small is not a shimmer of skipped pixels.
pub fn upload(gl: &Gl, rgba: &[u8], width: u32, height: u32) -> Result<Texture, String> {
    let g = gl.api();
    unsafe {
        let raw = g.create_texture()?;
        g.bind_texture(glow::TEXTURE_2D, Some(raw));
        g.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        g.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, width as i32, height as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(rgba)));
        g.generate_mipmap(glow::TEXTURE_2D);
        g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
        g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
        Ok(Texture::from_raw(raw, width, height))
    }
}

fn replace(gl: &Gl, texture: Texture, rgba: &[u8]) {
    let g = gl.api();
    unsafe {
        g.bind_texture(glow::TEXTURE_2D, Some(texture.raw()));
        g.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, texture.width as i32, texture.height as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(rgba)));
        g.generate_mipmap(glow::TEXTURE_2D);
    }
}

/// A picture on the GPU, and what it came with.
pub struct Shown {
    picture: Picture,
    texture: Option<Texture>,
    right: Option<Texture>,
    /// Quarter turns clockwise.
    turns: u8,
    /// Whether the picture has been turned a quarter (or three) since it was read: whether a
    /// pair is squeezed is judged by the frame's shape as it was read, as a squeezed pair turned
    /// is a film's frame on its side.
    sideways: bool,
    /// Which frame of a moving picture is on show, and since when.
    frame: usize,
    frame_since: Instant,
    /// Which frame the GPU's copy holds: put there once each time the frame changes, not each
    /// time the window is drawn.
    uploaded: usize,
    /// The packing the wearer chose over the one guessed.
    eyes: Option<Eyes>,
    swapped: bool,
    /// How far the two eyes' views are slid towards each other, as a fraction of a view's
    /// width: what is at zero depth moves nearer or further.
    depth: f32,
    /// None: fitted to the window. Some: so many of the picture's pixels to a logical one.
    zoom: Option<f32>,
    /// The point of the picture at the middle of the window, as fractions of it.
    centre: (f32, f32),
    /// When it was first drawn, for it to come in over the tile that stood in for it.
    drawn_at: Option<Instant>,
    /// Whether the GPU's copies are drawn a pixel at a time, as a picture magnified more than
    /// twice is: its pixels crisp, not a blur.
    nearest: bool,
    /// Quarter turns clockwise from how it was read, settled or not: markup saves a picture as
    /// it is shown.
    orientation: u8,
    /// The part of the picture shown, as fractions of it as it is shown (left, top, right,
    /// bottom): all of it, or what markup cropped it to.
    cut: [f32; 4],
    /// How far down the window the room for the picture begins: under the bar, and under the
    /// markup tools too when they are out.
    head: f32,
}

impl Shown {
    fn new(picture: Picture) -> Shown {
        Shown {
            picture,
            texture: None,
            right: None,
            turns: 0,
            sideways: false,
            frame: 0,
            frame_since: Instant::now(),
            uploaded: 0,
            eyes: None,
            swapped: false,
            depth: 0.0,
            zoom: None,
            centre: (0.5, 0.5),
            drawn_at: None,
            nearest: false,
            orientation: 0,
            cut: [0.0, 0.0, 1.0, 1.0],
            head: HEAD,
        }
    }

    /// The picture's size as it is shown, turned as it is turned, in its own pixels: the units
    /// markup draws in.
    fn frame_size(&self) -> (f32, f32) {
        let (w, h) = (self.picture.original.0 as f32, self.picture.original.1 as f32);
        if self.turns % 2 == 1 { (h, w) } else { (w, h) }
    }

    /// How much of the shown picture the cut keeps, across and down.
    fn cut_share(&self) -> (f32, f32) {
        ((self.cut[2] - self.cut[0]).max(1e-4), (self.cut[3] - self.cut[1]).max(1e-4))
    }

    /// Whether markup can draw on it: one still picture, flat.
    fn markable(&self) -> bool {
        self.eyes() == Eyes::Mono && self.picture.frames.is_empty() && self.picture.right.is_none()
    }

    /// The room a picture is fitted to: the window, but for what floats along its top.
    fn stage(&self, all: Rect) -> Rect {
        stage_under(all, self.head)
    }

    /// How tall one eye's view is at the picture's own size, in logical pixels: the file's pixels
    /// one to one, not the GPU's copy's.
    fn own_height(&self) -> f32 {
        let ratio = self.picture.original.0.max(1) as f32 / self.picture.width.max(1) as f32;
        let (w, h) = (self.picture.width as f32, self.picture.height as f32);
        let squeeze = self.squeezed();
        let (w, h) = if self.turns % 2 == 1 { (h, w) } else { (w, h) };
        eye_height_as(w, h, self.eyes(), squeeze) * ratio * self.cut_share().1
    }

    /// How wide it is fitted to `area`: filling it, or at its own size if that is smaller.
    fn fitted_width(&self, area: Rect) -> f32 {
        shown_width(area, self.eye_aspect(), self.own_height())
    }

    fn eyes(&self) -> Eyes {
        self.eyes.unwrap_or(self.picture.eyes)
    }

    /// The width over the height of what one eye sees.
    fn eye_aspect(&self) -> f32 {
        let (w, h) = (self.picture.width as f32, self.picture.height as f32);
        let (w, h) = if self.turns % 2 == 1 { (h, w) } else { (w, h) };
        let (cw, ch) = self.cut_share();
        eye_aspect_as(w, h, self.eyes(), self.squeezed()) * cw / ch
    }

    /// Whether a pair's views are squeezed to half: judged by the frame's shape as it was read,
    /// so that a squeezed pair turned a quarter is still shown at its eyes' true shape, the
    /// squeeze then running along the packing it was turned to.
    fn squeezed(&self) -> bool {
        let (w, h) = (self.picture.width as f32, self.picture.height as f32);
        let (w, h) = if self.sideways { (h, w) } else { (w, h) };
        squeezed(w / h.max(1.0))
    }

    /// How far it has come in over its stand-in, 0 to 1.
    fn faded_in(&self) -> f32 {
        self.drawn_at.map_or(0.0, |at| (at.elapsed().as_secs_f32() / FADES_IN.as_secs_f32()).min(1.0))
    }

    /// The size, in the picture's own pixels, of what one eye sees.
    fn eye_pixels(&self) -> (f32, f32) {
        let (w, h) = (self.picture.width as f32, self.picture.height as f32);
        let (w, h) = if self.turns % 2 == 1 { (h, w) } else { (w, h) };
        let (cw, ch) = self.cut_share();
        match self.eyes() {
            Eyes::Mono | Eyes::Pair => (w * cw, h * ch),
            Eyes::SideBySide | Eyes::Crossed => (w * 0.5, h),
            Eyes::TopBottom => (w, h * 0.5),
        }
    }

    /// Where the picture is drawn, to fill `area` as it is zoomed and moved: below the bar
    /// along the top, so that a tall picture is not under it. A picture cropped by markup: where
    /// what is kept of it is drawn.
    fn placed(&self, area: Rect) -> Rect {
        let area = self.stage(area);
        let aspect = self.eye_aspect();
        let (pw, _) = self.eye_pixels();
        let fit_w = self.fitted_width(area);
        let w = match self.zoom {
            None => fit_w,
            Some(z) => pw / z,
        };
        let h = w / aspect;
        let (cx, cy) = (area.x + area.w * 0.5, area.y + area.h * 0.5);
        Rect::new(cx - self.centre.0 * w, cy - self.centre.1 * h, w, h)
    }

    /// So many of the GPU copy's pixels to a logical one, fitted.
    fn fit_scale(&self, area: Rect) -> f32 {
        self.eye_pixels().0 / self.fitted_width(self.stage(area))
    }

    /// The same, filling the window: what a small picture is zoomed to, at most, beside eight
    /// times its own size.
    fn fill_scale(&self, area: Rect) -> f32 {
        self.eye_pixels().0 / fit_width(self.stage(area), self.eye_aspect())
    }

    /// Zoom by `by` (more than one is nearer), keeping the point of the picture under
    /// (`x`, `y`) where it is.
    fn zoom_by(&mut self, by: f32, area: Rect, x: f32, y: f32) {
        let before = self.placed(area);
        let fit = self.fit_scale(area);
        let now = self.zoom.unwrap_or(fit);
        // From the fitted size to eight times the picture's own, or the window filled if that is
        // nearer still.
        let next = (now / by).clamp((1.0f32 / 8.0).min(self.fill_scale(area)), fit.max(1.0 / 8.0));
        if next >= fit * 0.999 {
            self.zoom = None;
            self.centre = (0.5, 0.5);
            return;
        }
        let under = ((x - before.x) / before.w, (y - before.y) / before.h);
        self.zoom = Some(next);
        let after = self.placed(area);
        // Slide so that what was under the pointer is again.
        let (ux, uy) = (after.x + under.0 * after.w, after.y + under.1 * after.h);
        self.centre.0 += (ux - x) / after.w;
        self.centre.1 += (uy - y) / after.h;
        self.clamp(area);
    }

    /// Keep the picture over the window: zoomed in, its edge never comes inside the window.
    fn clamp(&mut self, area: Rect) {
        let placed = self.placed(area);
        let area = self.stage(area);
        for (centre, size, room) in [(&mut self.centre.0, placed.w, area.w), (&mut self.centre.1, placed.h, area.h)] {
            if size <= room {
                *centre = 0.5;
            } else {
                let margin = room * 0.5 / size;
                *centre = centre.clamp(margin, 1.0 - margin);
            }
        }
    }

    /// Texture coordinates for `eye`'s part of the picture, turned as it is turned.
    fn uv(&self, eye: Eye) -> [f32; 4] {
        let eyes = self.eyes();
        // Cropped by markup (a flat picture only): what is kept of the picture as shown, which
        // half a turn puts the other way round in the texture.
        if self.cut != [0.0, 0.0, 1.0, 1.0] && eyes == Eyes::Mono {
            let [l, t, r, b] = self.cut;
            return if self.turns % 4 == 2 { [1.0 - r, 1.0 - b, 1.0 - l, 1.0 - t] } else { [l, t, r, b] };
        }
        let (left, right) = eyes.halves();
        let mut uv = match (eye, self.swapped) {
            (Eye::Right, false) | (Eye::Left, true) => right,
            _ => left,
        };
        if eyes.is_stereo() && eye != Eye::Both {
            // Each eye's view slid a little: the left one way, the right the other.
            let along = (uv[2] - uv[0]) * self.depth * if eye == Eye::Left { 1.0 } else { -1.0 };
            let trim = (uv[2] - uv[0]) * self.depth.abs();
            uv = [uv[0] + trim + along, uv[1], uv[2] - trim + along, uv[3]];
        }
        uv
    }
}

/// How far down the window the room for a picture or a page begins: under the bar.
const HEAD: f32 = 96.0;

/// The part of the window a picture is fitted to: all of it but the bar along the top.
fn stage(all: Rect) -> Rect {
    stage_under(all, HEAD)
}

/// The same, beginning `head` down: under markup's tools too, while they are out.
fn stage_under(all: Rect, head: f32) -> Rect {
    Rect::new(all.x, all.y + head, all.w, (all.h - head - 12.0).max(1.0))
}

/// Whether a picture of this shape holding two views has squeezed each to half: a film's
/// frame, 16:9 or near it, is how half-width and half-height pairs are made.
fn squeezed(aspect: f32) -> bool {
    (1.65..=1.95).contains(&aspect)
}

/// A PDF, as pages laid out down a column.
pub struct Paper {
    pdf: Pdf,
    pages: Vec<pdf::Page>,
    outline: Vec<pdf::Heading>,
    title: Option<String>,
    /// Each page drawn so far, and the width it was drawn at.
    drawn: HashMap<usize, (Texture, u32)>,
    asked: HashMap<usize, u32>,
    /// How far down the column the top of the window is, in logical pixels.
    scroll: f32,
    /// How far the column is moved sideways from the middle, in logical pixels: a page zoomed
    /// wider than the window is moved about in.
    across: f32,
    zoom: f32,
    /// Which search is the one wanted: a search's answer under an older number -- one asked
    /// before the text was changed, or the field closed -- is not this one's.
    finds: u64,
    contents: Option<List>,
    find: Option<Field>,
    /// The page number being typed, after `p`.
    page_field: Option<Field>,
    found: Vec<pdf::Found>,
    found_at: usize,
    finding: bool,
    /// The page the wearer last asked to see, which the column is moved to once it is laid out.
    go_to: Option<usize>,
    stale: Vec<Texture>,
    /// The words on each page asked for, once they have come: what the room's speech is told
    /// is being read. `None` while asked for.
    words: HashMap<usize, Option<String>>,
    /// Pages drawn by the PDF's thread, waiting for the GL thread to put them on the GPU: this
    /// document's own.
    pending: Vec<(usize, u32, u32, Vec<u8>)>,
    /// Pages markup has cropped: what is kept of each, in its points as it is shown.
    cuts: HashMap<usize, Area>,
    /// How far down the column the first page begins: under the bar, and under markup's tools
    /// while they are out.
    head: f32,
    /// Where each page near the window was drawn last, for markup to draw on.
    placed: Vec<(usize, Rect)>,
    /// The question a locked document asks, until it opens.
    lock: Option<Lock>,
    /// What its owner allows done to it: what markup offers.
    rights: pdf::Rights,
    /// Each page's text, for Live Text: its own characters, or read from a picture of it.
    texts: HashMap<usize, lens::PageText>,
}

/// A locked PDF's question: its password, typed into a field that shows dots, given to the PDF's
/// thread with Enter and not kept here after.
struct Lock {
    field: Field,
    /// The password given did not open it.
    wrong: bool,
    /// One given, and being tried.
    trying: bool,
}

impl Lock {
    fn new(wrong: bool) -> Lock {
        let mut field = Field::default();
        field.secret = true;
        field.focused = true;
        field.hint = "Password".into();
        Lock { field, wrong, trying: false }
    }

    /// What was typed, wiped from memory.
    fn forget(&mut self) {
        let mut typed = std::mem::take(&mut self.field.text);
        zeroize::Zeroize::zeroize(&mut typed);
        self.field.set("");
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        self.forget();
    }
}

const PAGE_GAP: f32 = 28.0;

impl Paper {
    fn new(pdf: Pdf) -> Paper {
        Paper {
            pdf,
            pages: Vec::new(),
            outline: Vec::new(),
            title: None,
            drawn: HashMap::new(),
            asked: HashMap::new(),
            scroll: 0.0,
            across: 0.0,
            zoom: 1.0,
            finds: 0,
            contents: None,
            find: None,
            page_field: None,
            found: Vec::new(),
            found_at: 0,
            finding: false,
            go_to: None,
            stale: Vec::new(),
            words: HashMap::new(),
            pending: Vec::new(),
            cuts: HashMap::new(),
            head: HEAD,
            placed: Vec::new(),
            lock: None,
            rights: pdf::Rights::ALL,
            texts: HashMap::new(),
        }
    }

    /// What is shown of page `page`, in its points: all of it, or its crop.
    fn region(&self, page: usize) -> Area {
        self.cuts.get(&page).copied().unwrap_or_else(|| self.pages.get(page).map_or(Area::new(0.0, 0.0, 1.0, 1.0), |p| Area::new(0.0, 0.0, p.width, p.height)))
    }

    /// The words on `page`, if they have come; asked for, if they have not been.
    fn words_on(&mut self, page: usize) -> Option<&str> {
        if !self.words.contains_key(&page) {
            self.words.insert(page, None);
            self.pdf.ask(Ask::Text { page });
        }
        self.words.get(&page).and_then(|w| w.as_deref())
    }

    fn width(&self, area: Rect) -> f32 {
        (area.w * 0.62).min(1000.0) * self.zoom
    }

    /// Where each page is down the column: top, height. A page cropped by markup is what is kept
    /// of it, as wide as the others.
    fn layout(&self, area: Rect) -> Vec<(f32, f32)> {
        let width = self.width(area);
        let mut y = self.head;
        (0..self.pages.len())
            .map(|page| {
                let shown = self.region(page);
                let h = width * shown.h() / shown.w().max(1.0);
                let at = (y, h);
                y += h + PAGE_GAP;
                at
            })
            .collect()
    }

    fn length(&self, area: Rect) -> f32 {
        self.layout(area).last().map_or(0.0, |(y, h)| y + h + 140.0)
    }

    /// The page most of the window is showing.
    fn current(&self, area: Rect) -> usize {
        let middle = self.scroll + area.h * 0.4;
        self.layout(area).iter().position(|(y, h)| middle < y + h + PAGE_GAP).unwrap_or(self.pages.len().saturating_sub(1))
    }

    fn scroll_by(&mut self, by: f32, area: Rect) {
        self.scroll = (self.scroll + by).clamp(0.0, (self.length(area) - area.h).max(0.0));
    }

    /// How far the column may be moved sideways either way: a page wider than the window to its
    /// edges and a little more, one that fits not at all.
    fn across_room(&self, area: Rect) -> f32 {
        let width = self.width(area);
        if width > area.w { (width - area.w) * 0.5 + PAGE_GAP } else { 0.0 }
    }

    /// Whether the pages are wider than the window: moved about in sideways.
    fn wide(&self, area: Rect) -> bool {
        self.across_room(area) > 0.0
    }

    /// Move the column sideways by `by`: more than nought shows more of the pages' right.
    fn pan_by(&mut self, by: f32, area: Rect) {
        let room = self.across_room(area);
        self.across = (self.across - by).clamp(-room, room);
    }

    fn show_page(&mut self, page: usize, area: Rect) {
        if let Some((y, _)) = self.layout(area).get(page) {
            self.scroll = (y - 72.0).max(0.0);
            self.scroll_by(0.0, area);
        }
    }
}

/// An object, once it is on the GPU.
pub struct Object {
    renderer: Option<Renderer>,
    scene: Option<model::Scene>,
    orbit: Orbit,
    turning: bool,
    turned_at: Instant,
    triangles: usize,
    name: Option<String>,
}

pub enum Doc {
    Loading(Instant),
    Failed(String),
    Picture(Box<Shown>),
    Pdf(Box<Paper>),
    Model(Box<Object>),
}

/// What the viewer wants of the program around it.
pub enum Want {
    Nothing,
    /// Back to the folder.
    Leave,
    /// Another of the folder's files, by its place in the folder.
    Open(usize),
    /// SpatiLoupe to end: asked first where markup was not saved.
    Quit,
    /// A picture marked up, to be written (by a worker: it is read whole and drawn on).
    Save(Box<crate::save::Saving>),
    /// A file written -- the one looked at, or its copy beside it -- to be shown as it now is,
    /// with what was done said; a locked PDF with the password it opens with.
    Saved { path: PathBuf, said: String, password: Option<pdf::Secret> },
}

/// What follows the yes of a question.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Then {
    /// Markup not saved: let go of, to go back to the folder, to slide on (by so many, or to a
    /// file's place among the files), or to quit.
    Leave,
    Slide(i32),
    SlideTo(usize),
    Quit,
    /// The file written over with its markup.
    Replace,
    /// A copy written, losing an HDR layer the file has.
    Copy,
}

/// All of a picture: no crop.
const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

pub struct View {
    /// The file, and its place in the folder.
    pub entry: Entry,
    pub index: usize,
    pub doc: Doc,
    pub ticket: u64,
    /// The folder's files, by their place in the folder, for the strip along the foot.
    files: Vec<usize>,
    /// Where this one is among them: page 0 of the carousel.
    place: usize,
    /// The files beside it as pages, and the hand moving between them.
    carousel: Carousel,
    /// Where the carousel is for the frame being drawn: worked out once, in [`View::tick`], so
    /// that two eyes drawn one after the other see it in the same place.
    slid: f32,
    /// A file chosen from the strip, or by Home or End, that is not a neighbour: the page it
    /// comes in on, beside this one, and its place among the files.
    jump: Option<(i32, usize)>,
    /// The file the carousel came to rest on, by its place among the files, for the program to
    /// open in place of this one.
    arrived: Option<usize>,
    /// The drag going on is the carousel's, not a pan or a turn.
    drag_slides: bool,
    /// The page the strip was last told the carousel is going to.
    strip_page: i32,
    strip: Strip,
    back: Button,
    layout: Button,
    info_button: Button,
    info: bool,
    /// When the controls are on show; their fade is this view's own.
    clock: AutoHide,
    hands_heard: bool,
    pointer: (f32, f32),
    pressed_at: Option<(f32, f32, Instant)>,
    dragging: bool,
    last_click: Option<Instant>,
    bounds: Rect,
    /// Where the controls are, for what the pointer is over.
    bar: Rect,
    foot: Rect,
    side: Rect,
    trouble: Option<String>,
    /// When `trouble` was set: it goes 8 seconds after.
    trouble_at: Option<Instant>,
    /// A press on the controls while they were faded out, which only brought them back: its
    /// release is not a click either.
    swallowing: bool,
    /// Markup on this file -- the tools, and what is drawn with them -- once begun.
    desk: Option<Box<Desk>>,
    markup_button: Button,
    save_button: Button,
    copy_button: Button,
    /// A question being asked, and what follows its yes.
    asking: Option<(Confirm, Then)>,
    /// A save going on (a copy, or over the file), since when: the foot says so, and nothing is
    /// changed meanwhile.
    saving: Option<(Instant, bool)>,
    /// What was done, said at the foot for five seconds.
    said: Option<(String, Instant)>,
    /// Where the pages shown are on the window, as last drawn: what markup draws on.
    placed: Vec<Placed>,
    /// What the program is to do, come of something done between inputs (a save).
    wanted: Option<Want>,
    /// The picture's text (Live Text).
    lens: lens::Lens,
}

/// How long what was done stays on show.
const SAID_SHOWN: Duration = Duration::from_secs(5);

impl View {
    pub fn new(entry: Entry, index: usize, ticket: u64, files: Vec<usize>) -> View {
        let mut strip = Strip::default();
        strip.shape = Shape::Square;
        let place = files.iter().position(|&i| i == index).unwrap_or(0);
        strip.cursor = place;
        let reach = Reach { before: place, after: files.len().saturating_sub(place + 1) };
        View {
            entry,
            index,
            doc: Doc::Loading(Instant::now()),
            ticket,
            files,
            place,
            carousel: Carousel::new(reach),
            slid: 0.0,
            jump: None,
            arrived: None,
            drag_slides: false,
            strip_page: 0,
            strip,
            back: Button::default(),
            layout: Button::default(),
            info_button: Button::default(),
            info: false,
            clock: AutoHide::new(Instant::now()),
            hands_heard: false,
            pointer: (0.0, 0.0),
            pressed_at: None,
            dragging: false,
            last_click: None,
            bounds: Rect::new(0.0, 0.0, crate::WINDOW.0 as f32, crate::WINDOW.1 as f32),
            bar: Rect::default(),
            foot: Rect::default(),
            side: Rect::default(),
            trouble: None,
            trouble_at: None,
            swallowing: false,
            desk: None,
            markup_button: Button::default(),
            save_button: Button::default(),
            copy_button: Button::default(),
            asking: None,
            saving: None,
            said: None,
            placed: Vec::new(),
            wanted: None,
            lens: lens::Lens::new(),
        }
    }

    // --- markup ---

    /// Whether markup can draw on what is shown: a still flat picture, or a PDF's pages. Not an
    /// object, a stereo pair or a moving picture, whose button is not there.
    fn markable(&self) -> bool {
        match &self.doc {
            Doc::Picture(shown) => shown.markable(),
            Doc::Pdf(paper) => !paper.pages.is_empty(),
            _ => false,
        }
    }

    /// Whether markup's tools are out.
    fn marking(&self) -> bool {
        self.desk.as_ref().is_some_and(|d| d.on)
    }

    /// Whether anything is drawn or cut that the file has not got.
    pub fn unsaved(&self) -> bool {
        self.desk.as_ref().is_some_and(|d| d.edited())
    }

    /// Whether this file can be written over in its own format (a PDF always; a picture in what
    /// SpatiLoupe writes, as it was read), or only copied.
    fn can_save_in_place(&self) -> bool {
        match &self.doc {
            Doc::Pdf(_) => true,
            Doc::Picture(shown) => crate::save::in_place(&self.entry.name).is_some() && shown.picture.saves.not_over.is_none(),
            _ => crate::save::in_place(&self.entry.name).is_some(),
        }
    }

    /// What the foot says of a picture that is only copied: why, and what its copy is.
    fn copy_only_words(&self) -> String {
        let (why, depth) = match &self.doc {
            Doc::Picture(shown) => (shown.picture.saves.not_over.as_deref(), shown.picture.saves.depth),
            _ => (None, crate::save::Depth::Eight),
        };
        crate::save::copy_only(&self.entry.name, why.filter(|_| crate::save::in_place(&self.entry.name).is_some()), depth)
    }

    /// Why markup does not draw on this file, though there is something to draw on: a PDF whose
    /// owner does not allow annotating it (its permissions, as Preview keeps to them).
    fn forbidden(&self) -> Option<String> {
        match &self.doc {
            Doc::Pdf(paper) if !paper.rights.annotate => Some(format!("The owner of {} does not allow marking it up: its permissions keep notes out.", self.entry.name)),
            _ => None,
        }
    }

    /// The tools out, or put away; put away with nothing drawn, markup is let go of altogether.
    fn toggle_markup(&mut self) {
        if self.marking() {
            if let Some(desk) = self.desk.as_mut() {
                desk.put_away();
            }
            if !self.unsaved() {
                self.desk = None;
            }
        } else if let Some(why) = self.forbidden().filter(|_| self.markable()) {
            // Not offered: said why, at the foot.
            self.trouble = Some(why);
            self.trouble_at = Some(Instant::now());
            return;
        } else if self.markable() {
            // A picture is drawn on with Chromium's tools; a PDF with its own, for now.
            let (turns, may_crop, picture) = match &self.doc {
                Doc::Picture(shown) => (shown.orientation, true, true),
                Doc::Pdf(paper) => (0, paper.rights.modify, false),
                _ => (0, true, false),
            };
            let desk = self.desk.get_or_insert_with(|| Box::new(Desk::new(turns, picture)));
            desk.on = true;
            // A PDF whose pages its owner does not allow changed is not cropped.
            desk.may_crop = may_crop;
            self.info = false;
            // The room's fonts read now, beside the window's thread, rather than at the first
            // word painted.
            std::thread::spawn(|| drop(crate::ink::fonts()));
        }
        self.sync_markup();
    }

    /// What is shown made to agree with the marks: a picture turned and cropped as they say, a
    /// PDF's pages cropped; and the room for it under the tools, while they are out.
    fn sync_markup(&mut self) {
        // Under the tools: the bar's foot (108), their 12 above and 12 below.
        let head = if self.marking() { HEAD + BAR + 36.0 } else { HEAD };
        let marks = self.desk.as_ref().map(|d| d.markup.marks());
        match &mut self.doc {
            Doc::Picture(shown) => {
                shown.head = head;
                let turns = marks.map_or(shown.orientation, |m| m.turns);
                if turns != shown.orientation {
                    // Undone or done again: the picture turned back, or on, with its marks.
                    shown.turns = (shown.turns + (turns + 4 - shown.orientation)) % 4;
                    shown.orientation = turns;
                    shown.zoom = None;
                    shown.centre = (0.5, 0.5);
                }
                let (w, h) = shown.frame_size();
                let cut = marks.and_then(|m| m.crop(0)).map_or(WHOLE, |a| [a.x0 / w.max(1.0), a.y0 / h.max(1.0), a.x1 / w.max(1.0), a.y1 / h.max(1.0)]);
                if cut != shown.cut {
                    shown.cut = cut;
                    shown.zoom = None;
                    shown.centre = (0.5, 0.5);
                }
            }
            Doc::Pdf(paper) => {
                if head != paper.head {
                    // The pages stay where they were on the window as the tools come and go -- but
                    // for the first page's top, which comes out from under them.
                    if paper.scroll > 0.0 {
                        paper.scroll = (paper.scroll + head - paper.head).max(0.0);
                    }
                    paper.head = head;
                }
                let cuts: HashMap<usize, Area> = marks.map(|m| m.crops.iter().copied().collect()).unwrap_or_default();
                if cuts != paper.cuts {
                    paper.cuts = cuts;
                }
            }
            _ => {}
        }
    }

    /// Each page's part shown, in its units, by its number: what markup's weights are a share of.
    fn regions(&self) -> Vec<Area> {
        match &self.doc {
            Doc::Picture(shown) => {
                let (w, h) = shown.frame_size();
                let c = shown.cut;
                vec![Area::new(c[0] * w, c[1] * h, c[2] * w, c[3] * h)]
            }
            Doc::Pdf(paper) => (0..paper.pages.len()).map(|page| paper.region(page)).collect(),
            _ => Vec::new(),
        }
    }

    /// Where the pages shown are on the window `all`.
    fn placements(&self, all: Rect) -> Vec<Placed> {
        match &self.doc {
            Doc::Picture(shown) if shown.markable() => {
                let at = shown.placed(all);
                let region = self.regions()[0];
                vec![Placed { page: 0, rect: Rect { x: at.x + self.slid, ..at }, region }]
            }
            Doc::Pdf(paper) => paper.placed.iter().map(|&(page, rect)| Placed { page, rect, region: paper.region(page) }).collect(),
            _ => Vec::new(),
        }
    }

    /// Whether the picture has an HDR gain map SpatiLoupe cannot carry over: saved, it is SDR.
    fn loses_hdr(&self) -> bool {
        matches!(&self.doc, Doc::Picture(shown) if shown.picture.saves.hdr == Some(false))
    }

    /// A question asked, with what follows its yes. Each destroys something, and starts on Cancel.
    fn ask(&mut self, then: Then) {
        let name = &self.entry.name;
        let text = match then {
            Then::Replace if self.loses_hdr() => format!("Replace {name} with the edited picture? Its HDR layer will be lost: SpatiLoupe cannot carry it over. This cannot be undone."),
            Then::Replace => {
                let what = if matches!(self.doc, Doc::Pdf(_)) { "PDF" } else { "picture" };
                format!("Replace {name} with the edited {what}? This cannot be undone.")
            }
            Then::Copy => format!("Save a copy of {name} without its HDR layer? SpatiLoupe cannot carry it over."),
            _ => format!("Discard your changes to {name}?"),
        };
        let yes = match then {
            Then::Replace => "Replace",
            Then::Copy => "Save a copy",
            _ => "Discard",
        };
        if let Some(desk) = self.desk.as_mut() {
            desk.finish_typing();
        }
        self.asking = Some((Confirm::careful(text, yes, "Cancel"), then));
    }

    /// A copy saved: asked first only if it would lose an HDR layer the picture has.
    fn save_copy(&mut self) -> Want {
        if self.loses_hdr() {
            self.ask(Then::Copy);
            return Want::Nothing;
        }
        self.save(true)
    }

    /// The window closed, or SpatiLoupe quit, with markup not saved: asked first.
    pub fn ask_quit(&mut self) {
        self.ask(Then::Quit);
    }

    fn answered(&mut self, yes: bool, then: Then) -> Want {
        if !yes {
            return Want::Nothing;
        }
        match then {
            Then::Replace => return self.save(false),
            Then::Copy => return self.save(true),
            _ => {}
        }
        // The changes let go of, and on to what was asked.
        self.desk = None;
        self.sync_markup();
        match then {
            Then::Leave => Want::Leave,
            Then::Slide(by) => self.go_by(by),
            Then::SlideTo(place) => self.go_to_place(place),
            Then::Quit => Want::Quit,
            Then::Replace | Then::Copy => Want::Nothing,
        }
    }

    /// Back to the folder: asked first if markup would be lost.
    fn leave(&mut self) -> Want {
        if self.unsaved() {
            self.ask(Then::Leave);
            return Want::Nothing;
        }
        Want::Leave
    }

    /// Markup written: over the file (asked already), or as a copy beside it.
    fn save(&mut self, copy: bool) -> Want {
        let Some(desk) = self.desk.as_mut() else { return Want::Nothing };
        desk.finish_typing();
        desk.cropping = None;
        let marks = desk.markup.marks().clone();
        let want = match &self.doc {
            Doc::Picture(shown) => Want::Save(Box::new(crate::save::Saving { source: self.entry.path.clone(), marks, frame: shown.frame_size(), copy })),
            Doc::Pdf(paper) => {
                paper.pdf.ask(Ask::Save { marks, copy });
                Want::Nothing
            }
            _ => return Want::Nothing,
        };
        self.saving = Some((Instant::now(), copy));
        want
    }

    /// What came of a save: the file shown again as it now is -- a locked PDF opened with the
    /// `password` it was opened with -- or why it was not written.
    pub fn saved(&mut self, result: Result<PathBuf, String>, password: Option<pdf::Secret>) {
        let copy = self.saving.take().is_some_and(|(_, copy)| copy);
        match result {
            Ok(path) => {
                let said = if copy { format!("Saved a copy as {}.", crate::place::name(&path)) } else { format!("Saved {}.", self.entry.name) };
                if let Some(desk) = self.desk.as_mut() {
                    desk.markup.saved();
                }
                self.wanted = Some(Want::Saved { path, said, password });
            }
            Err(e) => {
                log::warn!("{}: not saved: {e}", self.entry.path.display());
                self.trouble = Some(format!("SpatiLoupe could not save {}: {}.", self.entry.name, e.trim_end_matches('.')));
                self.trouble_at = Some(Instant::now());
            }
        }
    }

    /// What the program is to do, if something done between inputs came to anything.
    pub fn take_wanted(&mut self) -> Option<Want> {
        self.wanted.take()
    }

    /// Something done, said at the foot.
    pub fn say(&mut self, words: String) {
        self.said = Some((words, Instant::now()));
    }

    /// A quarter turn: through markup's steps while there is markup, which turns the marks with
    /// the picture and can be undone.
    fn turn(&mut self, clockwise: bool) {
        let Doc::Picture(shown) = &mut self.doc else { return };
        if let Some(desk) = self.desk.as_mut() {
            desk.finish_typing();
            desk.cropping = None;
            desk.markup.turn(shown.frame_size(), clockwise);
            self.sync_markup();
            return;
        }
        shown.turns = (shown.turns + if clockwise { 1 } else { 3 }) % 4;
        shown.orientation = (shown.orientation + if clockwise { 1 } else { 3 }) % 4;
        shown.zoom = None;
        shown.centre = (0.5, 0.5);
        // A pair's text was read in one eye as it was: not carried through the turn.
        self.text_turned();
    }

    /// The PDF opened; `password` one it was opened with before (saved, or its copy written),
    /// tried first.
    pub fn open_pdf(&mut self, password: Option<pdf::Secret>) {
        self.doc = Doc::Pdf(Box::new(Paper::new(Pdf::open(&self.entry.path, password))));
    }

    pub fn pictured(&mut self, result: Result<Picture, String>) {
        self.doc = match result {
            Ok(picture) => Doc::Picture(Box::new(Shown::new(picture))),
            Err(e) => Doc::Failed(e),
        };
        // Read already this session: its text at once.
        self.text_known();
        self.stir();
    }

    pub fn modelled(&mut self, result: Result<model::Scene, String>) {
        self.doc = match result {
            Ok(scene) => Doc::Model(Box::new(Object {
                triangles: scene.triangles,
                name: scene.name.clone(),
                renderer: None,
                scene: Some(scene),
                orbit: Orbit::default(),
                turning: true,
                turned_at: Instant::now(),
            })),
            Err(e) => Doc::Failed(format!("This object would not open: {e}")),
        };
        self.stir();
    }

    /// Whether this wants a picture for each eye, given the chance.
    /// The window's size, which everything here is laid out in.
    pub fn set_size(&mut self, size: (u32, u32)) {
        self.bounds = Rect::new(0.0, 0.0, size.0 as f32, size.1 as f32);
        self.stir();
    }

    /// Whether the window's ground is to be clear: a picture's, shown whole on nothing -- and one
    /// still being read, shown by its tile on nothing as it will be, rather than the window going
    /// opaque between one picture and the next.
    pub fn see_through(&self) -> bool {
        match self.doc {
            Doc::Picture(_) => true,
            Doc::Loading(_) => self.entry.kind == Kind::Picture,
            _ => false,
        }
    }

    pub fn has_depth(&self) -> bool {
        match &self.doc {
            Doc::Picture(shown) => shown.eyes().is_stereo(),
            Doc::Model(_) => true,
            // A stereo picture being read, by what its tile said: the window stays two eyes from
            // one stereo picture to the next, rather than going to one and back as each is read.
            Doc::Loading(_) => seen(self.place).is_some_and(|s| s.stereo),
            _ => false,
        }
    }

    /// A press, a key, a scroll, a picture arriving: the controls on show from now.
    fn stir(&mut self) {
        self.clock.wake(Instant::now());
    }

    /// How much of the controls is to be seen, 0 to 1.
    fn chrome(&self) -> f32 {
        // A document is read, not looked at: its controls stay. So do they while there is markup,
        // or a question.
        if matches!(self.doc, Doc::Pdf(_) | Doc::Loading(_) | Doc::Failed(_)) || self.info || self.desk.is_some() || self.asking.is_some() || self.saving.is_some() || self.said.is_some() || self.text_busy() {
            return 1.0;
        }
        match self.clock.off_since(Instant::now()) {
            None => 1.0,
            Some(gone) => (1.0 - gone.as_secs_f32() / 0.5).max(0.0),
        }
    }

    pub fn animating(&self) -> bool {
        let fading = self.chrome() > 0.0 && self.chrome() < 1.0 || self.clock.off_since(Instant::now()).is_none_or(|gone| gone < Duration::from_millis(600));
        let moving = match &self.doc {
            Doc::Loading(_) => true,
            // A moving picture is not drawn over and over for it: `next_wake` wakes the program
            // when its next frame is due, and `tick` says it is to be drawn.
            Doc::Picture(shown) => shown.faded_in() < 1.0,
            Doc::Model(object) => object.turning || object.renderer.is_none(),
            Doc::Pdf(paper) => paper.pages.is_empty() || paper.finding,
            Doc::Failed(_) => false,
        };
        let sliding = !self.carousel.resting() || self.carousel.busy();
        fading || moving || sliding
    }

    /// When the next picture is wanted: a moving picture's next frame, or the moment a scroll
    /// that has stopped coming is over and the carousel lets it go.
    pub fn next_wake(&self) -> Option<Duration> {
        let doc = match &self.doc {
            Doc::Picture(shown) if !shown.picture.frames.is_empty() => {
                let length = if shown.frame == 0 { shown.picture.first } else { shown.picture.frames[shown.frame - 1].1 };
                Some(length.saturating_sub(shown.frame_since.elapsed()))
            }
            Doc::Model(object) if object.turning => Some(Duration::from_millis(16)),
            _ => None,
        };
        let carousel = self.carousel.deadline().map(|at| at.saturating_duration_since(Instant::now()));
        let trouble = self.trouble_at.map(|at| TROUBLE_SHOWN.saturating_sub(at.elapsed()));
        let said = self.said.as_ref().map(|(_, at)| SAID_SHOWN.saturating_sub(at.elapsed()));
        [doc, carousel, trouble, said, self.text_wake()].into_iter().flatten().min()
    }

    /// What has come back from the PDF's thread.
    pub fn answers(&mut self) -> bool {
        let by_itself = self.lens.by_itself();
        let Doc::Pdf(paper) = &mut self.doc else { return false };
        let mut any = false;
        loop {
            let answer = match paper.pdf.answers.try_recv() {
                Ok(answer) => answer,
                // Its thread has gone -- panicked in PDFium -- before the document was open: said,
                // rather than waiting for ever.
                Err(std::sync::mpsc::TryRecvError::Disconnected) if paper.pages.is_empty() => {
                    self.doc = Doc::Failed("This PDF would not open: SpatiLoupe failed reading it.".into());
                    return true;
                }
                Err(_) => break,
            };
            any = true;
            match answer {
                pdf::Answer::Opened { pages, outline, title, rights } => {
                    if pages.is_empty() {
                        self.doc = Doc::Failed("This PDF has no pages.".into());
                        return true;
                    }
                    paper.pages = pages;
                    paper.outline = outline;
                    paper.title = title;
                    paper.rights = rights;
                    paper.lock = None;
                }
                // Its password asked for -- again, if the one given was wrong: what was typed is
                // not kept either way.
                pdf::Answer::Locked { wrong } => {
                    let lock = paper.lock.get_or_insert_with(|| Lock::new(wrong));
                    lock.forget();
                    lock.wrong = wrong;
                    lock.trying = false;
                    lock.field.focused = true;
                }
                pdf::Answer::Failed(why) => {
                    self.doc = Doc::Failed(why);
                    return true;
                }
                // A page drawn is always wanted: its texture is what it waited for, and it is no
                // longer asked for once it has come.
                pdf::Answer::Drawn { page, width, height, rgba } => {
                    paper.asked.remove(&page);
                    paper.drawn_pending(page, width, height, rgba);
                }
                // Left as asked, so that it is not asked for again and again: it stays paper.
                pdf::Answer::Undrawn { page } => log::warn!("page {} of {} would not draw", page + 1, self.entry.path.display()),
                pdf::Answer::Text { page, text } => {
                    // A few pages' worth kept: they are asked for as they are read.
                    if paper.words.len() > 64 {
                        paper.words.retain(|p, _| p.abs_diff(page) <= 8);
                    }
                    paper.words.insert(page, Some(text));
                }
                pdf::Answer::Found { found, ticket } => {
                    if ticket != paper.finds {
                        continue;
                    }
                    paper.finding = false;
                    paper.found = found;
                    paper.found_at = 0;
                    if let Some(first) = paper.found.first() {
                        paper.go_to = Some(first.page);
                    }
                }
                pdf::Answer::Saved { result, password } => {
                    self.saved(result, password);
                    return true;
                }
                // Live Text: a page's own characters, or word that it has none and is to be read;
                // and a page drawn to be read.
                pdf::Answer::Letters { page, lines } => paper.lettered(page, lines, by_itself),
                pdf::Answer::Scanned { page, width, height, rgba } => paper.scanned(page, width, height, rgba),
            }
        }
        any
    }

    /// Move the carousel on, and a frame of a moving picture, or turn an object, if it is time.
    pub fn tick(&mut self) -> bool {
        let slid = self.slide_on();
        let gone = self.trouble_at.is_some_and(|at| at.elapsed() >= TROUBLE_SHOWN);
        if gone {
            self.trouble = None;
            self.trouble_at = None;
        }
        let unsaid = self.said.as_ref().is_some_and(|(_, at)| at.elapsed() >= SAID_SHOWN);
        if unsaid {
            self.said = None;
        }
        // The picture's text: read once it has settled, and what came of it.
        let text = self.text_tick() | self.text_answers();
        self.frame_on() || slid || gone || unsaid || text
    }

    /// Move the carousel on to now, and say where it is for the frame about to be drawn.
    fn slide_on(&mut self) -> bool {
        if let Some(page) = self.carousel.tick(Instant::now(), self.page()) {
            self.arrived = self.place_of(page);
        }
        let was = std::mem::replace(&mut self.slid, self.carousel.offset());
        // The strip, and the count in the bar, say where the carousel is going.
        let going = self.carousel.settling_to().unwrap_or(0);
        if going != self.strip_page {
            self.strip_page = going;
            self.strip.cursor = self.place_of(going).unwrap_or(self.place);
        }
        // A file jumped to and not gone to is forgotten once the carousel is back in its place.
        if self.carousel.resting() {
            self.jump = None;
        }
        was != self.slid
    }

    /// The file the carousel has come to rest on, if it has: the program opens it in place of
    /// this one.
    pub fn arrival(&mut self) -> Want {
        match self.arrived.take().and_then(|place| self.files.get(place)) {
            Some(&index) if index != self.index => Want::Open(index),
            _ => Want::Nothing,
        }
    }

    /// The file the carousel is going to, or is being taken towards, by its place in the folder:
    /// worth reading before it gets there.
    pub fn heading(&self) -> Option<usize> {
        let page = self.carousel.heading(self.page())?;
        self.place_of(page).and_then(|place| self.files.get(place).copied())
    }

    /// What this view hands on to the one the carousel brings in: a throw's glide, still going.
    pub fn hand_on(&self) -> Option<Glide> {
        self.carousel.glide()
    }

    pub fn handed(&mut self, glide: Option<Glide>) {
        self.carousel.carry(glide);
    }

    /// How far one page of the carousel is from the next: the window's width, and a gap.
    fn page(&self) -> f32 {
        self.bounds.w + BESIDE_GAP
    }

    /// Where among the files the carousel's `page` is: the file that many along, or the one
    /// jumped to on the page it comes in on.
    fn place_of(&self, page: i32) -> Option<usize> {
        if let Some((_, place)) = self.jump.filter(|(jumped, _)| *jumped == page) {
            return Some(place);
        }
        let place = self.place as i64 + page as i64;
        (0..self.files.len() as i64).contains(&place).then_some(place as usize)
    }

    /// Whether a hand may take what is shown sideways to the next: a picture at its fitted size,
    /// and one still being read or that would not open. A zoomed picture is moved about in, a
    /// document's pages scroll, and an object turns.
    fn slides(&self) -> bool {
        // With markup -- its tools out, or something drawn not yet saved -- a drag draws, or moves
        // about in the picture; leaving it is asked about first.
        if self.desk.is_some() {
            return false;
        }
        match &self.doc {
            Doc::Picture(shown) => shown.zoom.is_none(),
            Doc::Loading(_) | Doc::Failed(_) => true,
            Doc::Pdf(_) | Doc::Model(_) => false,
        }
    }

    /// The next frame of a moving picture, or an object turned, if it is time.
    fn frame_on(&mut self) -> bool {
        match &mut self.doc {
            Doc::Picture(shown) if !shown.picture.frames.is_empty() => {
                let length = if shown.frame == 0 { shown.picture.first } else { shown.picture.frames[shown.frame - 1].1 };
                if shown.frame_since.elapsed() >= length {
                    shown.frame = (shown.frame + 1) % (shown.picture.frames.len() + 1);
                    shown.frame_since = Instant::now();
                    return true;
                }
                false
            }
            Doc::Model(object) if object.turning => {
                let dt = object.turned_at.elapsed().as_secs_f32().min(0.1);
                object.turned_at = Instant::now();
                object.orbit.yaw += dt * 0.35;
                true
            }
            _ => false,
        }
    }

    /// Draw one eye's picture -- `eye` -- into `part` of the window's picture of `size`.
    pub fn paint(&mut self, p: &mut Painter, gl: &Gl, size: (u32, u32), part: Rect, eye: Eye) {
        self.sync_markup();
        let logical = (self.bounds.w, self.bounds.h);
        // An object is drawn first, into a picture of its own, since it needs depth.
        let mut drawn_object = None;
        if let Doc::Model(object) = &mut self.doc {
            if object.renderer.is_none() {
                if let Some(scene) = object.scene.take() {
                    match Renderer::new(gl, scene) {
                        Ok(r) => object.renderer = Some(r),
                        Err(e) => {
                            self.doc = Doc::Failed(format!("This object would not draw: {e}"));
                        }
                    }
                }
            }
        }
        if let Doc::Model(object) = &mut self.doc {
            if let Some(renderer) = object.renderer.as_mut() {
                let canvas = (part.w.round() as u32, part.h.round() as u32);
                let texture = renderer.draw(gl, canvas, &object.orbit, eye, |gl| {
                    p.begin(gl, canvas, canvas.0 as f32 / logical.0, theme::ground());
                    look::backdrop(p, gl);
                    p.end(gl);
                });
                match texture {
                    Ok(texture) => drawn_object = Some(Texture::from_raw(texture, canvas.0, canvas.1)),
                    Err(e) => {
                        self.trouble = Some(e);
                        self.trouble_at = Some(Instant::now());
                    }
                }
                unsafe { gl.api().viewport(0, 0, size.0 as i32, size.1 as i32) };
            }
        }
        p.begin_over(gl, size, part, logical);
        let all = p.bounds();
        // This file's page of the carousel, where the hand or a settle has slid it, and as much
        // of it as is in the window and no more: in the room each eye's half is its own, and
        // what ran over the edge of one would be drawn into the other.
        let here = Rect { x: all.x + self.slid, ..all };
        let seen_here = overlap(here, all);
        let own = stand_in(self.place);
        match drawn_object {
            // Drawn by GL, foot first: turned the right way up here.
            Some(texture) => {
                p.clip(gl, seen_here);
                p.image(gl, texture, here, [0.0, 1.0, 1.0, 0.0], 0.0, [1.0, 1.0, 1.0, 1.0], true);
                p.unclip(gl);
            }
            // A picture is on nothing: where it does not fill the window, the room shows through.
            None if self.see_through() => {}
            None => look::backdrop(p, gl),
        }
        let chrome = self.chrome();
        p.clip(gl, seen_here);
        // Where the picture is drawn this frame, for its text to be drawn over it.
        let mut picture_at = None;
        match &mut self.doc {
            // What it will be, where it will be, until it is: its tile, and after a while the dots
            // that say it is still coming.
            Doc::Loading(since) => {
                if let Some(own) = &own {
                    draw_stand_in(p, gl, here, own);
                }
                if own.as_ref().map_or(true, |own| own.picture.is_none()) || since.elapsed() > STILL_READING {
                    look::waiting(p, gl, here, *since);
                }
            }
            Doc::Failed(why) => look::message(p, gl, here.inset(120.0), why, look::trouble()),
            Doc::Picture(shown) => {
                if shown.texture.is_none() {
                    shown.texture = upload(gl, &shown.picture.rgba, shown.picture.width, shown.picture.height).ok();
                    if let Some((w, h, rgba)) = shown.picture.right.as_ref() {
                        shown.right = upload(gl, rgba, *w, *h).ok();
                    }
                    shown.frame = 0;
                    shown.uploaded = 0;
                    // Once: put back after a quarter turn, it is not new.
                    shown.drawn_at.get_or_insert_with(Instant::now);
                }
                if let Some(texture) = shown.texture {
                    if !shown.picture.frames.is_empty() && shown.uploaded != shown.frame {
                        let rgba = if shown.frame == 0 { &shown.picture.rgba } else { &shown.picture.frames[shown.frame - 1].0 };
                        if rgba.len() == (texture.width * texture.height * 4) as usize {
                            replace(gl, texture, rgba);
                        }
                        shown.uploaded = shown.frame;
                    }
                    let use_right = shown.eyes() == Eyes::Pair && (eye == Eye::Right) != shown.swapped;
                    let which = if use_right { shown.right.unwrap_or(texture) } else { texture };
                    let at = shown.placed(all);
                    let at = Rect { x: at.x + self.slid, ..at };
                    let uv = turned(shown.uv(eye), shown.turns);
                    let fitted = shown.zoom.is_none();
                    // Magnified more than twice, a pixel at a time: the picture's pixels, not a blur.
                    let nearest = at.w * p.scale() > shown.eye_pixels().0 * 2.0;
                    if nearest != shown.nearest {
                        shown.nearest = nearest;
                        let filter = if nearest { glow::NEAREST } else { glow::LINEAR } as i32;
                        for texture in [shown.texture, shown.right].into_iter().flatten() {
                            unsafe {
                                gl.api().bind_texture(glow::TEXTURE_2D, Some(texture.raw()));
                                gl.api().tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter);
                            }
                        }
                    }
                    // Come in over the tile that stood in for it, rather than sharpening at once.
                    let fade = if fitted { shown.faded_in() } else { 1.0 };
                    if fade < 1.0 {
                        if let Some(own) = &own {
                            draw_stand_in(p, gl, here, own);
                        }
                    }
                    // A picture with see-through parts is shown on a checkerboard-free dark
                    // ground: the backdrop already is one.
                    p.image(gl, which, at, uv, if fitted { 6.0 } else { 0.0 }, [1.0, 1.0, 1.0, fade], false);
                    picture_at = Some(at);
                }
            }
            Doc::Pdf(paper) => draw_paper(p, gl, paper, here, seen_here, &self.entry.name),
            Doc::Model(_) => {}
        }
        if let Some(at) = picture_at {
            self.text_draw_picture(p, gl, at);
        }
        p.unclip(gl);
        // Markup: where the pages are on the window, and what is drawn on them.
        self.placed = self.placements(all);
        if self.desk.is_some() {
            let regions = self.regions();
            if let Some(desk) = self.desk.as_mut() {
                desk.regions = regions;
                desk.paint(p, gl, all, &self.placed);
            }
        }
        self.draw_neighbours(p, gl, all);
        if chrome > 0.0 {
            self.controls(p, gl, all, chrome);
        }
        // The picture's text being read: said at the foot, over the strip, till it has come.
        if self.text_reading() && self.trouble.is_none() && self.said.is_none() && self.asking.is_none() {
            let over = if self.foot.h > 0.0 && chrome > 0.05 { self.foot.y - 12.0 } else { all.bottom() - 18.0 };
            hint_pane_over(p, gl, all, over, "Reading the text\u{2026}");
        }
        // Over everything: the text's menu, or its translation.
        self.text_draw_over(p, gl, all);
        p.end(gl);
    }

    /// The files beside this one on the carousel, wherever it has been slid to: each its tile, on
    /// its own page, fitted as its picture will be when it is the one looked at. Both eyes are
    /// shown the same, as a flat picture beside the one with depth.
    fn draw_neighbours(&self, p: &mut Painter, gl: &Gl, all: Rect) {
        if self.slid == 0.0 {
            return;
        }
        let page = self.page();
        let at = -self.slid / page;
        for k in (at.floor() as i32 - 1)..=(at.ceil() as i32 + 1) {
            let slot = Rect { x: all.x + self.slid + k as f32 * page, ..all };
            let seen_there = overlap(slot, all);
            if k == 0 || seen_there.w <= 0.0 {
                continue;
            }
            let Some(neighbour) = self.place_of(k).and_then(stand_in) else { continue };
            p.clip(gl, seen_there);
            draw_stand_in(p, gl, slot, &neighbour);
            p.unclip(gl);
        }
    }

    fn title_words(&self) -> (String, String) {
        let name = self.entry.name.clone();
        let mut detail = Vec::new();
        match &self.doc {
            Doc::Picture(shown) => {
                // Cropped by markup: what is kept of it.
                let (w, h) = shown.frame_size();
                let (w, h) = ((w * (shown.cut[2] - shown.cut[0])).round() as u32, (h * (shown.cut[3] - shown.cut[1])).round() as u32);
                detail.push(format!("{w} \u{d7} {h}"));
                if let Some(zoom) = shown.zoom {
                    let ratio = shown.picture.original.0 as f32 / shown.picture.width.max(1) as f32;
                    detail.push(format!("{:.0}%", 100.0 / zoom / ratio));
                }
                if !shown.picture.frames.is_empty() {
                    detail.push(format!("{} frames", shown.picture.frames.len() + 1));
                }
            }
            Doc::Pdf(paper) if !paper.pages.is_empty() => {
                let page = paper.current(self.bounds) + 1;
                detail.push(format!("Page {page} of {}", paper.pages.len()));
                if (paper.zoom - 1.0).abs() > 0.01 {
                    detail.push(format!("{:.0}%", paper.zoom * 100.0));
                }
            }
            Doc::Model(object) => {
                if let Some(n) = &object.name {
                    detail.push(n.clone());
                }
                detail.push(format!("{} triangles", object.triangles));
            }
            _ => {}
        }
        detail.push(look::size(self.entry.size));
        (name, detail.join("   \u{b7}   "))
    }

    fn controls(&mut self, p: &mut Painter, gl: &Gl, all: Rect, alpha: f32) {
        let ink = |c: [f32; 4]| [c[0], c[1], c[2], c[3] * alpha];
        // The bar along the top.
        let bar = Rect::new(all.x + 28.0, all.y + 24.0, all.w - 56.0, theme::ROW_HEIGHT + 24.0);
        self.bar = bar;
        look::glass(p, gl, bar, 22.0, alpha);
        let mut inner = bar.inset(12.0);
        let back = inner.take_left(Button::icon_width(p, gl, "Folder"));
        self.back.draw_icon(p, gl, back, Icon::Back, "Folder");
        inner.take_left(20.0);
        let (name, detail) = self.title_words();
        // On the right: what kind of pair it is, and where in the folder.
        let place = format!("{} / {}", self.strip.cursor + 1, self.files.len().max(1));
        let place_w = p.measure(gl, &place, theme::DETAIL_EM, 8192.0).0;
        let info_w = Button::HEIGHT;
        let mut right = Rect { x: inner.right() - info_w, w: info_w, ..inner };
        self.info_button.draw_icon(p, gl, right, Icon::Info, "");
        // How a pair is packed: not while there is markup, which is drawn on it as it is.
        match &self.doc {
            Doc::Picture(shown) if self.desk.is_none() => {
                let label = if shown.eyes().is_stereo() { format!("3D  \u{b7}  {}", short(shown.eyes())) } else { "2D".to_string() };
                let width = Button::icon_width(p, gl, &label);
                right = Rect { x: right.x - width - 10.0, w: width, ..right };
                self.layout.draw_icon(p, gl, right, if shown.eyes().is_stereo() { Icon::Cube } else { Icon::Eye }, &label);
            }
            _ => self.layout = Button::default(),
        }
        // Markup's button, wherever there is something to draw on: lit while the tools are out.
        if self.markable() {
            right = Rect { x: right.x - Button::HEIGHT - 10.0, w: Button::HEIGHT, ..right };
            self.markup_button.focused = self.marking();
            self.markup_button.draw_icon(p, gl, right, Icon::Markup, "");
            self.markup_button.focused = false;
        } else {
            self.markup_button = Button::default();
        }
        let place_rect = Rect { x: right.x - place_w - 22.0, w: place_w, ..inner };
        p.label(gl, &place, place_rect, theme::DETAIL_EM, ink(theme::ink_hint()), 1.0);
        // With markup not saved: Save (over the file, where SpatiLoupe writes its format) and
        // Save a copy.
        let mut end = place_rect.x - 20.0;
        if self.unsaved() && self.saving.is_none() {
            let copy_w = Button::icon_width(p, gl, "Save a copy");
            let at = Rect { x: end - copy_w, w: copy_w, ..inner };
            end = at.x - 10.0;
            if self.can_save_in_place() {
                let save_w = Button::icon_width(p, gl, "Save");
                let save = Rect { x: at.x + copy_w - save_w, w: save_w, ..inner };
                let copy = Rect { x: save.x - 10.0 - copy_w, ..at };
                self.save_button.draw_icon(p, gl, save, Icon::Save, "Save");
                self.copy_button.draw_icon(p, gl, copy, Icon::Copy, "Save a copy");
                end = copy.x - 10.0;
            } else {
                self.save_button = Button::default();
                self.copy_button.draw_icon(p, gl, at, Icon::Copy, "Save a copy");
            }
        } else {
            self.save_button = Button::default();
            self.copy_button = Button::default();
        }
        let words = Rect { w: (end - inner.x).max(0.0), ..inner };
        let taken = p.label(gl, &name, words, theme::ROW_EM, ink(theme::ink()), 0.0);
        let after = Rect { x: words.x + taken + 22.0, w: (words.w - taken - 22.0).max(0.0), y: words.y + 2.0, ..words };
        p.label(gl, &detail, after, theme::DETAIL_EM * 0.95, ink(theme::ink_hint()), 0.0);
        look::rule(p, gl, Rect::new(bar.x + 22.0, bar.bottom() - 2.0, bar.w - 44.0, 2.0));

        // Markup's tools, under the bar, while they are out.
        let marking = self.marking();
        if let Some(desk) = self.desk.as_mut().filter(|d| d.on) {
            let w = desk.bar_width(p, gl).min(all.w - 56.0);
            desk.draw_bar(p, gl, Rect::new(all.x + (all.w - w) * 0.5, bar.bottom() + 12.0, w, BAR));
        }

        // The folder's other files, along the foot -- not while markup's tools are out, which keep
        // to this file; then which keys do what, as on a document always.
        self.foot = Rect::default();
        let hint = if self.saving.is_some() {
            Some("Saving\u{2026}".to_string())
        } else if marking {
            self.desk.as_ref().map(|d| d.hint())
        } else if let Doc::Pdf(paper) = &self.doc {
            Some(paper_hint(paper).to_string())
        } else {
            None
        };
        if let Some(hint) = hint {
            hint_pane(p, gl, all, &hint);
            // A picture SpatiLoupe does not write in its own format: said, over the keys.
            if self.unsaved() && !self.can_save_in_place() && self.saving.is_none() {
                let words = self.copy_only_words();
                hint_pane_over(p, gl, all, all.bottom() - 74.0, &words);
            }
        } else if self.files.len() > 1 && !matches!(self.doc, Doc::Pdf(_)) {
            let width = 118.0;
            let height = Strip::height(Shape::Square, width) - 34.0;
            let wanted = (self.files.len() as f32 * (width + theme::TILE_GAP) + 28.0).min(all.w - 200.0);
            let foot = Rect::new(all.x + (all.w - wanted) * 0.5, all.bottom() - height - 26.0, wanted, height);
            self.foot = foot;
            look::glass(p, gl, foot, 22.0, alpha);
            TILES.with(|tiles| self.strip.draw(p, gl, foot, &tiles.borrow(), width));
        } else if let Doc::Model(_) = self.doc {
            let foot = Rect::new(all.x + 28.0, all.bottom() - 70.0, all.w - 56.0, 40.0);
            look::keys(p, gl, foot, "[Drag] Turn  [Scroll] Nearer  [Space] Spin  [0] Reset", 0.5, alpha);
        }

        // About it, on the right.
        self.side = Rect::default();
        if self.info {
            let rows = self.about();
            let side = Rect::new(all.right() - 520.0, bar.bottom() + 18.0, 492.0, (rows.len() as f32 * 64.0 + 40.0).min(all.h - bar.bottom() - 140.0));
            self.side = side;
            look::glass(p, gl, side, 22.0, 1.0);
            let mut inner = side.inset(22.0);
            for (what, value) in rows {
                if inner.h < 50.0 {
                    break;
                }
                let line = inner.take_top(24.0);
                p.label(gl, &what.to_uppercase(), line, theme::FOOTER_EM * 0.82, theme::ink_hint(), 0.0);
                let line = inner.take_top(34.0);
                p.label(gl, &value, line, theme::DETAIL_EM, theme::ink_detail(), 0.0);
                inner.take_top(6.0);
            }
        }
        if let Some(trouble) = &self.trouble {
            // Left, wrapped, as every foot line is: over the picture's foot, where it floats.
            let mut line = Rect::new(all.x + 40.0, all.bottom() - 100.0, all.w - 80.0, 80.0);
            look::foot(p, gl, &mut line, "", Some(trouble));
        } else if let Some((said, _)) = &self.said {
            // What was done: words only, on their pane, over whatever is at the foot.
            let over = if self.foot.h > 0.0 { self.foot.y - 12.0 } else { all.bottom() - 74.0 };
            hint_pane_over(p, gl, all, over, said);
        }
        // Over everything: markup's colours or weights, open; and a question.
        if let Some(desk) = self.desk.as_mut().filter(|d| d.on) {
            desk.draw_popup(p, gl, all);
        }
        if let Some((confirm, _)) = &mut self.asking {
            confirm.draw(p, gl, all);
        }
    }

    /// What this shows, for the room's speech (`Event::Describe`): the words being typed into
    /// the find field or the page number if one is open, the current page's words of a PDF --
    /// as far as they have come, never waited for -- and what is known of a picture or an object.
    pub fn describe(&mut self) -> (String, String) {
        let title = self.title_words().0;
        let all = self.bounds;
        let text = match &mut self.doc {
            Doc::Pdf(paper) => {
                // Never what is typed into the password's field.
                if paper.lock.is_some() {
                    "This PDF is locked with a password, which it asks for.".to_string()
                } else if let Some(field) = paper.find.as_ref().or(paper.page_field.as_ref()) {
                    field.text.clone()
                } else if paper.pages.is_empty() {
                    String::new()
                } else {
                    let page = paper.current(all);
                    // The next page asked for too: it is the one read next.
                    if page + 1 < paper.pages.len() {
                        paper.words_on(page + 1);
                    }
                    paper.words_on(page).unwrap_or_default().to_string()
                }
            }
            _ => self.about().into_iter().map(|(what, value)| format!("{what}: {value}")).collect::<Vec<_>>().join("\n"),
        };
        // The text read in the picture, or in a scanned page: what is printed on it, for the room's
        // speech to answer from rather than read it again off the picture.
        let text = match self.text_described() {
            Some(read) => format!("{text}\n\n{read}"),
            None => text,
        };
        // Markup: the words being typed onto the page, or that its tools are out, and which.
        let text = match self.desk.as_ref().filter(|d| d.on) {
            Some(desk) => match &desk.typing {
                Some(typing) => typing.field.text.clone(),
                None => format!("{}\n{text}", desk.words_for_speech()),
            },
            None => text,
        };
        (title, text)
    }

    fn about(&self) -> Vec<(String, String)> {
        let mut rows = vec![("Name".to_string(), self.entry.name.clone())];
        if let Some(folder) = crate::place::parent(&self.entry.path) {
            rows.push(("Folder".to_string(), folder.display().to_string()));
        }
        rows.push(("Size".to_string(), look::size(self.entry.size)));
        match &self.doc {
            Doc::Picture(shown) => {
                let (w, h) = shown.picture.original;
                rows.push(("Pixels".to_string(), format!("{w} \u{d7} {h}  ({:.1} megapixels)", w as f64 * h as f64 / 1e6)));
                rows.push(("Eyes".to_string(), shown.eyes().name().to_string()));
                rows.extend(shown.picture.about.iter().cloned());
            }
            Doc::Pdf(paper) => {
                if let Some(title) = &paper.title {
                    rows.push(("Title".to_string(), title.clone()));
                }
                rows.push(("Pages".to_string(), paper.pages.len().to_string()));
                if let Some(page) = paper.pages.first() {
                    rows.push(("Page size".to_string(), format!("{:.0} \u{d7} {:.0} mm", page.width / 72.0 * 25.4, page.height / 72.0 * 25.4)));
                }
                let rights = paper.rights;
                if rights.encrypted {
                    let allowed = match (rights.annotate, rights.modify) {
                        (true, true) => "Encrypted; marking up allowed",
                        (true, false) => "Encrypted; notes allowed, its pages not to be changed (no crop)",
                        (false, _) => "Encrypted; its owner does not allow marking it up",
                    };
                    rows.push(("Security".to_string(), allowed.to_string()));
                }
            }
            Doc::Model(object) => {
                rows.push(("Triangles".to_string(), object.triangles.to_string()));
            }
            _ => {}
        }
        rows
    }

    fn over_controls(&self, x: f32, y: f32) -> bool {
        self.chrome() > 0.05 && (self.bar.contains(x, y) || self.foot.contains(x, y) || self.side.contains(x, y))
    }

    /// Slide on `by` files, as the arrow keys do: pressed again on the way, one further. The file
    /// is opened when the carousel arrives at it ([`View::arrival`]). Asked first, if markup would
    /// be lost; while markup's tools are out, nowhere.
    fn slide_by(&mut self, by: i32) -> Want {
        if self.marking() {
            return Want::Nothing;
        }
        if self.unsaved() {
            self.ask(Then::Slide(by));
            return Want::Nothing;
        }
        self.go_by(by)
    }

    fn go_by(&mut self, by: i32) -> Want {
        self.jump = None;
        self.carousel.slide(by, Instant::now(), self.page());
        Want::Nothing
    }

    /// Slide to the file at `place` among the files, as the strip and Home and End do: a
    /// neighbour as the keys do, and any other in beside this one, from the side it is on. Asked
    /// first, as [`View::slide_by`] is.
    fn slide_to(&mut self, place: usize) -> Want {
        if self.marking() {
            return Want::Nothing;
        }
        if self.unsaved() && place != self.place {
            self.ask(Then::SlideTo(place));
            return Want::Nothing;
        }
        self.go_to_place(place)
    }

    fn go_to_place(&mut self, place: usize) -> Want {
        let by = place as i64 - self.place as i64;
        let page = by.clamp(-1, 1) as i32;
        self.jump = (by.abs() > 1).then_some((page, place));
        self.carousel.go_to(page, Instant::now(), self.page());
        Want::Nothing
    }

    /// The room says the wearer's hands stirred: the controls come back, and from now on
    /// pointer motion alone (which sweeps with the head) does not bring them.
    pub fn hands(&mut self) {
        self.hands_heard = true;
        self.clock.stir(Instant::now());
    }

    /// The room says the hands (`App::says_hands`): pointer motion never wakes the controls,
    /// for this file as for the ones before it.
    pub fn hands_said(&mut self, said: bool) {
        self.hands_heard |= said;
    }

    pub fn input(&mut self, input: &Input) -> Want {
        let motion = matches!(input, Input::Pointer { .. } | Input::PointerLeft);
        // Whether the controls were there to be scrolled before this woke them.
        let controls_were_up = self.chrome() > 0.05;
        if !motion || !self.hands_heard || self.pressed_at.is_some() {
            self.stir();
        }
        // A question has everything until it is answered.
        if let Some((confirm, _)) = &mut self.asking {
            if let Input::Pointer { x, y } = input {
                self.pointer = (*x, *y);
            }
            return match confirm.input(input) {
                Some(yes) => {
                    let then = self.asking.take().map_or(Then::Leave, |(_, then)| then);
                    self.answered(yes, then)
                }
                None => Want::Nothing,
            };
        }
        // Being written: nothing is changed meanwhile.
        if self.saving.is_some() {
            if let Input::Pointer { x, y } = input {
                self.pointer = (*x, *y);
            }
            return Want::Nothing;
        }
        // Markup's own keys, whether its tools are out or not: Command (Control) and Shift and A
        // as Preview has them, S to save, and with Shift a copy.
        if let Input::Key { key: Key::Char(c @ ('a' | 's')), modifiers, .. } = input {
            if modifiers.control || modifiers.logo {
                match (*c, modifiers.shift) {
                    ('a', true) => {
                        self.toggle_markup();
                        return Want::Nothing;
                    }
                    ('s', copy) if self.unsaved() => {
                        if copy || !self.can_save_in_place() {
                            return self.save_copy();
                        }
                        self.ask(Then::Replace);
                        return Want::Nothing;
                    }
                    _ => {}
                }
            }
        }
        // The tools, while they are out, before anything else on the window: but for a press on
        // the window's own controls, which are theirs.
        if self.marking() {
            // (A press says nothing of where it is: the pointer as last told.)
            let (x, y) = self.pointer;
            let on_controls = matches!(input, Input::Press { pressed: true, .. }) && self.over_controls(x, y);
            if !on_controls {
                let placed = std::mem::take(&mut self.placed);
                let said = self.desk.as_mut().map_or(Said::Pass, |desk| desk.input(input, &placed));
                self.placed = placed;
                self.sync_markup();
                match said {
                    Said::Taken => return Want::Nothing,
                    Said::Done => {
                        self.toggle_markup();
                        return Want::Nothing;
                    }
                    Said::Pass => {}
                }
            }
        }
        let all = self.bounds;
        if let Input::Pointer { x, y } = input {
            let (dx, dy) = (x - self.pointer.0, y - self.pointer.1);
            self.pointer = (*x, *y);
            if let Some((px, py, _)) = self.pressed_at {
                if !self.dragging && ((x - px).abs() > 6.0 || (y - py).abs() > 6.0) {
                    self.dragging = true;
                    // The same swipe with markup not saved, its tools put away: still to the next
                    // file, asked about first.
                    if self.swipes_with_markup() && (x - px).abs() > (y - py).abs() {
                        self.pressed_at = None;
                        self.dragging = false;
                        self.ask(Then::Slide(if *x < px { 1 } else { -1 }));
                        return Want::Nothing;
                    }
                    // Mostly sideways over a fitted picture, the drag takes it to the next, from
                    // where the button went down; and any drag takes hold of a picture on its way.
                    self.drag_slides = (self.slides() && (x - px).abs() > (y - py).abs()) || !self.carousel.resting();
                    if self.drag_slides {
                        let (now, page) = (Instant::now(), self.page());
                        self.carousel.grab(now, page);
                        self.carousel.drag(x - px, now, page);
                        return Want::Nothing;
                    }
                }
                if self.dragging {
                    if self.drag_slides {
                        self.carousel.drag(dx, Instant::now(), self.page());
                    } else {
                        self.drag(dx, dy, all);
                    }
                    return Want::Nothing;
                }
            }
        }
        // The pointer taken away mid-drag: the picture is let go of, not held for ever.
        if matches!(input, Input::PointerLeft) && std::mem::take(&mut self.drag_slides) {
            self.carousel.release(Instant::now(), self.page());
        }
        // A locked document's password field has the keys: Enter gives what was typed to the
        // PDF's thread to try (and keeps none of it here), Escape gives up asking.
        if let Doc::Pdf(paper) = &mut self.doc {
            if let Some(lock) = paper.lock.as_mut() {
                if let Input::Key { key: Key::Escape, .. } = input {
                    let name = self.entry.name.clone();
                    self.doc = Doc::Failed(format!("{name} is locked with a password."));
                    return Want::Nothing;
                }
                if let Some(typed) = lock.field.input(input) {
                    if typed == Typed::Entered && !lock.trying {
                        let password = std::mem::take(&mut lock.field.text);
                        lock.forget();
                        if !password.is_empty() {
                            lock.trying = true;
                            lock.wrong = false;
                            paper.pdf.ask(Ask::Unlock(pdf::Secret::new(password)));
                        }
                    }
                    return Want::Nothing;
                }
                if matches!(input, Input::Key { .. }) {
                    return Want::Nothing;
                }
            }
        }
        // The search field, while there is one, has the keys.
        if let Doc::Pdf(paper) = &mut self.doc {
            if let Some(field) = paper.page_field.as_mut() {
                if let Input::Key { key: Key::Escape, .. } = input {
                    paper.page_field = None;
                    return Want::Nothing;
                }
                if let Some(typed) = field.input(input) {
                    if typed == Typed::Entered {
                        if let Ok(page) = field.text.trim().parse::<usize>() {
                            paper.go_to = Some(page.clamp(1, paper.pages.len().max(1)) - 1);
                        }
                        paper.page_field = None;
                    } else {
                        // Numbers only.
                        field.text.retain(|c| c.is_ascii_digit());
                    }
                    return Want::Nothing;
                }
                if matches!(input, Input::Key { .. }) {
                    return Want::Nothing;
                }
            }
            if let Some(field) = paper.find.as_mut() {
                if let Input::Key { key: Key::Escape, .. } = input {
                    paper.find = None;
                    paper.found.clear();
                    paper.forget_finding();
                    return Want::Nothing;
                }
                let before = field.text.clone();
                if let Some(typed) = field.input(input) {
                    if typed == Typed::Entered {
                        let text = field.text.trim().to_string();
                        if paper.found.is_empty() || paper.finding {
                            if !text.is_empty() {
                                paper.finds += 1;
                                paper.finding = true;
                                paper.pdf.ask(Ask::Find { text, ticket: paper.finds });
                            }
                        } else {
                            paper.found_at = (paper.found_at + 1) % paper.found.len();
                            paper.go_to = Some(paper.found[paper.found_at].page);
                        }
                    } else if field.text != before {
                        // Other words: what was found, or is being found, was for the old ones.
                        paper.found.clear();
                        paper.forget_finding();
                    }
                    return Want::Nothing;
                }
                if matches!(input, Input::Key { .. }) {
                    return Want::Nothing;
                }
            }
            if let Some(list) = paper.contents.as_mut() {
                if let Input::Key { key: Key::Escape, .. } = input {
                    paper.contents = None;
                    return Want::Nothing;
                }
                if let Some(row) = list.input(input) {
                    if let Some(page) = paper.outline.get(row).and_then(|h| h.page) {
                        paper.go_to = Some(page);
                    }
                    paper.contents = None;
                    return Want::Nothing;
                }
                if matches!(input, Input::Key { .. }) {
                    return Want::Nothing;
                }
            }
        }
        // The controls faded out are not there to be worked: a press where they were brings them
        // back and does nothing else, as a scroll there already did not scroll the strip.
        if let Input::Press { pressed, .. } = input {
            let (x, y) = self.pointer;
            if *pressed && !controls_were_up && (self.bar.contains(x, y) || self.foot.contains(x, y) || self.side.contains(x, y)) {
                self.swallowing = true;
                return Want::Nothing;
            }
            if !*pressed && std::mem::take(&mut self.swallowing) {
                return Want::Nothing;
            }
        }
        // The text in the picture or the page: a press and drag on it chooses, its menu and its
        // translation have everything while they are open. A press or a drag off it is the
        // window's own, as it always was.
        if let Some(want) = self.text_input(input) {
            return want;
        }
        if self.back.input(input) {
            return self.leave();
        }
        if self.info_button.input(input) {
            self.info = !self.info;
            return Want::Nothing;
        }
        if self.layout.input(input) {
            self.next_layout();
            return Want::Nothing;
        }
        if self.markup_button.input(input) {
            self.toggle_markup();
            return Want::Nothing;
        }
        if self.save_button.input(input) {
            self.ask(Then::Replace);
            return Want::Nothing;
        }
        if self.copy_button.input(input) {
            return self.save_copy();
        }
        if !self.foot.w.eq(&0.0) {
            if let Some(chosen) = self.strip.input(input) {
                return self.slide_to(chosen);
            }
            // A scroll that begins over the strip is the strip's, which it has had: it neither
            // zooms the picture nor slides it.
            if matches!(input, Input::Scroll { .. }) && controls_were_up && self.foot.contains(self.pointer.0, self.pointer.1) && !self.carousel.busy() {
                return Want::Nothing;
            }
        }
        match input {
            Input::Press { pressed: true, .. } => {
                if !self.over_controls(self.pointer.0, self.pointer.1) {
                    self.pressed_at = Some((self.pointer.0, self.pointer.1, Instant::now()));
                    self.dragging = false;
                }
            }
            Input::Press { pressed: false, .. } => {
                let was = self.pressed_at.take();
                let dragged = std::mem::take(&mut self.dragging);
                if std::mem::take(&mut self.drag_slides) {
                    self.carousel.release(Instant::now(), self.page());
                }
                if let Some(object) = self.object_mut() {
                    object.turned_at = Instant::now();
                }
                // A picture on its way somewhere is not zoomed by a click on it.
                if was.is_some() && !dragged && self.carousel.resting() {
                    // Twice quickly: nearer, or back to fitted.
                    let double = self.last_click.is_some_and(|t| t.elapsed() < DOUBLE_CLICK);
                    self.last_click = Some(Instant::now());
                    if double {
                        self.last_click = None;
                        self.toggle_zoom();
                    }
                }
            }
            Input::Scroll { dx, dy } => self.scrolled(*dx, *dy, all),
            Input::Pinch { scale } => self.pinched(*scale, all),
            Input::Key { key, text, modifiers, letter } => return self.key(*key, text.as_deref(), *letter, modifiers.control || modifiers.logo, modifiers.shift),
            _ => {}
        }
        Want::Nothing
    }

    fn object_mut(&mut self) -> Option<&mut Object> {
        match &mut self.doc {
            Doc::Model(object) => Some(object),
            _ => None,
        }
    }

    fn drag(&mut self, dx: f32, dy: f32, all: Rect) {
        match &mut self.doc {
            Doc::Picture(shown) if shown.zoom.is_some() => {
                let at = shown.placed(all);
                shown.centre.0 -= dx / at.w;
                shown.centre.1 -= dy / at.h;
                shown.clamp(all);
            }
            Doc::Model(object) => {
                object.turning = false;
                object.orbit.yaw -= dx * 0.008;
                object.orbit.pitch = (object.orbit.pitch + dy * 0.006).clamp(-1.45, 1.45);
            }
            Doc::Pdf(paper) => {
                paper.scroll_by(-dy, all);
                paper.pan_by(-dx, all);
            }
            _ => {}
        }
    }

    /// Whether a sideways swipe over the picture would have gone to the next file, but for markup
    /// not saved with its tools put away: it asks first.
    fn swipes_with_markup(&self) -> bool {
        !self.marking() && self.unsaved() && self.files.len() > 1 && matches!(&self.doc, Doc::Picture(shown) if shown.zoom.is_none())
    }

    fn scrolled(&mut self, dx: f32, dy: f32, all: Rect) {
        let (x, y) = self.pointer;
        if self.swipes_with_markup() && dx.abs() > 4.0 && dx.abs() > dy.abs() * 2.0 {
            self.ask(Then::Slide(if dx > 0.0 { 1 } else { -1 }));
            return;
        }
        // A scroll the carousel has begun on stays the carousel's whatever comes under it -- a
        // swipe is not made a pan by the picture it brings in, nor a zoom begun fitted a slide
        // once it has zoomed in -- and a picture on its way is the carousel's to take hold of.
        if self.slides() || self.carousel.busy() || !self.carousel.resting() {
            if let Scrolled::Zoom(dy) = self.carousel.scroll(dx, dy, Instant::now(), self.page()) {
                if let Doc::Picture(shown) = &mut self.doc {
                    shown.zoom_by((-dy * 0.004).exp(), all, x, y);
                }
            }
            return;
        }
        match &mut self.doc {
            // Zoomed in: across moves about in it, up and down zooms.
            Doc::Picture(shown) => {
                if dx.abs() > dy.abs() {
                    let at = shown.placed(all);
                    shown.centre.0 += dx / at.w;
                    shown.clamp(all);
                } else {
                    shown.zoom_by((-dy * 0.004).exp(), all, x, y);
                }
            }
            Doc::Model(object) => object.orbit.zoom = (object.orbit.zoom * (-dy * 0.003).exp()).clamp(0.3, 6.0),
            Doc::Pdf(paper) => {
                paper.scroll_by(dy, all);
                paper.pan_by(dx, all);
            }
            _ => {}
        }
    }

    /// Two fingers spreading or drawing together: what is shown zooms, about the pointer -- but
    /// not a picture on its way to the next.
    fn pinched(&mut self, scale: f32, all: Rect) {
        let (x, y) = self.pointer;
        if !self.carousel.resting() {
            return;
        }
        match &mut self.doc {
            Doc::Picture(shown) => shown.zoom_by(scale, all, x, y),
            Doc::Pdf(paper) => {
                let page = paper.current(all);
                paper.zoom = (paper.zoom * scale).clamp(0.4, 4.0);
                paper.go_to = Some(page);
            }
            Doc::Model(object) => object.orbit.zoom = (object.orbit.zoom * scale).clamp(0.3, 6.0),
            _ => {}
        }
    }

    fn toggle_zoom(&mut self) {
        let all = self.bounds;
        let (x, y) = self.pointer;
        match &mut self.doc {
            Doc::Picture(shown) => {
                if shown.zoom.is_some() {
                    shown.zoom = None;
                    shown.centre = (0.5, 0.5);
                } else {
                    // The picture's own pixels, one to one -- or as near as the GPU's copy has; a
                    // picture already shown at its own size, the window filled.
                    let fit = shown.fit_scale(all);
                    let to = if fit <= 1.0001 { shown.fill_scale(all) } else { 1.0 };
                    shown.zoom_by((fit / to).max(1.0001), all, x, y);
                }
            }
            Doc::Model(object) => object.orbit = Orbit::default(),
            Doc::Pdf(paper) => {
                let page = paper.current(all);
                paper.zoom = if paper.zoom > 1.01 { 1.0 } else { 1.5 };
                paper.go_to = Some(page);
            }
            _ => {}
        }
    }

    fn next_layout(&mut self) {
        if let Doc::Picture(shown) = &mut self.doc {
            let order = if shown.picture.right.is_some() {
                vec![Eyes::Pair, Eyes::Mono]
            } else {
                vec![Eyes::Mono, Eyes::SideBySide, Eyes::Crossed, Eyes::TopBottom]
            };
            let now = order.iter().position(|e| *e == shown.eyes()).unwrap_or(0);
            shown.eyes = Some(order[(now + 1) % order.len()]);
            shown.zoom = None;
            shown.centre = (0.5, 0.5);
        }
    }

    /// A key: `letter` is its letter for a shortcut, the same key whatever the layout (Russian,
    /// Greek ...) writes on it; `text` what it types, for the signs (+, -, 0, /).
    fn key(&mut self, key: Key, text: Option<&str>, letter: Option<char>, command: bool, shift: bool) -> Want {
        let all = self.bounds;
        let typed = text.unwrap_or("");
        // An unmodified letter's shortcut, Shift or not: R and r are one key.
        let letter = letter.filter(|_| !command).map(|l| if shift { l.to_ascii_uppercase() } else { l });
        match key {
            Key::Escape => {
                if self.info {
                    self.info = false;
                    return Want::Nothing;
                }
                return self.leave();
            }
            // A document zoomed wider than the window is moved about in by them.
            Key::Left | Key::Right if matches!(&self.doc, Doc::Pdf(paper) if paper.wide(all)) => {
                if let Doc::Pdf(paper) = &mut self.doc {
                    paper.pan_by(if key == Key::Left { -90.0 } else { 90.0 }, all);
                }
                return Want::Nothing;
            }
            Key::Left => return self.slide_by(-1),
            Key::Right => return self.slide_by(1),
            Key::Home if !matches!(self.doc, Doc::Pdf(_)) => return self.slide_to(0),
            Key::End if !matches!(self.doc, Doc::Pdf(_)) => return self.slide_to(self.files.len().saturating_sub(1)),
            _ => {}
        }
        if letter == Some('i') {
            self.info = !self.info;
            return Want::Nothing;
        }
        // A quarter turn; with markup, its marks turned with it, and a step to undo.
        if matches!(letter, Some('r' | 'R')) && matches!(self.doc, Doc::Picture(_)) {
            self.turn(letter == Some('r'));
            return Want::Nothing;
        }
        // The packing of a pair is not changed under markup drawn on the picture as it is.
        if typed == "3" && self.desk.is_some() {
            return Want::Nothing;
        }
        let (x, y) = (all.x + all.w * 0.5, all.y + all.h * 0.5);
        match &mut self.doc {
            Doc::Picture(shown) => match (key, typed, letter) {
                (_, "+" | "=", _) => shown.zoom_by(1.25, all, x, y),
                (_, "-" | "_", _) => shown.zoom_by(0.8, all, x, y),
                (_, "0", _) => {
                    shown.zoom = None;
                    shown.centre = (0.5, 0.5);
                }
                (_, "1", _) => {
                    let fit = shown.fit_scale(all);
                    shown.zoom = None;
                    shown.zoom_by(fit.max(1.0001), all, x, y);
                }
                (_, "3", _) => self.next_layout(),
                (_, _, Some('x' | 'X')) => shown.swapped = !shown.swapped,
                (_, ",", _) => shown.depth = (shown.depth - 0.005).max(-0.08),
                (_, ".", _) => shown.depth = (shown.depth + 0.005).min(0.08),
                (Key::Space, _, _) => return self.slide_by(if shift { -1 } else { 1 }),
                (Key::Up, _, _) if shown.zoom.is_some() => {
                    shown.centre.1 -= 0.05;
                    shown.clamp(all);
                }
                (Key::Down, _, _) if shown.zoom.is_some() => {
                    shown.centre.1 += 0.05;
                    shown.clamp(all);
                }
                _ => {}
            },
            Doc::Model(object) => match (key, typed) {
                (Key::Up, _) => object.orbit.pitch = (object.orbit.pitch + 0.08).min(1.45),
                (Key::Down, _) => object.orbit.pitch = (object.orbit.pitch - 0.08).max(-1.45),
                (_, "+" | "=") => object.orbit.zoom = (object.orbit.zoom * 1.2).min(6.0),
                (_, "-" | "_") => object.orbit.zoom = (object.orbit.zoom / 1.2).max(0.3),
                (_, "0") => {
                    object.orbit = Orbit::default();
                    object.turning = true;
                    object.turned_at = Instant::now();
                }
                (Key::Space, _) => {
                    object.turning = !object.turning;
                    object.turned_at = Instant::now();
                }
                _ => {}
            },
            Doc::Pdf(paper) => {
                if paper.pages.is_empty() {
                    return Want::Nothing;
                }
                let page = paper.current(all);
                match (key, typed) {
                    (Key::Up, _) => paper.scroll_by(-90.0, all),
                    (Key::Down, _) => paper.scroll_by(90.0, all),
                    (Key::PageUp, _) => paper.go_to = Some(page.saturating_sub(1)),
                    (Key::PageDown, _) => paper.go_to = Some((page + 1).min(paper.pages.len() - 1)),
                    (Key::Space, _) => paper.scroll_by(if shift { -1.0 } else { 1.0 } * (all.h - 160.0), all),
                    (Key::Home, _) => paper.go_to = Some(0),
                    (Key::End, _) => paper.go_to = Some(paper.pages.len() - 1),
                    (_, "+" | "=") => {
                        paper.zoom = (paper.zoom * 1.2).min(4.0);
                        paper.go_to = Some(page);
                    }
                    (_, "-" | "_") => {
                        paper.zoom = (paper.zoom / 1.2).max(0.4);
                        paper.go_to = Some(page);
                    }
                    (_, "0") => {
                        paper.zoom = 1.0;
                        paper.go_to = Some(page);
                    }
                    (Key::Char('f'), _) if command => paper.open_find(),
                    (_, "/") => paper.open_find(),
                    (_, _) if letter == Some('n') && !paper.found.is_empty() => {
                        paper.found_at = (paper.found_at + 1) % paper.found.len();
                        paper.go_to = Some(paper.found[paper.found_at].page);
                    }
                    (_, _) if letter == Some('N') && !paper.found.is_empty() => {
                        paper.found_at = (paper.found_at + paper.found.len() - 1) % paper.found.len();
                        paper.go_to = Some(paper.found[paper.found_at].page);
                    }
                    (_, _) if matches!(letter, Some('p' | 'P')) => paper.open_page_field(),
                    (_, _) if matches!(letter, Some('o' | 'O')) && !paper.outline.is_empty() => {
                        let mut list = List::default();
                        list.focused = true;
                        list.cursor = paper.outline.iter().rposition(|h| h.page.is_some_and(|p| p <= page)).unwrap_or(0);
                        paper.contents = Some(list);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Want::Nothing
    }
}

impl Paper {
    fn open_page_field(&mut self) {
        let mut field = Field::default();
        field.focused = true;
        field.hint = format!("A page, 1 to {}", self.pages.len());
        self.page_field = Some(field);
    }

    fn open_find(&mut self) {
        let mut field = Field::default();
        field.focused = true;
        field.hint = "Find in this document".into();
        self.find = Some(field);
    }

    fn drawn_pending(&mut self, page: usize, width: u32, height: u32, rgba: Vec<u8>) {
        self.pending.push((page, width, height, rgba));
    }

    /// A search going on is not waited for: its answer, when it comes, is not this one's.
    fn forget_finding(&mut self) {
        self.finds += 1;
        self.finding = false;
    }
}

thread_local! {
    /// The folder's tiles, for the strip along the foot: set by the program before drawing.
    pub static TILES: std::cell::RefCell<Vec<Tile>> = const { std::cell::RefCell::new(Vec::new()) };
    /// What the same tiles said of their pictures, beside them.
    pub static SEEN: std::cell::RefCell<Vec<Option<Seen>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// What stands in for a file not yet read, or for one beside the file being looked at: its tile
/// from the folder.
struct StandIn {
    picture: Option<Texture>,
    seen: Option<Seen>,
    icon: Option<Icon>,
    tint: Option<(Color, Color)>,
}

/// The stand-in for the file at `place` among the folder's files, once the folder has its tile.
fn stand_in(place: usize) -> Option<StandIn> {
    TILES.with(|tiles| tiles.borrow().get(place).map(|tile| StandIn { picture: tile.picture, seen: seen(place), icon: tile.icon, tint: tile.tint }))
}

/// What the file at `place`'s tile said of its picture.
fn seen(place: usize) -> Option<Seen> {
    SEEN.with(|seen| seen.borrow().get(place).copied().flatten())
}

/// Draw a stand-in on its `page`: a picture's tile where its picture will be and at the picture's
/// shape -- a stereo pair's one eye -- or, for anything with no picture of its own yet, what kind
/// of thing it is, in the middle.
fn draw_stand_in(p: &mut Painter, gl: &Gl, page: Rect, stand_in: &StandIn) {
    match stand_in.picture {
        Some(texture) => {
            let aspect = stand_in.seen.map_or(texture.width as f32 / texture.height.max(1) as f32, |seen| seen.aspect);
            let own = stand_in.seen.map_or(f32::MAX, |seen| seen.height);
            p.image(gl, texture, fitted(page, aspect, own), [0.0, 0.0, 1.0, 1.0], 6.0, [1.0, 1.0, 1.0, 1.0], false);
        }
        None => {
            let area = stage(page);
            let side = area.w.min(area.h) * 0.4;
            let at = Rect::new(area.x + (area.w - side) * 0.5, area.y + (area.h - side) * 0.5, side, side);
            let (top, foot) = stand_in.tint.unwrap_or(TILE_TINT);
            p.gradient(gl, at, 22.0, top, foot);
            if let Some(icon) = stand_in.icon {
                draw_icon(p, gl, icon, at.inset(side * 0.32), [1.0, 1.0, 1.0, 0.82]);
            }
        }
    }
}

fn short(eyes: Eyes) -> &'static str {
    match eyes {
        Eyes::Mono => "2D",
        Eyes::SideBySide => "Side by side",
        Eyes::Crossed => "Crossed",
        Eyes::TopBottom => "Over and under",
        Eyes::Pair => "Pair",
    }
}

/// Texture coordinates for a picture turned `turns` quarter turns. Half a turn is the
/// coordinates the other way round; a quarter turn cannot be said with a rectangle of them,
/// so a picture turned by a quarter is turned on the CPU when it is shown ([`View`] keeps
/// `turns` even and the rest is done there).
fn turned(uv: [f32; 4], turns: u8) -> [f32; 4] {
    if turns % 4 == 2 {
        [uv[2], uv[3], uv[0], uv[1]]
    } else {
        uv
    }
}

/// The document's pages laid out on `all`, its page of the carousel, and drawn no further out than
/// `seen`, the part of that page in the window. Which keys do what is the controls'.
/// A locked document's question, in the middle of `all`: what it is, the field its password is
/// typed into (dots), and whether the one given was wrong or is being tried.
fn draw_lock(p: &mut Painter, gl: &Gl, lock: &mut Lock, all: Rect, name: &str) {
    let w = 720.0f32.min(all.w - 80.0);
    let h = 300.0;
    let panel = Rect::new(all.x + (all.w - w) * 0.5, all.y + (all.h - h) * 0.5, w, h);
    look::glass(p, gl, panel, 22.0, 1.0);
    let mut inner = panel.inset(32.0);
    let head = inner.take_top(44.0);
    draw_icon(p, gl, Icon::Lock, Rect { w: 40.0, h: 40.0, ..head }, theme::ink());
    p.label(gl, &format!("{name} is locked"), Rect { x: head.x + 56.0, w: head.w - 56.0, ..head }, theme::ROW_EM, theme::ink(), 0.0);
    inner.take_top(6.0);
    p.label(gl, "Enter its password to open it.", inner.take_top(34.0), theme::DETAIL_EM, theme::ink_detail(), 0.0);
    inner.take_top(20.0);
    lock.field.draw(p, gl, inner.take_top(Field::HEIGHT));
    inner.take_top(16.0);
    let (said, colour) = if lock.trying {
        ("Opening\u{2026}", theme::ink_hint())
    } else if lock.wrong {
        ("That password does not open it. Try again.", look::trouble())
    } else {
        ("", theme::ink_hint())
    };
    p.label(gl, said, inner.take_top(34.0), theme::DETAIL_EM, colour, 0.0);
}

fn draw_paper(p: &mut Painter, gl: &Gl, paper: &mut Paper, all: Rect, seen: Rect, name: &str) {
    paper.placed.clear();
    if let Some(lock) = paper.lock.as_mut() {
        draw_lock(p, gl, lock, all, name);
        return;
    }
    for texture in paper.stale.drain(..) {
        p.forget(gl, texture);
    }
    let pending = std::mem::take(&mut paper.pending);
    for (page, w, h, rgba) in pending {
        if let Ok(texture) = upload(gl, &rgba, w, h) {
            if let Some((old, _)) = paper.drawn.insert(page, (texture, w)) {
                p.forget(gl, old);
            }
        }
    }
    if paper.pages.is_empty() {
        look::waiting(p, gl, all, Instant::now() - Duration::from_millis(1));
        return;
    }
    if let Some(page) = paper.go_to.take() {
        paper.show_page(page, all);
    }
    // The page in view's words, at hand for the room's speech when it asks.
    let page = paper.current(all);
    paper.words_on(page);
    let width = paper.width(all);
    // Zoomed out again, or the window wider: the column kept within its room.
    paper.pan_by(0.0, all);
    let left = all.x + (all.w - width) * 0.5 + paper.across;
    let layout = paper.layout(all);
    let pixels = (width * p.scale()).round() as u32;
    let mut visible = Vec::new();
    p.clip(gl, seen);
    for (index, (y, h)) in layout.iter().enumerate() {
        let top = all.y + y - paper.scroll;
        if top > all.bottom() + all.h || top + h < all.y - all.h {
            continue;
        }
        let near = top < all.bottom() && top + h > all.y;
        visible.push((index, near));
        let rect = Rect::new(left, top, width, *h);
        if !near {
            continue;
        }
        paper.placed.push((index, rect));
        // What is shown of the page -- all of it, or what markup cropped it to -- as fractions.
        let (pw, ph) = paper.pages.get(index).map_or((1.0, 1.0), |page| (page.width.max(1.0), page.height.max(1.0)));
        let region = paper.region(index);
        let uv = [region.x0 / pw, region.y0 / ph, region.x1 / pw, region.y1 / ph];
        p.glow(gl, rect, 4.0, [0.0, 0.0, 0.0, 0.5], 28.0);
        match paper.drawn.get(&index) {
            Some((texture, _)) => p.image(gl, *texture, rect, uv, 4.0, [1.0, 1.0, 1.0, 1.0], true),
            None => p.fill(gl, rect, 4.0, [0.92, 0.92, 0.94, 1.0]),
        }
        // What a search found on this page, washed in the room's teal.
        for (n, found) in paper.found.iter().enumerate().filter(|(_, f)| f.page == index) {
            let current = n == paper.found_at;
            for r in &found.rects {
                // Fractions of the page, as fractions of what is shown of it.
                let fx = |f: f32| (f * pw - region.x0) / region.w().max(1e-3);
                let fy = |f: f32| (f * ph - region.y0) / region.h().max(1e-3);
                let r = [fx(r[0]), fy(r[1]), fx(r[2]), fy(r[3])];
                let at = Rect::new(rect.x + r[0] * rect.w - 2.0, rect.y + r[1] * rect.h - 2.0, (r[2] - r[0]) * rect.w + 4.0, (r[3] - r[1]) * rect.h + 4.0);
                let colour = if current { [look::TEAL[0], look::TEAL[1], look::TEAL[2], 0.45] } else { [look::BLUE[0], look::BLUE[1], look::BLUE[2], 0.25] };
                p.fill(gl, at, 3.0, colour);
            }
        }
        // Its text, for Live Text: its own characters asked for once, and drawn where it is.
        paper.ask_letters(index);
        paper.text_draw_page(p, gl, index, rect);
    }
    p.unclip(gl);
    // Pages are drawn at the width they are shown, and drawn again when that changes much.
    for (index, near) in visible.iter().copied() {
        // A cropped page is drawn whole, as wide as its kept part needs.
        let whole = paper.pages.get(index).map_or(1.0, |page| page.width.max(1.0));
        let pixels = (pixels as f32 * whole / paper.region(index).w().max(1.0)).round() as u32;
        let have = paper.drawn.get(&index).map(|(_, w)| *w);
        let asked = paper.asked.get(&index).copied();
        let good = |w: u32| (w as f32) >= pixels.min(6000) as f32 * 0.9 && (w as f32) <= pixels as f32 * 1.6;
        if !have.is_some_and(good) && !asked.is_some_and(good) && (near || have.is_none()) {
            paper.asked.insert(index, pixels);
            paper.pdf.ask(Ask::Draw { page: index, width: pixels.min(6000), most_height: crate::picture::LARGEST });
        }
    }
    // Pages far from view let go of: a thousand-page book is not a thousand textures.
    let keep: Vec<usize> = visible.iter().map(|(i, _)| *i).collect();
    let far: Vec<usize> = paper.drawn.keys().copied().filter(|i| !keep.iter().any(|k| k.abs_diff(*i) <= 3)).collect();
    for index in far {
        if let Some((texture, _)) = paper.drawn.remove(&index) {
            paper.stale.push(texture);
        }
    }
    // The search field, and the contents, over the pages.
    if let Some(field) = paper.find.as_mut() {
        let rect = Rect::new(all.right() - 640.0, all.y + 24.0 + theme::ROW_HEIGHT + 40.0, 600.0, Field::HEIGHT + 24.0);
        look::glass(p, gl, rect, 20.0, 1.0);
        let inner = rect.inset(12.0);
        let count_text = if paper.finding {
            "Finding\u{2026}".to_string()
        } else if !paper.found.is_empty() {
            format!("{} of {}", paper.found_at + 1, paper.found.len())
        } else if !field.text.is_empty() {
            "Enter finds".to_string()
        } else {
            String::new()
        };
        let count_w = p.measure(gl, &count_text, theme::DETAIL_EM, 8192.0).0;
        field.draw(p, gl, Rect { w: inner.w - count_w - 20.0, ..inner });
        p.label(gl, &count_text, Rect { x: inner.right() - count_w, w: count_w, ..inner }, theme::DETAIL_EM, theme::ink_hint(), 1.0);
    }
    if let Some(field) = paper.page_field.as_mut() {
        let rect = Rect::new(all.x + (all.w - 420.0) * 0.5, all.y + 24.0 + theme::ROW_HEIGHT + 40.0, 420.0, Field::HEIGHT + 24.0);
        look::glass(p, gl, rect, 20.0, 1.0);
        let inner = rect.inset(12.0);
        let of = format!("of {}", paper.pages.len());
        let of_w = p.measure(gl, &of, theme::DETAIL_EM, 8192.0).0;
        field.draw(p, gl, Rect { w: inner.w - of_w - 20.0, ..inner });
        p.label(gl, &of, Rect { x: inner.right() - of_w, w: of_w, ..inner }, theme::DETAIL_EM, theme::ink_hint(), 1.0);
    }
    if let Some(list) = paper.contents.as_mut() {
        let rect = Rect::new(all.x + 28.0, all.y + 24.0 + theme::ROW_HEIGHT + 42.0, 560.0, all.h - theme::ROW_HEIGHT - 110.0);
        look::glass(p, gl, rect, 22.0, 1.0);
        let rows: Vec<Row> = paper
            .outline
            .iter()
            .map(|h| {
                let label = format!("{}{}", "    ".repeat(h.depth.min(4)), h.title);
                match h.page {
                    Some(page) => Row::with(label, (page + 1).to_string()),
                    None => Row::new(label),
                }
            })
            .collect();
        list.draw(p, gl, rect.inset(16.0), &rows);
    }
}

/// Which keys do what in a document.
fn paper_hint(paper: &Paper) -> &'static str {
    if paper.lock.is_some() {
        "[Enter] Open  [Esc] Give up"
    } else if paper.outline.is_empty() {
        "[/] Find  [+] [\u{2212}] Zoom  [P] Page  [PgUp] [PgDn] Turn"
    } else {
        "[/] Find  [+] [\u{2212}] Zoom  [O] Contents  [P] Page  [PgUp] [PgDn] Turn"
    }
}

/// Which keys do what, on a pane of glass of its own at the foot of `all`: over a page or a
/// picture it would be unreadable.
fn hint_pane(p: &mut Painter, gl: &Gl, all: Rect, hint: &str) {
    hint_pane_over(p, gl, all, all.bottom() - 18.0, hint);
}

/// The same, its foot at `bottom`: words over the keys, or over the strip.
fn hint_pane_over(p: &mut Painter, gl: &Gl, all: Rect, bottom: f32, hint: &str) {
    // Measured where it cannot be seen, then drawn on its pane.
    let width = look::keys(p, gl, Rect::new(-10000.0, -10000.0, 4000.0, 40.0), hint, 0.0, 0.0) + 44.0;
    let foot = Rect::new(all.x + (all.w - width).max(0.0) * 0.5, bottom - 44.0, width.min(all.w), 44.0);
    look::glass(p, gl, foot, 22.0, 1.0);
    look::keys(p, gl, foot.inset(4.0), hint, 0.5, 1.0);
}

/// Turn a picture a quarter `turns` times on the CPU: what [`turned`] cannot.
pub fn quarter_turn(picture: &mut Picture, clockwise: bool) {
    let (w, h) = (picture.width as usize, picture.height as usize);
    let turn = |rgba: &[u8]| -> Vec<u8> {
        let mut out = vec![0u8; rgba.len()];
        for y in 0..h {
            for x in 0..w {
                let (nx, ny) = if clockwise { (h - 1 - y, x) } else { (y, w - 1 - x) };
                let from = (y * w + x) * 4;
                let to = (ny * h + nx) * 4;
                out[to..to + 4].copy_from_slice(&rgba[from..from + 4]);
            }
        }
        out
    };
    picture.rgba = turn(&picture.rgba);
    for frame in &mut picture.frames {
        frame.0 = turn(&frame.0);
    }
    if let Some((rw, rh, rgba)) = picture.right.take() {
        if rw as usize == w && rh as usize == h {
            picture.right = Some((rh, rw, turn(&rgba)));
        }
    }
    picture.width = h as u32;
    picture.height = w as u32;
    picture.original = (picture.original.1, picture.original.0);
}

/// Turn a picture holding two views packed side by side or one over the other a quarter: each
/// eye's view turned where it is, not the frame whole -- which would put one eye's view over the
/// other's, upside down for one of them. Side by side comes out one over the other, the left eye
/// on top, and one over the other side by side, the left eye on the left: the packing it is now.
pub fn quarter_turn_pair(picture: &mut Picture, eyes: Eyes, clockwise: bool) -> Eyes {
    let (w, h) = (picture.width as usize, picture.height as usize);
    let across = match eyes {
        Eyes::SideBySide | Eyes::Crossed => true,
        Eyes::TopBottom => false,
        Eyes::Mono | Eyes::Pair => {
            quarter_turn(picture, clockwise);
            return eyes;
        }
    };
    // Each eye's view, its own size: an odd column or row between them is left out.
    let (ew, eh) = if across { (w / 2, h) } else { (w, h / 2) };
    if ew == 0 || eh == 0 {
        quarter_turn(picture, clockwise);
        return eyes;
    }
    let (left, right) = eyes.halves();
    let corner = |half: [f32; 4]| ((half[0] * w as f32).round() as usize, (half[1] * h as f32).round() as usize);
    let (l, r) = (corner(left), corner(right));
    let turn = |rgba: &[u8]| -> Vec<u8> {
        let take = |(x0, y0): (usize, usize)| -> Picture {
            let mut eye = Vec::with_capacity(ew * eh * 4);
            for y in y0..y0 + eh {
                eye.extend_from_slice(&rgba[(y * w + x0) * 4..(y * w + x0 + ew) * 4]);
            }
            let mut picture = Picture { width: ew as u32, height: eh as u32, rgba: eye, eyes: Eyes::Mono, right: None, frames: Vec::new(), first: Duration::ZERO, original: (0, 0), about: Vec::new(), saves: Default::default() };
            quarter_turn(&mut picture, clockwise);
            picture
        };
        let (a, b) = (take(l), take(r));
        // Turned, each eye is `eh` across and `ew` down.
        let (tw, th) = (eh, ew);
        if across {
            // One over the other: the left eye's rows, then the right's.
            let mut out = a.rgba;
            out.extend_from_slice(&b.rgba);
            out
        } else {
            let mut out = Vec::with_capacity(tw * th * 8);
            for y in 0..th {
                out.extend_from_slice(&a.rgba[y * tw * 4..(y + 1) * tw * 4]);
                out.extend_from_slice(&b.rgba[y * tw * 4..(y + 1) * tw * 4]);
            }
            out
        }
    };
    picture.rgba = turn(&picture.rgba);
    let frames = std::mem::take(&mut picture.frames);
    picture.frames = frames.into_iter().map(|(rgba, length)| (if rgba.len() == w * h * 4 { turn(&rgba) } else { rgba }, length)).collect();
    let (tw, th) = if across { (eh, ew * 2) } else { (eh * 2, ew) };
    picture.width = tw as u32;
    picture.height = th as u32;
    picture.original = (picture.original.1, picture.original.0);
    let turned = if across { Eyes::TopBottom } else { Eyes::SideBySide };
    picture.eyes = turned;
    turned
}

impl Shown {
    /// A quarter turn asked for, done on the CPU: whether it was, and the GPU's copies are to go.
    fn settle_turn(&mut self) -> bool {
        if self.turns % 2 == 0 {
            return false;
        }
        let clockwise = self.turns == 1;
        let eyes = self.eyes();
        let turned = quarter_turn_pair(&mut self.picture, eyes, clockwise);
        if self.eyes.is_some() {
            self.eyes = Some(turned);
        }
        // A quarter either way is done, and the picture is the right way up: nothing is left over.
        self.turns = 0;
        self.sideways = !self.sideways;
        true
    }
}

impl View {
    /// Pictures turned by a quarter are turned for real: called before drawing.
    pub fn settle_turns(&mut self, gl: &Gl) {
        if let Doc::Picture(shown) = &mut self.doc {
            if shown.settle_turn() {
                for texture in [shown.texture.take(), shown.right.take()].into_iter().flatten() {
                    unsafe { gl.api().delete_texture(texture.raw()) };
                }
            }
        }
    }

    pub fn forget(mut self, gl: &Gl) {
        if let Some(desk) = self.desk.as_mut() {
            desk.forget(gl);
        }
        match self.doc {
            Doc::Picture(shown) => {
                for texture in [shown.texture, shown.right].into_iter().flatten() {
                    unsafe { gl.api().delete_texture(texture.raw()) };
                }
            }
            Doc::Pdf(paper) => {
                for texture in paper.drawn.into_values().map(|(texture, _)| texture).chain(paper.stale) {
                    unsafe { gl.api().delete_texture(texture.raw()) };
                }
            }
            Doc::Model(object) => {
                if let Some(renderer) = object.renderer {
                    renderer.forget(gl);
                }
            }
            _ => {}
        }
    }

    /// The path, for the folder to put its cursor back on.
    pub fn path(&self) -> PathBuf {
        self.entry.path.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spatiand_app::ui::CONTROLS_SHOWN;

    fn picture(width: u32, height: u32) -> Picture {
        Picture {
            width,
            height,
            rgba: (0..width * height).flat_map(|i| [(i % 251) as u8, (i / 251 % 251) as u8, 7, 255]).collect(),
            eyes: Eyes::Mono,
            right: None,
            frames: Vec::new(),
            first: Duration::ZERO,
            original: (width, height),
            about: Vec::new(),
            saves: Default::default(),
        }
    }

    #[test]
    fn four_quarter_turns_come_back_to_where_they_started() {
        let mut p = picture(5, 3);
        let before = p.rgba.clone();
        quarter_turn(&mut p, true);
        assert_eq!((p.width, p.height), (3, 5));
        for _ in 0..3 {
            quarter_turn(&mut p, true);
        }
        assert_eq!(p.rgba, before);
        quarter_turn(&mut p, true);
        quarter_turn(&mut p, false);
        assert_eq!(p.rgba, before);
    }

    #[test]
    fn shift_r_turns_anticlockwise() {
        let mut shown = Shown::new(picture(5, 3));
        let mut expected = picture(5, 3);
        quarter_turn(&mut expected, false);
        shown.turns = 3;
        assert!(shown.settle_turn());
        assert_eq!(shown.turns, 0);
        assert_eq!(shown.picture.rgba, expected.rgba);
        assert_eq!((shown.picture.width, shown.picture.height), (3, 5));
    }

    #[test]
    fn a_quarter_turn_of_a_pair_turns_each_eye_where_it_is() {
        // Side by side, each eye 3 wide and 2 tall: the left eye's pixels 1 to 6, the right's 11
        // to 16, in reading order.
        let rows: [[u8; 6]; 2] = [[1, 2, 3, 11, 12, 13], [4, 5, 6, 14, 15, 16]];
        let mut pair = picture(6, 2);
        pair.rgba = rows.iter().flatten().flat_map(|&v| [v, 0, 0, 255]).collect();
        let turned = quarter_turn_pair(&mut pair, Eyes::SideBySide, true);
        assert_eq!(turned, Eyes::TopBottom);
        assert_eq!((pair.width, pair.height), (2, 6));
        let reds: Vec<u8> = pair.rgba.chunks_exact(4).map(|p| p[0]).collect();
        // Each eye turned clockwise: its foot row becomes its left column. The left eye on top.
        assert_eq!(reds, [4, 1, 5, 2, 6, 3, 14, 11, 15, 12, 16, 13]);
        // And back the other way: side by side again, as it was.
        let back = quarter_turn_pair(&mut pair, turned, false);
        assert_eq!(back, Eyes::SideBySide);
        assert_eq!((pair.width, pair.height), (6, 2));
        let reds: Vec<u8> = pair.rgba.chunks_exact(4).map(|p| p[0]).collect();
        assert_eq!(reds, rows.iter().flatten().copied().collect::<Vec<_>>());
        // Crossed, the right eye's view on the left: the left eye still comes out on top.
        let mut crossed = picture(6, 2);
        crossed.rgba = rows.iter().flatten().flat_map(|&v| [v, 0, 0, 255]).collect();
        quarter_turn_pair(&mut crossed, Eyes::Crossed, true);
        let reds: Vec<u8> = crossed.rgba.chunks_exact(4).map(|p| p[0]).collect();
        assert_eq!(reds, [14, 11, 15, 12, 16, 13, 4, 1, 5, 2, 6, 3]);
    }

    #[test]
    fn a_squeezed_pair_turned_keeps_its_eyes_true_shape() {
        let shape = |packing: Eyes, (w, h): (u32, u32), turns: &[u8]| {
            let mut pair = picture(w, h);
            pair.eyes = packing;
            let mut shown = Shown::new(pair);
            for &turn in turns {
                shown.turns = turn;
                assert!(shown.settle_turn());
            }
            (shown.picture.eyes, shown.picture.width, shown.picture.height, shown.eye_aspect(), shown.own_height())
        };
        let near = |a: f32, b: f32| (a - b).abs() < 0.01;
        // Half-width side by side, a film's frame of 32 by 18: each eye 16 by 18, shown 16:9.
        let (eyes, _, _, aspect, _) = shape(Eyes::SideBySide, (32, 18), &[]);
        assert!(eyes == Eyes::SideBySide && near(aspect, 16.0 / 9.0), "{aspect}");
        // Turned clockwise: one over the other, 18 by 32, each eye 18 by 16 squeezed in height,
        // shown 9:16 and 32 tall -- not 18:16 and 16 tall, the squeezed shape.
        let (eyes, w, h, aspect, tall) = shape(Eyes::SideBySide, (32, 18), &[1]);
        assert_eq!((eyes, w, h), (Eyes::TopBottom, 18, 32));
        assert!(near(aspect, 9.0 / 16.0) && near(tall, 32.0), "{aspect} {tall}");
        // And back: 16:9 again.
        let (eyes, _, _, aspect, _) = shape(Eyes::SideBySide, (32, 18), &[1, 3]);
        assert!(eyes == Eyes::SideBySide && near(aspect, 16.0 / 9.0), "{aspect}");
        // Half-height one over the other turned anticlockwise: side by side, each eye 9 by 32
        // squeezed in width, shown 9:16.
        let (eyes, w, h, aspect, tall) = shape(Eyes::TopBottom, (32, 18), &[3]);
        assert_eq!((eyes, w, h), (Eyes::SideBySide, 18, 32));
        assert!(near(aspect, 9.0 / 16.0) && near(tall, 32.0), "{aspect} {tall}");
        // A pair at full size is not squeezed either way: two 15 by 20 views turned are 20 by 15.
        let (_, _, _, aspect, tall) = shape(Eyes::SideBySide, (30, 20), &[1]);
        assert!(near(aspect, 4.0 / 3.0) && near(tall, 15.0), "{aspect} {tall}");
    }

    #[test]
    fn a_squeezed_pair_is_shown_at_its_true_shape() {
        let mut shown = Shown {
            picture: picture(1920, 1080),
            texture: None,
            right: None,
            turns: 0,
            sideways: false,
            frame: 0,
            frame_since: Instant::now(),
            uploaded: 0,
            eyes: Some(Eyes::SideBySide),
            swapped: false,
            depth: 0.0,
            zoom: None,
            centre: (0.5, 0.5),
            drawn_at: None,
            nearest: false,
            orientation: 0,
            cut: WHOLE,
            head: HEAD,
        };
        assert!((shown.eye_aspect() - 16.0 / 9.0).abs() < 0.01);
        shown.picture = picture(3840, 1080);
        assert!((shown.eye_aspect() - 16.0 / 9.0).abs() < 0.01);
        // Two portrait views side by side are two portrait views.
        shown.picture = picture(3000, 2000);
        assert!((shown.eye_aspect() - 0.75).abs() < 0.01);
    }

    #[test]
    fn zooming_keeps_what_is_under_the_pointer_under_it() {
        let mut shown = Shown {
            picture: picture(4000, 3000),
            texture: None,
            right: None,
            turns: 0,
            sideways: false,
            frame: 0,
            frame_since: Instant::now(),
            uploaded: 0,
            eyes: None,
            swapped: false,
            depth: 0.0,
            zoom: None,
            centre: (0.5, 0.5),
            drawn_at: None,
            nearest: false,
            orientation: 0,
            cut: WHOLE,
            head: HEAD,
        };
        let area = Rect::new(0.0, 0.0, 1440.0, 900.0);
        let before = shown.placed(area);
        let (x, y) = (900.0, 300.0);
        let under = ((x - before.x) / before.w, (y - before.y) / before.h);
        shown.zoom_by(2.0, area, x, y);
        let after = shown.placed(area);
        let now = ((x - after.x) / after.w, (y - after.y) / after.h);
        assert!((under.0 - now.0).abs() < 0.002 && (under.1 - now.1).abs() < 0.002, "{under:?} {now:?}");
    }

    #[test]
    fn a_tile_stands_in_where_its_picture_will_be_and_at_its_shape() {
        let all = Rect::new(0.0, 0.0, 1440.0, 900.0);
        for (w, h, eyes) in [(4032, 3024, Eyes::Mono), (3024, 4032, Eyes::Mono), (1920, 1080, Eyes::SideBySide), (3840, 1080, Eyes::Crossed), (1920, 2160, Eyes::TopBottom)] {
            let mut picture = picture(w / 8, h / 8);
            (picture.width, picture.height, picture.eyes, picture.original) = (w, h, eyes, (w, h));
            let shown = Shown { picture, texture: None, right: None, turns: 0, sideways: false, frame: 0, frame_since: Instant::now(), uploaded: 0, eyes: None, swapped: false, depth: 0.0, zoom: None, centre: (0.5, 0.5), drawn_at: None, nearest: false, orientation: 0, cut: WHOLE, head: HEAD };
            // What the tile's thread works out from the picture's own size, and where the view
            // puts the picture once it has it.
            let tile = fitted(all, eye_aspect(w as f32, h as f32, eyes), eye_height(w as f32, h as f32, eyes));
            let picture = shown.placed(all);
            for (a, b) in [(tile.x, picture.x), (tile.y, picture.y), (tile.w, picture.w), (tile.h, picture.h)] {
                assert!((a - b).abs() < 0.01, "{w}x{h} {eyes:?}: {tile:?} {picture:?}");
            }
        }
    }

    #[test]
    fn a_small_picture_is_shown_at_its_own_size_and_can_be_zoomed_to_the_window() {
        let all = Rect::new(0.0, 0.0, 1440.0, 900.0);
        let mut shown = Shown::new(picture(64, 32));
        assert_eq!(shown.placed(all).w, 64.0, "its own pixels, one to one");
        // Its tile stands in at the same size.
        let tile = fitted(all, eye_aspect(64.0, 32.0, Eyes::Mono), eye_height(64.0, 32.0, Eyes::Mono));
        assert_eq!((tile.w, tile.h), (64.0, 32.0));
        // Nearer, as far as the window filled or eight times its size, whichever is more.
        for _ in 0..100 {
            shown.zoom_by(2.0, all, 720.0, 450.0);
        }
        let filled = fit_width(stage(all), 2.0);
        assert!((shown.placed(all).w - filled).abs() < 0.5, "{} {filled}", shown.placed(all).w);
        // And back: never smaller than its own size.
        for _ in 0..100 {
            shown.zoom_by(0.5, all, 720.0, 450.0);
        }
        assert_eq!(shown.zoom, None);
        assert_eq!(shown.placed(all).w, 64.0);
        // Two clicks: the window filled.
        let mut view = viewing(3, 1);
        view.doc = Doc::Picture(Box::new(Shown::new(picture(64, 32))));
        view.toggle_zoom();
        let Doc::Picture(shown) = &view.doc else { unreachable!() };
        assert!((shown.placed(view.bounds).w - filled).abs() < 0.5);
        // A large picture is still fitted to the window as it was.
        let large = Shown::new(picture(4000, 3000));
        assert!((large.placed(all).w - fit_width(stage(all), 4.0 / 3.0)).abs() < 0.01);
    }

    #[test]
    fn a_moving_picture_wakes_for_its_next_frame_and_not_before() {
        let mut moving = picture(2000, 1500);
        moving.first = Duration::from_millis(500);
        moving.frames = vec![(moving.rgba.clone(), Duration::from_millis(500))];
        let mut view = viewing(3, 1);
        view.doc = Doc::Picture(Box::new(Shown::new(moving)));
        if let Doc::Picture(shown) = &mut view.doc {
            shown.drawn_at = Some(Instant::now() - FADES_IN);
        }
        view.clock = AutoHide::new(Instant::now() - CONTROLS_SHOWN - Duration::from_secs(2));
        assert!(!view.animating(), "not drawn every 16 ms");
        let wake = view.next_wake().expect("woken for the next frame");
        assert!(wake > Duration::from_millis(400) && wake <= Duration::from_millis(500), "{wake:?}");
    }

    #[test]
    fn what_two_rectangles_share() {
        let window = Rect::new(0.0, 0.0, 1440.0, 900.0);
        assert_eq!(overlap(Rect::new(-500.0, 0.0, 1440.0, 900.0), window), Rect::new(0.0, 0.0, 940.0, 900.0));
        assert_eq!(overlap(Rect::new(1000.0, 0.0, 1440.0, 900.0), window), Rect::new(1000.0, 0.0, 440.0, 900.0));
        assert_eq!(overlap(Rect::new(1464.0, 0.0, 1440.0, 900.0), window).w, 0.0);
    }

    /// The picture at `at` of `files`, read and fitted.
    fn viewing(files: usize, at: usize) -> View {
        let entry = Entry { name: "b.jpg".into(), path: PathBuf::from("/nowhere/b.jpg"), kind: Kind::Picture, size: 1, modified: String::new() };
        let mut view = View::new(entry, at, 1, (0..files).collect());
        view.pictured(Ok(picture(2000, 1500)));
        view
    }

    fn zoom(view: &View) -> Option<f32> {
        match &view.doc {
            Doc::Picture(shown) => shown.zoom,
            _ => None,
        }
    }

    fn key(key: Key, text: &str) -> Input {
        let letter = text.chars().next().filter(|c| c.is_ascii_alphabetic() && text.len() == 1).map(|c| c.to_ascii_lowercase());
        let modifiers = spatiand_app::Modifiers { shift: text.chars().next().is_some_and(|c| c.is_ascii_uppercase()), ..Default::default() };
        Input::Key { key, text: Some(text.to_string()).filter(|t| !t.is_empty()), modifiers, letter }
    }

    #[test]
    fn a_sideways_scroll_slides_a_fitted_picture_and_never_zooms_it() {
        let mut view = viewing(3, 1);
        for _ in 0..10 {
            view.input(&Input::Scroll { dx: 6.0, dy: 1.5 });
            view.tick();
        }
        assert!((view.slid + 60.0).abs() < 1.0, "{}", view.slid);
        assert_eq!(zoom(&view), None);
        assert_eq!(view.heading(), None, "not far enough yet to read the next ahead");
        for _ in 0..30 {
            view.input(&Input::Scroll { dx: 6.0, dy: 0.0 });
        }
        view.tick();
        assert_eq!(view.heading(), Some(2));
    }

    #[test]
    fn an_up_and_down_scroll_zooms_and_never_slides() {
        let mut view = viewing(3, 1);
        for _ in 0..5 {
            view.input(&Input::Scroll { dx: 2.0, dy: -30.0 });
            view.tick();
        }
        assert!(zoom(&view).is_some(), "zoomed in");
        assert_eq!(view.slid, 0.0);
    }

    #[test]
    fn a_zoomed_picture_is_moved_about_in_rather_than_slid() {
        let mut view = viewing(3, 1);
        view.input(&key(Key::Char('='), "+"));
        view.input(&key(Key::Char('='), "+"));
        let centre = |view: &View| match &view.doc {
            Doc::Picture(shown) => shown.centre,
            _ => unreachable!(),
        };
        let before = centre(&view);
        view.input(&Input::Scroll { dx: 40.0, dy: 0.0 });
        view.tick();
        assert!(centre(&view).0 > before.0, "moved along it");
        assert_eq!(view.slid, 0.0);
    }

    /// A PDF of two pages being read, its thread played by the test.
    fn reading() -> (View, std::sync::mpsc::Sender<pdf::Answer>, std::sync::mpsc::Receiver<Ask>) {
        let entry = Entry { name: "a.pdf".into(), path: PathBuf::from("/nowhere/a.pdf"), kind: Kind::Pdf, size: 1, modified: String::new() };
        let mut view = View::new(entry, 0, 1, vec![0]);
        let (pdf, tell, asked) = Pdf::played();
        view.doc = Doc::Pdf(Box::new(Paper::new(pdf)));
        let page = pdf::Page { width: 600.0, height: 800.0 };
        tell.send(pdf::Answer::Opened { pages: vec![page.clone(), page], outline: Vec::new(), title: None, rights: pdf::Rights::ALL }).unwrap();
        view.answers();
        (view, tell, asked)
    }

    fn paper(view: &mut View) -> &mut Paper {
        match &mut view.doc {
            Doc::Pdf(paper) => paper,
            _ => unreachable!(),
        }
    }

    #[test]
    fn a_page_drawn_while_a_search_began_is_kept() {
        let (mut view, tell, asked) = reading();
        paper(&mut view).asked.insert(0, 800);
        view.input(&key(Key::Char('/'), "/"));
        view.input(&key(Key::Char('a'), "a"));
        view.input(&key(Key::Enter, ""));
        assert!(matches!(asked.try_recv(), Ok(Ask::Find { .. })));
        // Drawn before the search was asked for, and come after it.
        tell.send(pdf::Answer::Drawn { page: 0, width: 800, height: 1066, rgba: Vec::new() }).unwrap();
        view.answers();
        let paper = paper(&mut view);
        assert!(paper.asked.is_empty(), "no longer waited for");
        assert_eq!(paper.pending.len(), 1, "on its way to the GPU");
    }

    #[test]
    fn what_was_found_for_words_since_changed_is_not_taken() {
        let (mut view, tell, asked) = reading();
        view.input(&key(Key::Char('/'), "/"));
        view.input(&key(Key::Char('a'), "a"));
        view.input(&key(Key::Enter, ""));
        let Ok(Ask::Find { ticket, .. }) = asked.try_recv() else { panic!("a search") };
        // The words changed while it was being found: its answer is for the old ones.
        view.input(&key(Key::Char('b'), "b"));
        assert!(!paper(&mut view).finding);
        tell.send(pdf::Answer::Found { found: vec![pdf::Found { page: 1, rects: Vec::new() }], ticket }).unwrap();
        view.answers();
        assert!(paper(&mut view).found.is_empty());
        // Asked again, for the words now there, it is taken.
        view.input(&key(Key::Enter, ""));
        let Ok(Ask::Find { ticket, text }) = asked.try_recv() else { panic!("a search") };
        assert_eq!(text, "ab");
        tell.send(pdf::Answer::Found { found: vec![pdf::Found { page: 1, rects: Vec::new() }], ticket }).unwrap();
        view.answers();
        assert_eq!(paper(&mut view).found.len(), 1);
    }

    #[test]
    fn a_document_zoomed_wider_than_the_window_moves_sideways() {
        let (mut view, _tell, _asked) = reading();
        let all = view.bounds;
        // Fitted, it does not move.
        view.input(&Input::Scroll { dx: 40.0, dy: 0.0 });
        assert_eq!(paper(&mut view).across, 0.0);
        for _ in 0..6 {
            view.input(&key(Key::Char('='), "+"));
        }
        assert!(paper(&mut view).wide(all));
        // A scroll to the right shows more of the right: the column goes left.
        view.input(&Input::Scroll { dx: 40.0, dy: 0.0 });
        assert_eq!(paper(&mut view).across, -40.0);
        view.input(&key(Key::Right, ""));
        assert_eq!(paper(&mut view).across, -130.0);
        assert!(view.carousel.resting(), "the arrow moved the page, not to the next file");
        // As far as its edge, and no further.
        for _ in 0..100 {
            view.input(&key(Key::Left, ""));
        }
        let room = paper(&mut view).across_room(all);
        assert_eq!(paper(&mut view).across, room);
    }

    #[test]
    fn the_words_on_the_page_are_told_once_they_have_come() {
        let (mut view, tell, asked) = reading();
        // Asked for, not waited for: nothing yet.
        assert_eq!(view.describe(), ("a.pdf".to_string(), String::new()));
        let mut words = Vec::new();
        while let Ok(Ask::Text { page }) = asked.try_recv() {
            words.push(page);
        }
        assert_eq!(words, [1, 0]);
        tell.send(pdf::Answer::Text { page: 0, text: "Dear reader".into() }).unwrap();
        view.answers();
        assert_eq!(view.describe().1, "Dear reader");
        assert!(asked.try_recv().is_err(), "asked for once");
        // The find field's words, while it is open.
        view.input(&key(Key::Char('/'), "/"));
        view.input(&key(Key::Char('q'), "q"));
        assert_eq!(view.describe().1, "q");
    }

    #[test]
    fn letter_shortcuts_are_the_key_s_place_on_any_layout() {
        let mut view = viewing(3, 1);
        // R on a Russian keyboard: it types к, and is the key where r is.
        let russian = |text: &str, shift: bool| Input::Key {
            key: Key::Char(text.chars().next().unwrap().to_lowercase().next().unwrap()),
            text: Some(text.to_string()),
            modifiers: spatiand_app::Modifiers { shift, ..Default::default() },
            letter: Some('r'),
        };
        let turns = |view: &View| match &view.doc {
            Doc::Picture(shown) => shown.turns,
            _ => unreachable!(),
        };
        view.input(&russian("к", false));
        assert_eq!(turns(&view), 1);
        view.input(&russian("К", true));
        assert_eq!(turns(&view), 0, "Shift-R the other way");
        // I, by its place, for the picture's account.
        view.input(&Input::Key { key: Key::Char('ш'), text: Some("ш".into()), modifiers: Default::default(), letter: Some('i') });
        assert!(view.info);
    }

    #[test]
    fn a_new_file_in_a_room_that_says_the_hands_is_not_woken_by_the_head() {
        // The owner, 2026-11-19: the strip "not go away, even I wait long enough". Each file's
        // view began taking pointer motion -- the head's sweep -- for the hands.
        let mut view = viewing(3, 1);
        view.hands_said(true);
        view.clock = AutoHide::new(Instant::now() - CONTROLS_SHOWN - Duration::from_secs(2));
        assert_eq!(view.chrome(), 0.0);
        for step in 0..20 {
            view.input(&Input::Pointer { x: 400.0 + step as f32, y: 300.0 });
        }
        assert_eq!(view.chrome(), 0.0, "the head looking about leaves the controls away");
        // A room too old to say the hands: motion is all there is to go by.
        let mut old = viewing(3, 1);
        old.hands_said(false);
        old.clock = AutoHide::new(Instant::now() - CONTROLS_SHOWN - Duration::from_secs(2));
        old.input(&Input::Pointer { x: 400.0, y: 300.0 });
        assert_eq!(old.chrome(), 1.0);
    }

    #[test]
    fn a_press_on_faded_controls_only_brings_them_back() {
        let mut view = viewing(3, 1);
        view.bar = Rect::new(28.0, 24.0, 1384.0, 80.0);
        // In the room, where the pointer sweeping with the head does not bring them back.
        view.hands_heard = true;
        view.clock = AutoHide::new(Instant::now() - CONTROLS_SHOWN - Duration::from_secs(2));
        assert_eq!(view.chrome(), 0.0);
        view.input(&Input::Pointer { x: 60.0, y: 60.0 });
        let press = |pressed| Input::Press { pressed, modifiers: spatiand_app::Modifiers::default() };
        assert!(matches!(view.input(&press(true)), Want::Nothing));
        assert!(view.swallowing && view.pressed_at.is_none());
        assert_eq!(view.chrome(), 1.0, "back");
        assert!(matches!(view.input(&press(false)), Want::Nothing));
        assert!(!view.swallowing);
        // Up, the next press is the controls' own.
        view.input(&press(true));
        assert!(!view.swallowing);
    }

    #[test]
    fn the_arrow_keys_slide_and_open_what_they_arrive_at() {
        let mut view = viewing(3, 1);
        assert!(matches!(view.input(&key(Key::Right, "")), Want::Nothing), "not at once");
        view.tick();
        assert_eq!(view.heading(), Some(2), "read ahead as soon as it is off");
        assert_eq!(view.strip.cursor, 2, "the strip says where it is going");
        let until = Instant::now() + Duration::from_secs(2);
        let opened = loop {
            std::thread::sleep(Duration::from_millis(10));
            view.tick();
            if let Want::Open(index) = view.arrival() {
                break index;
            }
            assert!(view.slid < 0.0 && view.slid > -view.page(), "on its way: {}", view.slid);
            assert!(Instant::now() < until, "never arrived");
        };
        assert_eq!(opened, 2);
        // At the first, there is nothing before it to go to.
        let mut first = viewing(3, 0);
        first.input(&key(Key::Left, ""));
        first.tick();
        assert!(first.carousel.resting());
    }

    // --- markup ---

    use crate::markup::{turn_area, Shape as Drawn};
    use crate::tools::Tool;

    /// Command (Control) with a letter, and Shift if `shift`.
    fn chord(c: char, shift: bool) -> Input {
        Input::Key { key: Key::Char(c), text: None, modifiers: spatiand_app::Modifiers { control: true, shift, ..Default::default() }, letter: Some(c) }
    }

    fn click(pressed: bool) -> Input {
        Input::Press { pressed, modifiers: spatiand_app::Modifiers::default() }
    }

    /// A press at `from`, dragged to `to` and let go, as the window has it: where the pages are
    /// worked out first, as drawing does.
    fn drag(view: &mut View, from: (f32, f32), to: (f32, f32)) {
        view.placed = view.placements(view.bounds);
        view.input(&Input::Pointer { x: from.0, y: from.1 });
        view.input(&click(true));
        for step in 1..=4 {
            let t = step as f32 / 4.0;
            view.input(&Input::Pointer { x: from.0 + (to.0 - from.0) * t, y: from.1 + (to.1 - from.1) * t });
        }
        view.input(&click(false));
    }

    /// The picture at `at` of `files` with markup's tools out.
    fn marking(files: usize, at: usize) -> View {
        let mut view = viewing(files, at);
        view.input(&chord('a', true));
        assert!(view.marking(), "Command-Shift-A takes the tools out");
        view.placed = view.placements(view.bounds);
        // What drawing tells the tools of the pages before any input reaches them.
        let regions = view.regions();
        desk(&mut view).regions = regions;
        view
    }

    fn desk(view: &mut View) -> &mut Desk {
        view.desk.as_mut().expect("markup")
    }

    fn shown(view: &View) -> &Shown {
        match &view.doc {
            Doc::Picture(shown) => shown,
            _ => unreachable!(),
        }
    }

    #[test]
    fn what_is_drawn_lands_on_the_picture_where_it_was_drawn_through_zoom_and_pan() {
        let mut view = marking(3, 1);
        desk(&mut view).choose(Tool::Box);
        let place = view.placements(view.bounds)[0];
        // The tools' room is taken from the top: the picture is fitted under them.
        assert!(place.rect.y >= HEAD + BAR + 36.0 - 0.01, "{:?}", place.rect);
        assert_eq!(place.region, Area::new(0.0, 0.0, 2000.0, 1500.0));
        drag(&mut view, (400.0, 300.0), (600.0, 500.0));
        let first = Area::between(place.to_page((400.0, 300.0)), place.to_page((600.0, 500.0)));
        assert_eq!(desk(&mut view).markup.marks().notes[0].shape, Drawn::Box(first));
        assert_eq!(desk(&mut view).tool, Tool::Select, "what was drawn is chosen, as Preview does");
        // A picture's point and the window's agree both ways.
        let back = place.to_window(place.to_page((400.0, 300.0)));
        assert!((back.0 - 400.0).abs() < 0.01 && (back.1 - 300.0).abs() < 0.01);
        // Zoomed in about a point and moved: the next box is where it is drawn on the picture as
        // it now is, and the first is still at its own place in the picture.
        if let Doc::Picture(shown) = &mut view.doc {
            shown.zoom_by(2.5, view.bounds, 700.0, 500.0);
            shown.centre.0 += 0.05;
            shown.clamp(view.bounds);
        }
        let zoomed = view.placements(view.bounds)[0];
        assert!(zoomed.scale() > place.scale() * 2.0);
        desk(&mut view).choose(Tool::Circle);
        drag(&mut view, (500.0, 400.0), (700.0, 640.0));
        let second = Area::between(zoomed.to_page((500.0, 400.0)), zoomed.to_page((700.0, 640.0)));
        assert_eq!(desk(&mut view).markup.marks().notes[1].shape, Drawn::Oval(second));
        assert_eq!(desk(&mut view).markup.marks().notes[0].shape, Drawn::Box(first));
        // Turned a quarter: the marks go round with the picture, which is shown turned and is
        // 1500 across now; and undone, back as they were.
        view.input(&key(Key::Char('r'), "r"));
        assert_eq!(shown(&view).frame_size(), (1500.0, 2000.0));
        assert_eq!(desk(&mut view).markup.marks().notes[0].shape, Drawn::Box(turn_area(&first, (2000.0, 1500.0), true)));
        assert_eq!(desk(&mut view).markup.marks().turns, 1);
        if let Doc::Picture(shown) = &mut view.doc {
            assert!(shown.settle_turn(), "the pixels turned as the picture is drawn");
        }
        desk(&mut view).undo();
        view.sync_markup();
        assert_eq!(shown(&view).frame_size(), (2000.0, 1500.0), "turned back");
        assert_eq!(desk(&mut view).markup.marks().notes[0].shape, Drawn::Box(first));
    }

    #[test]
    fn a_crop_is_shown_at_once_and_undone_like_the_rest() {
        let mut view = marking(3, 1);
        desk(&mut view).choose(Tool::Crop);
        drag(&mut view, (500.0, 350.0), (900.0, 650.0));
        let place = view.placed[0];
        let kept = Area::between(place.to_page((500.0, 350.0)), place.to_page((900.0, 650.0)));
        assert_eq!(desk(&mut view).cropping, Some((0, kept)), "drawn, not made yet");
        assert_eq!(shown(&view).cut, WHOLE);
        view.input(&key(Key::Enter, ""));
        assert_eq!(desk(&mut view).markup.marks().crop(0), Some(kept));
        // Fitted again, to what is kept, which the window's pages now say.
        let c = shown(&view).cut;
        assert!((c[0] - kept.x0 / 2000.0).abs() < 1e-6 && (c[3] - kept.y1 / 1500.0).abs() < 1e-6, "{c:?}");
        assert_eq!(view.placements(view.bounds)[0].region, kept);
        assert!(view.title_words().1.starts_with(&format!("{} \u{d7} {}", kept.w().round(), kept.h().round())), "{}", view.title_words().1);
        // Undone and done again, as every change.
        view.input(&chord('z', false));
        assert_eq!(shown(&view).cut, WHOLE);
        view.input(&chord('z', true));
        assert_ne!(shown(&view).cut, WHOLE);
    }

    #[test]
    fn leaving_or_sliding_on_with_markup_asks_first_and_cancel_is_lit() {
        let mut view = marking(3, 1);
        desk(&mut view).choose(Tool::Arrow);
        drag(&mut view, (400.0, 400.0), (800.0, 500.0));
        assert!(view.unsaved());
        // The arrows stay on this file while the tools are out.
        view.input(&key(Key::Right, ""));
        assert!(view.carousel.resting() && view.asking.is_none());
        // Put away (Escape lets go of what is chosen, then of the tools): the marks stay, to save.
        view.input(&key(Key::Escape, ""));
        view.input(&key(Key::Escape, ""));
        assert!(!view.marking() && view.unsaved());
        // Back to the folder: asked, Cancel lit -- Return keeps all.
        assert!(matches!(view.input(&key(Key::Escape, "")), Want::Nothing));
        assert!(matches!(view.asking, Some((_, Then::Leave))));
        assert!(matches!(view.input(&key(Key::Enter, "")), Want::Nothing));
        assert!(view.asking.is_none() && view.unsaved(), "Cancel: nothing lost");
        // The next file: asked; Discard lets go and slides on.
        view.input(&key(Key::Right, ""));
        assert!(matches!(view.asking, Some((_, Then::Slide(1)))));
        view.input(&key(Key::Left, ""));
        view.input(&key(Key::Enter, ""));
        assert!(!view.unsaved() && view.desk.is_none());
        view.tick();
        assert_eq!(view.heading(), Some(2), "on its way to the next");
        // The window closed, or quit: asked, and then quit.
        let mut view = marking(3, 1);
        desk(&mut view).choose(Tool::Box);
        drag(&mut view, (400.0, 400.0), (800.0, 600.0));
        view.ask_quit();
        assert!(matches!(view.input(&key(Key::Escape, "")), Want::Nothing));
        view.ask_quit();
        view.input(&key(Key::Tab, ""));
        assert!(matches!(view.input(&key(Key::Enter, "")), Want::Quit));
    }

    #[test]
    fn a_swipe_with_markup_put_away_still_means_the_next_file_and_asks() {
        let mut view = marking(3, 1);
        desk(&mut view).choose(Tool::Box);
        drag(&mut view, (400.0, 400.0), (800.0, 600.0));
        // While the tools are out, a drag draws: never the next file.
        assert!(view.asking.is_none() && view.carousel.resting());
        view.input(&chord('a', true));
        // Put away: a sideways drag over the fitted picture is the next file's, asked about.
        drag(&mut view, (900.0, 500.0), (500.0, 510.0));
        assert!(matches!(view.asking, Some((_, Then::Slide(1)))));
        view.input(&key(Key::Escape, ""));
        // And a sideways scroll, the previous one's.
        view.input(&Input::Scroll { dx: -12.0, dy: 1.0 });
        assert!(matches!(view.asking, Some((_, Then::Slide(-1)))));
        assert!(view.carousel.resting(), "nothing slid before the answer");
    }

    #[test]
    fn save_asks_before_replacing_and_a_copy_does_not() {
        let mut view = marking(3, 1);
        desk(&mut view).choose(Tool::Box);
        drag(&mut view, (400.0, 400.0), (800.0, 600.0));
        view.input(&chord('s', false));
        let Some((confirm, Then::Replace)) = &view.asking else { panic!("asked") };
        assert_eq!(confirm.text, "Replace b.jpg with the edited picture? This cannot be undone.");
        assert!(matches!(view.input(&key(Key::Enter, "")), Want::Nothing), "Cancel lit");
        view.input(&chord('s', false));
        view.input(&key(Key::Right, ""));
        match view.input(&key(Key::Enter, "")) {
            Want::Save(saving) => {
                assert!(!saving.copy && saving.source == PathBuf::from("/nowhere/b.jpg") && saving.frame == (2000.0, 1500.0));
                assert_eq!(saving.marks.notes.len(), 1);
            }
            _ => panic!("saved"),
        }
        // While it is written nothing is changed; its end shows the file again as it now is.
        assert!(view.saving.is_some());
        assert!(matches!(view.input(&chord('z', false)), Want::Nothing));
        assert_eq!(desk(&mut view).markup.marks().notes.len(), 1);
        view.saved(Ok(PathBuf::from("/nowhere/b.jpg")), None);
        assert!(matches!(view.take_wanted(), Some(Want::Saved { said, .. }) if said == "Saved b.jpg."));
        // A copy is not asked about.
        let mut view = marking(3, 1);
        desk(&mut view).choose(Tool::Box);
        drag(&mut view, (400.0, 400.0), (800.0, 600.0));
        assert!(matches!(view.input(&chord('s', true)), Want::Save(saving) if saving.copy));
        view.saved(Err("the disk is full".into()), None);
        assert_eq!(view.trouble.as_deref(), Some("SpatiLoupe could not save b.jpg: the disk is full."));
        assert!(view.unsaved(), "nothing lost when it was not written");
    }

    #[test]
    fn a_picture_whose_hdr_layer_would_be_lost_says_so_before_it_is_saved() {
        let mut view = marking(3, 1);
        if let Doc::Picture(shown) = &mut view.doc {
            shown.picture.saves.hdr = Some(false);
        }
        desk(&mut view).choose(Tool::Box);
        drag(&mut view, (400.0, 400.0), (800.0, 600.0));
        // Save: the question says the HDR layer goes.
        view.input(&chord('s', false));
        let Some((confirm, Then::Replace)) = &view.asking else { panic!("asked") };
        assert_eq!(confirm.text, "Replace b.jpg with the edited picture? Its HDR layer will be lost: SpatiLoupe cannot carry it over. This cannot be undone.");
        assert!(matches!(view.input(&key(Key::Enter, "")), Want::Nothing), "Cancel lit");
        // Save a copy: asked too, which it otherwise is not.
        view.input(&chord('s', true));
        let Some((confirm, Then::Copy)) = &view.asking else { panic!("asked") };
        assert_eq!(confirm.text, "Save a copy of b.jpg without its HDR layer? SpatiLoupe cannot carry it over.");
        assert!(matches!(view.input(&key(Key::Enter, "")), Want::Nothing), "Cancel lit");
        view.input(&chord('s', true));
        view.input(&key(Key::Right, ""));
        assert!(matches!(view.input(&key(Key::Enter, "")), Want::Save(saving) if saving.copy));
        // A gain map that is carried: no more asked than of any picture.
        let mut view = marking(3, 1);
        if let Doc::Picture(shown) = &mut view.doc {
            shown.picture.saves.hdr = Some(true);
        }
        desk(&mut view).choose(Tool::Box);
        drag(&mut view, (400.0, 400.0), (800.0, 600.0));
        assert!(matches!(view.input(&chord('s', true)), Want::Save(saving) if saving.copy));
    }

    #[test]
    fn a_picture_in_a_format_not_written_is_only_copied() {
        let mut view = viewing(3, 1);
        view.entry.name = "b.heic".into();
        view.input(&chord('a', true));
        desk(&mut view).choose(Tool::Box);
        drag(&mut view, (400.0, 400.0), (800.0, 600.0));
        assert!(!view.can_save_in_place());
        assert!(matches!(view.input(&chord('s', false)), Want::Save(saving) if saving.copy), "Save is a copy, without a question");
        assert_eq!(view.copy_only_words(), "SpatiLoupe does not write HEIC pictures: Save a copy writes a JPEG beside it, or a PNG if it has see-through parts.");
        // Ten bits: its copy keeps them, and the foot says so.
        if let Doc::Picture(shown) = &mut view.doc {
            shown.picture.saves.depth = crate::save::Depth::Sixteen;
        }
        assert_eq!(view.copy_only_words(), "SpatiLoupe does not write HEIC pictures: Save a copy writes a 16-bit PNG beside it, which keeps its depth.");
        // A TIFF of pages, which SpatiLoupe would write but not as it was: only copied, and why.
        view.entry.name = "scan.tif".into();
        assert!(view.can_save_in_place());
        if let Doc::Picture(shown) = &mut view.doc {
            shown.picture.saves.not_over = Some("it holds more than one picture".into());
        }
        assert!(!view.can_save_in_place());
        assert_eq!(view.copy_only_words(), "SpatiLoupe does not write this TIF picture over itself: it holds more than one picture. Save a copy writes a 16-bit PNG beside it, which keeps its depth.");
    }

    #[test]
    fn markup_is_offered_for_flat_still_pictures_and_pdfs_only() {
        let mut stereo = viewing(3, 1);
        if let Doc::Picture(shown) = &mut stereo.doc {
            shown.eyes = Some(Eyes::SideBySide);
        }
        stereo.input(&chord('a', true));
        assert!(stereo.desk.is_none());
        let mut moving = viewing(3, 1);
        if let Doc::Picture(shown) = &mut moving.doc {
            shown.picture.frames = vec![(Vec::new(), Duration::from_millis(100))];
        }
        moving.input(&chord('a', true));
        assert!(moving.desk.is_none());
        let (mut pdf, _tell, _asked) = reading();
        pdf.input(&chord('a', true));
        assert!(pdf.marking());
        // Taken out and put back with nothing drawn: nothing is kept, nothing asked.
        pdf.input(&chord('a', true));
        assert!(pdf.desk.is_none());
    }

    #[test]
    fn words_are_typed_where_pressed_and_kept_by_a_press_elsewhere() {
        let mut view = marking(3, 1);
        desk(&mut view).choose(Tool::Text);
        view.placed = view.placements(view.bounds);
        let place = view.placed[0];
        view.input(&Input::Pointer { x: 500.0, y: 400.0 });
        view.input(&click(true));
        view.input(&click(false));
        assert!(desk(&mut view).typing.is_some());
        for c in "Hi there".chars() {
            view.input(&Input::Key { key: Key::Char(c), text: Some(c.to_string()), modifiers: Default::default(), letter: None });
        }
        // The room's speech is told the words being typed.
        assert_eq!(view.describe().1, "Hi there");
        // A press elsewhere keeps them (the look and feel's rule 9), as a note where it began.
        view.input(&Input::Pointer { x: 900.0, y: 700.0 });
        view.input(&click(true));
        view.input(&click(false));
        let note = desk(&mut view).markup.marks().notes[0].clone();
        let Drawn::Text { at, words, extent, .. } = &note.shape else { panic!("words") };
        assert_eq!(words, "Hi there");
        let start = place.to_page((500.0, 400.0));
        assert!((at.0 - start.0).abs() < 0.01 && (at.1 - start.1).abs() < 0.01);
        assert!(extent.0 > note.size && extent.1 > note.size, "measured: {extent:?}");
        // Chromium's words: black, 12, on a picture 1500 short 22.7 of its pixels.
        assert_eq!((note.colour, note.pick), (crate::markup::Colour::BLACK, crate::markup::Pick::Chromium(12.0)));
        assert!((note.size - 12.0 * 0.75 * 1500.0 / 595.0).abs() < 0.01, "{}", note.size);
        // What the room's speech is told while the tools are out: the tool, and -- the press
        // elsewhere having chosen nothing -- nothing it draws with.
        assert!(view.describe().1.starts_with("Markup is on, with the select tool.\n"), "{}", view.describe().1);
        desk(&mut view).choose(Tool::Text);
        assert!(view.describe().1.starts_with("Markup is on, with the text tool, in black, 12 sans serif."), "{}", view.describe().1);
    }

    #[test]
    fn a_pdf_page_is_drawn_on_in_its_points() {
        let (mut view, _tell, asked) = reading();
        view.input(&chord('a', true));
        // Where drawing put the pages: the first 600 points wide shown 800 wide.
        if let Doc::Pdf(paper) = &mut view.doc {
            paper.placed = vec![(0, Rect::new(320.0, 216.0, 800.0, 1066.67)), (1, Rect::new(320.0, 1310.0, 800.0, 1066.67))];
        }
        desk(&mut view).choose(Tool::Box);
        drag(&mut view, (400.0, 300.0), (720.0, 456.0));
        let note = desk(&mut view).markup.marks().notes[0].clone();
        assert_eq!(note.page, 0);
        let Drawn::Box(a) = note.shape else { panic!("a box") };
        // (400 - 320) / 800 * 600 = 60 points in, (300 - 216) * 0.75 = 63 down.
        assert!((a.x0 - 60.0).abs() < 0.01 && (a.y0 - 63.0).abs() < 0.01 && (a.x1 - 300.0).abs() < 0.01 && (a.y1 - 180.0).abs() < 0.01, "{a:?}");
        // Saved: asked of the document's own thread, which writes it.
        view.input(&chord('s', true));
        let saved = std::iter::from_fn(|| asked.try_recv().ok()).find_map(|ask| match ask {
            Ask::Save { marks, copy } => Some((marks.notes.len(), copy)),
            _ => None,
        });
        assert_eq!(saved, Some((1, true)));
    }

    fn typed(view: &mut View, words: &str) {
        for c in words.chars() {
            view.input(&Input::Key { key: Key::Char(c), text: Some(c.to_string()), modifiers: Default::default(), letter: None });
        }
    }

    fn lock(view: &mut View) -> Option<&mut Lock> {
        paper(view).lock.as_mut()
    }

    #[test]
    fn a_locked_pdf_asks_for_its_password_and_keeps_none_of_it() {
        let entry = Entry { name: "locked.pdf".into(), path: PathBuf::from("/nowhere/locked.pdf"), kind: Kind::Pdf, size: 1, modified: String::new() };
        let mut view = View::new(entry, 0, 1, vec![0]);
        let (pdf, tell, asked) = Pdf::played();
        view.doc = Doc::Pdf(Box::new(Paper::new(pdf)));
        tell.send(pdf::Answer::Locked { wrong: false }).unwrap();
        view.answers();
        let field = &lock(&mut view).expect("asked").field;
        assert!(field.secret && field.focused, "a field of dots, typed into at once");
        assert_eq!(paper_hint(paper(&mut view)), "[Enter] Open  [Esc] Give up");
        // Typed: never said to the room's speech, nor anywhere but the field.
        typed(&mut view, "pässwörd");
        assert_eq!(lock(&mut view).unwrap().field.text, "pässwörd");
        let said = view.describe().1;
        assert!(!said.contains("pässwörd") && said.contains("locked"), "{said}");
        // The arrows are the field's, not the folder's.
        assert!(matches!(view.input(&key(Key::Left, "")), Want::Nothing));
        assert!(view.carousel.resting());
        // Enter: given to the PDF's thread, the field emptied.
        view.input(&key(Key::Enter, ""));
        let given = std::iter::from_fn(|| asked.try_recv().ok()).find_map(|ask| match ask {
            Ask::Unlock(secret) => Some(secret.expose().to_string()),
            _ => None,
        });
        assert_eq!(given.as_deref(), Some("pässwörd"));
        let lock_now = lock(&mut view).unwrap();
        assert!(lock_now.field.text.is_empty() && lock_now.trying && !lock_now.wrong);
        // A wrong one is said, and asked again.
        tell.send(pdf::Answer::Locked { wrong: true }).unwrap();
        view.answers();
        let lock_now = lock(&mut view).unwrap();
        assert!(lock_now.wrong && !lock_now.trying && lock_now.field.text.is_empty());
        // Enter with nothing typed gives nothing.
        view.input(&key(Key::Enter, ""));
        assert!(asked.try_recv().is_err());
        // The right one: the pages, and the question gone.
        typed(&mut view, "user");
        view.input(&key(Key::Enter, ""));
        let page = pdf::Page { width: 600.0, height: 800.0 };
        tell.send(pdf::Answer::Opened { pages: vec![page], outline: Vec::new(), title: None, rights: pdf::Rights { encrypted: true, annotate: true, modify: true } }).unwrap();
        view.answers();
        assert!(lock(&mut view).is_none() && paper(&mut view).pages.len() == 1);
        assert!(view.about().iter().any(|(what, value)| what == "Security" && value == "Encrypted; marking up allowed"));
        // Escape gives up asking: the file says it is locked.
        let entry = Entry { name: "locked.pdf".into(), path: PathBuf::from("/nowhere/locked.pdf"), kind: Kind::Pdf, size: 1, modified: String::new() };
        let mut view = View::new(entry, 0, 1, vec![0]);
        let (pdf, tell, _asked) = Pdf::played();
        view.doc = Doc::Pdf(Box::new(Paper::new(pdf)));
        tell.send(pdf::Answer::Locked { wrong: false }).unwrap();
        view.answers();
        typed(&mut view, "half");
        assert!(matches!(view.input(&key(Key::Escape, "")), Want::Nothing), "not back to the folder");
        assert!(matches!(&view.doc, Doc::Failed(why) if why == "locked.pdf is locked with a password."));
    }

    #[test]
    fn markup_keeps_to_what_a_pdf_s_owner_allows() {
        // Notes not allowed: not offered, and said why.
        let (mut view, tell, _asked) = reading();
        tell.send(pdf::Answer::Opened { pages: vec![pdf::Page { width: 600.0, height: 800.0 }], outline: Vec::new(), title: None, rights: pdf::Rights { encrypted: true, annotate: false, modify: true } }).unwrap();
        view.answers();
        view.input(&chord('a', true));
        assert!(view.desk.is_none() && !view.marking());
        assert_eq!(view.trouble.as_deref(), Some("The owner of a.pdf does not allow marking it up: its permissions keep notes out."));
        // Notes allowed, the pages not to be changed: markup, without Crop.
        let (mut view, tell, _asked) = reading();
        tell.send(pdf::Answer::Opened { pages: vec![pdf::Page { width: 600.0, height: 800.0 }], outline: Vec::new(), title: None, rights: pdf::Rights { encrypted: true, annotate: true, modify: false } }).unwrap();
        view.answers();
        view.input(&chord('a', true));
        assert!(view.marking());
        assert!(!desk(&mut view).may_crop);
        desk(&mut view).choose(Tool::Crop);
        assert_eq!(desk(&mut view).tool, Tool::Select, "Crop is not taken up");
        desk(&mut view).choose(Tool::Box);
        assert_eq!(desk(&mut view).tool, Tool::Box);
        // Not encrypted, or opened with the owner's password: everything.
        let (mut view, _tell, _asked) = reading();
        view.input(&chord('a', true));
        assert!(view.marking() && desk(&mut view).may_crop);
    }

    #[test]
    fn a_locked_pdf_written_is_opened_again_with_its_password() {
        let (mut view, tell, _asked) = reading();
        view.input(&chord('a', true));
        if let Doc::Pdf(paper) = &mut view.doc {
            paper.placed = vec![(0, Rect::new(320.0, 216.0, 800.0, 1066.67))];
        }
        desk(&mut view).choose(Tool::Box);
        drag(&mut view, (400.0, 300.0), (720.0, 456.0));
        view.input(&chord('s', true));
        tell.send(pdf::Answer::Saved { result: Ok(PathBuf::from("/nowhere/a (edited).pdf")), password: Some(pdf::Secret::new("user".into())) }).unwrap();
        view.answers();
        match view.take_wanted() {
            Some(Want::Saved { path, password: Some(password), .. }) => {
                assert_eq!(path, PathBuf::from("/nowhere/a (edited).pdf"));
                assert_eq!(password.expose(), "user");
                assert_eq!(format!("{password:?}"), "Secret", "never printed");
            }
            _ => panic!("written, with its password"),
        }
    }
}
