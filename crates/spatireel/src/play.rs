//! A film being watched: where its picture goes, and the controls over it.
//!
//! Where the picture goes is decided from two things -- what the film is, and what the wearer
//! is looking through -- and decided again whenever either changes, which they do: a layout
//! corrected by hand, the glasses unplugged from a Mac.
//!
//! | the film | in the room, two eyes | one picture |
//! |---|---|---|
//! | flat, one picture | the window | the window |
//! | flat, two eyes packed | the window, each eye given its half | the window, one eye |
//! | the inside of a sphere | the sky itself; the window is the controls | the window, looking about |
//!
//! The frame goes to the room as it is in the file. Its halves are not taken apart here to be
//! put together there: the room samples each eye's half itself, at the file's own sharpness.

use std::time::{Duration, Instant};

use std::sync::Arc;

use spatiand_app::ui::{theme, Button, Input, List, Painter, Rect, Row, Texture};
use spatiand_media::subs::{self, Cue, Words};
use spatiand_app::{Anchor, App, EyeLayout, Gl, Key, Layer, Mode, WindowId};
use spatiand_media::hints;
use spatiand_media::player::Player;
use spatiand_media::screen::{Gaze, Screen};
use spatiand_media::{Eye, Packing, Projection, StereoLayout, VideoLayout};

/// Where the film's picture is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Staging {
    /// In the window, as it is in the file, with the room told how its eyes are packed.
    Window { eyes: EyeLayout },
    /// In the window, one eye of it.
    OneEye,
    /// All round the wearer, in a surface of its own; the window is the controls.
    Sky { half: bool, eyes: EyeLayout },
    /// In the window, as a view out from the middle of the sphere.
    Look { half: bool },
}

fn eyes_of(layout: &VideoLayout) -> EyeLayout {
    match layout.stereo {
        StereoLayout::Mono => EyeLayout::Mono,
        StereoLayout::SideBySide => EyeLayout::SideBySide,
        StereoLayout::OverUnder => EyeLayout::TopBottom,
    }
}

/// Where a film of this layout goes. `sky_refused` is the room having said no to the sky --
/// another film has it -- after which a sphere is looked at through the window like anywhere
/// else.
pub fn staging(layout: &VideoLayout, mode: Mode, in_the_room: bool, sky_refused: bool) -> Staging {
    let eyes = eyes_of(layout);
    match layout.projection {
        Projection::Flat => {
            if layout.is_stereo() && mode == Mode::Stereo && in_the_room {
                Staging::Window { eyes }
            } else if layout.is_stereo() {
                Staging::OneEye
            } else {
                Staging::Window { eyes: EyeLayout::Mono }
            }
        }
        projection => {
            let half = projection == Projection::Dome180;
            if in_the_room && !sky_refused {
                Staging::Sky { half, eyes }
            } else {
                Staging::Look { half }
            }
        }
    }
}

/// The most a window's picture is across and down when it is not the film's own frame.
const WINDOW: (f32, f32) = (1920.0, 1080.0);
/// The controls on their own, with the film all round.
const CONTROLS_ALONE: (u32, u32) = (1280, 250);
const CONTROLS_WITH_MENU: (u32, u32) = (1280, 860);
const CONTROLS_SHOWN: Duration = Duration::from_secs(3);

/// The size a window showing a picture of this shape is: as large as fits in [`WINDOW`].
fn fitted(aspect: f32) -> (u32, u32) {
    let (w, h) = if aspect >= WINDOW.0 / WINDOW.1 { (WINDOW.0, WINDOW.0 / aspect) } else { (WINDOW.1 * aspect, WINDOW.1) };
    // Even, which every scaler and half the world's video paths prefer.
    (((w / 2.0).round() * 2.0) as u32, ((h / 2.0).round() * 2.0) as u32)
}

fn clock(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// What the wearer did that the caller has to act on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    Nothing,
    /// Back to the library.
    Close,
}

/// The rows of the menu that corrects what the film was taken to be.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Choice {
    Projection(Projection),
    Stereo(StereoLayout),
    Packing(Packing),
    Swap,
    Sound(usize),
    Words(Said),
    /// Subtitles, or pictures against sound, a quarter of a second earlier or later.
    WordsBy(f64),
    LipBy(f64),
}

/// Where the subtitles on show come from.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Said {
    Nothing,
    /// The `.srt` beside the film.
    Beside,
    /// One of the film's own tracks.
    Track(usize),
}

