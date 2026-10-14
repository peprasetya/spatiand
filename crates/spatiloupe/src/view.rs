//! Looking at one thing: a picture, a PDF, an object.
//!
//! What is looked at fills the window; the controls float over it on glass and fade when the
//! wearer has been still for a while, coming back at a touch. Everything is drawn by
//! [`View::paint`] for one eye at a time, so that in the room a stereo photograph or an object
//! gets two pictures and the controls are the same in both.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use spatiand_app::glow::{self, HasContext};
use spatiand_app::ui::{look, theme, Button, Field, Input, List, Painter, Rect, Row, Shape, Strip, Texture, Tile, Typed};
use spatiand_app::{Gl, Key};

use crate::folder::{size_words, Entry};
use crate::model::{self, Eye, Orbit, Renderer};
use crate::pdf::{self, Ask, Pdf};
use crate::picture::Picture;
use crate::stereo::Eyes;

/// How long the controls stay after the wearer last did anything.
const CHROME_STAYS: Duration = Duration::from_millis(3200);

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
    /// Which frame of a moving picture is on show, and since when.
    frame: usize,
    frame_since: Instant,
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
}

impl Shown {
    fn eyes(&self) -> Eyes {
        self.eyes.unwrap_or(self.picture.eyes)
    }

    /// The width over the height of what one eye sees.
    fn eye_aspect(&self) -> f32 {
        let (w, h) = (self.picture.width as f32, self.picture.height.max(1) as f32);
        let (w, h) = if self.turns % 2 == 1 { (h, w) } else { (w, h) };
        match self.eyes() {
            Eyes::Mono | Eyes::Pair => w / h,
            // A pair squeezed into a frame of a film's shape -- sixteen by nine -- is each eye at
            // half its width (or height); any other shape is two views at their own.
            Eyes::SideBySide | Eyes::Crossed => {
                let a = w * 0.5 / h;
                if squeezed(w / h) { a * 2.0 } else { a }
            }
            Eyes::TopBottom => {
                let a = w / (h * 0.5);
                if squeezed(w / h) { a * 0.5 } else { a }
            }
        }
    }

    /// The size, in the picture's own pixels, of what one eye sees.
    fn eye_pixels(&self) -> (f32, f32) {
        let (w, h) = (self.picture.width as f32, self.picture.height as f32);
        let (w, h) = if self.turns % 2 == 1 { (h, w) } else { (w, h) };
        match self.eyes() {
            Eyes::Mono | Eyes::Pair => (w, h),
            Eyes::SideBySide | Eyes::Crossed => (w * 0.5, h),
            Eyes::TopBottom => (w, h * 0.5),
        }
    }

    /// Where the picture is drawn, to fill `area` as it is zoomed and moved: below the bar
    /// along the top, so that a tall picture is not under it.
    fn placed(&self, area: Rect) -> Rect {
        let area = stage(area);
        let aspect = self.eye_aspect();
        let (pw, _) = self.eye_pixels();
        let fit_w = (area.w * 0.94).min(area.h * 0.92 * aspect);
        let w = match self.zoom {
            None => fit_w,
            Some(z) => pw / z,
        };
        let h = w / aspect;
        let (cx, cy) = (area.x + area.w * 0.5, area.y + area.h * 0.5);
        Rect::new(cx - self.centre.0 * w, cy - self.centre.1 * h, w, h)
    }

    fn fit_scale(&self, area: Rect) -> f32 {
        let area = stage(area);
        let aspect = self.eye_aspect();
        let fit_w = (area.w * 0.94).min(area.h * 0.92 * aspect);
        self.eye_pixels().0 / fit_w
    }

