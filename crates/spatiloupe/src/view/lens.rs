//! Live Text in SpatiLoupe: the text in a picture, chosen, copied and translated as text
//! (`spatiand_app::live_text`, the same piece as SpatiReel's).
//!
//! **A picture** is read by itself when it has settled on the window (the AI settings' "read text
//! in pictures by itself", on unless the wearer turned it off) -- not while it slides through the
//! strip -- and the reading is called off if the wearer moves on. What is sent is the file itself
//! where hemera reads its kind (a photograph as its own file: JPEG, PNG, WebP, TIFF), its places
//! coming back in its stored pixels and stood upright by its EXIF orientation here; anything else
//! (HEIC, AVIF, JPEG XL, JPEG 2000, a drawing, a GIF's still) as its pixels -- a JPEG of a
//! photograph's, a PNG of the rest. A stereo pair is read in its left eye only; a moving picture
//! and an object are not read. What was read is kept for the session by the file's path, size and
//! time, in memory only, so coming back to a picture shows its text at once.
//!
//! **A PDF**'s text is SpatiMesh's viewer's, which shows it: its own text, and a scanned page's
//! words read by hemera there too (SpatiMesh's patch 0037), chosen, copied and found in it.
//!
//! **Markup** draws on the picture: while its tools are out, the text is not chosen.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use spatiand_app::live_text::{self, turned_size, Kind, LiveText, Mapping, Picture as Sent, ReadText, Reading, Said};
use spatiand_app::ui::{Input, Painter, Rect};
use spatiand_app::Gl;

use super::{Doc, Shown, View, Want};
use crate::model::Eye;
use crate::stereo::Eyes;

/// How long a picture stays still before its text is read: a picture slid past is not read.
const SETTLE: Duration = Duration::from_millis(350);

/// Where a picture's read text is kept: the picture as SpatiLoupe read it, upright, before the
/// wearer turned it -- or one eye of a pair, as it was when read.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Space {
    Upright,
    Eye,
}

/// A file's text read this session, by its path, size and time: memory only, never a file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Known {
    path: PathBuf,
    size: u64,
    modified: String,
}

/// Each file's read text, and the space it is kept in.
type Read = HashMap<Known, Arc<(ReadText, Space)>>;

static KNOWN: Mutex<Option<Read>> = Mutex::new(None);

/// What went wrong reading, last said: said once, not again for every picture after it.
static TOLD: Mutex<Option<String>> = Mutex::new(None);

/// What has to be known of a reading's picture for its places to land on the picture shown: from
/// the pixels sent into the space it is kept in, and that space's size.
#[derive(Debug, Clone, Copy)]
struct Basis {
    to_space: Mapping,
    size: (f32, f32),
}

/// A picture's text, as Live Text has it.
pub struct Lens {
    pub live: LiveText,
    reading: Option<(Reading, Arc<Mutex<Option<Basis>>>)>,
    space: Space,
    /// Since when the picture has been still on the window, settled.
    still: Option<Instant>,
    /// Read, being read, or not to be: asked once.
    asked: bool,
    /// Read by itself, as the AI settings have it.
    by_itself: bool,
}

impl Lens {
    pub fn new() -> Lens {
        // A test never asks hemera, whatever the machine's own settings say; nor anything with no
        // hemera to ask.
        let by_itself = !cfg!(test) && spatiand_app::live_text::reads_by_itself(&spatiand_app::ai::settings());
        let mut live = LiveText::new();
        live.can_read = !by_itself;
        Lens { live, reading: None, space: Space::Upright, still: None, asked: false, by_itself }
    }

    pub fn reading(&self) -> bool {
        self.reading.is_some()
    }

}

/// Say what went wrong, unless it was said last.
fn tell(view_trouble: &mut Option<String>, at: &mut Option<Instant>, words: String) {
    let mut told = TOLD.lock().unwrap();
    if told.as_deref() == Some(words.as_str()) {
        return;
    }
    *told = Some(words.clone());
    *view_trouble = Some(words);
    *at = Some(Instant::now());
}