pub struct Playing {
    pub name: String,
    player: Player,
    screen: Screen,
    pub layout: VideoLayout,
    swapped: bool,
    staging: Staging,
    /// The film's own surface while it is the sky.
    sky: Option<WindowId>,
    sky_refused: bool,
    /// What the window was last told to be, so it is told again only when that changes.
    arranged: Option<(Staging, (u32, u32), (u32, u32), bool)>,
    gaze: Gaze,
    /// The pointer is held down on the picture, turning the view: where it was last.
    turning: Option<(f32, f32)>,
    pointer: (f32, f32),
    controls_until: Instant,
    toggle: Button,
    back: Button,
    options: Button,
    menu: Option<(List, Vec<(Row, Option<Choice>)>)>,
    /// Where the seek bar was drawn, to turn a press on it into a time.
    bar: Rect,
    seeking: bool,
    /// Something changed that the next picture must show.
    pub dirty: bool,
    /// The subtitles from a file beside the film, if there is one.
    beside: Vec<Cue>,
    said: Said,
    /// How much later than the film says the subtitles are shown.
    words_late: f64,
    /// The subtitle last drawn, so that a new one is a reason to draw; and a picture
    /// subtitle's texture, kept for as long as it is the one on show.
    words_shown: Option<Arc<Cue>>,
    words_picture: Option<(usize, Texture)>,
}

/// The `.srt` that goes with a film on this machine: the one with its name, or with its
/// name and a language after it (`Film.en.srt`).
fn beside(film: &str) -> Vec<Cue> {
    let path = std::path::Path::new(film);
    let (Some(folder), Some(stem)) = (path.parent(), path.file_stem().and_then(|s| s.to_str())) else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut found: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("srt")) && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(stem)))
        .collect();
    // The plainest name first: `Film.srt` before `Film.fr.srt`.
    found.sort_by_key(|p| p.as_os_str().len());
    found.first().and_then(|p| std::fs::read(p).ok()).map(|bytes| subs::srt(&String::from_utf8_lossy(&bytes))).unwrap_or_default()
}

impl Playing {
    /// Play what is at `url`. `name` is the file's name, for what it says of the film's layout;
    /// `said` is a layout the film's source vouches for, which outranks the name; `title` is
    /// what the wearer calls it; `resume` is where to take it up, in seconds.
    #[allow(clippy::too_many_arguments)]
    pub fn open(gl: &Gl, url: &str, options: &[(String, String)], name: &str, title: &str, said: Option<VideoLayout>, resume: f64) -> Result<Playing, String> {
        let mut player = Player::open_with(url, options)?;
        if resume > 5.0 {
            player.seek(resume);
        }
        let info = player.info();
        let named = hints::detect(name, info.hints);
        // A source that says how the eyes are packed is believed for that; where the picture
        // goes -- a screen, a dome -- it never says, and the name still may.
        let layout = said.map_or(named, |said| VideoLayout { projection: named.projection, ..said });
        let swapped = info.eyes_swapped;
        let name = title;
        let beside = beside(url);
        let said = if beside.is_empty() { Said::Nothing } else { Said::Beside };
        log::info!("{name}: taken to be {:?}, {:?}, {:?}", layout.projection, layout.stereo, layout.packing);
        Ok(Playing {
            name: name.to_string(),
            player,
            screen: Screen::new(gl)?,
            layout,
            swapped,
            staging: Staging::Window { eyes: EyeLayout::Mono },
            sky: None,
            // `SPATIREEL_SKY=off` keeps a sphere in the window even in the room, to be looked
            // about in by hand.
            sky_refused: std::env::var("SPATIREEL_SKY").as_deref() == Ok("off"),
            arranged: None,
            gaze: Gaze::default(),
            turning: None,
            pointer: (0.0, 0.0),
            controls_until: Instant::now() + CONTROLS_SHOWN,
            toggle: Button::default(),
            back: Button::default(),
            options: Button::default(),
            menu: None,
            bar: Rect::default(),
            seeking: false,
            dirty: true,
            beside,
            said,
            words_late: 0.0,
            words_shown: None,
            words_picture: None,
        })
    }

    pub fn player(&mut self) -> &mut Player {
        &mut self.player
    }

    pub fn sky(&self) -> Option<WindowId> {
        self.sky
    }

