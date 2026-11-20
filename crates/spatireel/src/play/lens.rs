//! Live Text in SpatiReel: the text in a paused film's frame, chosen, copied and translated as text
//! (the owner, 2026-11-19: "spatireel also can be treated same too... so when play video, any text,
//! when paused, can be selected"). The same piece as SpatiLoupe's (`spatiand_app::live_text`).
//!
//! A flat film paused on one frame for half a second has that frame read: taken at its own size
//! as it is shown -- upright, its pixels' shape, one eye of a film with two -- by drawing it once
//! into a picture of that size and reading the picture back off the GPU, which is the same for a
//! hardware frame on every platform (VAAPI's, VideoToolbox's, MediaCodec's) as for one in memory;
//! then sent as a PNG, upright. Playing on, or moving to another frame, calls the reading off and
//! lets go of its text. A film in the sky (VR180, 360) is not read. A subtitle on show, in words,
//! is chosen from its own words where they are drawn: nothing is read for it.
//!
//! The controls' own rules stay: a tap on the picture puts them away, over text too; a press and
//! drag on the text chooses it instead.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use spatiand_app::glow::{self, HasContext};
use spatiand_app::live_text::{self, Line, LiveText, Mapping, Picture, ReadText, Reading, Said};
use spatiand_app::ui::{Input, Painter, Rect, DOUBLE_CLICK};
use spatiand_app::{EyeLayout, Gl};
use spatiand_media::screen::Screen;
use spatiand_media::subs::{Cue, Words};
use spatiand_media::StereoLayout;

use super::{app_shape, Outcome, Playing, Staging};

/// How long a film is paused on one frame before the frame is read: a pause to look, not one on
/// the way somewhere.
const PAUSED_FOR: Duration = Duration::from_millis(500);

/// What went wrong reading, last said: said once, not for every pause after it.
static TOLD: Mutex<Option<String>> = Mutex::new(None);

/// A paused film's text: its frame's, and its subtitles'.
pub struct Lens {
    pub frame: LiveText,
    pub cues: LiveText,
    /// The frame being read, and the size it was taken at.
    reading: Option<(Reading, (u32, u32))>,
    /// The size of the picture whose text `frame` has.
    read_size: (f32, f32),
    /// The frame paused on: where the film is, and since when.
    still: Option<(f64, Instant)>,
    /// That frame read, or asked for: once.
    asked: bool,
    /// Its pixels wanted at the next draw, to be read.
    capture: bool,
    /// Which subtitles' words `cues` has, and at what size of window.
    cue_key: Option<(Vec<String>, (f32, f32))>,
    /// How far the subtitles were lifted when last drawn: kept while a press on them goes on, or
    /// the controls coming up under it would lift them away from it.
    cue_lift: f32,
    /// A tap on the text, let go of then, and whether the controls were up: what it does -- the
    /// controls put away, or brought up, as a tap on the picture does -- once no second press can
    /// make it two.
    tap: Option<(Instant, bool)>,
    /// Until when the room's word that the hands stirred is not taken to want the controls: it
    /// comes a frame or a few after each press and key, and those were the text's.
    deaf_until: Option<Instant>,
    by_itself: bool,
}

/// How long after a press or a key the text took the room's `hands` for it are not the controls'.
const HANDS_DEAF: Duration = Duration::from_millis(600);

impl Lens {
    pub fn new() -> Lens {
        // A test never asks hemera, whatever the machine's own settings say; nor anything with no
        // hemera to ask.
        let by_itself = !cfg!(test) && spatiand_app::live_text::reads_by_itself(&spatiand_app::ai::settings());
        Lens { frame: LiveText::new(), cues: LiveText::new(), reading: None, read_size: (1.0, 1.0), still: None, asked: false, capture: false, cue_key: None, cue_lift: 0.0, tap: None, deaf_until: None, by_itself }
    }

    /// Everything let go of: the film plays on, or is not one to read.
    fn let_go(&mut self) -> bool {
        let had = self.frame.has_text() || self.cues.has_text() || self.reading.is_some() || self.frame.busy() || self.cues.busy();
        self.frame.set_text(None);
        self.cues.set_text(None);
        self.frame.reading = false;
        self.frame.can_read = false;
        self.reading = None;
        self.still = None;
        self.asked = false;
        self.capture = false;
        self.cue_key = None;
        had
    }
}