/// Whether a file's own bytes go to hemera as they are, by its name: the kinds it reads.
fn sent_as_it_is(name: &str) -> bool {
    let ending = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    matches!(ending.as_str(), "jpg" | "jpeg" | "jpe" | "jfif" | "mpo" | "png" | "webp" | "tif" | "tiff")
}

/// Whether a picture of this name is a photograph's kind, whose pixels go as a JPEG.
fn photographic(name: &str) -> bool {
    let ending = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    matches!(ending.as_str(), "heic" | "heif" | "hif" | "avif" | "jxl" | "jp2" | "j2k" | "jpf" | "jpx" | "j2c" | "jps" | "jpg" | "jpeg" | "mpo")
}

/// Pixels as what hemera is sent: a JPEG of a photograph's (it is a tenth of a PNG's size and as
/// readable), a PNG of anything else; made smaller first only if hemera would refuse it.
fn as_sent(rgba: Vec<u8>, width: u32, height: u32, photo: bool) -> Result<(Sent, (u32, u32)), String> {
    let (w, h) = live_text::within_limit((width, height));
    let (rgba, w, h) = if (w, h) != (width, height) {
        let image = image::RgbaImage::from_raw(width, height, rgba).ok_or("the picture's pixels are not whole")?;
        (image::imageops::resize(&image, w, h, image::imageops::FilterType::Triangle).into_raw(), w, h)
    } else {
        (rgba, width, height)
    };
    if !photo {
        return Ok((Sent::png(&rgba, w, h, false)?, (w, h)));
    }
    let rgb: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut bytes = Vec::with_capacity(rgb.len() / 8);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 92).encode(&rgb, w, h, image::ExtendedColorType::Rgb8).map_err(|e| e.to_string())?;
    Ok((Sent { bytes, kind: Kind::Jpeg, upright: false }, (w, h)))
}

/// A file's own bytes as what hemera is sent, and where its places go: from its stored pixels,
/// upright as its EXIF orientation says. Too large for hemera, it is made smaller, as a JPEG.
fn file_as_sent(bytes: Vec<u8>) -> Result<(Sent, Basis), String> {
    use image::ImageDecoder;
    let mut decoder = image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().map_err(|e| e.to_string())?.into_decoder().map_err(|e| e.to_string())?;
    let stored = decoder.dimensions();
    let orientation = decoder.orientation().map_or(1, |o| o.to_exif());
    drop(decoder);
    let stored_f = (stored.0 as f32, stored.1 as f32);
    let upright = live_text::upright_size(orientation, stored_f);
    match Sent::file(bytes.clone()).filter(|_| live_text::within_limit(stored) == stored) {
        Some(sent) => Ok((sent, Basis { to_space: Mapping::exif(orientation, stored_f), size: upright })),
        None => {
            // Larger than hemera reads: its pixels, smaller, stood upright here.
            let image = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
            let mut image = image;
            if let Some(o) = image::metadata::Orientation::from_exif(orientation) {
                image.apply_orientation(o);
            }
            let rgba = image.to_rgba8();
            let (w, h) = rgba.dimensions();
            let (sent, size) = as_sent(rgba.into_raw(), w, h, true)?;
            Ok((sent, Basis { to_space: Mapping::resized((size.0 as f32, size.1 as f32), upright), size: upright }))
        }
    }
}

impl View {
    /// The text of what is shown, ready to be read where it has settled: called each tick.
    pub(super) fn text_tick(&mut self) -> bool {
        let mut changed = false;
        let now = Instant::now();
        let resting = self.carousel.resting() && !self.carousel.busy();
        match &mut self.doc {
            Doc::Picture(shown) => {
                changed |= self.lens.live.tick(now);
                // Slid away from, or being: not read now; read again once it has settled.
                if !resting {
                    self.lens.still = None;
                    if self.lens.reading.take().is_some() {
                        self.lens.asked = false;
                        changed = true;
                    }
                    return changed;
                }
                let still = *self.lens.still.get_or_insert(now);
                let due = !self.lens.asked && self.lens.by_itself && now.duration_since(still) >= SETTLE && shown.faded_in() >= 1.0;
                if due {
                    self.lens.asked = true;
                    changed |= start_picture(&mut self.lens, shown, &self.entry);
                }
            }
            _ => {}
        }
        changed
    }