    /// The room would not give the film the sky.
    pub fn sky_was_refused(&mut self) {
        self.sky_refused = true;
        self.dirty = true;
    }

    fn controls_shown(&self) -> bool {
        // Always, when the controls are all the window is; and while a menu is open, the film
        // is paused, or the wearer has just done something.
        // `SPATIREEL_CONTROLS=stay` is for a picture of them: a snapshot is taken long after
        // they would have gone.
        matches!(self.staging, Staging::Sky { .. })
            || self.menu.is_some()
            || self.player.paused()
            || Instant::now() < self.controls_until
            || std::env::var("SPATIREEL_CONTROLS").as_deref() == Ok("stay")
    }

    fn stir(&mut self) {
        if !self.controls_shown() {
            self.dirty = true;
        }
        self.controls_until = Instant::now() + CONTROLS_SHOWN;
    }

    /// When this next needs to be drawn whether or not anything happens: the next picture,
    /// or the controls' clock moving on.
    pub fn next_wake(&self) -> Duration {
        if !self.player.paused() {
            Duration::from_millis(8)
        } else {
            Duration::from_millis(250)
        }
    }

    /// The size of one eye's picture as it should be seen, in the film's own pixels down.
    fn eye_shape(&self) -> (f32, (u32, u32)) {
        let info = self.player.info();
        let (fw, fh) = (info.width.max(2), info.height.max(2));
        let aspect = self.layout.eye_aspect(fw, fh);
        (aspect, (fw, fh))
    }

    /// Put the windows the way this film, looked at this way, wants them.
    fn arrange(&mut self, app: &mut App, window: WindowId) {
        let staging = staging(&self.layout, app.mode(), app.in_the_room(), self.sky_refused);
        let (aspect, frame) = self.eye_shape();
        let (picture, shape) = match staging {
            // The file's own frame, and the window the shape of one eye of it.
            Staging::Window { .. } => (frame, fitted(aspect)),
            // One eye, at the size it is in the file down and its true shape across.
            Staging::OneEye => {
                let eye_h = if self.layout.stereo == StereoLayout::OverUnder { frame.1 / 2 } else { frame.1 }.max(2);
                (((eye_h as f32 * aspect / 2.0).round() as u32 * 2, eye_h), fitted(aspect))
            }
            // With the film all round, the window is the controls -- and grows upwards from
            // its foot, which stays where it was, to hold the menu while that is open.
            Staging::Sky { .. } if self.menu.is_some() => (CONTROLS_WITH_MENU, CONTROLS_WITH_MENU),
            Staging::Sky { .. } => (CONTROLS_ALONE, CONTROLS_ALONE),
            Staging::Look { .. } => (fitted(16.0 / 9.0), fitted(16.0 / 9.0)),
        };
        let wanted = (staging, picture, shape, self.swapped);
        if self.arranged == Some(wanted) {
            return;
        }
        log::info!("the film is now {staging:?}: a picture of {}x{} in a window of {}x{}", picture.0, picture.1, shape.0, shape.1);
        self.staging = staging;
        self.arranged = Some(wanted);
        app.resize(window, picture);
        app.set_shape(window, Some(shape));
        match staging {
            Staging::Window { eyes } => app.set_eyes(window, eyes, self.swapped),
            _ => app.set_eyes(window, EyeLayout::Mono, false),
        }
        match staging {
            Staging::Sky { half, eyes } => {
                // The controls keep their foot where the window's was, and get out of the way.
                app.set_anchor(window, Anchor::Bottom);
                app.set_idle_fade(window, Some(CONTROLS_SHOWN));
                let sky = *self.sky.get_or_insert_with(|| app.open(&self.name, frame));
                app.resize(sky, frame);
                app.set_eyes(sky, eyes, self.swapped);
                app.set_layer(sky, if half { Layer::Sky180 } else { Layer::Sky360 });
            }
            _ => {
                app.set_anchor(window, Anchor::Centre);
                app.set_idle_fade(window, None);
                if let Some(sky) = self.sky.take() {
                    app.close(sky);
                }
            }
        }
        self.dirty = true;
    }

    /// Close whatever this opened besides the window it was given.
    pub fn close(mut self, app: &mut App, window: WindowId) {
        if let Some(sky) = self.sky.take() {
            app.close(sky);
        }
        if let Ok(gl) = app.gl() {
            self.screen.clear(gl);
        }
        app.set_eyes(window, EyeLayout::Mono, false);
        app.set_shape(window, None);
        app.set_anchor(window, Anchor::Centre);
        app.set_idle_fade(window, None);
    }

