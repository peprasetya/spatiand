//! The room: where the windows are, where the head is, and what the wearer is pointing at.
//!
//! This is the Mac's glasses path with the drawing taken out. The Swift side decodes each
//! window's pictures, owns the glasses' display and draws; everything that is *arithmetic* is
//! here, where it can be tested without a headset, a display or a GPU:
//!
//! * the head, from the glasses' sensors, by the same tracker the Deck uses;
//! * the windows, each on a cylinder round the wearer, placed and moved the way the Deck's are;
//! * the pointer, a cursor that lives in the view and is aimed at the room by the head;
//! * what to draw: both eyes' matrices, and every window as strips of triangles.
//!
//! Frame convention throughout, as everywhere else in Spatiand: **+X forward, +Y left, +Z up**.

use glam::{DQuat, DVec3, Mat4};
use std::collections::HashMap;

use spatiand_audio::stage::{place, Layout, Stage, NOMINAL_HALF_STAGE};
use spatiand_audio::{Binaural, Directness, Panner, Spatialise};
use spatiand_hmd::ImuSample;
use spatiand_render::camera::{eyes_for, StereoConfig};
use spatiand_render::pip::{self, Corner, Size};
use spatiand_render::ray::{intersect_plane, intersect_quad, Bend, Quad, Ray};
use spatiand_track::{AxisMap, HeadTracker, TrackerConfig, DEFAULT_PREDICTION_MAX_DEGREES, DEFAULT_PREDICTION_SECONDS};

/// A window's id on the wire, or the cursor.
pub const CURSOR: u32 = 0xFFFF;
/// Ids from here up are Spatiand's own panels, not an application's windows.
pub const PANEL_FIRST: u32 = 0xFFF0;
/// Added to a window's id for its title bar, in what is drawn.
pub const TITLE_FLAG: u32 = 0x10000;
/// A title bar's picture, in pixels: the same shape whatever the window's size, so the picture needs
/// drawing only once for each state.
pub const BAR_PX: (u32, u32) = (1024, 46);
/// How far above its window a title bar floats, metres.
const BAR_GAP: f64 = 0.006;

/// How far from the wearer a window is put, and how wide, until it is moved. The Deck's own.
const DEFAULT_RADIUS: f64 = 2.2;
const DEFAULT_WIDTH: f64 = 1.1;
/// How far apart two windows are kept when one is placed beside another, radians.
const GAP: f64 = 0.035;
/// How far the cursor may be from the middle of the view, degrees: very nearly the whole field.
const CURSOR_HALF_FOV: (f64, f64) = (19.0, 11.0);
/// How far a mouse's travel turns the cursor, degrees per point.
pub const DEGREES_PER_POINT: f64 = 0.045;
/// How far a window may be taken above or below the horizon, radians.
const PITCH_LIMIT: f64 = 89.0 * std::f64::consts::PI / 180.0;
/// The most a window may grow to, and the least it may shrink to, metres across.
const WIDTH_RANGE: (f64, f64) = (0.3, 4.0);

/// How far round the wearer one strip of a bent window may turn, radians.
const STRIP_STEP: f64 = 0.026;
const MAX_STRIPS: usize = 64;
const STRIP_OVERLAP_M: f64 = 0.00005;

/// Where a window sits, in the viewer-centred frame. The Deck's `Placement`, less what needs a
/// compositor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Round the wearer, radians; 0 is straight ahead, positive is to the left.
    pub yaw: f64,
    /// Above the horizon, radians.
    pub pitch: f64,
    pub radius: f64,
    /// Across, metres. Height follows from the picture's own shape.
    pub width: f64,
    /// For a window pinned to the glass: the way it faces, which is the head's.
    pub facing: Option<DQuat>,
    pub pinned: bool,
}

impl Default for Placement {
    fn default() -> Self {
        Placement { yaw: 0.0, pitch: 0.0, radius: DEFAULT_RADIUS, width: DEFAULT_WIDTH, facing: None, pinned: false }
    }
}

impl Placement {
    pub fn position(&self) -> DVec3 {
        let horizontal = self.radius * self.pitch.cos();
        DVec3::new(horizontal * self.yaw.cos(), horizontal * self.yaw.sin(), self.radius * self.pitch.sin())
    }

    pub fn orientation(&self) -> DQuat {
        if let Some(facing) = self.facing {
            return facing;
        }
        DQuat::from_axis_angle(DVec3::Z, self.yaw) * DQuat::from_axis_angle(DVec3::Y, -self.pitch)
    }

    pub fn bend(&self) -> Option<Bend> {
        (!self.pinned).then_some(Bend { radius: self.radius, offset: 0.0 })
    }
}

/// A point across a bent window put on the cylinder it is bent round; see the Deck's window.rs.
fn on_cylinder(radius: f64, y: f64, z: f64) -> (DVec3, f64) {
    let angle = y / radius.max(1e-6);
    (DVec3::new(radius * (angle.cos() - 1.0), radius * angle.sin(), z), angle)
}

struct Strip {
    y: f64,
    chord: f64,
    from: f64,
    to: f64,
}

fn strips(radius: f64, width: f64) -> Vec<Strip> {
    let radius = radius.max(1e-6);
    let span = width / radius;
    let count = ((span / STRIP_STEP).ceil() as usize).clamp(1, MAX_STRIPS);
    let step = span / count as f64;
    let overlap = if count > 1 { STRIP_OVERLAP_M } else { 0.0 };
    let share = overlap * 0.5 / width.max(1e-6);
    (0..count)
        .map(|i| {
            let from = i as f64 / count as f64;
            let to = (i + 1) as f64 / count as f64;
            Strip {
                y: width * (0.5 - (from + to) * 0.5),
                chord: 2.0 * radius * (step * 0.5).tan() + overlap,
                from: (from - share).max(0.0),
                to: (to + share).min(1.0),
            }
        })
        .collect()
}