    /// What the readings going on have come to.
    pub(super) fn text_answers(&mut self) -> bool {
        let mut changed = false;
        match &mut self.doc {
            Doc::Picture(_) => {
                let came = self.lens.reading.as_ref().and_then(|(reading, _)| reading.poll());
                if let Some(result) = came {
                    let (_, cell) = self.lens.reading.take().unwrap();
                    changed = true;
                    let basis = cell.lock().unwrap().take();
                    match (result, basis) {
                        (Ok(read), Some(basis)) => {
                            let read = read.mapped(&basis.to_space, basis.size);
                            log::info!("{}: its text read, {} lines", self.entry.name, read.lines.len());
                            *TOLD.lock().unwrap() = None;
                            let known = Known { path: self.entry.path.clone(), size: self.entry.size, modified: self.entry.modified.clone() };
                            KNOWN.lock().unwrap().get_or_insert_with(HashMap::new).insert(known, Arc::new((read.clone(), self.lens.space)));
                            self.lens.live.set_text(Some(read));
                        }
                        (Ok(_), None) => {}
                        (Err(words), _) => tell(&mut self.trouble, &mut self.trouble_at, words),
                    }
                }
            }
            _ => {}
        }
        changed
    }

    /// The live text that has the wearer's input now: the picture's, while markup's tools are not
    /// out.
    fn text_target(&mut self) -> Option<&mut LiveText> {
        if self.marking() {
            return None;
        }
        match &mut self.doc {
            Doc::Picture(_) => Some(&mut self.lens.live),
            _ => None,
        }
    }

    /// Whether a menu or a translation of the text is open.
    pub(super) fn text_busy(&self) -> bool {
        match &self.doc {
            Doc::Picture(_) => self.lens.live.busy(),
            _ => false,
        }
    }

    /// Live Text's part of an input: `Some` when it took it. A press on the window's controls is
    /// theirs, whatever text is under them.
    pub(super) fn text_input(&mut self, input: &Input) -> Option<Want> {
        let (x, y) = match input {
            Input::Pointer { x, y } => (*x, *y),
            _ => self.pointer,
        };
        let busy = self.text_busy();
        let on_controls = self.over_controls(x, y);
        if !busy && matches!(input, Input::Press { pressed: true, .. }) && on_controls {
            return None;
        }
        let live = self.text_target()?;
        match live.input(input) {
            Said::Pass => None,
            Said::Read => {
                self.read_now();
                Some(Want::Nothing)
            }
            Said::Taken | Said::Tapped | Said::Copied => Some(Want::Nothing),
        }
    }

    /// The right button pressed where the pointer is: the text's menu, where there is text or it
    /// may be read. Whether it opened.
    pub fn text_menu(&mut self) -> bool {
        let (x, y) = self.pointer;
        if self.over_controls(x, y) || self.asking.is_some() || self.saving.is_some() {
            return false;
        }
        let opened = self.text_target().is_some_and(|live| live.context(x, y));
        if opened {
            self.stir();
        }
        opened
    }

    /// Read what is shown now, asked for from its menu (the settings having it read only so).
    fn read_now(&mut self) {
        if let Doc::Picture(shown) = &mut self.doc {
            self.lens.asked = true;
            self.lens.live.can_read = false;
            start_picture(&mut self.lens, shown, &self.entry);
        }
    }

    /// When the text next wants a tick: a press held, a translation, a reading's answer.
    pub(super) fn text_wake(&self) -> Option<Duration> {
        let poll = Duration::from_millis(100);
        match &self.doc {
            Doc::Picture(shown) => {
                let reading = self.lens.reading.as_ref().map(|_| poll);
                // A moving picture is never read: nothing to settle for.
                let readable = shown.picture.frames.is_empty();
                let settling = (readable && !self.lens.asked && self.lens.by_itself).then(|| self.lens.still.map_or(poll, |at| SETTLE.saturating_sub(at.elapsed())));
                [self.lens.live.next_wake(), reading, settling].into_iter().flatten().min()
            }
            _ => None,
        }
    }