    /// The part of the frame that is the eye shown when only one is: (left, top, right, bottom).
    fn one_eye(&self) -> [f32; 4] {
        let [u, v, w, h] = self.layout.eye_rect(if self.swapped { Eye::Right } else { Eye::Left });
        [u, v, u + w, v + h]
    }

    /// Draw whatever needs drawing. Returns whether anything was.
    pub fn draw(&mut self, app: &mut App, window: WindowId, painter: &mut Option<Painter>) -> Result<bool, String> {
        self.arrange(app, window);
        let (picture, new) = self.player.picture();
        let shown = self.controls_shown();
        let cue = self.cue();
        let words_changed = match (&cue, &self.words_shown) {
            (Some(now), Some(before)) => !Arc::ptr_eq(now, before),
            (None, None) => false,
            _ => true,
        };
        if !new && !self.dirty && !words_changed {
            return Ok(false);
        }
        self.words_shown = cue.clone();
        self.dirty = false;
        let staging = self.staging;
        let one_eye = self.one_eye();
        let (position, duration, paused) = (self.player.position(), self.player.info().duration, self.player.paused());
        let name = self.name.clone();
        let gaze = self.gaze;
        let logical = app_shape(self.arranged);

        // The film's own surface, when it is the sky: the frame, whole, and nothing else.
        if let (Some(sky), Staging::Sky { .. }) = (self.sky, staging) {
            if new || !self.screen.has_picture() {
                let screen = &mut self.screen;
                app.draw(sky, |gl, size| {
                    if let Some(picture) = picture.as_ref() {
                        screen.load(gl, picture);
                    }
                    clear(gl);
                    screen.draw(gl, size, [0.0, 0.0, size.0 as f32, size.1 as f32], [0.0, 0.0, 1.0, 1.0]);
                })?;
            }
        }

        let this = &mut *self;
        app.draw(window, |gl, size| {
            let whole = [0.0, 0.0, size.0 as f32, size.1 as f32];
            if let Some(picture) = picture.as_ref() {
                this.screen.load(gl, picture);
            }
            clear(gl);
            match staging {
                Staging::Window { .. } => this.screen.draw(gl, size, whole, [0.0, 0.0, 1.0, 1.0]),
                Staging::OneEye => this.screen.draw(gl, size, whole, one_eye),
                Staging::Look { half } => this.screen.look(gl, size, whole, one_eye, half, gaze),
                Staging::Sky { .. } => {}
            }
            let alone = matches!(staging, Staging::Sky { .. });
            if !shown && !alone && cue.is_none() {
                return;
            }
            let p = painter.get_or_insert_with(|| Painter::new(gl).expect("the painter"));
            // Once for each eye's part of the picture, each as though it were the whole
            // window: the room stretches each part over the window for its eye.
            let parts: &[Rect] = match staging {
                Staging::Window { eyes: EyeLayout::SideBySide } => {
                    &[Rect::new(0.0, 0.0, whole[2] / 2.0, whole[3]), Rect::new(whole[2] / 2.0, 0.0, whole[2] / 2.0, whole[3])]
                }
                Staging::Window { eyes: EyeLayout::TopBottom } => {
                    &[Rect::new(0.0, 0.0, whole[2], whole[3] / 2.0), Rect::new(0.0, whole[3] / 2.0, whole[2], whole[3] / 2.0)]
                }
                _ => &[Rect::new(0.0, 0.0, whole[2], whole[3])],
            };
            for (eye, part) in parts.iter().enumerate() {
                p.begin_over(gl, size, *part, logical);
                if let (Some(cue), false) = (cue.as_ref(), alone) {
                    // In a film with two eyes the words stand a little in front of the
                    // screen: each eye's are moved towards the other's side, so that the
                    // eyes cross slightly to read them, as they do for anything nearer.
                    let nearer = if parts.len() == 2 { logical.0 * 0.0035 } else { 0.0 };
                    let first_is_left = !this.swapped;
                    let shift = if (eye == 0) == first_is_left { nearer } else { -nearer };
                    this.words(p, gl, cue, shift, if shown { 230.0 } else { 0.0 });
                }
                if shown || alone {
                    this.controls(p, gl, &name, position, duration, paused, alone);
                }
                p.end(gl);
            }
        })?;
        Ok(true)
    }

