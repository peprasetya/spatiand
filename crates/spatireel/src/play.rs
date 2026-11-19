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

use spatiand_app::ui::{theme, AutoHide, Button, Input, Menu, MenuSaid, Painter, Rect, Row, Texture, Icon, CONTROLS_SHOWN};
use spatiand_media::subs::{self, Cue, Words};
use spatiand_app::{Anchor, App, EyeLayout, Gl, Key, Layer, Mode, WindowId};
use spatiand_media::hints;
use spatiand_media::player::Player;
use spatiand_media::screen::{Gaze, Screen};
use spatiand_media::still::Upright;

use crate::source::Play;
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
    /// Nothing to see -- a file of sound alone -- so the window is the controls.
    Sound,
}

impl Staging {
    /// The window is the controls and nothing else.
    fn controls_alone(self) -> bool {
        matches!(self, Staging::Sky { .. } | Staging::Sound)
    }
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
/// How far the pointer may go between a press and its release on the picture for the two to be
/// a tap, rather than the view being turned.
const TAP_SLACK: f32 = 12.0;

/// Whether a press at `from` let go at `to` is a tap.
fn is_tap(from: (f32, f32), to: (f32, f32)) -> bool {
    (to.0 - from.0).hypot(to.1 - from.1) <= TAP_SLACK
}

/// The size a window showing a picture of this shape is: as large as fits in [`WINDOW`].
fn fitted(aspect: f32) -> (u32, u32) {
    let (w, h) = if aspect >= WINDOW.0 / WINDOW.1 { (WINDOW.0, WINDOW.0 / aspect) } else { (WINDOW.1 * aspect, WINDOW.1) };
    // Even, which every scaler and half the world's video paths prefer.
    (((w / 2.0).round() * 2.0) as u32, ((h / 2.0).round() * 2.0) as u32)
}

/// The most of a film's picture worth drawing into a window, per eye: the glasses' own panel,
/// which a window never fills. A film's frame drawn at its own size was a 4K picture -- or two
/// eyes of one side by side -- made and handed over every frame, in every SpatiReel at once;
/// on the Mac, where a window's picture is copied to the room rather than shared, that was
/// most of the machine.
const EYE_MOST: (f32, f32) = (1920.0, 1080.0);

/// The least of a picture worth drawing into, per eye, across or down (whichever comes first):
/// what the controls and the subtitles are drawn at. A small film's own frame (320 x 180) had
/// them drawn at its size, blurred to nothing when the window was made large.
const EYE_LEAST: (f32, f32) = (1280.0, 720.0);

/// `frame`, scaled down so that each eye of it fits [`EYE_MOST`], or up so that it is no
/// smaller than [`EYE_LEAST`].
fn capped(frame: (u32, u32), eyes: EyeLayout) -> (u32, u32) {
    let (w, h) = (frame.0.max(2) as f32, frame.1.max(2) as f32);
    let (eye_w, eye_h) = match eyes {
        EyeLayout::SideBySide => (w / 2.0, h),
        EyeLayout::TopBottom => (w, h / 2.0),
        _ => (w, h),
    };
    let up = (EYE_LEAST.0 / eye_w).min(EYE_LEAST.1 / eye_h).max(1.0);
    let scale = (EYE_MOST.0 / eye_w).min(EYE_MOST.1 / eye_h).min(up);
    let even = |v: f32| (((v * scale) / 2.0).round() * 2.0).max(2.0) as u32;
    (even(w), even(h))
}

/// The shape one eye of a `frame` of pixels `sample_aspect` times as wide as tall is seen at:
/// the frame as it is shown, not as it is stored. Taken as stored, a DVD's 720 x 576 frame meant
/// for 16:9 was shown squeezed at 5:4.
fn eye_aspect(layout: &VideoLayout, frame: (u32, u32), sample_aspect: f64) -> f32 {
    let sample = if sample_aspect.is_finite() && sample_aspect > 0.0 { sample_aspect } else { 1.0 };
    let shown = ((frame.0 as f64 * sample).round() as u32).max(1);
    layout.eye_aspect(shown, frame.1)
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

/// Where the film is and how long it is, for the controls: the length said to be "about" that
/// where it is only judged from the file's size (a file that keeps no times), and left out
/// where nothing is known of it yet (it was "0:00").
fn times(position: f64, duration: f64, guessed: bool) -> String {
    // The length to the nearest second: ten seconds counted up in twenty-fourths come to a
    // hair under ten, and were "0:09".
    let length = clock(duration.round());
    if duration <= 0.0 {
        clock(position)
    } else if guessed {
        format!("{} / about {length}", clock(position))
    } else {
        format!("{} / {length}", clock(position))
    }
}

/// What the wearer did that the caller has to act on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    Nothing,
    /// Back to the library.
    Close,
    /// On to the next episode: Next Episode pressed, or N.
    Next,
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
    Speed(f32),
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
    clock: AutoHide,
    /// Where a press on the picture, with the controls on show, began: let go there, it puts
    /// them away.
    tap: Option<(f32, f32)>,
    /// Whether the controls were in the last picture drawn: paused, nothing else would draw
    /// the one without them.
    drawn_shown: Option<bool>,
    /// The shade the controls are drawn on, over the foot of the film.
    panel: Rect,
    /// Whether the room has ever said the wearer's hands stirred (`Event::Hands`).
    hands_heard: bool,
    /// Whether the room has the keyboard on this window, and whether the pointer is over it:
    /// the controls come for the wearer's hands only when one of the two is so.
    focused: bool,
    inside: bool,
    toggle: Button,
    /// Ten seconds back and on, either side of the toggle.
    rewind: Button,
    skip: Button,
    back: Button,
    options: Button,
    /// The episode after this one, by its title, where there is one: offered at the controls'
    /// right end near the end of this one (`next::near_end`).
    next: Option<String>,
    next_button: Button,
    /// The controls have come up by themselves for the offer, which they do once.
    offer_shown: bool,
    menu: Option<(Menu, Vec<(Row, Option<Choice>)>)>,
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
    words_shown: Vec<Arc<Cue>>,
    /// Held by the cue itself: by its address alone, a cue dropped and a new one made where it
    /// had been showed the old one's picture.
    words_picture: Vec<(Arc<Cue>, Texture)>,
}

/// The `.srt` that goes with a film on this machine: the one with its name, or with its
/// name and a language after it (`Film.en.srt`, `Film.pt-BR.forced.srt`).
fn beside(path: &std::path::Path) -> Vec<Cue> {
    let (Some(folder), Some(stem)) = (path.parent(), path.file_stem().and_then(|s| s.to_str())) else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut found: Vec<std::path::PathBuf> = entries.flatten().map(|entry| entry.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| goes_with(stem, n))).collect();
    // The plainest name first: `Film.srt` before `Film.fr.srt`.
    found.sort_by_key(|p| p.as_os_str().len());
    found.first().and_then(|p| std::fs::read(p).ok()).map(|bytes| subs::srt(&subs::text_of(&bytes))).unwrap_or_default()
}