    /// The marks over a picture, where it is drawn this frame, `at` (one eye's place of it).
    pub(super) fn text_draw_picture(&mut self, p: &mut Painter, gl: &Gl, at: Rect) {
        let Doc::Picture(shown) = &self.doc else { return };
        if self.marking() || !self.lens.live.has_text() {
            return;
        }
        self.lens.live.place(picture_mapping(shown, self.lens.space, at));
        self.lens.live.draw(p, gl);
    }

    /// Over everything: the text's menu or translation, the status of a reading.
    pub(super) fn text_draw_over(&mut self, p: &mut Painter, gl: &Gl, all: Rect) {
        // What was done is said over the strip, where it is on show.
        let foot = (self.foot.h > 0.0 && self.chrome() > 0.05).then_some(self.foot.y - 12.0);
        self.lens.live.note_foot = foot;
        if let Doc::Picture(_) = &self.doc {
            self.lens.live.draw_over(p, gl, all);
        }
    }

    /// Whether a reading is going on, for the foot to say so.
    pub(super) fn text_reading(&self) -> bool {
        match &self.doc {
            Doc::Picture(_) => self.lens.reading(),
            _ => false,
        }
    }

    /// The text of what is shown, for the room's speech: a picture's, read.
    pub(super) fn text_described(&self) -> Option<String> {
        match &self.doc {
            Doc::Picture(_) => self.lens.live.described(),
            _ => None,
        }
    }

    /// The AI settings changed: read by itself, or only when asked, from now on.
    pub fn ai_changed(&mut self) {
        let by_itself = spatiand_app::live_text::reads_by_itself(&spatiand_app::ai::settings());
        self.lens.by_itself = by_itself;
        self.lens.live.can_read = !by_itself && !self.lens.live.has_text();
    }

    /// A stereo pair turned: its text, read in an eye as it was, is let go of.
    pub(super) fn text_turned(&mut self) {
        if self.lens.space == Space::Eye {
            self.lens.live.set_text(None);
            self.lens.reading = None;
        }
    }

    /// A picture come to be shown: its text at once if it was read this session.
    pub(super) fn text_known(&mut self) {
        let known = Known { path: self.entry.path.clone(), size: self.entry.size, modified: self.entry.modified.clone() };
        let found = KNOWN.lock().unwrap().as_ref().and_then(|k| k.get(&known).cloned());
        if let Some(found) = found {
            let (read, space) = &*found;
            self.lens.space = *space;
            self.lens.asked = true;
            self.lens.live.set_text(Some(read.clone()));
        }
    }
}

/// Where a picture's read text lands on the window, the picture drawn at `at`: from the space it
/// is kept in, through the wearer's quarter turns and markup's crop.
fn picture_mapping(shown: &Shown, space: Space, at: Rect) -> Mapping {
    match space {
        Space::Eye => Mapping::onto(eye_size(shown), at),
        Space::Upright => {
            let frame = shown.frame_size();
            let upright = turned_size(shown.orientation, frame);
            Mapping::turned(shown.orientation, upright).then(&Mapping::cut_onto(frame, shown.cut, at))
        }
    }
}

/// The size of one eye of a pair, in the picture's pixels as they are on the GPU.
fn eye_size(shown: &Shown) -> (f32, f32) {
    let (w, h) = (shown.picture.width as f32, shown.picture.height as f32);
    match shown.eyes() {
        Eyes::SideBySide | Eyes::Crossed => (w * 0.5, h),
        Eyes::TopBottom => (w, h * 0.5),
        Eyes::Mono | Eyes::Pair => (w, h),
    }
}