    /// The subtitle to show now, wherever subtitles are coming from.
    fn cue(&self) -> Option<Arc<Cue>> {
        match self.said {
            Said::Nothing => None,
            Said::Beside => subs::at(&self.beside, self.player.position() - self.words_late).cloned().map(Arc::new).map(|cue| {
                // The same cue is the same thing each time it is asked for, or every picture
                // would look like a new subtitle.
                match &self.words_shown {
                    Some(before) if **before == *cue => before.clone(),
                    _ => cue,
                }
            }),
            Said::Track(_) => self.player.subtitle(),
        }
    }

    /// Draw a subtitle: at the foot of the picture, `lifted` above it when the controls are
    /// there, and `shift` to the right.
    fn words(&mut self, p: &mut Painter, gl: &Gl, cue: &Arc<Cue>, shift: f32, lifted: f32) {
        let all = p.bounds();
        match &cue.words {
            Words::Text(text) => {
                // A size that is the same part of the picture whatever the window: a little
                // over a degree in the glasses, which is what a line to be read at a glance is.
                let em = all.h * 0.046;
                let lines: Vec<&str> = text.lines().collect();
                let mut y = all.bottom() - all.h * 0.06 - lifted - lines.len() as f32 * em * 1.35;
                for line in lines {
                    let row = Rect::new(all.x + all.w * 0.06 + shift, y, all.w * 0.88, em * 1.35);
                    // Dark all round the letters, then the letters: readable over snow.
                    for (dx, dy) in [(-2.0, 0.0), (2.0, 0.0), (0.0, -2.0), (0.0, 2.0), (2.0, 3.0)] {
                        p.label(gl, line, Rect { x: row.x + dx, y: row.y + dy, ..row }, em, [0.0, 0.0, 0.0, 0.85], 0.5);
                    }
                    p.label(gl, line, row, em, [1.0, 1.0, 1.0, 1.0], 0.5);
                    y += em * 1.35;
                }
            }
            Words::Picture { rgba, width, height, place } => {
                let key = Arc::as_ptr(cue) as usize;
                if self.words_picture.map(|(held, _)| held) != Some(key) {
                    if let Some((_, old)) = self.words_picture.take() {
                        p.forget(gl, old);
                    }
                    self.words_picture = p.texture(gl, rgba, *width, *height).ok().map(|texture| (key, texture));
                }
                if let Some((_, texture)) = self.words_picture {
                    let rect = Rect::new(all.x + place[0] * all.w + shift, all.y + place[1] * all.h - lifted.min(all.h * place[1]), place[2] * all.w, place[3] * all.h);
                    p.image(gl, texture, rect, [0.0, 0.0, 1.0, 1.0], 0.0, [1.0, 1.0, 1.0, 1.0], false);
                }
            }
        }
    }