/// Say what went wrong, unless it was said last.
fn tell(live: &mut LiveText, words: String) {
    let mut told = TOLD.lock().unwrap();
    if told.as_deref() != Some(words.as_str()) {
        *told = Some(words.clone());
        live.tell(words);
    }
}

/// The frame as it is shown, drawn into a picture `size` of its own -- the part `uv` of it -- and
/// read back: four bytes a pixel, red first, top row first. GL is left as it was found.
fn capture(gl: &Gl, screen: &Screen, size: (u32, u32), uv: [f32; 4], window: (u32, u32)) -> Option<Vec<u8>> {
    let g = gl.api();
    let (w, h) = (size.0.max(1) as i32, size.1.max(1) as i32);
    unsafe {
        let before = g.get_parameter_framebuffer(glow::FRAMEBUFFER_BINDING);
        let texture = g.create_texture().ok()?;
        g.bind_texture(glow::TEXTURE_2D, Some(texture));
        g.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w, h, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
        let Ok(target) = g.create_framebuffer() else {
            g.delete_texture(texture);
            return None;
        };
        g.bind_framebuffer(glow::FRAMEBUFFER, Some(target));
        g.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
        let mut rgba = None;
        if g.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE {
            g.viewport(0, 0, w, h);
            g.disable(glow::SCISSOR_TEST);
            g.clear_color(0.0, 0.0, 0.0, 1.0);
            g.clear(glow::COLOR_BUFFER_BIT);
            screen.draw(gl, (w as u32, h as u32), [0.0, 0.0, w as f32, h as f32], uv);
            let mut out = vec![0u8; w as usize * h as usize * 4];
            g.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            g.read_pixels(0, 0, w, h, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut out)));
            // Drawn opaque, whatever the frame's own alpha said.
            for px in out.chunks_exact_mut(4) {
                px[3] = 255;
            }
            rgba = Some(out);
        }
        g.bind_framebuffer(glow::FRAMEBUFFER, before);
        g.viewport(0, 0, window.0 as i32, window.1 as i32);
        g.delete_framebuffer(target);
        g.delete_texture(texture);
        rgba
    }
}

/// What of a film's frame is read, and at what size, if it is read at all: for a film `staged` so,
/// packed `stereo`, one eye of it seen at `aspect` from a `frame` (upright) whose shown eye is
/// `one_eye`. A flat film whole, one eye of two, at its own height and as wide as it is seen -- a
/// DVD's frame unsqueezed, an eye of a half-width pair at its true shape -- and none in the sky.
fn read_shape(staged: Staging, stereo: StereoLayout, aspect: f32, frame: (u32, u32), one_eye: [f32; 4]) -> Option<((u32, u32), [f32; 4])> {
    let (uv, eye_h) = match staged {
        Staging::Window { eyes: EyeLayout::Mono } => ([0.0, 0.0, 1.0, 1.0], frame.1),
        Staging::Window { .. } | Staging::OneEye => (one_eye, if stereo == StereoLayout::OverUnder { frame.1 / 2 } else { frame.1 }),
        _ => return None,
    };
    let eye_h = eye_h.max(2);
    let w = ((eye_h as f32 * aspect).round() as u32).max(2);
    Some((live_text::within_limit((w, eye_h)), uv))
}

impl Playing {
    /// Where the film is in its window, and which part of its frame is one eye, if it is a film
    /// whose text is read: flat, in the window (one eye of two), not in the sky.
    fn readable(&self) -> Option<((u32, u32), [f32; 4])> {
        let (aspect, frame) = self.eye_shape();
        read_shape(self.staging, self.layout.stereo, aspect, frame, self.one_eye())
    }