/// Whether the file `name` is subtitles for a film whose name without its ending is `stem`:
/// `stem.srt`, or `stem.<tag>.srt` where the tag is words (a language, "forced", "sdh"). Taken
/// by what the name began with, "Alien.mkv" took "Aliens.srt", and "Alien" its sequel's.
fn goes_with(stem: &str, name: &str) -> bool {
    let Some(before) = name.len().checked_sub(4).filter(|&at| name.is_char_boundary(at) && name[at..].eq_ignore_ascii_case(".srt")).map(|at| &name[..at]) else { return false };
    if before == stem {
        return true;
    }
    let Some(tag) = before.strip_prefix(stem).and_then(|rest| rest.strip_prefix('.')) else { return false };
    !tag.is_empty() && tag.split('.').all(|word| !word.is_empty() && word.len() <= 12 && word.chars().all(|c| c.is_alphabetic() || c == '-' || c == '_'))
}

impl Playing {
    /// Open what is at `url`, and take it up `resume` seconds in. It may take a long time -- a
    /// long film's index is fetched first -- so it is not done on the thread that answers the
    /// room.
    pub fn open_player(play: &Play, resume: f64) -> Result<Player, String> {
        let mut player = match &play.path {
            Some(path) => Player::open_file(path)?,
            None => Player::open_with(&play.url, &play.options)?,
        };
        if resume > 5.0 {
            player.seek(resume);
        }
        Ok(player)
    }