/// A window's strips, as triangles, appended to `verts`: a flat piece for each, standing on the
/// window's own cylinder, `rise` metres up the window's own vertical from its middle.
fn push_strips(verts: &mut Vec<f32>, place: &Placement, height: f64, rise: f64) {
    let origin = place.position();
    let turn = place.orientation();
    let pieces = if place.pinned { vec![Strip { y: 0.0, chord: place.width, from: 0.0, to: 1.0 }] } else { strips(place.radius, place.width) };
    for s in pieces {
        // The flat piece's middle on the window's own cylinder, and which way it runs.
        let (centre, angle) = if place.pinned { (DVec3::new(0.0, s.y, 0.0), 0.0) } else { on_cylinder(place.radius, s.y, 0.0) };
        let centre = centre + DVec3::Z * rise;
        let along = DVec3::new(-angle.sin(), angle.cos(), 0.0) * (s.chord * 0.5);
        let up = DVec3::Z * (height * 0.5);
        // Left, right, top, bottom.
        let tl = origin + turn * (centre + along + up);
        let tr = origin + turn * (centre - along + up);
        let bl = origin + turn * (centre + along - up);
        let br = origin + turn * (centre - along - up);
        let (u0, u1) = (s.from as f32, s.to as f32);
        let corner = |p: DVec3, u: f32, v: f32| [p.x as f32, p.y as f32, p.z as f32, u, v];
        for v in [
            corner(tl, u0, 0.0), corner(bl, u0, 1.0), corner(tr, u1, 0.0),
            corner(tr, u1, 0.0), corner(bl, u0, 1.0), corner(br, u1, 1.0),
        ] {
            verts.extend_from_slice(&v);
        }
    }
}

#[derive(Debug, Clone)]
pub struct Win {
    pub id: u32,
    /// The picture's size, in the host's pixels.
    pub size: (u32, u32),
    pub place: Placement,
    /// Whether it has a picture yet; a window that has none is not drawn.
    pub shown: bool,
}

impl Win {
    fn aspect(&self) -> f64 {
        self.size.0.max(1) as f64 / self.size.1.max(1) as f64
    }

    fn height(&self, place: &Placement) -> f64 {
        place.width / self.aspect()
    }

    /// The title bar above this window: its quad, how high above the window's middle it is, and how tall.
    fn bar(&self, place: &Placement) -> Option<(Quad, f64, f64)> {
        if place.pinned || Room::is_panel(self.id) {
            return None;
        }
        let height = place.width * BAR_PX.1 as f64 / BAR_PX.0 as f64;
        let rise = self.height(place) * 0.5 + BAR_GAP + height * 0.5;
        let quad = Quad {
            centre: place.position() + place.orientation() * DVec3::new(0.0, 0.0, rise),
            orientation: place.orientation(),
            width: place.width,
            height,
            bend: place.bend(),
        };
        Some((quad, rise, height))
    }

    fn quad(&self, place: &Placement) -> Quad {
        Quad {
            centre: place.position(),
            orientation: place.orientation(),
            width: place.width,
            height: self.height(place),
            bend: place.bend(),
        }
    }
}

/// What the wearer is pointing at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aim {
    pub window: Option<u32>,
    /// Where in that window, in the host's pixels, clamped to it.
    pub x: f64,
    pub y: f64,
    /// Where in the room: the point the cursor is drawn at.
    pub point: DVec3,
    /// Set when what is aimed at is the window's title bar, whose pixels are [`BAR_PX`].
    pub title: bool,
}

/// Something being dragged about the room with the mouse.
#[derive(Debug, Clone, Copy)]
struct Grab {
    id: u32,
    /// Where the window's middle was, in angles from where the pointer was, when it was taken.
    yaw: f64,
    pitch: f64,
}

/// One application's sound, being placed.
struct Voice {
    layout: Layout,
    binaural: Binaural,
    /// What was left of the last chunk after the last whole frame.
    carry: Vec<u8>,
}

const AUDIO_RATE: u32 = 48_000;

/// A measured head when this Mac has the library and a dataset (the app carries both), the
/// parametric one when it does not: a sound is placed either way, but only a measured head puts one
/// convincingly behind you. Said once, in the log, which it is.
fn make_head() -> Box<dyn Spatialise> {
    static SAID: std::sync::Once = std::sync::Once::new();
    match spatiand_audio::hrtf::Hrtf::system(AUDIO_RATE) {
        Ok(measured) => {
            SAID.call_once(|| log::info!("spatial audio: a measured head"));
            Box::new(measured)
        }
        Err(e) => {
            SAID.call_once(|| log::info!("spatial audio: no measured head ({e}); using the parametric one"));
            Box::new(Panner::new(AUDIO_RATE))
        }
    }
}

pub struct Room {
    tracker: HeadTracker,
    /// Which application each window belongs to, so its sound can be put where the window is.
    app_of: HashMap<u32, String>,
    voices: HashMap<String, Voice>,
    stereo: StereoConfig,
    /// Back to front: the last is the one drawn over the rest.
    windows: Vec<Win>,
    focus: Option<u32>,
    /// Where the cursor is in the view, radians: `(left, up)`.
    cursor: (f64, f64),
    grab: Option<Grab>,
    corner: Corner,
    size: Size,
    /// A head to use before the glasses have said anything, for the preview and tests.
    fixed_head: Option<DQuat>,
    /// The pointer's picture: its size and where its hot spot is, in the picture's pixels.
    cursor_shape: (f64, f64, f64, f64),
}