    /// Zoom by `by` (more than one is nearer), keeping the point of the picture under
    /// (`x`, `y`) where it is.
    fn zoom_by(&mut self, by: f32, area: Rect, x: f32, y: f32) {
        let before = self.placed(area);
        let fit = self.fit_scale(area);
        let now = self.zoom.unwrap_or(fit);
        // From a twentieth of the fitted size to eight times the picture's own.
        let next = (now / by).clamp(1.0 / 8.0, fit.max(1.0) * 1.0);
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
        let area = stage(area);
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

/// The part of the window a picture is fitted to: all of it but the bar along the top.
fn stage(all: Rect) -> Rect {
    Rect::new(all.x, all.y + 96.0, all.w, (all.h - 96.0 - 12.0).max(1.0))
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
    zoom: f32,
    ticket: u64,
    contents: Option<List>,
    find: Option<Field>,
    found: Vec<pdf::Found>,
    found_at: usize,
    finding: bool,
    /// The page the wearer last asked to see, which the column is moved to once it is laid out.
    go_to: Option<usize>,
    stale: Vec<Texture>,
}

const PAGE_GAP: f32 = 28.0;

impl Paper {
    fn width(&self, area: Rect) -> f32 {
        (area.w * 0.62).min(1000.0) * self.zoom
    }

    /// Where each page is down the column: top, height.
    fn layout(&self, area: Rect) -> Vec<(f32, f32)> {
        let width = self.width(area);
        let mut y = 96.0;
        self.pages
            .iter()
            .map(|page| {
                let h = width * page.height / page.width.max(1.0);
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
}

pub struct View {
    /// The file, and its place in the folder.
    pub entry: Entry,
    pub index: usize,
    pub doc: Doc,
    pub ticket: u64,
    /// The folder's files, by their place in the folder, for the strip along the foot.
    files: Vec<usize>,
    strip: Strip,
    back: Button,
    layout: Button,
    info_button: Button,
    info: bool,
    last_stir: Instant,
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
}

impl View {
    pub fn new(entry: Entry, index: usize, ticket: u64, files: Vec<usize>) -> View {
        let mut strip = Strip::default();
        strip.shape = Shape::Square;
        strip.cursor = files.iter().position(|&i| i == index).unwrap_or(0);
        View {
            entry,
            index,
            doc: Doc::Loading(Instant::now()),
            ticket,
            files,
            strip,
            back: Button::default(),
            layout: Button::default(),
            info_button: Button::default(),
            info: false,
            last_stir: Instant::now(),
            pointer: (0.0, 0.0),
            pressed_at: None,
            dragging: false,
            last_click: None,
            bounds: Rect::new(0.0, 0.0, crate::WINDOW.0 as f32, crate::WINDOW.1 as f32),
            bar: Rect::default(),
            foot: Rect::default(),
            side: Rect::default(),
            trouble: None,
        }
    }

    pub fn open_pdf(&mut self) {
        self.doc = Doc::Loading(Instant::now());
        let pdf = Pdf::open(&self.entry.path);
        self.doc = Doc::Pdf(Box::new(Paper {
            pdf,
            pages: Vec::new(),
            outline: Vec::new(),
            title: None,
            drawn: HashMap::new(),
            asked: HashMap::new(),
            scroll: 0.0,
            zoom: 1.0,
            ticket: 0,
            contents: None,
            find: None,
            found: Vec::new(),
            found_at: 0,
            finding: false,
            go_to: None,
            stale: Vec::new(),
        }));
    }

    pub fn pictured(&mut self, result: Result<Picture, String>) {
        self.doc = match result {
            Ok(picture) => Doc::Picture(Box::new(Shown {
                picture,
                texture: None,
                right: None,
                turns: 0,
                frame: 0,
                frame_since: Instant::now(),
                eyes: None,
                swapped: false,
                depth: 0.0,
                zoom: None,
                centre: (0.5, 0.5),
            })),
            Err(e) => Doc::Failed(e),
        };
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
    pub fn has_depth(&self) -> bool {
        match &self.doc {
            Doc::Picture(shown) => shown.eyes().is_stereo(),
            Doc::Model(_) => true,
            _ => false,
        }
    }

    fn stir(&mut self) {
        self.last_stir = Instant::now();
    }

    /// How much of the controls is to be seen, 0 to 1.
    fn chrome(&self) -> f32 {
        // A document is read, not looked at: its controls stay.
        if matches!(self.doc, Doc::Pdf(_) | Doc::Loading(_) | Doc::Failed(_)) || self.info {
            return 1.0;
        }
        let idle = self.last_stir.elapsed();
        if idle < CHROME_STAYS {
            1.0
        } else {
            (1.0 - (idle - CHROME_STAYS).as_secs_f32() / 0.5).max(0.0)
        }
    }

    pub fn animating(&self) -> bool {
        let fading = self.chrome() > 0.0 && self.chrome() < 1.0 || self.last_stir.elapsed() < CHROME_STAYS + Duration::from_millis(600);
        let moving = match &self.doc {
            Doc::Loading(_) => true,
            Doc::Picture(shown) => !shown.picture.frames.is_empty(),
            Doc::Model(object) => object.turning || object.renderer.is_none(),
            Doc::Pdf(paper) => paper.pages.is_empty() || paper.finding,
            Doc::Failed(_) => false,
        };
        fading || moving
    }

    /// When the next picture is wanted, for a moving picture's next frame.
    pub fn next_wake(&self) -> Option<Duration> {
        match &self.doc {
            Doc::Picture(shown) if !shown.picture.frames.is_empty() => {
                let length = if shown.frame == 0 { shown.picture.first } else { shown.picture.frames[shown.frame - 1].1 };
                Some(length.saturating_sub(shown.frame_since.elapsed()))
            }
            Doc::Model(object) if object.turning => Some(Duration::from_millis(16)),
            _ => None,
        }
    }

    /// What has come back from the PDF's thread.
    pub fn answers(&mut self) -> bool {
        let Doc::Pdf(paper) = &mut self.doc else { return false };
        let mut any = false;
        while let Ok(answer) = paper.pdf.answers.try_recv() {
            any = true;
            match answer {
                pdf::Answer::Opened { pages, outline, title } => {
                    if pages.is_empty() {
                        self.doc = Doc::Failed("This PDF has no pages.".into());
                        return true;
                    }
                    paper.pages = pages;
                    paper.outline = outline;
                    paper.title = title;
                }
                pdf::Answer::Failed(why) => {
                    self.doc = Doc::Failed(why);
                    return true;
                }
                pdf::Answer::Drawn { page, width, height, rgba, ticket } => {
                    if ticket != paper.ticket {
                        continue;
                    }
                    paper.asked.remove(&page);
                    paper.drawn_pending(page, width, height, rgba);
                }
                pdf::Answer::Found { found, ticket } => {
                    if ticket != paper.ticket {
                        continue;
                    }
                    paper.finding = false;
                    paper.found = found;
                    paper.found_at = 0;
                    if let Some(first) = paper.found.first() {
                        paper.go_to = Some(first.page);
                    }
                }
            }
        }
        any
    }

    /// Move on a frame of a moving picture, or turn an object, if it is time.
    pub fn tick(&mut self) -> bool {
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
                    p.begin(gl, canvas, canvas.0 as f32 / logical.0, theme::GROUND);
                    look::backdrop(p, gl);
                    p.end(gl);
                });
                match texture {
                    Ok(texture) => drawn_object = Some(Texture::from_raw(texture, canvas.0, canvas.1)),
                    Err(e) => self.trouble = Some(e),
                }
                unsafe { gl.api().viewport(0, 0, size.0 as i32, size.1 as i32) };
            }
        }
        p.begin_over(gl, size, part, logical);
        let all = p.bounds();
        match drawn_object {
            // Drawn by GL, foot first: turned the right way up here.
            Some(texture) => p.image(gl, texture, all, [0.0, 1.0, 1.0, 0.0], 0.0, theme::INK, true),
            None => look::backdrop(p, gl),
        }
        let chrome = self.chrome();
        match &mut self.doc {
            Doc::Loading(since) => look::waiting(p, gl, all, *since),
            Doc::Failed(why) => look::message(p, gl, all.inset(120.0), why, look::TROUBLE),
            Doc::Picture(shown) => {
                if shown.texture.is_none() {
                    shown.texture = upload(gl, &shown.picture.rgba, shown.picture.width, shown.picture.height).ok();
                    if let Some((w, h, rgba)) = shown.picture.right.as_ref() {
                        shown.right = upload(gl, rgba, *w, *h).ok();
                    }
                    shown.frame = 0;
                }
                if let Some(texture) = shown.texture {
                    if !shown.picture.frames.is_empty() && eye != Eye::Right {
                        let rgba = if shown.frame == 0 { &shown.picture.rgba } else { &shown.picture.frames[shown.frame - 1].0 };
                        if rgba.len() == (texture.width * texture.height * 4) as usize {
                            replace(gl, texture, rgba);
                        }
                    }
                    let use_right = shown.eyes() == Eyes::Pair && (eye == Eye::Right) != shown.swapped;
                    let which = if use_right { shown.right.unwrap_or(texture) } else { texture };
                    let at = shown.placed(all);
                    // A soft shadow under it, as a print on a table has.
                    if shown.zoom.is_none() {
                        p.glow(gl, at, 6.0, [0.0, 0.0, 0.0, 0.55], 40.0);
                    }
                    let uv = turned(shown.uv(eye), shown.turns);
                    p.clip(gl, all);
                    // A picture with see-through parts is shown on a checkerboard-free dark
                    // ground: the backdrop already is one.
                    p.image(gl, which, at, uv, if shown.zoom.is_none() { 6.0 } else { 0.0 }, theme::INK, false);
                    p.unclip(gl);
                }
            }
            Doc::Pdf(paper) => draw_paper(p, gl, paper, all),
            Doc::Model(_) => {}
        }
        if chrome > 0.0 {
            self.controls(p, gl, all, chrome);
        }
        p.end(gl);
    }

    fn title_words(&self) -> (String, String) {
        let name = self.entry.name.clone();
        let mut detail = Vec::new();
        match &self.doc {
            Doc::Picture(shown) => {
                let (w, h) = shown.picture.original;
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
        detail.push(size_words(self.entry.size));
        (name, detail.join("   \u{b7}   "))
    }

    fn controls(&mut self, p: &mut Painter, gl: &Gl, all: Rect, alpha: f32) {
        let ink = |c: [f32; 4]| [c[0], c[1], c[2], c[3] * alpha];
        // The bar along the top.
        let bar = Rect::new(all.x + 28.0, all.y + 24.0, all.w - 56.0, theme::ROW_HEIGHT + 24.0);
        self.bar = bar;
        look::glass(p, gl, bar, 22.0, alpha);
        let mut inner = bar.inset(12.0);
        let back = inner.take_left(Button::width(p, gl, "\u{2039}  Folder"));
        self.back.draw(p, gl, back, "\u{2039}  Folder");
        inner.take_left(20.0);
        let (name, detail) = self.title_words();
        // On the right: what kind of pair it is, and where in the folder.
        let place = format!("{} / {}", self.strip.cursor + 1, self.files.len().max(1));
        let place_w = p.measure(gl, &place, theme::DETAIL_EM, 8192.0).0;
        let info_w = Button::width(p, gl, "Info");
        let mut right = Rect { x: inner.right() - info_w, w: info_w, ..inner };
        self.info_button.draw(p, gl, right, "Info");
        if let Doc::Picture(shown) = &self.doc {
            let label = if shown.eyes().is_stereo() { format!("3D  \u{b7}  {}", short(shown.eyes())) } else { "2D".to_string() };
            let width = Button::width(p, gl, &label);
            right = Rect { x: right.x - width - 10.0, w: width, ..right };
            self.layout.draw(p, gl, right, &label);
        }
        let place_rect = Rect { x: right.x - place_w - 22.0, w: place_w, ..inner };
        p.label(gl, &place, place_rect, theme::DETAIL_EM, ink(theme::INK_HINT), 1.0);
        let words = Rect { w: (place_rect.x - inner.x - 20.0).max(0.0), ..inner };
        let taken = p.label(gl, &name, words, theme::ROW_EM, ink(theme::INK), 0.0);
        let after = Rect { x: words.x + taken + 22.0, w: (words.w - taken - 22.0).max(0.0), y: words.y + 2.0, ..words };
        p.label(gl, &detail, after, theme::DETAIL_EM * 0.95, ink(theme::INK_HINT), 0.0);
        look::rule(p, gl, Rect::new(bar.x + 22.0, bar.bottom() - 2.0, bar.w - 44.0, 2.0));

        // The folder's other files, along the foot.
        self.foot = Rect::default();
        if self.files.len() > 1 && !matches!(self.doc, Doc::Pdf(_)) {
            let width = 118.0;
            let height = Strip::height(Shape::Square, width) - 34.0;
            let wanted = (self.files.len() as f32 * (width + theme::TILE_GAP) + 28.0).min(all.w - 200.0);
            let foot = Rect::new(all.x + (all.w - wanted) * 0.5, all.bottom() - height - 26.0, wanted, height);
            self.foot = foot;
            look::glass(p, gl, foot, 22.0, alpha);
            let tiles = TILES.with(|t| t.borrow().clone());
            self.strip.draw(p, gl, foot, &tiles, width);
        } else if let Doc::Model(_) = self.doc {
            let foot = Rect::new(all.x + 28.0, all.bottom() - 70.0, all.w - 56.0, 40.0);
            p.label(gl, "Drag to turn it    Scroll to come nearer    Space stops or starts it turning    0 starts again", foot, theme::FOOTER_EM, ink(theme::INK_HINT), 0.5);
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
                p.label(gl, &what.to_uppercase(), line, theme::FOOTER_EM * 0.82, theme::INK_HINT, 0.0);
                let line = inner.take_top(34.0);
                p.label(gl, &value, line, theme::DETAIL_EM, theme::INK_DETAIL, 0.0);
                inner.take_top(6.0);
            }
        }
        if let Some(trouble) = &self.trouble {
            let line = Rect::new(all.x + 40.0, all.bottom() - 60.0, all.w - 80.0, 40.0);
            p.label(gl, trouble, line, theme::FOOTER_EM, look::TROUBLE, 0.5);
        }
    }

    fn about(&self) -> Vec<(String, String)> {
        let mut rows = vec![("Name".to_string(), self.entry.name.clone())];
        if let Some(folder) = crate::place::parent(&self.entry.path) {
            rows.push(("Folder".to_string(), folder.display().to_string()));
        }
        rows.push(("Size".to_string(), size_words(self.entry.size)));
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

    fn step(&self, by: isize) -> Want {
        let at = self.files.iter().position(|&i| i == self.index).unwrap_or(0) as isize + by;
        match self.files.get(at.clamp(0, self.files.len() as isize - 1) as usize) {
            Some(&index) if index != self.index => Want::Open(index),
            _ => Want::Nothing,
        }
    }

    pub fn input(&mut self, input: &Input) -> Want {
        self.stir();
        let all = self.bounds;
        if let Input::Pointer { x, y } = input {
            let (dx, dy) = (x - self.pointer.0, y - self.pointer.1);
            self.pointer = (*x, *y);
            if let Some((px, py, _)) = self.pressed_at {
                if !self.dragging && ((x - px).abs() > 6.0 || (y - py).abs() > 6.0) {
                    self.dragging = true;
                }
                if self.dragging {
                    self.drag(dx, dy, all);
                    return Want::Nothing;
                }
            }
        }
        // The search field, while there is one, has the keys.
        if let Doc::Pdf(paper) = &mut self.doc {
            if let Some(field) = paper.find.as_mut() {
                if let Input::Key { key: Key::Escape, .. } = input {
                    paper.find = None;
                    paper.found.clear();
                    return Want::Nothing;
                }
                if let Some(typed) = field.input(input) {
                    if typed == Typed::Entered {
                        let text = field.text.trim().to_string();
                        if paper.found.is_empty() || paper.finding {
                            if !text.is_empty() {
                                paper.ticket += 1;
                                paper.finding = true;
                                paper.pdf.ask(Ask::Find { text, ticket: paper.ticket });
                            }
                        } else {
                            paper.found_at = (paper.found_at + 1) % paper.found.len();
                            paper.go_to = Some(paper.found[paper.found_at].page);
                        }
                    } else {
                        paper.found.clear();
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
        if self.back.input(input) {
            return Want::Leave;
        }
        if self.info_button.input(input) {
            self.info = !self.info;
            return Want::Nothing;
        }
        if self.layout.input(input) {
            self.next_layout();
            return Want::Nothing;
        }
        if !self.foot.w.eq(&0.0) {
            if let Some(chosen) = self.strip.input(input) {
                if let Some(&index) = self.files.get(chosen) {
                    if index != self.index {
                        return Want::Open(index);
                    }
                }
                return Want::Nothing;
            }
        }
        match input {
            Input::Press { pressed: true } => {
                if !self.over_controls(self.pointer.0, self.pointer.1) {
                    self.pressed_at = Some((self.pointer.0, self.pointer.1, Instant::now()));
                    self.dragging = false;
                }
            }
            Input::Press { pressed: false } => {
                let was = self.pressed_at.take();
                let dragged = std::mem::take(&mut self.dragging);
                if let Some(object) = self.object_mut() {
                    object.turned_at = Instant::now();
                }
                if was.is_some() && !dragged {
                    // Twice quickly: nearer, or back to fitted.
                    let double = self.last_click.is_some_and(|t| t.elapsed() < Duration::from_millis(400));
                    self.last_click = Some(Instant::now());
                    if double {
                        self.last_click = None;
                        self.toggle_zoom();
                    }
                }
            }
            Input::Scroll { dx, dy } => self.scrolled(*dx, *dy, all),
            Input::Key { key, text, modifiers } => return self.key(*key, text.as_deref(), modifiers.control || modifiers.logo, modifiers.shift),
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
            Doc::Pdf(paper) => paper.scroll_by(-dy, all),
            _ => {}
        }
    }

    fn scrolled(&mut self, dx: f32, dy: f32, all: Rect) {
        let (x, y) = self.pointer;
        match &mut self.doc {
            Doc::Picture(shown) => {
                if shown.zoom.is_some() && dx.abs() > dy.abs() {
                    let at = shown.placed(all);
                    shown.centre.0 += dx / at.w;
                    shown.clamp(all);
                } else {
                    shown.zoom_by((-dy * 0.004).exp(), all, x, y);
                }
            }
            Doc::Model(object) => object.orbit.zoom = (object.orbit.zoom * (-dy * 0.003).exp()).clamp(0.3, 6.0),
            Doc::Pdf(paper) => paper.scroll_by(dy, all),
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
                    // The picture's own pixels, one to one -- or as near as the GPU's copy has.
                    let fit = shown.fit_scale(all);
                    shown.zoom_by(fit.max(1.0001), all, x, y);
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

    fn key(&mut self, key: Key, text: Option<&str>, command: bool, shift: bool) -> Want {
        let all = self.bounds;
        let typed = text.unwrap_or("");
        match key {
            Key::Escape => {
                if self.info {
                    self.info = false;
                    return Want::Nothing;
                }
                return Want::Leave;
            }
            Key::Left => return self.step(-1),
            Key::Right => return self.step(1),
            Key::Home if !matches!(self.doc, Doc::Pdf(_)) => return self.step(-(self.files.len() as isize)),
            Key::End if !matches!(self.doc, Doc::Pdf(_)) => return self.step(self.files.len() as isize),
            _ => {}
        }
        if typed == "i" {
            self.info = !self.info;
            return Want::Nothing;
        }
        let (x, y) = (all.x + all.w * 0.5, all.y + all.h * 0.5);
        match &mut self.doc {
            Doc::Picture(shown) => match (key, typed) {
                (_, "+" | "=") => shown.zoom_by(1.25, all, x, y),
                (_, "-" | "_") => shown.zoom_by(0.8, all, x, y),
                (_, "0") => {
                    shown.zoom = None;
                    shown.centre = (0.5, 0.5);
                }
                (_, "1") => {
                    let fit = shown.fit_scale(all);
                    shown.zoom = None;
                    shown.zoom_by(fit.max(1.0001), all, x, y);
                }
                (_, "r") => {
                    shown.turns = (shown.turns + 1) % 4;
                    shown.zoom = None;
                    shown.centre = (0.5, 0.5);
                }
                (_, "R") => {
                    shown.turns = (shown.turns + 3) % 4;
                    shown.zoom = None;
                    shown.centre = (0.5, 0.5);
                }
                (_, "3") => self.next_layout(),
                (_, "x") => shown.swapped = !shown.swapped,
                (_, ",") => shown.depth = (shown.depth - 0.005).max(-0.08),
                (_, ".") => shown.depth = (shown.depth + 0.005).min(0.08),
                (Key::Space, _) => return self.step(if shift { -1 } else { 1 }),
                (Key::Up, _) if shown.zoom.is_some() => {
                    shown.centre.1 -= 0.05;
                    shown.clamp(all);
                }
                (Key::Down, _) if shown.zoom.is_some() => {
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
                    (_, "n") if !paper.found.is_empty() => {
                        paper.found_at = (paper.found_at + 1) % paper.found.len();
                        paper.go_to = Some(paper.found[paper.found_at].page);
                    }
                    (_, "N") if !paper.found.is_empty() => {
                        paper.found_at = (paper.found_at + paper.found.len() - 1) % paper.found.len();
                        paper.go_to = Some(paper.found[paper.found_at].page);
                    }
                    (_, "o") if !paper.outline.is_empty() => {
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
    fn open_find(&mut self) {
        let mut field = Field::default();
        field.focused = true;
        field.hint = "Find in this document".into();
        self.find = Some(field);
    }

    fn drawn_pending(&mut self, page: usize, width: u32, height: u32, rgba: Vec<u8>) {
        PENDING.with(|p| p.borrow_mut().push((page, width, height, rgba)));
    }
}

thread_local! {
    /// Pages drawn by the PDF's thread, waiting for the GL thread to put them on the GPU.
    static PENDING: std::cell::RefCell<Vec<(usize, u32, u32, Vec<u8>)>> = const { std::cell::RefCell::new(Vec::new()) };
    /// The folder's tiles, for the strip along the foot: set by the program before drawing.
    pub static TILES: std::cell::RefCell<Vec<Tile>> = const { std::cell::RefCell::new(Vec::new()) };
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

fn draw_paper(p: &mut Painter, gl: &Gl, paper: &mut Paper, all: Rect) {
    for texture in paper.stale.drain(..) {
        p.forget(gl, texture);
    }
    let pending = PENDING.with(|q| std::mem::take(&mut *q.borrow_mut()));
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
    let width = paper.width(all);
    let left = all.x + (all.w - width) * 0.5;
    let layout = paper.layout(all);
    let pixels = (width * p.scale()).round() as u32;
    let mut visible = Vec::new();
    p.clip(gl, all);
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
        p.glow(gl, rect, 4.0, [0.0, 0.0, 0.0, 0.5], 28.0);
        match paper.drawn.get(&index) {
            Some((texture, _)) => p.image(gl, *texture, rect, [0.0, 0.0, 1.0, 1.0], 4.0, theme::INK, true),
            None => p.fill(gl, rect, 4.0, [0.92, 0.92, 0.94, 1.0]),
        }
        // What a search found on this page, washed in the room's teal.
        for (n, found) in paper.found.iter().enumerate().filter(|(_, f)| f.page == index) {
            let current = n == paper.found_at;
            for r in &found.rects {
                let at = Rect::new(rect.x + r[0] * rect.w - 2.0, rect.y + r[1] * rect.h - 2.0, (r[2] - r[0]) * rect.w + 4.0, (r[3] - r[1]) * rect.h + 4.0);
                let colour = if current { [0.086, 0.902, 0.753, 0.45] } else { [0.216, 0.651, 0.969, 0.25] };
                p.fill(gl, at, 3.0, colour);
            }
        }
    }
    p.unclip(gl);
    // Pages are drawn at the width they are shown, and drawn again when that changes much.
    for (index, near) in visible.iter().copied() {
        let have = paper.drawn.get(&index).map(|(_, w)| *w);
        let asked = paper.asked.get(&index).copied();
        let good = |w: u32| (w as f32) >= pixels as f32 * 0.9 && (w as f32) <= pixels as f32 * 1.6;
        if !have.is_some_and(good) && !asked.is_some_and(good) && (near || have.is_none()) {
            paper.asked.insert(index, pixels);
            paper.pdf.ask(Ask::Draw { page: index, width: pixels.min(6000), ticket: paper.ticket });
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
        p.label(gl, &count_text, Rect { x: inner.right() - count_w, w: count_w, ..inner }, theme::DETAIL_EM, theme::INK_HINT, 1.0);
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
    // Which keys do what, on a pane of its own: over a page it would be unreadable.
    let hint = if paper.outline.is_empty() { "/ finds    + and \u{2212} zoom    PageUp and PageDown turn pages" } else { "/ finds    O shows the contents    + and \u{2212} zoom    PageUp and PageDown turn pages" };
    let width = p.measure(gl, hint, theme::FOOTER_EM * 0.9, 8192.0).0 + 48.0;
    let foot = Rect::new(all.x + (all.w - width) * 0.5, all.bottom() - 62.0, width, 42.0);
    look::glass(p, gl, foot, 21.0, 1.0);
    p.label(gl, hint, foot, theme::FOOTER_EM * 0.9, theme::INK_HINT, 0.5);
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

impl View {
    /// Pictures turned by a quarter are turned for real: called before drawing.
    pub fn settle_turns(&mut self, gl: &Gl) {
        if let Doc::Picture(shown) = &mut self.doc {
            while shown.turns % 2 == 1 {
                let clockwise = shown.turns == 1;
                quarter_turn(&mut shown.picture, clockwise);
                shown.turns = if clockwise { 0 } else { 2 };
                for texture in [shown.texture.take(), shown.right.take()].into_iter().flatten() {
                    unsafe { gl.api().delete_texture(texture.raw()) };
                }
            }
        }
    }

    pub fn forget(self, gl: &Gl) {
        match self.doc {
            Doc::Picture(shown) => {
                for texture in [shown.texture, shown.right].into_iter().flatten() {
                    unsafe { gl.api().delete_texture(texture.raw()) };
                }
            }
            Doc::Pdf(paper) => {
                for (texture, _) in paper.drawn.into_values() {
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
    fn a_squeezed_pair_is_shown_at_its_true_shape() {
        let mut shown = Shown {
            picture: picture(1920, 1080),
            texture: None,
            right: None,
            turns: 0,
            frame: 0,
            frame_since: Instant::now(),
            eyes: Some(Eyes::SideBySide),
            swapped: false,
            depth: 0.0,
            zoom: None,
            centre: (0.5, 0.5),
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
            frame: 0,
            frame_since: Instant::now(),
            eyes: None,
            swapped: false,
            depth: 0.0,
            zoom: None,
            centre: (0.5, 0.5),
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
}