    /// The controls: over the foot of the film, or all the window is.
    #[allow(clippy::too_many_arguments)]
    fn controls(&mut self, p: &mut Painter, gl: &Gl, name: &str, position: f64, duration: f64, paused: bool, alone: bool) {
        let all = p.bounds();
        if alone {
            p.fill(gl, all, 0.0, theme::GROUND);
        }
        let height = 210.0f32.min(all.h);
        let mut panel = Rect::new(all.x, all.bottom() - height, all.w, height);
        if !alone {
            // A shade behind the controls, so they can be read over a bright scene.
            p.fill(gl, panel, 0.0, [0.02, 0.025, 0.04, 0.78]);
        }
        panel = panel.inset(theme::PAD * 0.6);
        let mut top = panel.take_top(theme::ROW_EM * 1.5);
        let time = format!("{} / {}", clock(position), clock(duration));
        let taken = p.label(gl, &time, top, theme::ROW_EM * 0.8, theme::INK_DETAIL, 1.0);
        top.w -= taken + 24.0;
        p.label(gl, name, top, theme::ROW_EM * 0.9, theme::INK, 0.0);

        // How far through, as a bar: pressed anywhere along it, the film goes there.
        let mut line = panel.take_top(34.0);
        line = line.inset(0.0);
        self.bar = line;
        let track = Rect::new(line.x, line.y + line.h * 0.5 - 4.0, line.w, 8.0);
        p.fill(gl, track, 4.0, theme::SCROLL_TRACK);
        let through = if duration > 0.0 { (position / duration).clamp(0.0, 1.0) as f32 } else { 0.0 };
        p.fill(gl, Rect { w: track.w * through, ..track }, 4.0, theme::MARK);
        p.fill(gl, Rect::new(track.x + track.w * through - 9.0, track.y - 5.0, 18.0, 18.0), 9.0, theme::INK);

        panel.take_top(10.0);
        let mut row = panel.take_top(Button::HEIGHT.min(panel.h));
        let play = if paused { "Play" } else { "Pause" };
        let width = Button::width(p, gl, "Pause").max(Button::width(p, gl, "Play"));
        self.toggle.draw(p, gl, row.take_left(width), play);
        row.take_left(12.0);
        let width = Button::width(p, gl, "Picture and sound");
        self.options.draw(p, gl, row.take_left(width), "Picture and sound");
        let width = Button::width(p, gl, "Library");
        self.back.draw(p, gl, Rect { x: row.right() - width, w: width, ..row }, "Library");

        if let Some((list, rows)) = self.menu.as_mut() {
            let rows: Vec<Row> = rows.iter().map(|(row, _)| row.clone()).collect();
            let tall = (rows.len() as f32 * (theme::ROW_HEIGHT + theme::ROW_GAP)).min(all.h - height - 40.0).max(theme::ROW_HEIGHT);
            let card = Rect::new(all.x + theme::PAD * 0.6, (all.bottom() - height - tall - 34.0).max(0.0), 640.0f32.min(all.w - theme::PAD), tall + 28.0);
            p.fill(gl, card.inset(-2.0), theme::CARD_RADIUS * 0.6, theme::RIM);
            p.fill(gl, card, theme::CARD_RADIUS * 0.6, [0.043, 0.05, 0.072, 0.97]);
            list.focused = true;
            list.draw(p, gl, card.inset(14.0), &rows);
        }
    }

    fn open_menu(&mut self) {
        let mut rows: Vec<(Row, Option<Choice>)> = Vec::new();
        let mark = |on: bool| if on { "\u{2713}" } else { "" };
        rows.push((Row::heading("Where the picture is"), None));
        for projection in [Projection::Flat, Projection::Dome180, Projection::Sphere360] {
            rows.push((Row::with(projection.label(), mark(self.layout.projection == projection)), Some(Choice::Projection(projection))));
        }
        rows.push((Row::heading("In the file"), None));
        for stereo in [StereoLayout::Mono, StereoLayout::SideBySide, StereoLayout::OverUnder] {
            rows.push((Row::with(stereo.label(), mark(self.layout.stereo == stereo)), Some(Choice::Stereo(stereo))));
        }
        if self.layout.is_stereo() {
            if self.layout.projection == Projection::Flat {
                for packing in [Packing::Auto, Packing::Half, Packing::Full] {
                    rows.push((Row::with(packing.label(), mark(self.layout.packing == packing)), Some(Choice::Packing(packing))));
                }
            }
            rows.push((Row::with("Right eye first", mark(self.swapped)), Some(Choice::Swap)));
        }
        let info = self.player.info();
        if info.audio.len() > 1 {
            rows.push((Row::heading("Sound"), None));
            for (index, track) in info.audio.iter().enumerate() {
                let what = track.title.clone().or(track.language.clone()).unwrap_or_else(|| format!("Track {}", index + 1));
                let label = format!("{what} \u{b7} {} {}", track.codec, spatiand_media::audio::layout_name(track.channels));
                rows.push((Row::with(label, mark(info.audio_playing == Some(index))), Some(Choice::Sound(index))));
            }
        }
        let offset = |seconds: f64| if seconds == 0.0 { "in step".to_string() } else { format!("{seconds:+.2} s") };
        let tracks = &self.player.info().subtitles;
        if !tracks.is_empty() || !self.beside.is_empty() {
            rows.push((Row::heading("Subtitles"), None));
            rows.push((Row::with("None", mark(self.said == Said::Nothing)), Some(Choice::Words(Said::Nothing))));
            if !self.beside.is_empty() {
                rows.push((Row::with("From the file beside the film", mark(self.said == Said::Beside)), Some(Choice::Words(Said::Beside))));
            }
            for (index, track) in tracks.iter().enumerate() {
                let what = track.title.clone().or(track.language.clone()).unwrap_or_else(|| format!("Track {}", index + 1));
                rows.push((Row::with(format!("{what} \u{b7} {}", track.codec), mark(self.said == Said::Track(index))), Some(Choice::Words(Said::Track(index)))));
            }
            rows.push((Row::with("Subtitles earlier", offset(self.words_late)), Some(Choice::WordsBy(-0.25))));
            rows.push((Row::with("Subtitles later", offset(self.words_late)), Some(Choice::WordsBy(0.25))));
        }
        rows.push((Row::heading("Picture against sound"), None));
        rows.push((Row::with("Picture earlier", offset(self.player.lip())), Some(Choice::LipBy(-0.05))));
        rows.push((Row::with("Picture later", offset(self.player.lip())), Some(Choice::LipBy(0.05))));
        let mut list = List::default();
        list.cursor = 1;
        self.menu = Some((list, rows));
    }