    /// Play `player`, opened from `url`. `name` is the file's name, for what it says of the
    /// film's layout; `said` is a layout the film's source vouches for, which outranks the name;
    /// `title` is what the wearer calls it.
    pub fn new(gl: &Gl, mut player: Player, play: &Play, name: &str, title: &str, said: Option<VideoLayout>) -> Result<Playing, String> {
        // `SPATIREEL_SPEED=1.5` starts at that pace, for a test that cannot reach the menu.
        if let Some(speed) = std::env::var("SPATIREEL_SPEED").ok().and_then(|s| s.parse().ok()) {
            player.set_speed(speed);
        }
        let info = player.info();
        let named = hints::detect(name, info.hints);
        // A source that says how the eyes are packed is believed for that; where the picture
        // goes -- a screen, a dome -- it never says, and the name still may.
        let layout = said.map_or(named, |said| VideoLayout { projection: named.projection, ..said });
        let swapped = info.eyes_swapped;
        let name = title;
        let beside = play.path.as_deref().map(beside).unwrap_or_default();
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
            clock: AutoHide::new(Instant::now()),
            tap: None,
            drawn_shown: None,
            panel: Rect::default(),
            hands_heard: false,
            focused: false,
            inside: false,
            toggle: Button::default(),
            rewind: Button::default(),
            skip: Button::default(),
            back: Button::default(),
            options: Button::default(),
            next: None,
            next_button: Button::default(),
            offer_shown: false,
            menu: None,
            bar: Rect::default(),
            seeking: false,
            dirty: true,
            beside,
            said,
            words_late: 0.0,
            words_shown: Vec::new(),
            words_picture: Vec::new(),
        })
    }

    pub fn player(&mut self) -> &mut Player {
        &mut self.player
    }

    pub fn sky(&self) -> Option<WindowId> {
        self.sky
    }

    /// The episode after this one, by its title, now that it is known whether there is one.
    pub fn offer(&mut self, next: Option<String>) {
        self.next = next;
        self.dirty = true;
    }

    /// Whether Next Episode is on offer now: there is a next episode, and this one is at its
    /// credits.
    fn offered(&self) -> bool {
        let info = self.player.info();
        self.next.is_some() && crate::next::near_end(self.player.position(), info.duration, info.duration_guessed)
    }

    /// This one is over and `said` is what comes next: it is the name on the controls, which
    /// come up to show it, and nothing more is offered.
    pub fn announce(&mut self, said: &str) {
        self.name = said.to_string();
        self.next = None;
        self.clock.wake(Instant::now());
        self.dirty = true;
    }

    /// What is on show, in words: where the film is, and the next episode while it is offered.
    pub fn said(&self) -> String {
        let info = self.player.info();
        let mut said = format!("{}, at {}.", self.name, times(self.player.position(), info.duration, info.duration_guessed));
        if let (true, Some(next)) = (self.offered(), self.next.as_ref()) {
            said.push_str(&format!(" Next episode is offered: {next}. N or the Next episode button plays it."));
        }
        said
    }

    /// The room would not give the film the sky.
    pub fn sky_was_refused(&mut self) {
        self.sky_refused = true;
        self.dirty = true;
    }

    fn controls_shown(&self) -> bool {
        // Always, when the controls are all the window is (the room fades that window itself);
        // and while a menu is open, or the wearer has just done something -- paused or not:
        // paused, the frame is what is being looked at.
        // `SPATIREEL_CONTROLS=stay` is for a picture of them: a snapshot is taken long after
        // they would have gone.
        self.staging.controls_alone()
            || self.menu.is_some()
            || self.clock.on(Instant::now())
            || std::env::var("SPATIREEL_CONTROLS").as_deref() == Ok("stay")
    }

    /// The room says the wearer's hands did something (`Event::Hands`): the controls come back.
    /// From then on, pointer motion alone no longer brings them, since it sweeps with the head.
    /// The room says the hands (`App::says_hands`): pointer motion never wakes the controls,
    /// from the film's first frame.
    pub fn hands_said(&mut self, said: bool) {
        self.hands_heard |= said;
    }

    pub fn hands(&mut self) {
        self.hands_heard = true;
        // Hands on some other window are not this one's business, and typing into another
        // program counts as hands too, whatever the room thinks has the keyboard: only the
        // pointer over this window is told apart. Keys this window receives wake it by
        // themselves, in `input`.
        if self.inside {
            self.clock.stir(Instant::now());
        }
    }

    /// The room gave the keyboard to this window, or took it away.
    pub fn focus(&mut self, focused: bool) {
        self.focused = focused;
        self.dirty = true;
    }

    /// When this next needs to be drawn whether or not anything happens: the next picture,
    /// or the controls' clock running out.
    pub fn next_wake(&self) -> Duration {
        if !self.player.paused() {
            Duration::from_millis(8)
        } else {
            let most = Duration::from_millis(250);
            self.clock.left(Instant::now()).map_or(most, |left| (left + Duration::from_millis(5)).min(most))
        }
    }

    /// How the picture is turned to be the right way up: as its film's display matrix says, for
    /// a flat film of one picture -- a phone's, taken upright. A film of two eyes, or a sphere,
    /// is taken as it is stored.
    fn upright(&self) -> Upright {
        let flat = self.layout.projection == Projection::Flat && self.layout.stereo == StereoLayout::Mono;
        if flat { self.player.info().upright } else { Upright::default() }
    }

    /// The size of one eye's picture as it should be seen, in the film's own pixels down: the
    /// frame the right way up.
    fn eye_shape(&self) -> (f32, (u32, u32)) {
        let info = self.player.info();
        let frame = (info.width.max(2), info.height.max(2));
        if self.upright().swap {
            // On its side: what is across is down, and its pixels' shape turned with it.
            let turned = (frame.1, frame.0);
            (eye_aspect(&self.layout, turned, 1.0 / info.sample_aspect), turned)
        } else {
            (eye_aspect(&self.layout, frame, info.sample_aspect), frame)
        }
    }

    /// Put the windows the way this film, looked at this way, wants them.
    fn arrange(&mut self, app: &mut App, window: WindowId) {
        // A file with no pictures has only its controls to show.
        let pictured = self.player.info().width > 0 && self.player.info().height > 0;
        let staging = if pictured { staging(&self.layout, app.mode(), app.in_the_room(), self.sky_refused) } else { Staging::Sound };
        let (aspect, frame) = self.eye_shape();
        let (picture, shape) = match staging {
            // The file's own frame, and the window the shape of one eye of it.
            Staging::Window { eyes } => (capped(frame, eyes), fitted(aspect)),
            // One eye, at the size it is in the file down and its true shape across.
            Staging::OneEye => {
                let eye_h = if self.layout.stereo == StereoLayout::OverUnder { frame.1 / 2 } else { frame.1 }.max(2);
                (capped(((eye_h as f32 * aspect / 2.0).round() as u32 * 2, eye_h), EyeLayout::Mono), fitted(aspect))
            }
            // With the film all round, the window is the controls -- and grows upwards from
            // its foot, which stays where it was, to hold the menu while that is open.
            Staging::Sky { .. } | Staging::Sound if self.menu.is_some() => (CONTROLS_WITH_MENU, CONTROLS_WITH_MENU),
            Staging::Sky { .. } | Staging::Sound => (CONTROLS_ALONE, CONTROLS_ALONE),
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
    pub fn close(mut self, app: &mut App, window: WindowId, painter: &mut Option<Painter>) {
        if let Some(sky) = self.sky.take() {
            app.close(sky);
        }
        if let Ok(gl) = app.gl() {
            if let Some(p) = painter.as_mut() {
                for (_, old) in self.words_picture.drain(..) {
                    p.forget(gl, old);
                }
            }
            self.screen.delete(gl);
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
    /// Over the film, so in the dark colours whatever the platform is set to.
    pub fn draw(&mut self, app: &mut App, window: WindowId, painter: &mut Option<Painter>) -> Result<bool, String> {
        theme::always_dark(|| self.draw_over_film(app, window, painter))
    }

    fn draw_over_film(&mut self, app: &mut App, window: WindowId, painter: &mut Option<Painter>) -> Result<bool, String> {
        self.arrange(app, window);
        let (picture, new) = self.player.picture();
        // Next Episode offered: the controls come up by themselves to show it, once, and go
        // as they always do.
        let offered = self.offered();
        if offered && !self.offer_shown {
            self.offer_shown = true;
            self.clock.wake(Instant::now());
        }
        let shown = self.controls_shown();
        let cues = self.cues();
        let words_changed = cues.len() != self.words_shown.len() || cues.iter().zip(&self.words_shown).any(|(now, before)| !Arc::ptr_eq(now, before));
        // The controls coming or going is a reason to draw too: paused, there is no next
        // picture that would.
        let shown_changed = self.drawn_shown != Some(shown);
        if !new && !self.dirty && !words_changed && !shown_changed {
            return Ok(false);
        }
        self.drawn_shown = Some(shown);
        self.words_shown = cues.clone();
        self.dirty = false;
        let staging = self.staging;
        let one_eye = self.one_eye();
        let (position, duration, paused) = (self.player.position(), self.player.info().duration, self.player.paused());
        let name = self.name.clone();
        let gaze = self.gaze;
        let logical = app_shape(self.arranged);
        let upright = self.upright();
        self.screen.set_upright(upright);

        // The film's own surface, when it is the sky: the frame, whole, and the subtitle in
        // front of the wearer. The window is the controls, which fade when left alone: the
        // words are not drawn there (they were drawn nowhere).
        if let (Some(sky), Staging::Sky { half, eyes }) = (self.sky, staging) {
            if new || !self.screen.has_picture() || words_changed {
                let this = &mut *self;
                app.draw(sky, |gl, size| {
                    if let Some(picture) = picture.as_ref() {
                        this.screen.load(gl, picture);
                    }
                    clear(gl);
                    this.screen.draw(gl, size, [0.0, 0.0, size.0 as f32, size.1 as f32], [0.0, 0.0, 1.0, 1.0]);
                    if cues.is_empty() {
                        return;
                    }
                    let p = painter.get_or_insert_with(|| Painter::new(gl).expect("the painter"));
                    let (w, h) = (size.0 as f32, size.1 as f32);
                    let parts: Vec<Rect> = match eyes {
                        EyeLayout::SideBySide => vec![Rect::new(0.0, 0.0, w / 2.0, h), Rect::new(w / 2.0, 0.0, w / 2.0, h)],
                        EyeLayout::TopBottom => vec![Rect::new(0.0, 0.0, w, h / 2.0), Rect::new(0.0, h / 2.0, w, h / 2.0)],
                        _ => vec![Rect::new(0.0, 0.0, w, h)],
                    };
                    for (eye, part) in parts.iter().enumerate() {
                        p.begin_over(gl, size, sky_screen(*part, half), WINDOW);
                        let nearer = if parts.len() == 2 { WINDOW.0 * 0.0035 } else { 0.0 };
                        let shift = if (eye == 0) != this.swapped { nearer } else { -nearer };
                        this.words(p, gl, &cues, shift, 0.0);
                        p.end(gl);
                    }
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
            // A picture subtitle no longer on show lets its texture go.
            if let Some(p) = painter.as_mut() {
                let (kept, gone): (Vec<_>, Vec<_>) = this.words_picture.drain(..).partition(|(held, _)| cues.iter().any(|cue| Arc::ptr_eq(cue, held)));
                this.words_picture = kept;
                for (_, old) in gone {
                    p.forget(gl, old);
                }
            }
            match staging {
                Staging::Window { .. } => this.screen.draw(gl, size, whole, [0.0, 0.0, 1.0, 1.0]),
                Staging::OneEye => this.screen.draw(gl, size, whole, one_eye),
                Staging::Look { half } => this.screen.look(gl, size, whole, one_eye, half, gaze),
                Staging::Sky { .. } | Staging::Sound => {}
            }
            let alone = staging.controls_alone();
            if !shown && !alone && cues.is_empty() {
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
                if !cues.is_empty() && !alone {
                    // In a film with two eyes the words stand a little in front of the
                    // screen: each eye's are moved towards the other's side, so that the
                    // eyes cross slightly to read them, as they do for anything nearer.
                    let nearer = if parts.len() == 2 { logical.0 * 0.0035 } else { 0.0 };
                    let first_is_left = !this.swapped;
                    let shift = if (eye == 0) == first_is_left { nearer } else { -nearer };
                    this.words(p, gl, &cues, shift, if shown { 230.0 } else { 0.0 });
                }
                if shown || alone {
                    this.controls(p, gl, &name, position, duration, paused, alone, offered);
                }
                p.end(gl);
            }
        })?;
        Ok(true)
    }

    /// The subtitles to show now, wherever subtitles are coming from: every one on show, in the
    /// order they began.
    fn cues(&self) -> Vec<Arc<Cue>> {
        match self.said {
            Said::Nothing => Vec::new(),
            Said::Beside => subs::at(&self.beside, self.player.position() - self.words_late).cloned().map(Arc::new).map(|cue| {
                // The same cue is the same thing each time it is asked for, or every picture
                // would look like a new subtitle.
                match self.words_shown.first() {
                    Some(before) if **before == *cue => before.clone(),
                    _ => cue,
                }
            }).into_iter().collect(),
            Said::Track(_) => self.player.subtitles(),
        }
    }

    /// Draw the subtitles on show: their words at the foot of the picture, `lifted` above it when
    /// the controls are there, and `shift` to the right; their pictures where they were placed.
    fn words(&mut self, p: &mut Painter, gl: &Gl, cues: &[Arc<Cue>], shift: f32, lifted: f32) {
        let all = p.bounds();
        // The lines of every one in words, the earliest begun highest.
        let lines: Vec<&str> = cues.iter().filter_map(|cue| if let Words::Text(text) = &cue.words { Some(text.lines()) } else { None }).flatten().collect();
        if !lines.is_empty() {
            // A size that is the same part of the picture whatever the window: a little over a
            // degree in the glasses, which is what a line to be read at a glance is.
            let em = all.h * 0.046;
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
        for cue in cues {
            let Words::Picture { rgba, width, height, place } = &cue.words else { continue };
            let held = self.words_picture.iter().find(|(held, _)| Arc::ptr_eq(held, cue)).map(|(_, texture)| *texture);
            let texture = match held {
                Some(texture) => Some(texture),
                None => p.texture(gl, rgba, *width, *height).ok().inspect(|texture| self.words_picture.push((cue.clone(), *texture))),
            };
            if let Some(texture) = texture {
                let rect = Rect::new(all.x + place[0] * all.w + shift, all.y + place[1] * all.h - lifted.min(all.h * place[1]), place[2] * all.w, place[3] * all.h);
                p.image(gl, texture, rect, [0.0, 0.0, 1.0, 1.0], 0.0, [1.0, 1.0, 1.0, 1.0], false);
            }
        }
    }

    /// The controls: over the foot of the film, or all the window is.
    #[allow(clippy::too_many_arguments)]
    fn controls(&mut self, p: &mut Painter, gl: &Gl, name: &str, position: f64, duration: f64, paused: bool, alone: bool, offered: bool) {
        let all = p.bounds();
        if alone {
            p.fill(gl, all, 0.0, theme::ground());
        }
        let height = 210.0f32.min(all.h);
        let mut panel = Rect::new(all.x, all.bottom() - height, all.w, height);
        self.panel = panel;
        if !alone {
            // A shade behind the controls, so they can be read over a bright scene.
            p.fill(gl, panel, 0.0, { let mut g = theme::ground(); g[3] = 0.78; g });
        }
        panel = panel.inset(theme::PAD * 0.6);
        let mut top = panel.take_top(theme::ROW_EM * 1.5);
        let time = times(position, duration, self.player.info().duration_guessed);
        let taken = p.label(gl, &time, top, theme::ROW_EM * 0.8, theme::ink_detail(), 1.0);
        top.w -= taken + 24.0;
        p.label(gl, name, top, theme::ROW_EM * 0.9, theme::ink(), 0.0);

        // How far through, as a bar: pressed anywhere along it, the film goes there.
        let mut line = panel.take_top(34.0);
        line = line.inset(0.0);
        self.bar = line;
        let track = Rect::new(line.x, line.y + line.h * 0.5 - 4.0, line.w, 8.0);
        p.fill(gl, track, 4.0, theme::scroll_track());
        let through = if duration > 0.0 { (position / duration).clamp(0.0, 1.0) as f32 } else { 0.0 };
        p.fill(gl, Rect { w: track.w * through, ..track }, 4.0, theme::mark());
        p.fill(gl, Rect::new(track.x + track.w * through - 9.0, track.y - 5.0, 18.0, 18.0), 9.0, theme::ink());

        panel.take_top(10.0);
        let mut row = panel.take_top(Button::HEIGHT.min(panel.h));
        // Round, and lit: the one control a film is mostly about.
        self.rewind.draw_icon(p, gl, row.take_left(Button::HEIGHT), Icon::Back10, "");
        row.take_left(10.0);
        let toggle = row.take_left(Button::HEIGHT);
        self.toggle.draw_icon(p, gl, toggle, if paused { Icon::Play } else { Icon::Pause }, "");
        row.take_left(10.0);
        self.skip.draw_icon(p, gl, row.take_left(Button::HEIGHT), Icon::Forward10, "");
        row.take_left(22.0);
        let width = Button::icon_width(p, gl, "Picture and sound");
        self.options.draw_icon(p, gl, row.take_left(width), Icon::Options, "Picture and sound");
        // At the right end: Next Episode while it is offered, and the library.
        let mut right = row.right();
        if offered {
            let width = Button::icon_width(p, gl, "Next episode");
            self.next_button.draw_icon(p, gl, Rect { x: right - width, w: width, ..row }, Icon::Forward, "Next episode");
            right -= width + 10.0;
        }
        let width = Button::icon_width(p, gl, "Library");
        self.back.draw_icon(p, gl, Rect { x: right - width, w: width, ..row }, Icon::Back, "Library");

        if let Some((menu, rows)) = self.menu.as_mut() {
            let rows: Vec<Row> = rows.iter().map(|(row, _)| row.clone()).collect();
            // Over the controls, at the left, its foot just above them: the room above the
            // controls is what it is kept in, and it opens at that room's foot.
            let above = Rect { h: (all.bottom() - height + 6.0 - all.y).max(0.0), ..all };
            menu.at = (all.x + theme::PAD * 0.6, above.bottom());
            menu.draw(p, gl, &rows, above);
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
        rows.push((Row::heading("How fast"), None));
        for speed in [0.75f32, 1.0, 1.25, 1.5, 2.0] {
            let label = if speed == 1.0 { "As it was made".to_string() } else { format!("{speed}\u{d7}") };
            rows.push((Row::with(label, mark((self.player.speed() - speed).abs() < 0.01)), Some(Choice::Speed(speed))));
        }
        rows.push((Row::heading("Picture against sound"), None));
        rows.push((Row::with("Picture earlier", offset(self.player.lip())), Some(Choice::LipBy(-0.05))));
        rows.push((Row::with("Picture later", offset(self.player.lip())), Some(Choice::LipBy(0.05))));
        let mut menu = Menu::new((0.0, 0.0));
        menu.list.cursor = 1;
        self.menu = Some((menu, rows));
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
            Choice::Speed(speed) => self.player.set_speed(speed),
            Choice::WordsBy(seconds) => self.words_late = ((self.words_late + seconds) * 100.0).round() / 100.0,
            Choice::LipBy(seconds) => {
                let lip = ((self.player.lip() + seconds) * 100.0).round() / 100.0;
                self.player.set_lip(lip);
            }
        }
        // Open again, on the same row, with the marks where they now are.
        let cursor = self.menu.as_ref().map_or(1, |(menu, _)| menu.list.cursor);
        self.open_menu();
        if let Some((menu, rows)) = self.menu.as_mut() {
            menu.list.cursor = cursor.min(rows.len() - 1);
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
        let now = Instant::now();
        let was_shown = self.controls_shown();
        let menu_was_open = self.menu.is_some();
        match input {
            Input::Pointer { .. } => self.inside = true,
            Input::PointerLeft => self.inside = false,
            _ => {}
        }
        match input {
            // A pointer leaving is no reason to show anything.
            Input::PointerLeft => {}
            // Motion is the head's as often as the hand's: only a room too old to say which
            // (no `Event::Hands` ever) has it wake the controls.
            Input::Pointer { .. } => {
                if !self.hands_heard {
                    self.clock.stir(now);
                }
            }
            _ => self.clock.wake(now),
        }
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
        if let Some((menu, rows)) = self.menu.as_mut() {
            match menu.input(input, self.pointer) {
                Some(MenuSaid::Chose(chosen)) => {
                    if let Some(choice) = rows[chosen].1 {
                        self.choose(choice);
                    }
                    return Outcome::Nothing;
                }
                // Escape, or a press anywhere else, puts the menu away.
                Some(MenuSaid::Closed) => self.menu = None,
                None => {}
            }
            if self.menu.is_some() || matches!(input, Input::Key { .. }) {
                return Outcome::Nothing;
            }
        }
        // Buttons that are not on show are not pressed: the press that brings the controls
        // back is only that. Where the pointer is they are always told, or one passed over while
        // they were hidden came back lit.
        if was_shown || matches!(input, Input::Pointer { .. } | Input::PointerLeft) {
            if self.toggle.input(input) {
                let paused = self.player.paused();
                self.player.set_paused(!paused);
            }
            if self.rewind.input(input) {
                self.player.seek((self.player.position() - 10.0).max(0.0));
            }
            if self.skip.input(input) {
                self.player.seek(self.player.position() + 10.0);
            }
            if self.options.input(input) {
                self.open_menu();
            }
            if self.back.input(input) {
                return Outcome::Close;
            }
            // Only while it is on show: where it was last drawn is not a button now.
            if self.next_button.input(input) && self.offered() {
                return Outcome::Next;
            }
        }
        match input {
            Input::Press { pressed: true, .. } => {
                // A press on the picture itself, with the controls on show (and not only just
                // brought back by this same hand) and no menu being put away by it, may be a tap
                // that puts them away; it never pauses or plays.
                let alone = self.staging.controls_alone();
                let on_picture = !self.panel.contains(self.pointer.0, self.pointer.1);
                let meant = was_shown && self.clock.settled(now) && !alone && !menu_was_open && on_picture;
                self.tap = meant.then_some(self.pointer);
                if was_shown && self.bar.inset(-12.0).contains(self.pointer.0, self.pointer.1) {
                    self.seeking = true;
                    self.seek_to_pointer();
                } else if self.pointer.1 < self.bar.y - 40.0 || !was_shown {
                    self.turning = Some(self.pointer);
                }
            }
            Input::Press { pressed: false, .. } => {
                // Let go where it was pressed: a tap, not the view being turned.
                if let Some(from) = self.tap.take() {
                    if is_tap(from, self.pointer) && self.menu.is_none() {
                        self.clock.put_away(now);
                    }
                }
                self.seeking = false;
                self.turning = None;
            }
            Input::PointerLeft => {
                self.tap = None;
                self.seeking = false;
                self.turning = None;
            }
            Input::Key { key, letter, modifiers, .. } => {
                let now = self.player.position();
                // Held with Control or Command, a key is a shortcut of something else's:
                // Command-M is the Mac's own (minimise), not the menu.
                let plain = !modifiers.control && !modifiers.logo;
                // M for the menu, the key where M is whatever the layout writes on it.
                if *letter == Some('m') && plain {
                    self.open_menu();
                    return Outcome::Nothing;
                }
                // N for Next Episode, while it is offered, by the key's place as M is.
                if *letter == Some('n') && plain && self.offered() {
                    return Outcome::Next;
                }
                match key {
                    Key::Space | Key::Enter if !plain => {}
                    Key::Space | Key::Enter => {
                        let paused = self.player.paused();
                        self.player.set_paused(!paused);
                    }
                    Key::Left => self.player.seek(now - 10.0),
                    Key::Right => self.player.seek(now + 10.0),
                    Key::Down => self.player.seek(now - 60.0),
                    Key::Up => self.player.seek(now + 60.0),
                    Key::Escape | Key::Backspace => return Outcome::Close,
                    _ => {}
                }
            }
            _ => {}
        }
        Outcome::Nothing
    }
}

/// Where in one eye's part of a sphere's frame (`part`, the whole of the sphere, or the front
/// half of it) a screen of 64 by 36 degrees straight ahead is: what the subtitles of a film in
/// the sky are drawn on, at its foot, a little below the horizon.
fn sky_screen(part: Rect, half: bool) -> Rect {
    let degree = part.w / if half { 180.0 } else { 360.0 };
    let (w, h) = (64.0 * degree, 36.0 * degree);
    Rect::new(part.x + (part.w - w) / 2.0, part.y + (part.h - h) / 2.0, w, h)
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
    fn a_large_film_is_drawn_no_larger_than_the_glasses_show_it() {
        assert_eq!(capped((3840, 2160), EyeLayout::Mono), (1920, 1080));
        // Both eyes side by side: each eye at most the panel, so twice as wide in all.
        assert_eq!(capped((7680, 2160), EyeLayout::SideBySide), (3840, 1080));
        assert_eq!(capped((1920, 2160), EyeLayout::TopBottom), (1920, 2160));
        // Smaller than that, it is left alone.
        assert_eq!(capped((1280, 720), EyeLayout::Mono), (1280, 720));
        // Smaller still, it is drawn larger, for the controls' and the subtitles' sake.
        assert_eq!(capped((320, 180), EyeLayout::Mono), (1280, 720));
        assert_eq!(capped((180, 320), EyeLayout::Mono), (406, 720));
        assert_eq!(capped((640, 180), EyeLayout::SideBySide), (2560, 720));
    }

    #[test]
    fn a_press_let_go_where_it_was_is_a_tap_and_a_drag_is_not() {
        assert!(is_tap((100.0, 100.0), (100.0, 100.0)));
        assert!(is_tap((100.0, 100.0), (108.0, 106.0)));
        assert!(!is_tap((100.0, 100.0), (140.0, 100.0)));
    }

    #[test]
    fn a_frame_of_pixels_that_are_not_square_is_seen_at_the_shape_it_is_shown() {
        let flat = layout(Projection::Flat, StereoLayout::Mono);
        assert!((eye_aspect(&flat, (720, 576), 64.0 / 45.0) - 16.0 / 9.0).abs() < 0.01);
        assert!((eye_aspect(&flat, (720, 576), 16.0 / 15.0) - 4.0 / 3.0).abs() < 0.01);
        // Square pixels, or none said.
        assert!((eye_aspect(&flat, (1920, 1080), 1.0) - 16.0 / 9.0).abs() < 0.01);
        assert!((eye_aspect(&flat, (1920, 1080), 0.0) - 16.0 / 9.0).abs() < 0.01);
        // A half-width pair squeezed into a DVD's frame: each eye the whole frame's shape.
        let pair = layout(Projection::Flat, StereoLayout::SideBySide);
        assert!((eye_aspect(&pair, (720, 576), 64.0 / 45.0) - 16.0 / 9.0).abs() < 0.01);
    }

    #[test]
    fn a_film_in_the_sky_has_its_words_straight_ahead() {
        // A whole sphere 3840 x 1920: 64 degrees across is 3840 / 360 * 64, in the middle.
        let screen = sky_screen(Rect::new(0.0, 0.0, 3840.0, 1920.0), false);
        assert!((screen.w - 682.67).abs() < 0.1 && (screen.h - 384.0).abs() < 0.1);
        assert!((screen.x + screen.w / 2.0 - 1920.0).abs() < 0.1 && (screen.y + screen.h / 2.0 - 960.0).abs() < 0.1);
        // The right eye of a VR180 pair side by side: the middle of its own half.
        let right = sky_screen(Rect::new(2880.0, 0.0, 2880.0, 2880.0), true);
        assert!((right.x + right.w / 2.0 - 4320.0).abs() < 0.1 && (right.w - 1024.0).abs() < 0.1);
    }

    #[test]
    fn subtitles_go_with_the_film_of_their_name_and_no_other() {
        assert!(goes_with("Alien", "Alien.srt"));
        assert!(goes_with("Alien", "Alien.SRT"));
        assert!(goes_with("Alien", "Alien.en.srt"));
        assert!(goes_with("Alien", "Alien.pt-BR.forced.srt"));
        assert!(!goes_with("Alien", "Aliens.srt"));
        assert!(!goes_with("Alien", "Alien 3.srt"));
        assert!(!goes_with("Alien", "Alien.2.srt"));
        assert!(!goes_with("Alien", "Alien.en.sub"));
        assert!(!goes_with("Alien", "Alien..srt"));
    }

    #[test]
    fn a_clock_has_hours_only_when_there_are_some() {
        assert_eq!(clock(75.4), "1:15");
        assert_eq!(clock(3675.0), "1:01:15");
        assert_eq!(clock(-3.0), "0:00");
    }

    #[test]
    fn a_length_only_judged_is_said_to_be_about_that_and_one_not_known_is_not_said() {
        assert_eq!(times(75.4, 600.0, false), "1:15 / 10:00");
        assert_eq!(times(75.4, 598.2, true), "1:15 / about 9:58");
        assert_eq!(times(6.0, 9.999_999, false), "0:06 / 0:10");
        assert_eq!(times(3.0, 0.0, false), "0:03");
        assert_eq!(times(3.0, 0.0, true), "0:03");
    }
}
