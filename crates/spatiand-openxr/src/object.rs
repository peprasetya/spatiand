//! The things an application holds handles to.
//!
//! OpenXR hands out opaque 64-bit handles. Here each is a counter value that names an entry in a
//! table of reference-counted objects, and nothing is ever dereferenced from a handle the
//! application made up: an unknown handle is `XR_ERROR_HANDLE_INVALID`, not a crash. A call that
//! blocks (`xrWaitFrame`) clones the object's `Arc` out of the table first, so it never holds the
//! table's lock while it waits.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::pose::{History, Pose, Sample};
use crate::vk::{Exportable, Image, Vk};
use crate::wl::{Buffer, Link};

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(0x5350_0001);

pub fn new_handle() -> u64 {
    NEXT_HANDLE.fetch_add(1, Ordering::Relaxed)
}

/// A table of one kind of object, by handle.
pub struct Table<T> {
    items: OnceLock<Mutex<HashMap<u64, Arc<T>>>>,
}

impl<T> Table<T> {
    pub const fn new() -> Self {
        Table { items: OnceLock::new() }
    }

    fn map(&self) -> &Mutex<HashMap<u64, Arc<T>>> {
        self.items.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub fn insert(&self, handle: u64, item: Arc<T>) {
        self.map().lock().unwrap().insert(handle, item);
    }

    pub fn get(&self, handle: u64) -> Option<Arc<T>> {
        self.map().lock().unwrap().get(&handle).cloned()
    }

    pub fn remove(&self, handle: u64) -> Option<Arc<T>> {
        self.map().lock().unwrap().remove(&handle)
    }

    pub fn all(&self) -> Vec<Arc<T>> {
        self.map().lock().unwrap().values().cloned().collect()
    }
}

pub static INSTANCES: Table<Instance> = Table::new();
pub static SESSIONS: Table<Session> = Table::new();
pub static SPACES: Table<Space> = Table::new();
pub static SWAPCHAINS: Table<Swapchain> = Table::new();
pub static ACTION_SETS: Table<ActionSet> = Table::new();
pub static ACTIONS: Table<Action> = Table::new();

/// The one system this runtime has: the headset in the room.
pub const SYSTEM_ID: u64 = 1;

/// How far below the head the floor is taken to be, for the STAGE space. Spatiand does not know
/// how tall the wearer is, and the neck model puts the head at the origin.
pub const FLOOR_BELOW_HEAD: f32 = 1.6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    Ready,
    Synchronized,
    Visible,
    Focused,
    Stopping,
    Exiting,
}

impl SessionState {
    pub fn raw(self) -> openxr_sys::SessionState {
        use openxr_sys::SessionState as S;
        match self {
            SessionState::Idle => S::IDLE,
            SessionState::Ready => S::READY,
            SessionState::Synchronized => S::SYNCHRONIZED,
            SessionState::Visible => S::VISIBLE,
            SessionState::Focused => S::FOCUSED,
            SessionState::Stopping => S::STOPPING,
            SessionState::Exiting => S::EXITING,
        }
    }
}

pub enum Event {
    SessionState {
        session: u64,
        state: SessionState,
        time_ns: i64,
    },
    /// Which controllers the application has is now known (or has changed).
    InteractionProfileChanged { session: u64 },
}

pub struct Instance {
    pub handle: u64,
    pub app_name: String,
    pub extensions: Vec<String>,
    /// The minor version of OpenXR 1.x the application asked for.
    pub api_minor: u32,
    /// The application has asked what the graphics API it uses needs, as a session requires first.
    pub requirements_asked: std::sync::atomic::AtomicBool,
    pub events: Mutex<VecDeque<Event>>,
    paths: Mutex<Vec<String>>,
    /// What the application suggested each controller type's components do, by profile path:
    /// `(action, binding path)`.
    pub bindings: Mutex<std::collections::HashMap<u64, Vec<(u64, u64)>>>,
}