    fn choose(&mut self, choice: Choice) {
        match choice {
            Choice::Projection(projection) => {
                self.layout.projection = projection;
                // Asked for again by hand, the sky is asked for again.
                self.sky_refused = false;
            }
            Choice::Stereo(stereo) => self.layout.stereo = stereo,
            Choice::Packing(packing) => self.layout.packing = packing,
            Choice::Swap => self.swapped = !self.swapped,
            Choice::Sound(index) => self.player.set_audio(index),
            Choice::Words(said) => {
                self.said = said;
                self.player.set_subtitle(match said {
                    Said::Track(index) => Some(index),
                    _ => None,
                });
            }
            Choice::WordsBy(seconds) => self.words_late = ((self.words_late + seconds) * 100.0).round() / 100.0,
            Choice::LipBy(seconds) => {
                let lip = ((self.player.lip() + seconds) * 100.0).round() / 100.0;
                self.player.set_lip(lip);
            }
        }
        // Open again, on the same row, with the marks where they now are.
        let cursor = self.menu.as_ref().map_or(1, |(list, _)| list.cursor);
        self.open_menu();
        if let Some((list, rows)) = self.menu.as_mut() {
            list.cursor = cursor.min(rows.len() - 1);
        }
    }

    fn seek_to_pointer(&mut self) {
        let duration = self.player.info().duration;
        if duration > 0.0 && self.bar.w > 0.0 {
            let through = ((self.pointer.0 - self.bar.x) / self.bar.w).clamp(0.0, 1.0);
            self.player.seek(through as f64 * duration);
        }
    }

    pub fn input(&mut self, input: &Input) -> Outcome {
        self.dirty = true;
        let was_shown = self.controls_shown();
        self.stir();
        if let Input::Pointer { x, y } = input {
            // Held down on a sphere seen through the window: the view turns with the hand.
            if let (Some((from_x, from_y)), Staging::Look { .. }) = (self.turning, self.staging) {
                let per_pixel = self.gaze.field / app_shape(self.arranged).0;
                self.gaze.yaw -= (x - from_x) * per_pixel;
                self.gaze.pitch = (self.gaze.pitch + (y - from_y) * per_pixel).clamp(-1.4, 1.4);
                self.turning = Some((*x, *y));
            }
            self.pointer = (*x, *y);
            if self.seeking {
                self.seek_to_pointer();
            }
        }
        if let Some((list, rows)) = self.menu.as_mut() {
            if let Some(chosen) = list.input(input) {
                if let Some(choice) = rows[chosen].1 {
                    self.choose(choice);
                }
                return Outcome::Nothing;
            }
            match input {
                Input::Key { key: Key::Escape, .. } => self.menu = None,
                // A press anywhere else puts the menu away.
                Input::Press { pressed: true } if !list.contains(self.pointer.0, self.pointer.1) => self.menu = None,
                _ => {}
            }
            if self.menu.is_some() || matches!(input, Input::Key { .. }) {
                return Outcome::Nothing;
            }
        }
        // Buttons that are not on show are not pressed: the press that brings the controls
        // back is only that.
        if was_shown {
            if self.toggle.input(input) {
                let paused = self.player.paused();
                self.player.set_paused(!paused);
            }
            if self.options.input(input) {
                self.open_menu();
            }
            if self.back.input(input) {
                return Outcome::Close;
            }
        }
        match input {
            Input::Press { pressed: true } => {
                if was_shown && self.bar.inset(-12.0).contains(self.pointer.0, self.pointer.1) {
                    self.seeking = true;
                    self.seek_to_pointer();
                } else if self.pointer.1 < self.bar.y - 40.0 || !was_shown {
                    self.turning = Some(self.pointer);
                }
            }
            Input::Press { pressed: false } => {
                self.seeking = false;
                self.turning = None;
            }
            Input::PointerLeft => {
                self.seeking = false;
                self.turning = None;
            }
            Input::Key { key, .. } => {
                let now = self.player.position();
                match key {
                    Key::Space | Key::Enter => {
                        let paused = self.player.paused();
                        self.player.set_paused(!paused);
                    }
                    Key::Left => self.player.seek(now - 10.0),
                    Key::Right => self.player.seek(now + 10.0),
                    Key::Down => self.player.seek(now - 60.0),
                    Key::Up => self.player.seek(now + 60.0),
                    Key::Char('m') => self.open_menu(),
                    Key::Escape | Key::Backspace => return Outcome::Close,
                    _ => {}
                }
            }
            _ => {}
        }
        Outcome::Nothing
    }
}