impl Default for Room {
    fn default() -> Self {
        Room::new()
    }
}

impl Room {
    pub fn new() -> Room {
        let axes = spatiand_track::config::load_axes().unwrap_or(AxisMap::XREAL_AIR);
        Room {
            tracker: HeadTracker::new(axes, TrackerConfig::default()),
            app_of: HashMap::new(),
            voices: HashMap::new(),
            stereo: StereoConfig::default(),
            windows: Vec::new(),
            focus: None,
            cursor: (0.0, 0.0),
            grab: None,
            corner: Corner::default(),
            size: Size::default(),
            fixed_head: None,
            cursor_shape: (0.0, 0.0, 0.0, 0.0),
        }
    }

    /// What the pointer looks like: the picture's width and height and its hot spot, in pixels. All
    /// zero is the plain arrow, whose tip is its top left corner.
    pub fn set_cursor_shape(&mut self, hot_x: f64, hot_y: f64, width: f64, height: f64) {
        self.cursor_shape = (hot_x, hot_y, width, height);
    }

    /// Hold the head where it is, whatever the sensors say. For the preview.
    pub fn set_fixed_head(&mut self, head: Option<DQuat>) {
        self.fixed_head = head;
    }

    // MARK: the head

    pub fn imu(&mut self, sample: &ImuSample) {
        self.tracker.integrate(sample);
    }

    pub fn recentre(&mut self) {
        self.tracker.recenter();
    }

    pub fn has_head(&self) -> bool {
        self.fixed_head.is_some() || self.tracker.has_samples()
    }

    /// The head's heading, pitch and roll in degrees, for a log line.
    pub fn euler_degrees(&self) -> (f64, f64, f64) {
        let e = self.tracker.euler_degrees();
        (e.yaw, e.pitch, e.roll)
    }

    /// Where the head will be when the photons arrive: one frame ahead.
    pub fn head(&self) -> DQuat {
        self.fixed_head.unwrap_or_else(|| {
            self.tracker
                .predicted_orientation(DEFAULT_PREDICTION_SECONDS, DEFAULT_PREDICTION_MAX_DEGREES)
        })
    }

    fn eye_centre(&self, head: DQuat) -> DVec3 {
        head * DVec3::X * self.stereo.neck_forward_m + head * DVec3::Z * self.stereo.neck_up_m
    }

    // MARK: the windows

    pub fn windows(&self) -> &[Win] {
        &self.windows
    }

    pub fn focus(&self) -> Option<u32> {
        self.focus
    }

    pub fn set_focus(&mut self, id: Option<u32>) {
        self.focus = id.filter(|i| !Self::is_panel(*i) && self.windows.iter().any(|w| w.id == *i));
        if let Some(id) = self.focus {
            self.raise(id);
        }
    }

    fn raise(&mut self, id: u32) {
        if let Some(at) = self.windows.iter().position(|w| w.id == id) {
            let w = self.windows.remove(at);
            self.windows.push(w);
        }
    }

    /// A panel is Spatiand's own, not an application's.
    fn is_panel(id: u32) -> bool {
        id >= PANEL_FIRST && id != CURSOR
    }