/// Read a picture's text: whether a reading began. A moving picture is not read; a pair in one
/// frame is read in its left eye, and not once turned.
fn start_picture(lens: &mut Lens, shown: &Shown, entry: &crate::folder::Entry) -> bool {
    if !shown.picture.frames.is_empty() {
        return false;
    }
    let eyes = shown.eyes();
    let basis = Arc::new(Mutex::new(None));
    let cell = basis.clone();
    let reading = if matches!(eyes, Eyes::SideBySide | Eyes::Crossed | Eyes::TopBottom) {
        if shown.orientation != 0 {
            return false;
        }
        // The left eye's part of the frame, as it is on the GPU.
        let (w, h) = (shown.picture.width as usize, shown.picture.height as usize);
        let half = shown.uv(Eye::Left);
        let (x0, y0) = ((half[0] * w as f32).round() as usize, (half[1] * h as f32).round() as usize);
        let (x1, y1) = ((half[2] * w as f32).round() as usize, (half[3] * h as f32).round() as usize);
        let (ew, eh) = (x1.saturating_sub(x0).min(w), y1.saturating_sub(y0).min(h));
        if ew == 0 || eh == 0 {
            return false;
        }
        let mut eye = Vec::with_capacity(ew * eh * 4);
        for row in y0..y0 + eh {
            eye.extend_from_slice(&shown.picture.rgba[(row * w + x0) * 4..(row * w + x0 + ew) * 4]);
        }
        let photo = photographic(&entry.name);
        lens.space = Space::Eye;
        Reading::start(move || {
            let (sent, size) = as_sent(eye, ew as u32, eh as u32, photo)?;
            *cell.lock().unwrap() = Some(Basis { to_space: Mapping::resized((size.0 as f32, size.1 as f32), (ew as f32, eh as f32)), size: (ew as f32, eh as f32) });
            Ok(sent)
        })
    } else if sent_as_it_is(&entry.name) {
        // The file itself: its stored pixels, stood upright by its EXIF orientation.
        lens.space = Space::Upright;
        let path = entry.path.clone();
        Reading::start(move || {
            let bytes = crate::place::read(&path)?;
            let (sent, basis) = file_as_sent(bytes)?;
            *cell.lock().unwrap() = Some(basis);
            Ok(sent)
        })
    } else {
        // Its pixels as they are on the GPU: turned as far as the wearer's turns have been made
        // on them, which are undone to stand them as they were read.
        lens.space = Space::Upright;
        let (w, h) = (shown.picture.width, shown.picture.height);
        let rgba = shown.picture.rgba.clone();
        let settled = (shown.orientation + 4 - shown.turns % 4) % 4;
        let original = (shown.picture.original.0 as f32, shown.picture.original.1 as f32);
        let back = (4 - settled) % 4;
        let upright = turned_size(back, original);
        let photo = photographic(&entry.name);
        Reading::start(move || {
            let (sent, size) = as_sent(rgba, w, h, photo)?;
            let to_space = Mapping::resized((size.0 as f32, size.1 as f32), original).then(&Mapping::turned(back, original));
            *cell.lock().unwrap() = Some(Basis { to_space, size: upright });
            Ok(sent)
        })
    };
    log::info!("{}: its text being read", entry.name);
    lens.reading = Some((reading, basis));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::folder::{Entry, Kind as FileKind};
    use spatiand_app::live_text::HEADING;

    fn picture(width: u32, height: u32, eyes: Eyes) -> crate::picture::Picture {
        crate::picture::Picture {
            width,
            height,
            rgba: vec![128; (width * height * 4) as usize],
            eyes,
            right: None,
            frames: Vec::new(),
            first: Duration::ZERO,
            original: (width, height),
            about: Vec::new(),
            saves: Default::default(),
        }
    }

    fn entry(name: &str) -> Entry {
        Entry { name: name.into(), path: PathBuf::from(format!("/nowhere/{name}")), kind: FileKind::Picture, size: 1234, modified: "2026-11-19 10:00".into() }
    }

    fn viewing(name: &str, width: u32, height: u32, eyes: Eyes) -> View {
        let mut view = View::new(entry(name), 0, 1, vec![0]);
        view.pictured(Ok(picture(width, height, eyes)));
        view
    }

    /// One upright line of `text` across a picture's pixels `x0..x1`, `y0..y1`, its graphemes
    /// side by side.
    fn read(text: &str, size: (f32, f32), [x0, y0, x1, y1]: [f32; 4]) -> ReadText {
        let n = text.chars().count() as f32;
        let places: Vec<(std::ops::Range<usize>, f32, f32)> = text.char_indices().enumerate().map(|(i, (at, c))| (at..at + c.len_utf8(), x0 + (x1 - x0) * i as f32 / n, x0 + (x1 - x0) * (i + 1) as f32 / n)).collect();
        ReadText::new(size, vec![live_text::Line::across(text, 0, [x0, y0, x1, y1], &places)])
    }

    fn shown(view: &mut View) -> &mut Shown {
        match &mut view.doc {
            Doc::Picture(shown) => shown,
            _ => unreachable!(),
        }
    }

    fn near(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01
    }

    #[test]
    fn a_picture_s_text_lands_where_it_is_shown_through_turns_and_a_crop() {
        let mut view = viewing("a.png", 400, 200, Eyes::Mono);
        // Drawn at its own size, 100 across and 50 down.
        let at = Rect::new(100.0, 50.0, 400.0, 200.0);
        assert!(near(picture_mapping(shown(&mut view), Space::Upright, at).apply([10.0, 20.0]), [110.0, 70.0]));
        // Turned a quarter clockwise -- before the turn is made on the CPU, and after -- its top
        // left is the shown picture's top right.
        view.turn(true);
        let tall = Rect::new(0.0, 0.0, 200.0, 400.0);
        assert!(near(picture_mapping(shown(&mut view), Space::Upright, tall).apply([0.0, 0.0]), [200.0, 0.0]));
        assert!(shown(&mut view).settle_turn());
        assert!(near(picture_mapping(shown(&mut view), Space::Upright, tall).apply([0.0, 0.0]), [200.0, 0.0]));
        assert!(near(picture_mapping(shown(&mut view), Space::Upright, tall).apply([400.0, 200.0]), [0.0, 400.0]));
        // Cropped by markup to the shown picture's lower half: what is kept is drawn over `tall`.
        shown(&mut view).cut = [0.0, 0.5, 1.0, 1.0];
        let p = picture_mapping(shown(&mut view), Space::Upright, tall).apply([200.0, 100.0]);
        assert!(near(p, [100.0, 0.0]), "{p:?}");
    }

    #[test]
    fn the_room_is_told_a_picture_s_text_and_markup_takes_its_presses() {
        let mut view = viewing("card.png", 400, 200, Eyes::Mono);
        view.lens.live.set_text(Some(read("NIK 3171234567890001", (400.0, 200.0), [20.0, 20.0, 380.0, 50.0])));
        let (_, said) = view.describe();
        assert!(said.contains(HEADING) && said.contains("NIK 3171234567890001"), "{said}");
        // Drawn 1:1 at the window's top left, and pressed on.
        view.lens.live.place(Mapping::IDENTITY);
        view.pointer = (100.0, 35.0);
        let press = Input::Press { pressed: true, modifiers: Default::default() };
        let release = Input::Press { pressed: false, modifiers: Default::default() };
        view.lens.live.input(&Input::Pointer { x: 100.0, y: 35.0 });
        assert!(view.text_input(&press).is_some(), "a press on the text is the text's");
        assert!(view.text_input(&release).is_some());
        // With markup's tools out, the press is markup's.
        view.toggle_markup();
        assert!(view.marking());
        assert!(view.text_input(&press).is_none());
    }

    #[test]
    fn a_pair_s_text_is_let_go_of_when_it_is_turned_and_a_flat_picture_s_kept() {
        let mut view = viewing("pair.jps", 800, 300, Eyes::SideBySide);
        view.lens.space = Space::Eye;
        view.lens.live.set_text(Some(read("left eye", (400.0, 300.0), [10.0, 10.0, 200.0, 40.0])));
        view.turn(true);
        assert!(!view.lens.live.has_text());
        let mut flat = viewing("flat.png", 400, 200, Eyes::Mono);
        flat.lens.live.set_text(Some(read("kept", (400.0, 200.0), [10.0, 10.0, 200.0, 40.0])));
        flat.turn(true);
        assert!(flat.lens.live.has_text());
    }

    #[test]
    fn a_picture_read_this_session_shows_its_text_at_once_and_a_changed_file_does_not() {
        let known = Known { path: PathBuf::from("/nowhere/seen.png"), size: 1234, modified: "2026-11-19 10:00".into() };
        KNOWN.lock().unwrap().get_or_insert_with(HashMap::new).insert(known, Arc::new((read("seen before", (400.0, 200.0), [10.0, 10.0, 200.0, 40.0]), Space::Upright)));
        let view = viewing("seen.png", 400, 200, Eyes::Mono);
        assert_eq!(view.lens.live.text().map(ReadText::all).as_deref(), Some("seen before"));
        // The same name written again since: another size, another file.
        let mut changed = View::new(Entry { size: 999, ..entry("seen.png") }, 0, 1, vec![0]);
        changed.pictured(Ok(picture(400, 200, Eyes::Mono)));
        assert!(!changed.lens.live.has_text());
    }

    #[test]
    fn a_pdf_s_text_is_its_viewer_s_and_none_of_it_is_read_here() {
        let mut view = View::new(Entry { kind: FileKind::Pdf, ..entry("scan.pdf") }, 0, 1, vec![0]);
        view.open_pdf();
        view.lens.by_itself = true;
        view.text_tick();
        assert!(view.lens.reading.is_none() && !view.text_reading() && view.text_described().is_none());
        // The right button over it opens no menu of SpatiLoupe's: the text is SpatiMesh's to choose.
        assert!(!view.text_menu());
    }

    #[test]
    fn a_file_goes_as_it_is_by_its_kind_and_a_photo_s_pixels_as_a_jpeg() {
        for name in ["a.jpg", "b.JPEG", "c.png", "d.webp", "e.tif", "f.tiff", "g.mpo"] {
            assert!(sent_as_it_is(name), "{name}");
        }
        for name in ["a.heic", "b.avif", "c.gif", "d.bmp", "e.svg", "f.jxl", "noending"] {
            assert!(!sent_as_it_is(name), "{name}");
        }
        assert!(photographic("IMG_0001.HEIC") && photographic("a.jxl") && !photographic("a.gif") && !photographic("a.svg"));
        let rgba: Vec<u8> = (0..16 * 16 * 4).map(|i| (i % 255) as u8).collect();
        let (jpeg, size) = as_sent(rgba.clone(), 16, 16, true).unwrap();
        assert_eq!((jpeg.kind, size), (Kind::Jpeg, (16, 16)));
        assert!(jpeg.bytes.starts_with(&[0xFF, 0xD8, 0xFF]));
        assert_eq!(as_sent(rgba, 16, 16, false).unwrap().0.kind, Kind::Png);
    }

    /// A JPEG `w` by `h`, with an EXIF orientation of `orientation`, as a phone writes one.
    fn jpeg_turned(w: u32, h: u32, orientation: u8) -> Vec<u8> {
        let rgb = vec![200u8; (w * h * 3) as usize];
        let mut plain = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut plain, 80).encode(&rgb, w, h, image::ExtendedColorType::Rgb8).unwrap();
        // APP1 Exif: a little-endian TIFF with one entry, Orientation (0x0112), SHORT.
        let mut tiff = b"II*\0\x08\0\0\0".to_vec();
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x0112u16.to_le_bytes());
        tiff.extend_from_slice(&3u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&(orientation as u32).to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let mut out = plain[..2].to_vec();
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&app1);
        out.extend_from_slice(&plain[2..]);
        out
    }

    #[test]
    fn a_photo_stored_on_its_side_is_read_as_stored_and_its_text_stood_upright() {
        // A phone held upright: 400 by 300 stored, orientation 6, shown 300 by 400.
        let (sent, basis) = file_as_sent(jpeg_turned(400, 300, 6)).unwrap();
        assert_eq!(sent.kind, Kind::Jpeg);
        assert_eq!(basis.size, (300.0, 400.0));
        // What hemera says is at the stored top left is the shown top right.
        let p = basis.to_space.apply([0.0, 0.0]);
        assert!((p[0] - 300.0).abs() < 1e-3 && p[1].abs() < 1e-3, "{p:?}");
        let (_, plain) = file_as_sent(jpeg_turned(400, 300, 1)).unwrap();
        assert_eq!(plain.size, (400.0, 300.0));
        assert_eq!(plain.to_space.apply([10.0, 20.0]), [10.0, 20.0]);
    }
}