/// The window's size in the pixels the pointer is told in, which the controls are laid out in.
fn app_shape(arranged: Option<(Staging, (u32, u32), (u32, u32), bool)>) -> (f32, f32) {
    arranged.map_or(WINDOW, |(_, _, shape, _)| (shape.0 as f32, shape.1 as f32))
}

fn clear(gl: &Gl) {
    use spatiand_app::glow::{self, HasContext};
    unsafe {
        gl.api().disable(glow::SCISSOR_TEST);
        gl.api().clear_color(0.0, 0.0, 0.0, 1.0);
        gl.api().clear(glow::COLOR_BUFFER_BIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(projection: Projection, stereo: StereoLayout) -> VideoLayout {
        VideoLayout { projection, stereo, packing: Packing::Auto }
    }

    #[test]
    fn a_plain_film_is_a_window_everywhere() {
        let film = layout(Projection::Flat, StereoLayout::Mono);
        for (mode, room) in [(Mode::Stereo, true), (Mode::Flat, true), (Mode::Flat, false)] {
            assert_eq!(staging(&film, mode, room, false), Staging::Window { eyes: EyeLayout::Mono });
        }
    }

    #[test]
    fn a_3d_film_gives_the_room_both_eyes_and_one_picture_one() {
        let film = layout(Projection::Flat, StereoLayout::SideBySide);
        assert_eq!(staging(&film, Mode::Stereo, true, false), Staging::Window { eyes: EyeLayout::SideBySide });
        // The glasses unplugged, or showing both eyes the same: one eye, at its true shape.
        assert_eq!(staging(&film, Mode::Flat, true, false), Staging::OneEye);
        assert_eq!(staging(&film, Mode::Flat, false, false), Staging::OneEye);
        let stacked = layout(Projection::Flat, StereoLayout::OverUnder);
        assert_eq!(staging(&stacked, Mode::Stereo, true, false), Staging::Window { eyes: EyeLayout::TopBottom });
    }

    #[test]
    fn a_sphere_is_the_sky_in_the_room_and_looked_about_in_anywhere_else() {
        let dome = layout(Projection::Dome180, StereoLayout::SideBySide);
        assert_eq!(staging(&dome, Mode::Stereo, true, false), Staging::Sky { half: true, eyes: EyeLayout::SideBySide });
        assert_eq!(staging(&dome, Mode::Flat, false, false), Staging::Look { half: true });
        let sphere = layout(Projection::Sphere360, StereoLayout::Mono);
        assert_eq!(staging(&sphere, Mode::Stereo, true, false), Staging::Sky { half: false, eyes: EyeLayout::Mono });
        // The sky is somebody else's: the film is still watched.
        assert_eq!(staging(&sphere, Mode::Stereo, true, true), Staging::Look { half: false });
    }

    #[test]
    fn a_window_is_the_films_shape_and_no_larger_than_it_may_be() {
        assert_eq!(fitted(16.0 / 9.0), (1920, 1080));
        assert_eq!(fitted(2.39), (1920, 804));
        assert_eq!(fitted(4.0 / 3.0), (1440, 1080));
    }

    #[test]
    fn a_clock_has_hours_only_when_there_are_some() {
        assert_eq!(clock(75.4), "1:15");
        assert_eq!(clock(3675.0), "1:01:15");
        assert_eq!(clock(-3.0), "0:00");
    }
}