    /// Live Text's turn before a picture is drawn: the paused frame to be read once it has stayed,
    /// what came of a reading, what waits on time. Whether to draw again.
    pub(super) fn text_tick(&mut self) -> bool {
        let now = Instant::now();
        let paused = self.player.paused() && self.player.info().width > 0;
        if !paused || self.readable().is_none() {
            return self.lens.let_go();
        }
        let mut changed = self.lens.frame.tick(now) | self.lens.cues.tick(now);
        let at = self.player.position();
        // Another frame -- moved to while paused -- is another picture: what was read of the last
        // is let go of.
        if self.lens.still.is_none_or(|(was, _)| (was - at).abs() > 0.001) {
            changed |= self.lens.let_go();
            self.lens.still = Some((at, now));
            self.lens.frame.can_read = !self.lens.by_itself;
        }
        let since = self.lens.still.map_or(now, |(_, since)| since);
        if !self.lens.asked && self.lens.by_itself && now.duration_since(since) >= PAUSED_FOR {
            self.lens.asked = true;
            self.lens.capture = true;
            changed = true;
        }
        if let Some(result) = self.lens.reading.as_ref().and_then(|(reading, _)| reading.poll()) {
            let (_, size) = self.lens.reading.take().unwrap();
            changed = true;
            match result {
                Ok(read) => {
                    *TOLD.lock().unwrap() = None;
                    log::info!("{}: the paused frame's text read, {} lines", self.name, read.lines.len());
                    self.lens.read_size = (size.0 as f32, size.1 as f32);
                    self.lens.frame.set_text(Some(read));
                }
                Err(words) => tell(&mut self.lens.frame, words),
            }
        }
        let reading = self.lens.reading.is_some() || self.lens.capture;
        if self.lens.frame.reading != reading {
            self.lens.frame.reading = reading;
            changed = true;
        }
        // A tap on the text, now that no second press has come: what a tap on the picture does.
        if let Some((at, was_shown)) = self.lens.tap.filter(|(at, _)| now.duration_since(*at) >= DOUBLE_CLICK) {
            self.lens.tap = None;
            if was_shown {
                self.clock.put_away(now);
            } else if now.duration_since(at) < DOUBLE_CLICK * 4 {
                self.clock.wake(now);
            }
            changed = true;
        }
        // The subtitles kept where they were for a press are let go to where the controls want
        // them, once no second press can come.
        if self.wanted_lift(self.controls_shown()) != self.lens.cue_lift {
            changed = true;
        }
        changed
    }