impl Instance {
    pub fn new(handle: u64, app_name: String, extensions: Vec<String>, api_minor: u32) -> Instance {
        Instance {
            handle,
            app_name,
            extensions,
            api_minor,
            requirements_asked: std::sync::atomic::AtomicBool::new(false),
            events: Mutex::new(VecDeque::new()),
            paths: Mutex::new(Vec::new()),
            bindings: Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn has_extension(&self, name: &str) -> bool {
        self.extensions.iter().any(|e| e == name)
    }

    /// A string's path handle, made on first use. Handles count from 1; zero is `XR_NULL_PATH`.
    pub fn path(&self, text: &str) -> u64 {
        let mut paths = self.paths.lock().unwrap();
        if let Some(i) = paths.iter().position(|p| p == text) {
            return i as u64 + 1;
        }
        paths.push(text.to_string());
        paths.len() as u64
    }

    /// The controller type the application is told it has: the best of ours that it has bound.
    pub fn active_profile(&self) -> Option<u64> {
        let bindings = self.bindings.lock().unwrap();
        crate::input::PROFILES
            .iter()
            .map(|p| self.path(p))
            .find(|p| bindings.get(p).is_some_and(|b| !b.is_empty()))
    }

    /// The bindings of `action` under the active profile, as path strings.
    pub fn bindings_of(&self, action: u64) -> Vec<String> {
        let Some(profile) = self.active_profile() else { return Vec::new() };
        let bindings = self.bindings.lock().unwrap();
        bindings
            .get(&profile)
            .map(|list| {
                list.iter()
                    .filter(|(a, _)| *a == action)
                    .filter_map(|(_, path)| self.path_text(*path))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn path_text(&self, path: u64) -> Option<String> {
        let paths = self.paths.lock().unwrap();
        path.checked_sub(1).and_then(|i| paths.get(i as usize).cloned())
    }
}

/// Everything about a session that changes as it runs.
pub struct SessionRun {
    pub state: SessionState,
    pub begun: bool,
    pub frame_open: bool,
    /// `xrWaitFrame`s not yet answered by an `xrBeginFrame`.
    pub waits: u32,
    /// The pose ring's write index at the last `xrWaitFrame`, so the next one waits for a new one.
    pub last_index: u64,
    /// When the last frame was due, for pacing without a compositor to pace us.
    pub next_frame_ns: i64,
    pub frame_period_ns: i64,
    /// The head when the session began: the origin of LOCAL space.
    pub local_origin: Option<Pose>,
    pub warned_layers: bool,
    pub frames: u64,
    /// Where the quad layers of the last frame stood in the world, and how big: what a laser from a
    /// hand can land on.
    pub quads: Vec<crate::input::QuadPlace>,
}

pub struct Session {
    pub handle: u64,
    pub instance: Arc<Instance>,
    pub vk: Option<Vk>,
    /// Set for an application that draws with OpenGL, whose swapchains are textures backed by
    /// `vk`'s memory.
    pub gl: Option<crate::gl::Gl>,
    pub link: Option<Link>,
    pub run: Mutex<SessionRun>,
    pub output: Mutex<Option<Output>>,
    pub input: Mutex<InputState>,
}

/// The controllers, as the session sees them.
#[derive(Default)]
pub struct InputState {
    pub pad: Option<crate::input::Pad>,
    pub tried: bool,
    pub state: crate::input::PadState,
    /// The wearer has a pointer to give: a seat on the compositor with one.
    pub has_pointer: bool,
    /// The application has attached its action sets and been told what it has.
    pub attached: bool,
    /// The action sets the application attached, by handle.
    pub attached_sets: Vec<u64>,
    /// The application has been told which controller it has, which is said once and when it syncs.
    pub profile_announced: bool,
    /// What each action read as last time, for "changed since last sync".
    pub last: std::collections::HashMap<(u64, Option<crate::input::Hand>), (crate::input::Value, i64)>,
}

impl InputState {
    /// Fold in whatever the pad has done since last time, finding the pad the first time.
    pub fn poll(&mut self) {
        if !self.tried {
            self.tried = true;
            self.pad = crate::input::Pad::open();
            match &self.pad {
                Some(pad) => log::info!("controllers: standing in with \"{}\"", pad.name),
                None => log::info!("controllers: no gamepad found; hands will rest and buttons stay up"),
            }
        }
        if let Some(pad) = self.pad.as_mut() {
            pad.poll(&mut self.state);
            // What the pad itself says, before any game has made anything of it: to tell a pad
            // that reads wrongly from a game that binds something other than was expected.
            static SAID: std::sync::Mutex<[f32; 6]> = std::sync::Mutex::new([0.0; 6]);
            let now = [
                self.state.sticks[0], self.state.sticks[1], self.state.sticks[2], self.state.sticks[3],
                self.state.triggers[0], self.state.triggers[1],
            ];
            let mut said = SAID.lock().unwrap();
            if now.iter().zip(said.iter()).any(|(a, b)| (a - b).abs() > 0.15) {
                *said = now;
                log::debug!(
                    "pad: left stick ({:.2}, {:.2}) right stick ({:.2}, {:.2}) triggers {:.2} {:.2}",
                    now[0], now[1], now[2], now[3], now[4], now[5]
                );
            }
        }
    }
}

/// The side-by-side frames that go to the compositor.
pub struct Output {
    pub eye_size: (u32, u32),
    pub format: ash::vk::Format,
    pub frames: Vec<Frame>,
    pub next: usize,
}

pub struct Frame {
    pub target: Exportable,
    pub buffer: Buffer,
}

impl Drop for Output {
    fn drop(&mut self) {
        // The images were allocated in a device that outlives this only by the session's own
        // order of destruction, which `Session::drop` takes care of; nothing to do here.
    }
}

/// How long after the pointer last moved or clicked it still holds the hands, in nanoseconds. A
/// wearer using a menu moves the pointer and clicks; one playing does not, and a hand that stayed
/// aimed at the last place the pointer rested would send every step that way.
const POINTER_HOLDS_NS: i64 = 4_000_000_000;

/// How far away a laser from a hand ends when the pointer is on nothing the game has drawn as a
/// layer: far enough that two hands' lasers look parallel, near enough to still cross.
const POINTER_REACH: f32 = 4.0;

impl Session {
    /// Fold in what the pad and the pointer have done since last time.
    pub fn poll_input(&self) {
        let mut input = self.input.lock().unwrap();
        input.poll();
        match &self.link {
            Some(link) if link.has_pointer() => {
                input.has_pointer = true;
                let p = link.pointer();
                input.state.mouse = if p.inside { p.buttons } else { [false; 2] };
            }
            _ => {}
        }
    }

    /// The ray the wearer's pointer is casting from the left eye, while it is in use.
    fn pointer_ray(&self, sample: &Sample, now_ns: i64) -> Option<(glam::Vec3, glam::Vec3)> {
        let link = self.link.as_ref()?;
        let p = link.pointer();
        if !p.inside || now_ns - p.active_ns > POINTER_HOLDS_NS {
            return None;
        }
        Some(crate::input::pointer_ray(&sample.eyes[0], link.picture_size()?, (p.x, p.y)))
    }

    /// Where a hand is: resting in front of the head, and while the wearer is using their pointer,
    /// turned to point where it does -- so the game's own laser from that hand lands on what the
    /// wearer's pointer is on.
    pub fn hand_world(&self, hand: crate::input::Hand, time_ns: i64) -> Pose {
        let sample = self.sample_at(time_ns);
        // The gyro, where a layout aims the right hand with it, comes before the pointer: it is
        // the more deliberate of the two.
        if hand == crate::input::Hand::Right {
            let aim = self.input.lock().unwrap().state.hand;
            if let Some(pose) = crate::input::aim_hand_by(hand, &sample.head, aim) {
                return pose;
            }
        }
        match self.pointer_ray(&sample, crate::time::now_ns()) {
            Some((origin, direction)) => {
                let target = crate::input::pointer_target(origin, direction, &self.run.lock().unwrap().quads);
                let distance = target.map_or(POINTER_REACH, |(t, _)| t);
                {
                    static LAST: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
                    let now = crate::time::now_ns();
                    if now - LAST.load(std::sync::atomic::Ordering::Relaxed) > 2_000_000_000 {
                        LAST.store(now, std::sync::atomic::Ordering::Relaxed);
                        let at = self.link.as_ref().map(|l| l.pointer()).unwrap_or_default();
                        let quads = self.run.lock().unwrap().quads.clone();
                        let shown = quads
                            .iter()
                            .map(|q| format!("{:.1}x{:.1} m at ({:.2}, {:.2}, {:.2})", q.shown_size[0], q.shown_size[1], q.shown.position.x, q.shown.position.y, q.shown.position.z))
                            .collect::<Vec<_>>()
                            .join("; ");
                        log::debug!(
                            "the hands aim at the pointer ({:.0}, {:.0}), {distance:.2} m along its ray; {} quads shown: {shown}; eye at ({:.2}, {:.2}, {:.2}), ray ({:.2}, {:.2}, {:.2})",
                            at.x, at.y, quads.len(), origin.x, origin.y, origin.z, direction.x, direction.y, direction.z
                        );
                    }
                }
                // At the place on the game's own panel that the pointer is on, not on the panel as
                // it is shown.
                let at = target.map_or(origin + direction * distance, |(_, point)| point);
                crate::input::aim_hand_at(hand, &sample.head, at)
            }
            None => crate::input::hand_pose(hand, &sample.head),
        }
    }

    pub fn now_history(&self) -> History {
        match &self.link {
            Some(link) if link.has_poses() => link.history(),
            _ => History::new(vec![fixed_sample(crate::time::now_ns())]),
        }
    }

    /// The head and eyes at `time_ns`, as the glasses have them. Everything the game is shown -- its
    /// views, its spaces, its hands, the layers composed for it -- is made from this one.
    pub fn sample_at(&self, time_ns: i64) -> Sample {
        let history = self.now_history();
        history.at(time_ns).unwrap_or_else(|| fixed_sample(time_ns))
    }

    /// Where a space is in the world the compositor's poses are expressed in.
    pub fn world_from(&self, space: &Space, time_ns: i64) -> Option<Pose> {
        let base = match &space.kind {
            SpaceKind::View => self.sample_at(time_ns).head,
            SpaceKind::Local => self.local_origin(),
            SpaceKind::Stage => self.local_origin().then(&Pose {
                orientation: glam::Quat::IDENTITY,
                position: glam::Vec3::new(0.0, -FLOOR_BELOW_HEAD, 0.0),
            }),
            SpaceKind::Action => {
                // A hand: wherever the controller stands in for it, which is in front of the head.
                let (action, subaction) = space.action?;
                let hand = self.hand_of(action, subaction)?;
                self.hand_world(hand, time_ns)
            }
        };
        Some(base.then(&space.offset))
    }

    /// Which hand an action's pose is: by the subaction path it was asked for, or by where the
    /// application bound it.
    pub fn hand_of(&self, action: u64, subaction: u64) -> Option<crate::input::Hand> {
        use crate::input::{split, Hand};
        let from_path = |text: &str| {
            if text.starts_with("/user/hand/left") {
                Some(Hand::Left)
            } else if text.starts_with("/user/hand/right") {
                Some(Hand::Right)
            } else {
                None
            }
        };
        if subaction != 0 {
            if let Some(hand) = self.instance.path_text(subaction).as_deref().and_then(from_path) {
                return Some(hand);
            }
        }
        self.instance
            .bindings_of(action)
            .iter()
            .find_map(|p| split(p).and_then(|(hand, name)| crate::input::pose_kind(name).map(|_| hand)))
    }

    pub fn local_origin(&self) -> Pose {
        let mut run = self.run.lock().unwrap();
        if let Some(origin) = run.local_origin {
            return origin;
        }
        // Taken from the first pose that actually exists, so a session started before the
        // compositor has written one does not fix its origin at a made-up place.
        let history = self.now_history();
        let head = history.newest().map(|s| s.head);
        let origin = head.map(|h| h.level()).unwrap_or(Pose::IDENTITY);
        if self.link.as_ref().is_none_or(|l| l.has_poses()) {
            run.local_origin = Some(origin);
        }
        origin
    }

    /// Say the session is to stop, by the states in between: a session that has the focus gives it up,
    /// stops being visible, and only then is stopping.
    pub fn push_stopping(&self) {
        let state = self.run.lock().unwrap().state;
        if state == SessionState::Focused {
            self.push_state(SessionState::Visible);
        }
        if matches!(state, SessionState::Ready | SessionState::Focused | SessionState::Visible) {
            self.push_state(SessionState::Synchronized);
        }
        self.push_state(SessionState::Stopping);
    }

    pub fn push_state(&self, state: SessionState) {
        self.run.lock().unwrap().state = state;
        self.instance.events.lock().unwrap().push_back(Event::SessionState {
            session: self.handle,
            state,
            time_ns: crate::time::now_ns(),
        });
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Frames first: their images and buffers belong to the device `vk` owns.
        if let Some(vk) = &self.vk {
            if let Some(output) = self.output.lock().unwrap().take() {
                for frame in &output.frames {
                    // SAFETY: the images were made by this device and are no longer in use: the
                    // session is being destroyed, and `Vk` waits for the device to be idle.
                    unsafe { vk.destroy_image(&frame.target.image) };
                }
            }
        }
    }
}

/// A head that is nowhere in particular, for a session with no tracking to report.
pub fn fixed_sample(time_ns: i64) -> Sample {
    use crate::pose::Eye;
    let eye = |x: f32| Eye {
        pose: Pose {
            orientation: glam::Quat::IDENTITY,
            position: glam::Vec3::new(x, 0.0, 0.0),
        },
        fov: [-0.7, 0.7, 0.5, -0.5],
    };
    Sample {
        token: 0,
        sample_ns: time_ns,
        predicted_ns: time_ns,
        head: Pose::IDENTITY,
        eyes: [eye(-0.032), eye(0.032)],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceKind {
    View,
    Local,
    Stage,
    Action,
}

pub struct Space {
    pub session: u64,
    pub kind: SpaceKind,
    pub offset: Pose,
    /// For an action space: the action and the subaction path it was made for.
    pub action: Option<(u64, u64)>,
}

pub struct Swapchain {
    pub session: u64,
    pub format: ash::vk::Format,
    pub width: u32,
    pub height: u32,
    pub array_size: u32,
    pub images: Vec<Image>,
    pub state: Mutex<SwapState>,
    /// The OpenGL side of an OpenGL swapchain: a texture for each image, and what backs it.
    pub gl: Option<GlSwapchain>,
}

pub struct GlSwapchain {
    /// What the application draws into: one texture per image, an array if it asked for layers.
    pub textures: Vec<u32>,
    pub array: bool,
    pub layers: u32,
    /// For each image, for each layer, the staging texture Vulkan reads and its memory object.
    pub staging: Vec<Vec<(u32, u32)>>,
}

impl Swapchain {
    /// How many images the application cycles through.
    pub fn count(&self) -> usize {
        self.gl.as_ref().map_or(self.images.len(), |g| g.textures.len())
    }

    /// Where the application leaves an image: Vulkan's colour attachment layout, or `GENERAL` for
    /// OpenGL's, which Vulkan never transitioned.
    pub fn layout(&self) -> ash::vk::ImageLayout {
        if self.gl.is_some() {
            ash::vk::ImageLayout::GENERAL
        } else {
            ash::vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
        }
    }

    pub fn is_gl(&self) -> bool {
        self.gl.is_some()
    }

    /// The Vulkan image that holds image `index`'s layer `layer`, and the layer to read in it.
    pub fn vulkan_image(&self, index: u32, layer: u32) -> Option<(ash::vk::Image, u32)> {
        match &self.gl {
            Some(side) => {
                let n = side.layers as usize;
                self.images.get(index as usize * n + (layer as usize).min(n - 1)).map(|i| (i.image, 0))
            }
            None => self.images.get(index as usize).map(|i| (i.image, layer)),
        }
    }
}

/// Which of a swapchain's images is whose.
#[derive(Default)]
pub struct SwapState {
    pub next: u32,
    /// Handed to the application and not yet released, oldest first.
    pub acquired: VecDeque<u32>,
    /// The image released most recently: the one a frame is made from.
    pub released: Option<u32>,
    /// The oldest acquired image has been waited on: it has to be released before another is.
    pub waited: bool,
    /// A static swapchain: one image, acquired once, which is then the picture for good.
    pub is_static: bool,
    pub static_acquired: bool,
}

pub struct ActionSet {
    pub instance: u64,
    pub name: String,
    pub localized: String,
}

pub struct Action {
    pub set: u64,
    pub name: String,
    pub localized: String,
    pub kind: openxr_sys::ActionType,
    pub subaction_paths: Vec<u64>,
}