    /// A window's picture is this big now. A window not yet here is put where the wearer is
    /// looking, beside whatever is there.
    pub fn set_window(&mut self, id: u32, size: (u32, u32)) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.size = size;
            return;
        }
        let heading = {
            let forward = self.head() * DVec3::X;
            forward.y.atan2(forward.x)
        };
        let width = (size.0 as f64 / 1280.0 * DEFAULT_WIDTH).clamp(0.6, 2.0);
        // Level with the eyes, which ride a little above the pivot the room is centred on.
        let level = (self.stereo.neck_up_m / DEFAULT_RADIUS).atan();
        let mut place = Placement { yaw: heading, pitch: level, width, ..Placement::default() };
        // To the right of whatever is in the way first, then to the left, a step at a time.
        let half = |w: f64| w / DEFAULT_RADIUS * 0.5;
        let taken = |yaw: f64, windows: &[Win]| {
            windows.iter().filter(|w| !w.place.pinned && !Room::is_panel(w.id)).any(|w| {
                let apart = (yaw - w.place.yaw + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
                apart.abs() < half(width) + half(w.place.width) + GAP
            })
        };
        if taken(place.yaw, &self.windows) {
            let step = half(width) * 2.0 + GAP;
            let mut found = None;
            for n in 1..24 {
                let k = n as f64;
                for sign in [-1.0, 1.0] {
                    let yaw = heading + sign * k * step * 0.5 * 2.0;
                    if !taken(yaw, &self.windows) {
                        found = Some(yaw);
                        break;
                    }
                }
                if found.is_some() {
                    break;
                }
            }
            place.yaw = found.unwrap_or(heading);
        }
        self.windows.push(Win { id, size, place, shown: false });
        self.focus = Some(id);
    }

    /// A panel Spatiand draws itself -- a menu -- put where the wearer is looking. It takes no
    /// keyboard focus and is always drawn over the windows. `width` and `radius` are metres.
    pub fn set_panel(&mut self, id: u32, size: (u32, u32), width: f64, radius: f64) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.size = size;
            return;
        }
        let forward = self.head() * DVec3::X;
        let place = Placement {
            yaw: forward.y.atan2(forward.x),
            // Level with the eyes, which are a little above the pivot the room is centred on.
            pitch: (forward.z.clamp(-1.0, 1.0).asin() + (self.stereo.neck_up_m / radius).atan()).clamp(-PITCH_LIMIT, PITCH_LIMIT),
            radius,
            width,
            ..Placement::default()
        };
        self.windows.push(Win { id, size, place, shown: true });
    }

    // MARK: sound

    pub fn set_app(&mut self, id: u32, app: &str) {
        self.app_of.insert(id, app.to_string());
    }

    /// Where an application's sound is from: its window, as the head sees it. The focused
    /// window of the application if there is one, else any; with none, straight ahead of the head.
    fn stage_for(&self, app: &str, head: DQuat) -> (Stage, f64) {
        let mut best: Option<&Win> = None;
        for w in &self.windows {
            if self.app_of.get(&w.id).is_some_and(|a| a == app) && best.is_none_or(|b| self.focus == Some(w.id) || b.id != self.focus.unwrap_or(u32::MAX)) {
                best = Some(w);
            }
        }
        let placed = self.placed(head);
        let (yaw, pitch, half_width) = match best.and_then(|w| placed.iter().find(|(i, _)| self.windows[*i].id == w.id)) {
            Some((_, p)) => (p.yaw, p.pitch, (p.width * 0.5).atan2(p.radius)),
            None => {
                let forward = head * DVec3::X;
                (forward.y.atan2(forward.x), forward.z.clamp(-1.0, 1.0).asin(), NOMINAL_HALF_STAGE)
            }
        };
        // How far off straight ahead the head is looking from it.
        let direction = DVec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin());
        let off_axis = (head.inverse() * direction).x.clamp(-1.0, 1.0).acos();
        (Stage { yaw, pitch, half_width }, off_axis)
    }

    /// An application's sound, signed 16-bit little-endian and interleaved, placed in the room and
    /// folded to two ears: interleaved stereo floats, appended to `out`. A chunk that ends part
    /// way through a frame keeps the rest for the next, or every channel after it would be out of place.
    pub fn render_audio(&mut self, app: &str, channels: usize, pcm: &[u8], out: &mut Vec<f32>) {
        let Some(layout) = Layout::from_count(channels) else { return };
        let head = self.head();
        let (stage, off_axis) = self.stage_for(app, head);
        let voice = self.voices.entry(app.to_string()).or_insert_with(|| Voice {
            layout,
            binaural: Binaural::new(layout, make_head(), Directness::default(), AUDIO_RATE),
            carry: Vec::new(),
        });
        if voice.layout != layout {
            *voice = Voice {
                layout,
                binaural: Binaural::new(layout, make_head(), Directness::default(), AUDIO_RATE),
                carry: Vec::new(),
            };
        }
        voice.carry.extend_from_slice(pcm);
        let frame_bytes = channels * 2;
        let whole = voice.carry.len() / frame_bytes * frame_bytes;
        if whole == 0 {
            return;
        }
        let input: Vec<f32> = voice.carry[..whole]
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
            .collect();
        voice.carry.drain(..whole);
        voice.binaural.aim(&place(layout, &stage, head), off_axis);
        let start = out.len();
        out.resize(start + input.len() / channels * 2, 0.0);
        voice.binaural.render(&input, &mut out[start..]);
    }

    /// Forget an application's sound, when it has stopped.
    pub fn drop_voice(&mut self, app: &str) {
        self.voices.remove(app);
    }

    pub fn show(&mut self, id: u32) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.shown = true;
        }
    }

    pub fn remove_window(&mut self, id: u32) {
        self.windows.retain(|w| w.id != id);
        self.app_of.remove(&id);
        if self.focus == Some(id) {
            self.focus = self.windows.iter().rev().find(|w| !Self::is_panel(w.id)).map(|w| w.id);
        }
        if self.grab.is_some_and(|g| g.id == id) {
            self.grab = None;
        }
    }

    pub fn clear(&mut self) {
        self.windows.clear();
        self.focus = None;
        self.grab = None;
    }

    /// Bring a window to where the wearer is looking.
    pub fn bring_here(&mut self, id: u32) {
        let forward = self.head() * DVec3::X;
        let heading = forward.y.atan2(forward.x);
        let pitch = forward.z.clamp(-1.0, 1.0).asin();
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.place.yaw = heading;
            w.place.pitch = pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT);
        }
        self.raise(id);
    }

    /// Make a window bigger or smaller, from where it stands.
    pub fn scale(&mut self, id: u32, factor: f64) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.place.width = (w.place.width * factor).clamp(WIDTH_RANGE.0, WIDTH_RANGE.1);
        }
    }

    pub fn set_pinned(&mut self, id: u32, pinned: bool) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.place.pinned = pinned;
            if !pinned {
                w.place.facing = None;
                self.bring_here(id);
            }
        }
    }

    pub fn is_pinned(&self, id: u32) -> bool {
        self.windows.iter().any(|w| w.id == id && w.place.pinned)
    }

    pub fn next_corner(&mut self) {
        self.corner = self.corner.next();
    }

    pub fn toggle_size(&mut self) {
        self.size = self.size.toggle();
    }

    pub fn set_pip(&mut self, corner: Corner, size: Size) {
        self.corner = corner;
        self.size = size;
    }

    pub fn pip(&self) -> (Corner, Size) {
        (self.corner, self.size)
    }

    /// Every window's place for a head: a pinned one is worked out from the head afresh.
    fn placed(&self, head: DQuat) -> Vec<(usize, Placement)> {
        let view = pip::View {
            fov_deg: (self.stereo.h_fov_deg, self.stereo.v_fov_deg()),
            eye: self.eye_centre(DQuat::IDENTITY),
        };
        let mut slot = 0;
        self.windows
            .iter()
            .enumerate()
            .map(|(i, w)| {
                if w.place.pinned {
                    let pose = pip::pose(head, &view, self.corner, self.size, w.aspect(), slot);
                    slot += 1;
                    (i, Placement {
                        yaw: pose.yaw,
                        pitch: pose.pitch,
                        radius: pose.radius,
                        width: pose.width,
                        facing: Some(pose.facing),
                        pinned: true,
                    })
                } else {
                    (i, w.place)
                }
            })
            .collect()
    }

    // MARK: the pointer

    /// The mouse moved, in points.
    pub fn move_pointer(&mut self, dx: f64, dy: f64) {
        let k = DEGREES_PER_POINT.to_radians();
        let (hx, hy) = (CURSOR_HALF_FOV.0.to_radians(), CURSOR_HALF_FOV.1.to_radians());
        // Mouse right is the view's right, which is negative to the left.
        self.cursor.0 = (self.cursor.0 - dx * k).clamp(-hx, hx);
        self.cursor.1 = (self.cursor.1 - dy * k).clamp(-hy, hy);
    }

    pub fn centre_pointer(&mut self) {
        self.cursor = (0.0, 0.0);
    }

    fn ray(&self, head: DQuat) -> Ray {
        let aim = DQuat::from_axis_angle(DVec3::Z, self.cursor.0) * DQuat::from_axis_angle(DVec3::Y, -self.cursor.1);
        Ray { origin: self.eye_centre(head), direction: (head * (aim * DVec3::X)).normalize() }
    }

    /// What the cursor is over, front first.
    pub fn aim(&self) -> Aim {
        let head = self.head();
        let ray = self.ray(head);
        let placed = self.placed(head);
        // Pinned windows are in front of the room, then the room's own, the last drawn first.
        let mut order: Vec<usize> = placed.iter().map(|(i, _)| *i).collect();
        order.sort_by_key(|i| (!Self::is_panel(self.windows[*i].id), !self.windows[*i].place.pinned, std::cmp::Reverse(*i)));
        for i in order {
            let (_, place) = placed[i];
            let w = &self.windows[i];
            if !w.shown {
                continue;
            }
            if let Some(hit) = intersect_quad(&ray, &w.quad(&place)) {
                return Aim {
                    window: Some(w.id),
                    x: (hit.u * w.size.0 as f64).clamp(0.0, w.size.0 as f64),
                    y: (hit.v * w.size.1 as f64).clamp(0.0, w.size.1 as f64),
                    point: hit.point,
                    title: false,
                };
            }
            if let Some((bar, _, _)) = w.bar(&place) {
                if let Some(hit) = intersect_quad(&ray, &bar) {
                    return Aim {
                        window: Some(w.id),
                        x: (hit.u * BAR_PX.0 as f64).clamp(0.0, BAR_PX.0 as f64),
                        y: (hit.v * BAR_PX.1 as f64).clamp(0.0, BAR_PX.1 as f64),
                        point: hit.point,
                        title: true,
                    };
                }
            }
        }
        Aim { window: None, x: 0.0, y: 0.0, point: ray.at(DEFAULT_RADIUS), title: false }
    }

    /// Where the cursor is in one window's own pixels, even when it has left it: a drag that
    /// started there goes on being measured against it.
    pub fn aim_at(&self, id: u32) -> Option<(f64, f64)> {
        let head = self.head();
        let ray = self.ray(head);
        let placed = self.placed(head);
        let (i, place) = placed.iter().find(|(i, _)| self.windows[*i].id == id).copied()?;
        let w = &self.windows[i];
        let hit = intersect_plane(&ray, &w.quad(&place))?;
        Some((
            (hit.u * w.size.0 as f64).clamp(0.0, w.size.0 as f64),
            (hit.v * w.size.1 as f64).clamp(0.0, w.size.1 as f64),
        ))
    }

    // MARK: moving a window

    /// Take a window by where the cursor is on it.
    pub fn begin_grab(&mut self, id: u32) {
        let head = self.head();
        let ray = self.ray(head);
        let (yaw, pitch) = Self::angles(ray.direction);
        if let Some(w) = self.windows.iter().find(|w| w.id == id && !w.place.pinned) {
            self.grab = Some(Grab { id, yaw: w.place.yaw - yaw, pitch: w.place.pitch - pitch });
            self.raise(id);
        }
    }

    pub fn grabbed(&self) -> Option<u32> {
        self.grab.map(|g| g.id)
    }

    /// Carry what was taken along with the cursor, which the head turns.
    pub fn drag(&mut self) {
        let Some(grab) = self.grab else { return };
        let head = self.head();
        let (yaw, pitch) = Self::angles(self.ray(head).direction);
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == grab.id) {
            w.place.yaw = (yaw + grab.yaw + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
            w.place.pitch = (pitch + grab.pitch).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        }
    }

    pub fn end_grab(&mut self) {
        self.grab = None;
    }

    fn angles(direction: DVec3) -> (f64, f64) {
        (direction.y.atan2(direction.x), direction.z.clamp(-1.0, 1.0).asin())
    }

    // MARK: what to draw

    pub fn stereo(&self) -> &StereoConfig {
        &self.stereo
    }

    pub fn set_per_eye(&mut self, width: u32, height: u32) {
        self.stereo.per_eye = (width.max(1), height.max(1));
    }

    /// Each eye's view-projection, left then right, in column-major order.
    pub fn eye_matrices(&self) -> [Mat4; 2] {
        let head = self.head();
        let [l, r] = eyes_for(head, DVec3::ZERO, &self.stereo);
        [l.view_projection(), r.view_projection()]
    }

    /// The windows as triangles, `x y z u v` five floats a vertex, and for each window the
    /// range of vertices it is. The cursor comes last, as [`CURSOR`].
    pub fn frame(&self) -> Frame {
        let head = self.head();
        let placed = self.placed(head);
        let aim = self.aim();
        let mut verts: Vec<f32> = Vec::new();
        let mut draws: Vec<Draw> = Vec::new();
        let mut bars: Vec<Draw> = Vec::new();

        // The room's windows first, in order, then the pinned ones over them.
        let mut order: Vec<usize> = placed.iter().map(|(i, _)| *i).collect();
        order.sort_by_key(|i| (Self::is_panel(self.windows[*i].id), self.windows[*i].place.pinned, *i));
        for i in order {
            let (_, place) = placed[i];
            let w = &self.windows[i];
            if !w.shown {
                continue;
            }
            let first = (verts.len() / 5) as u32;
            let height = w.height(&place);
            push_strips(&mut verts, &place, height, 0.0);
            let count = (verts.len() / 5) as u32 - first;
            let aimed = aim.window == Some(w.id) && !aim.title;
            draws.push(Draw {
                window: w.id,
                first,
                count,
                focused: self.focus == Some(w.id),
                aimed,
                pinned: place.pinned,
            });
            if let Some((_, rise, bar_height)) = w.bar(&place) {
                let first = (verts.len() / 5) as u32;
                push_strips(&mut verts, &place, bar_height, rise);
                let count = (verts.len() / 5) as u32 - first;
                bars.push(Draw {
                    window: w.id | TITLE_FLAG,
                    first,
                    count,
                    focused: self.focus == Some(w.id),
                    aimed: aim.window == Some(w.id) && aim.title,
                    pinned: false,
                });
            }
        }
        // The bars over the windows, and Spatiand's own panels over those.
        let panels_at = draws.iter().position(|d| Self::is_panel(d.window)).unwrap_or(draws.len());
        for (n, bar) in bars.into_iter().enumerate() {
            draws.insert(panels_at + n, bar);
        }

        // The cursor: a small flat square at the hit, turned to face the eye.
        let eye = self.eye_centre(head);
        let to_eye = (eye - aim.point).normalize_or_zero();
        let distance = (aim.point - eye).length().max(0.3);
        // About a degree and a half for a pointer 32 pixels tall, at any distance, so it reads as
        // the same size wherever it is.
        let (hot_x, hot_y, shape_w, shape_h) = self.cursor_shape;
        let (w_px, h_px) = if shape_w > 0.0 && shape_h > 0.0 { (shape_w, shape_h) } else { (32.0, 32.0) };
        let unit = distance * 0.026 / 32.0;
        let (width, height) = ((w_px * unit).clamp(0.0, distance * 0.2), (h_px * unit).clamp(0.0, distance * 0.2));
        let right = to_eye.cross(DVec3::Z).normalize_or_zero();
        let right = if right == DVec3::ZERO { DVec3::Y } else { right };
        let up = right.cross(to_eye).normalize_or_zero();
        // The hot spot is the point itself, a little in front of the surface so it is not buried in it.
        let tip = aim.point + to_eye * 0.01;
        let tl = tip - right * (hot_x * unit) + up * (hot_y * unit);
        let tr = tl + right * width;
        let bl = tl - up * height;
        let br = tr - up * height;
        let first = (verts.len() / 5) as u32;
        let corner = |p: DVec3, u: f32, v: f32| [p.x as f32, p.y as f32, p.z as f32, u, v];
        for v in [
            corner(tl, 0.0, 0.0), corner(bl, 0.0, 1.0), corner(tr, 1.0, 0.0),
            corner(tr, 1.0, 0.0), corner(bl, 0.0, 1.0), corner(br, 1.0, 1.0),
        ] {
            verts.extend_from_slice(&v);
        }
        draws.push(Draw { window: CURSOR, first, count: 6, focused: false, aimed: false, pinned: false });

        Frame { vertices: verts, draws, eyes: self.eye_matrices() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Draw {
    pub window: u32,
    pub first: u32,
    pub count: u32,
    pub focused: bool,
    pub aimed: bool,
    pub pinned: bool,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub vertices: Vec<f32>,
    pub draws: Vec<Draw>,
    pub eyes: [Mat4; 2],
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room() -> Room {
        let mut r = Room::new();
        r.set_fixed_head(Some(DQuat::IDENTITY));
        r
    }

    #[test]
    fn a_first_window_goes_straight_ahead_and_the_next_beside_it() {
        let mut r = room();
        r.set_window(1, (1280, 800));
        r.set_window(2, (1280, 800));
        let (a, b) = (r.windows()[0].place.yaw, r.windows()[1].place.yaw);
        assert!(a.abs() < 1e-9, "the first window should be ahead, not at {a}");
        assert!((a - b).abs() > 0.4, "the second window is on top of the first: {a} and {b}");
    }

    #[test]
    fn looking_straight_at_a_window_aims_at_its_middle() {
        let mut r = room();
        r.set_window(1, (1280, 800));
        r.show(1);
        let aim = r.aim();
        assert_eq!(aim.window, Some(1));
        assert!((aim.x - 640.0).abs() < 3.0 && (aim.y - 400.0).abs() < 3.0, "{aim:?}");
    }

    #[test]
    fn moving_the_mouse_right_moves_the_aim_right_across_the_window() {
        let mut r = room();
        r.set_window(1, (1280, 800));
        r.show(1);
        r.move_pointer(60.0, 0.0);
        let aim = r.aim();
        assert!(aim.x > 700.0, "the aim should have gone right, it is at {}", aim.x);
        r.move_pointer(-120.0, 0.0);
        assert!(r.aim().x < 580.0, "and back past the middle to the left");
        r.centre_pointer();
        r.move_pointer(0.0, 40.0);
        assert!(r.aim().y > 420.0, "mouse down is down the window");
    }

    #[test]
    fn turning_the_head_turns_what_is_aimed_at() {
        let mut r = room();
        r.set_window(1, (1280, 800));
        r.show(1);
        // Looking 40 degrees to the left: the window, straight ahead of the room, is not there.
        r.set_fixed_head(Some(DQuat::from_axis_angle(DVec3::Z, 40f64.to_radians())));
        assert_eq!(r.aim().window, None);
        // And a window placed where the head is looking is.
        r.set_window(2, (1280, 800));
        r.show(2);
        assert_eq!(r.aim().window, Some(2));
    }

    #[test]
    fn a_bent_window_is_aimed_at_by_its_arc_not_its_chord() {
        let mut r = room();
        r.set_window(1, (1000, 1000));
        r.show(1);
        r.windows[0].place.width = 1.1;
        // Quarter of the way across, along the surface: 0.275 m of arc at 2.2 m is 7.2 degrees.
        let angle = (0.275f64 / 2.2).to_degrees();
        r.cursor = (-angle.to_radians(), 0.0);
        let aim = r.aim();
        // From the eyes, a little forward of the pivot the room is round, so not exactly 750.
        assert!((730.0..765.0).contains(&aim.x), "a quarter of the way across should be near 750, got {}", aim.x);
    }

    #[test]
    fn a_window_taken_by_the_cursor_follows_it_round_the_room() {
        let mut r = room();
        r.set_window(1, (1280, 800));
        r.show(1);
        r.begin_grab(1);
        r.set_fixed_head(Some(DQuat::from_axis_angle(DVec3::Z, 30f64.to_radians())));
        r.drag();
        assert!((r.windows()[0].place.yaw - 30f64.to_radians()).abs() < 1e-6);
        r.end_grab();
        r.set_fixed_head(Some(DQuat::IDENTITY));
        r.drag();
        assert!((r.windows()[0].place.yaw - 30f64.to_radians()).abs() < 1e-6, "it stayed where it was put");
    }

    #[test]
    fn what_is_drawn_has_a_window_in_strips_and_a_cursor_last() {
        let mut r = room();
        r.set_window(1, (1280, 800));
        r.show(1);
        let frame = r.frame();
        assert_eq!(frame.draws.len(), 3, "the window, its title bar and the cursor");
        assert_eq!(frame.draws[0].window, 1);
        assert!(frame.draws[0].count >= 12, "a bent window is several strips");
        assert_eq!(frame.draws.last().unwrap().window, CURSOR);
        assert_eq!(frame.vertices.len() % 5, 0);
    }

    #[test]
    fn a_window_with_no_picture_is_neither_drawn_nor_aimed_at() {
        let mut r = room();
        r.set_window(1, (1280, 800));
        assert_eq!(r.aim().window, None);
        assert_eq!(r.frame().draws.len(), 1, "only the cursor");
    }

    #[test]
    fn a_pinned_window_stays_in_its_corner_whichever_way_the_head_turns() {
        let mut r = room();
        r.set_window(1, (1280, 720));
        r.show(1);
        r.set_pinned(1, true);
        let corner_of = |r: &Room| {
            let head = r.head();
            let placed = r.placed(head);
            (head.inverse() * placed[0].1.position()).normalize()
        };
        let before = corner_of(&r);
        r.set_fixed_head(Some(DQuat::from_axis_angle(DVec3::Z, 1.1)));
        let after = corner_of(&r);
        assert!((before - after).length() < 1e-9, "the corner moved with the head: {before:?} {after:?}");
        assert!(before.y < 0.0 && before.z < 0.0, "the default corner is the lower right: {before:?}");
    }

    #[test]
    fn removing_the_focused_window_hands_focus_to_another() {
        let mut r = room();
        r.set_window(1, (800, 600));
        r.set_window(2, (800, 600));
        assert_eq!(r.focus(), Some(2));
        r.remove_window(2);
        assert_eq!(r.focus(), Some(1));
    }

    #[test]
    fn a_panel_is_aimed_at_before_the_windows_behind_it_and_takes_no_focus() {
        let mut r = room();
        r.set_window(1, (1280, 800));
        r.show(1);
        r.set_panel(PANEL_FIRST + 2, (1000, 500), 0.9, 1.6);
        assert_eq!(r.aim().window, Some(PANEL_FIRST + 2));
        assert_eq!(r.focus(), Some(1), "a panel is not where the keyboard goes");
        r.set_focus(Some(PANEL_FIRST + 2));
        assert_eq!(r.focus(), None, "and cannot be given it");
        let frame = r.frame();
        let order: Vec<u32> = frame.draws.iter().map(|d| d.window).collect();
        assert_eq!(order, vec![1, 1 | TITLE_FLAG, PANEL_FIRST + 2, CURSOR], "drawn over the window and its bar, under the cursor");
        r.remove_window(PANEL_FIRST + 2);
        assert_eq!(r.aim().window, Some(1));
    }

    fn tone(channels: usize, frames: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for n in 0..frames {
            let v = ((n as f32 * 0.1).sin() * 12000.0) as i16;
            for _ in 0..channels {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out
    }

    fn loudness(stereo: &[f32]) -> (f32, f32) {
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for f in stereo.chunks_exact(2) {
            l += f[0] * f[0];
            r += f[1] * f[1];
        }
        (l, r)
    }

    #[test]
    fn a_window_to_the_left_is_heard_on_the_left_and_follows_the_head() {
        let mut r = room();
        r.set_window(1, (1280, 800));
        r.show(1);
        r.set_app(1, "chrome");
        // Put it a quarter turn to the left.
        r.windows[0].place.yaw = 80f64.to_radians();
        let mut out = Vec::new();
        // A few blocks, for the filters to settle after the first aim.
        for _ in 0..4 {
            out.clear();
            r.render_audio("chrome", 2, &tone(2, 480), &mut out);
        }
        let (l, rt) = loudness(&out);
        assert!(l > rt * 1.5, "a window on the left should be louder in the left ear: {l} against {rt}");
        // Turn to face it: now it is ahead, and the two ears are close.
        r.set_fixed_head(Some(DQuat::from_axis_angle(DVec3::Z, 80f64.to_radians())));
        for _ in 0..4 {
            out.clear();
            r.render_audio("chrome", 2, &tone(2, 480), &mut out);
        }
        let (l, rt) = loudness(&out);
        assert!((l / rt - 1.0).abs() < 0.5, "facing it, the ears should be about equal: {l} against {rt}");
    }

    #[test]
    fn a_chunk_that_stops_mid_frame_does_not_shift_the_channels() {
        let mut r = room();
        let bytes = tone(6, 100);
        let mut a = Vec::new();
        r.render_audio("film", 6, &bytes[..bytes.len() / 2 + 3], &mut a);
        r.render_audio("film", 6, &bytes[bytes.len() / 2 + 3..], &mut a);
        let mut r2 = room();
        let mut b = Vec::new();
        r2.render_audio("film", 6, &bytes, &mut b);
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-4), "the split changed what was heard");
    }

    #[test]
    fn a_menu_or_hint_in_the_way_does_not_push_a_window_aside() {
        let mut r = Room::new();
        r.set_fixed_head(Some(DQuat::IDENTITY));
        r.set_panel(PANEL_FIRST + 3, (1000, 300), 0.8, 1.6);
        r.set_window(7, (1280, 800));
        assert!(r.windows().iter().find(|w| w.id == 7).unwrap().place.yaw.abs() < 1e-9);
    }

    #[test]
    fn a_small_window_is_aimed_at_like_any_other() {
        let mut r = Room::new();
        r.set_fixed_head(Some(DQuat::IDENTITY));
        r.set_window(0x8000, (594, 421));
        r.show(0x8000);
        assert_eq!(r.aim().window, Some(0x8000), "{:?}", r.windows());
    }

    /// With `SPATIAND_MYSOFA_LIBRARY` and `SPATIAND_HRTF_DATASET` set (the app's bundle has both), a
    /// measured head is used, and a sound behind is not the same as the same sound ahead -- which
    /// the parametric one can hardly say.
    #[test]
    fn a_measured_head_tells_behind_from_ahead() {
        if std::env::var_os("SPATIAND_HRTF_DATASET").is_none() || std::env::var_os("SPATIAND_MYSOFA_LIBRARY").is_none() {
            eprintln!("skipped: no measured head given");
            return;
        }
        assert!(spatiand_audio::hrtf::Hrtf::system(AUDIO_RATE).is_ok(), "the library and dataset should open");
        let mut high = Vec::new();
        for n in 0..4800 {
            // Something bright, which is what a head's folds colour most.
            let v = ((n as f32 * 1.9).sin() * 9000.0) as i16;
            high.extend_from_slice(&v.to_le_bytes());
            high.extend_from_slice(&v.to_le_bytes());
        }
        let energy = |yaw_deg: f64| {
            let mut r = Room::new();
            r.set_fixed_head(Some(DQuat::IDENTITY));
            r.set_window(1, (1280, 800));
            r.show(1);
            r.set_app(1, "x");
            r.windows[0].place.yaw = yaw_deg.to_radians();
            let mut out = Vec::new();
            for _ in 0..3 {
                out.clear();
                r.render_audio("x", 2, &high, &mut out);
            }
            let (l, rt) = loudness(&out);
            (l + rt) as f64
        };
        let ahead = energy(0.0);
        let behind = energy(180.0);
        assert!((ahead - behind).abs() / ahead.max(behind) > 0.15, "behind and ahead should differ: {ahead} and {behind}");
    }

    #[test]
    fn a_title_bar_floats_above_a_window_and_is_aimed_at_on_its_own() {
        let mut r = room();
        r.set_window(1, (1280, 800));
        r.show(1);
        // The window is 0.69 m tall at 2.2 m: about 9 degrees up from its middle is its bar.
        r.cursor = (0.0, 10.0f64.to_radians());
        let aim = r.aim();
        assert_eq!(aim.window, Some(1));
        assert!(aim.title, "that is the title bar: {aim:?}");
        assert!((0.0..=BAR_PX.0 as f64).contains(&aim.x) && (0.0..=BAR_PX.1 as f64).contains(&aim.y));
        // And below it, the window.
        r.cursor = (0.0, 0.0);
        assert!(!r.aim().title);
        let frame = r.frame();
        let ids: Vec<u32> = frame.draws.iter().map(|d| d.window).collect();
        assert_eq!(ids, vec![1, 1 | TITLE_FLAG, CURSOR], "the bar is drawn over the window, under the cursor");
    }

    #[test]
    fn a_pinned_window_has_no_title_bar() {
        let mut r = room();
        r.set_window(1, (1280, 720));
        r.show(1);
        r.set_pinned(1, true);
        assert!(r.frame().draws.iter().all(|d| d.window & TITLE_FLAG == 0));
    }
}