    /// Whether an input is the text's: a press going on, or going down on the text (and not on
    /// the controls, while they are up); a key its menu or pane has, or one it acts on (copying,
    /// choosing all, letting go). It does not bring the controls up, which would lift the
    /// subtitles from under the pointer.
    pub(super) fn text_takes(&self, input: &Input, was_shown: bool) -> bool {
        let (frame, cues) = (&self.lens.frame, &self.lens.cues);
        if frame.busy() || cues.busy() {
            return true;
        }
        match input {
            Input::Press { pressed: true, .. } => {
                let (x, y) = self.pointer;
                let on_controls = was_shown && self.panel.contains(x, y);
                !on_controls && (frame.over_text(x, y) || cues.over_text(x, y))
            }
            Input::Press { pressed: false, .. } => frame.holding() || cues.holding(),
            Input::Key { key, modifiers, .. } => {
                let command = (modifiers.control || modifiers.logo) && !modifiers.shift;
                let chosen = frame.has_selection() || cues.has_selection();
                match key {
                    spatiand_app::Key::Char('c') => command && chosen,
                    spatiand_app::Key::Char('a') => command && (frame.has_text() || cues.has_text()),
                    spatiand_app::Key::Escape => chosen,
                    // Control or Command pressed on its own, on the way to a chord for the text.
                    spatiand_app::Key::Other => frame.has_text() || cues.has_text(),
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// When Live Text next wants a tick, while the film is paused.
    pub(super) fn text_wake(&self) -> Option<Duration> {
        let settling = self.lens.still.filter(|_| !self.lens.asked && self.lens.by_itself).map(|(_, since)| PAUSED_FOR.saturating_sub(since.elapsed()));
        let reading = self.lens.reading.as_ref().map(|_| Duration::from_millis(100));
        let tap = self.lens.tap.map(|(at, _)| DOUBLE_CLICK.saturating_sub(at.elapsed()));
        [settling, reading, tap, self.lens.frame.next_wake(), self.lens.cues.next_wake()].into_iter().flatten().min()
    }

    /// Whether there is anything of Live Text's to draw over the film.
    pub(super) fn text_shown(&self) -> bool {
        self.lens.frame.has_text() || self.lens.cues.has_text() || self.lens.frame.busy() || self.lens.cues.busy() || self.lens.frame.reading || self.lens.frame.next_wake().is_some()
    }

    /// The frame's pixels, if they are wanted: taken now, with the picture just loaded, and read.
    pub(super) fn text_capture(&mut self, gl: &Gl, window: (u32, u32)) {
        if !std::mem::take(&mut self.lens.capture) {
            return;
        }
        let Some((size, uv)) = self.readable() else { return };
        let started = Instant::now();
        let Some(rgba) = capture(gl, &self.screen, size, uv, window) else {
            log::warn!("{}: the paused frame could not be taken to be read", self.name);
            return;
        };
        log::info!("{}: the paused frame taken at {}x{} in {:.0} ms, to be read", self.name, size.0, size.1, started.elapsed().as_secs_f32() * 1000.0);
        self.lens.reading = Some((Reading::start(move || Picture::png(&rgba, size.0, size.1, true)), size));
    }

    /// The subtitles on show, in words, as text to be chosen where they are drawn (`words` draws
    /// them so, when the controls are not lifting them): made again only when they change. Picture
    /// subtitles are not read.
    pub(super) fn text_cues(&mut self, p: &mut Painter, gl: &Gl, cues: &[std::sync::Arc<Cue>]) {
        if !self.player.paused() {
            return;
        }
        let all = p.bounds();
        let texts: Vec<String> = cues.iter().filter_map(|cue| if let Words::Text(text) = &cue.words { Some(text.clone()) } else { None }).collect();
        let key = (texts.clone(), (all.w, all.h));
        if self.lens.cue_key.as_ref() == Some(&key) {
            return;
        }
        self.lens.cue_key = Some(key);
        let lines: Vec<(usize, &str)> = texts.iter().enumerate().flat_map(|(cue, text)| text.lines().map(move |line| (cue, line))).collect();
        if lines.is_empty() {
            self.lens.cues.set_text(None);
            return;
        }
        // As `words` lays them out, lifted by nothing: centred, a line each, from the foot up.
        let em = all.h * 0.046;
        let mut y = all.bottom() - all.h * 0.06 - lines.len() as f32 * em * 1.35;
        let mut read = Vec::new();
        for (cue, line) in lines {
            let row = Rect::new(all.x + all.w * 0.06, y, all.w * 0.88, em * 1.35);
            let (w, h) = p.measure(gl, line, em, 8192.0 / p.scale());
            let x = row.x + (row.w - w.min(row.w)) * 0.5;
            let top = row.y + (row.h - h) * 0.5;
            let places: Vec<(std::ops::Range<usize>, f32, f32)> = p.places(line, em).into_iter().map(|(bytes, a, b)| (bytes, x + a, x + b)).collect();
            read.push(Line::across(line, cue, [x, top, x + w.min(row.w), top + h], &places));
            y += em * 1.35;
        }
        self.lens.cues.set_text(Some(ReadText::new((all.w, all.h), read)));
    }

    /// How far the subtitles are to be lifted, `shown` the controls: as far as they were while a
    /// press on them goes on, or a second or third may yet come, so that what is pressed on stays
    /// under the pointer.
    fn wanted_lift(&self, shown: bool) -> f32 {
        if self.lens.cues.pressed_lately() || self.lens.cues.busy() {
            self.lens.cue_lift
        } else if shown {
            230.0
        } else {
            0.0
        }
    }

    /// How far the subtitles are lifted for this picture, `shown` the controls.
    pub(super) fn text_lift(&mut self, shown: bool) -> f32 {
        let lifted = self.wanted_lift(shown);
        self.lens.cue_lift = lifted;
        lifted
    }

    /// The marks of what is chosen over the film and its subtitles, drawn for one eye; `lifted` is
    /// how far the controls lift the subtitles.
    pub(super) fn text_draw(&mut self, p: &mut Painter, gl: &Gl, lifted: f32) {
        let all = p.bounds();
        let logical = app_shape(self.arranged);
        self.lens.frame.place(Mapping::onto(self.lens.read_size, Rect::new(all.x, all.y, logical.0, logical.1)));
        self.lens.frame.draw(p, gl);
        self.lens.cues.place(Mapping { m: [1.0, 0.0, 0.0, 0.0, 1.0, -lifted] });
        self.lens.cues.draw(p, gl);
    }

    /// Over everything: the text's menu, its translation, what was done.
    pub(super) fn text_draw_over(&mut self, p: &mut Painter, gl: &Gl, controls: Option<Rect>) {
        let all = p.bounds();
        // Over the controls' shade when it is there, at the window's foot when not.
        self.lens.frame.note_foot = Some(controls.map_or(all.bottom() - 40.0, |panel| panel.y - 12.0));
        self.lens.frame.draw_over(p, gl, all);
        self.lens.cues.draw_over(p, gl, all);
    }

    /// Live Text's part of an input: `Some` when it took it. `was_shown` and `meant` are the
    /// controls' rules: a tap on the picture puts them away, over text too.
    pub(super) fn text_input(&mut self, input: &Input, was_shown: bool, meant: bool) -> Option<Outcome> {
        let (frame, cues) = (&self.lens.frame, &self.lens.cues);
        if !frame.has_text() && !cues.has_text() && !frame.busy() && !cues.busy() && !frame.can_read {
            return None;
        }
        let (x, y) = self.pointer;
        let busy = frame.busy() || cues.busy() || frame.holding() || cues.holding();
        // A press on the controls is theirs, whatever is under them.
        if !busy && matches!(input, Input::Press { pressed: true, .. }) && was_shown && self.panel.contains(x, y) {
            return None;
        }
        if let Input::Pointer { .. } | Input::PointerLeft = input {
            let took = (self.lens.frame.input(input) != Said::Pass) | (self.lens.cues.input(input) != Said::Pass);
            return took.then_some(Outcome::Nothing);
        }
        // A key goes to what has something chosen, wherever the pointer is (the controls coming up
        // lift the subtitles from under it); a press to what is under the pointer.
        let (frame, cues) = (&self.lens.frame, &self.lens.cues);
        let on_cues = cues.busy()
            || cues.holding()
            || (!frame.busy() && !frame.holding() && if matches!(input, Input::Key { .. }) { cues.has_selection() } else { cues.over_text(x, y) });
        let live = if on_cues { &mut self.lens.cues } else { &mut self.lens.frame };
        let pressed = matches!(input, Input::Press { pressed: true, .. });
        let said = live.input(input);
        if said != Said::Pass {
            // The room's `hands` for this press or key are the text's too.
            self.lens.deaf_until = Some(Instant::now() + HANDS_DEAF);
        }
        match said {
            Said::Pass => None,
            Said::Tapped => {
                // The controls' tap, as it would have been off the text -- once it is not the first
                // of two.
                self.lens.tap = Some((Instant::now(), self.tap.take().is_some_and(|from| super::is_tap(from, self.pointer)) && self.menu.is_none()));
                Some(Outcome::Nothing)
            }
            Said::Read => {
                self.lens.asked = true;
                self.lens.capture = true;
                self.lens.frame.can_read = false;
                Some(Outcome::Nothing)
            }
            Said::Taken | Said::Copied => {
                // A press on the text may yet be a tap: remembered as the controls remember one;
                // and a tap before it, being the first of two, is no tap.
                if pressed {
                    self.tap = meant.then_some(self.pointer);
                    self.lens.tap = None;
                } else if matches!(input, Input::Press { pressed: false, .. }) {
                    self.tap = None;
                }
                Some(Outcome::Nothing)
            }
        }
    }

    /// The right button: the menu of the paused frame's text, or its subtitles'. Whether it opened.
    pub fn text_menu(&mut self) -> bool {
        if !self.player.paused() || self.readable().is_none() {
            return false;
        }
        let (x, y) = self.pointer;
        if self.controls_shown() && self.panel.contains(x, y) {
            return false;
        }
        let opened = if self.lens.cues.over_text(x, y) { self.lens.cues.context(x, y) } else { self.lens.frame.context(x, y) };
        if opened {
            // The controls are left as they are: coming up, they would lift the subtitles from
            // under the menu. The room's `hands` for this press are the menu's.
            self.lens.deaf_until = Some(Instant::now() + HANDS_DEAF);
            self.dirty = true;
        }
        opened
    }

    /// Whether the room's word that the hands stirred is the text's, not the controls': it came
    /// with a press or a key the text took, one is going on, or its menu or pane is open -- or the
    /// pointer is on the text, where the room says it for a press before the press itself comes.
    /// A tap on the text brings the controls up all the same, once it is known to be one.
    pub(super) fn text_deaf(&self) -> bool {
        let (frame, cues) = (&self.lens.frame, &self.lens.cues);
        let (x, y) = self.pointer;
        self.lens.deaf_until.is_some_and(|until| Instant::now() < until) || frame.holding() || cues.holding() || frame.busy() || cues.busy() || frame.over_text(x, y) || cues.over_text(x, y)
    }

    /// The paused frame's text for the room's speech, as read, its unsure words marked.
    pub fn text_described(&self) -> Option<String> {
        self.lens.frame.described()
    }

    /// The AI settings changed: a paused frame read by itself, or only when asked.
    pub fn ai_changed(&mut self) {
        self.lens.by_itself = spatiand_app::live_text::reads_by_itself(&spatiand_app::ai::settings());
        self.lens.frame.can_read = !self.lens.by_itself && !self.lens.frame.has_text() && self.lens.still.is_some();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_film_is_read_whole_one_of_two_eyes_alone_and_the_sky_not_at_all() {
        let whole = [0.0, 0.0, 1.0, 1.0];
        let left = [0.0, 0.0, 0.5, 1.0];
        // 1080p, flat: the frame as it is.
        assert_eq!(read_shape(Staging::Window { eyes: EyeLayout::Mono }, StereoLayout::Mono, 16.0 / 9.0, (1920, 1080), whole), Some(((1920, 1080), whole)));
        // 4K: as it is too.
        assert_eq!(read_shape(Staging::Window { eyes: EyeLayout::Mono }, StereoLayout::Mono, 16.0 / 9.0, (3840, 2160), whole).unwrap().0, (3840, 2160));
        // A DVD's 720 x 576 frame, seen at 16:9: read unsqueezed, at its own height.
        assert_eq!(read_shape(Staging::Window { eyes: EyeLayout::Mono }, StereoLayout::Mono, 16.0 / 9.0, (720, 576), whole).unwrap().0, (1024, 576));
        // Two eyes side by side, half width, in the room or seen as one: the left eye, at its true
        // shape.
        assert_eq!(read_shape(Staging::Window { eyes: EyeLayout::SideBySide }, StereoLayout::SideBySide, 16.0 / 9.0, (1920, 1080), left), Some(((1920, 1080), left)));
        assert_eq!(read_shape(Staging::OneEye, StereoLayout::SideBySide, 16.0 / 9.0, (3840, 1080), left), Some(((1920, 1080), left)));
        // One over the other: an eye is half the frame's height.
        let top = [0.0, 0.0, 1.0, 0.5];
        assert_eq!(read_shape(Staging::OneEye, StereoLayout::OverUnder, 16.0 / 9.0, (1920, 2160), top).unwrap().0, (1920, 1080));
        // VR180 and 360, in the sky or looked about in, and sound alone: not read.
        assert_eq!(read_shape(Staging::Sky { half: true, eyes: EyeLayout::SideBySide }, StereoLayout::SideBySide, 1.0, (3840, 1920), left), None);
        assert_eq!(read_shape(Staging::Look { half: false }, StereoLayout::Mono, 2.0, (3840, 1920), whole), None);
        assert_eq!(read_shape(Staging::Sound, StereoLayout::Mono, 1.0, (0, 0), whole), None);
    }
}
