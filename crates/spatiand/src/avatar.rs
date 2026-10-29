//! The assistant's figure in the room (`docs/ai-integrations.md`).
//!
//! **Where**: fixed to the view, low in the corner on the other side from the pinned windows, a
//! metre and a third away -- and following the head lazily rather than rigidly, so it drifts
//! after a turn of the head the way something that floats would, instead of being painted on
//! the glass.
//!
//! **What**: by default a small robot drawn from a few shapes -- a head with a visor and two
//! eyes, a body, two arms, an antenna -- made here rather than loaded, so the default needs no
//! file and no licence. The wearer's own figures (VRM, glTF) come through the same path.
//!
//! **What it is doing, at a glance** (the owner asked for it, 2026-10-29): each step of an answer
//! has its own gesture, so that a slow one is seen to be working rather than stuck. Listening for
//! its name it stands easy, its antenna glowing with the room's sound; in a conversation it leans
//! in, the lights at its ears lit -- no name is needed then. Hearing the wearer it cups a hand to
//! its ear, the sound drawn in as dots; while what was said is written down, its eyes narrow to a
//! line running along it and three amber dots blink over its head; the small model thinking, a
//! hand goes to its chin and a blue spark circles its head; the larger one, arms folded, three
//! dots circle slowly; working a window, it types; talking, its mouth moves and a hand gestures.
//! It smiles when an answer is heard out, waves when called, and dozes when not listening. A
//! wearer's own figure shows the same with its expressions and a light over its head. Poses
//! ease from one to the next.
//!
//! **How**, given that the room has no depth buffer (`scene.rs`): the figure is drawn for each eye
//! into a small picture of its own, with depth, through just the part of that eye's view it
//! covers -- an off-axis crop of the eye's own projection -- and that picture is then laid into
//! the eye as one quad. True stereo, correct self-occlusion, and a few thousand pixels per eye.

use std::cell::RefCell;
use std::time::Instant;

use glam::{DQuat, DVec3, Mat3, Mat4, Vec3, Vec4};
use smithay::backend::renderer::gles::ffi;
use spatiand_render::{Eye, EyeSide};

/// The figure's picture, per eye, in pixels square.
const PICTURE: i32 = 512;
/// How far from the eyes it floats, metres.
const DISTANCE_M: f64 = 1.35;
/// How long it takes to catch up with the head, seconds (to about two thirds of the way).
const FOLLOW_S: f32 = 0.45;
/// How big the figure is: its shapes are in metres at 1, about 22 cm tall, and that filled
/// half the view.
const SIZE: f32 = 0.58;
/// The figure's bounding sphere, from its middle, metres: what is cropped to -- the signs over
/// its head and the dots at its ear included.
const REACH_M: f32 = 0.175 * SIZE;
/// Where its middle is, above where it stands, metres.
const MIDDLE_M: f64 = 0.08 * SIZE as f64;
/// Its height, metres, for keeping all of it in view.
const TALL_M: f64 = 0.33 * SIZE as f64;

const VERT: &str = r#"
layout(location = 0) in vec3 a_pos;
layout(location = 1) in vec3 a_normal;
uniform mat4 u_mvp;
uniform mat4 u_model;
uniform mat3 u_normal;
out vec3 v_normal;
out vec3 v_world;
void main() {
    v_normal = normalize(u_normal * a_normal);
    v_world = (u_model * vec4(a_pos, 1.0)).xyz;
    gl_Position = u_mvp * vec4(a_pos, 1.0);
}
"#;

const FRAG: &str = r#"
in vec3 v_normal;
in vec3 v_world;
uniform vec3 u_color;
uniform vec3 u_emissive;
uniform vec3 u_light;
uniform vec3 u_eye;
uniform float u_gloss;
out vec4 f_color;
void main() {
    vec3 n = normalize(v_normal);
    vec3 v = normalize(u_eye - v_world);
    vec3 l = normalize(u_light);
    float diffuse = max(dot(n, l), 0.0);
    vec3 h = normalize(l + v);
    float spec = pow(max(dot(n, h), 0.0), 48.0) * u_gloss;
    // A cool rim, as the room's glass has: the figure reads against a dark room or a bright one.
    float rim = pow(1.0 - max(dot(n, v), 0.0), 3.0);
    vec3 c = u_color * (0.36 + 0.64 * diffuse) + vec3(spec) + rim * 0.25 * vec3(0.55, 0.85, 1.0) + u_emissive;
    f_color = vec4(c, 1.0);
}
"#;

/// A figure from a file: its own colours and pictures, lit softly -- figures made for VRM are
/// painted to be seen nearly flat, and a strong light makes them look like plastic.
const MODEL_VERT: &str = r#"
layout(location = 0) in vec3 a_pos;
layout(location = 1) in vec3 a_normal;
layout(location = 2) in vec2 a_uv;
layout(location = 3) in vec4 a_color;
uniform mat4 u_mvp;
uniform mat4 u_model;
uniform mat3 u_normal;
out vec3 v_normal;
out vec3 v_world;
out vec2 v_uv;
out vec4 v_color;
void main() {
    v_normal = normalize(u_normal * a_normal);
    v_world = (u_model * vec4(a_pos, 1.0)).xyz;
    v_uv = a_uv;
    v_color = a_color;
    gl_Position = u_mvp * vec4(a_pos, 1.0);
}
"#;

const MODEL_FRAG: &str = r#"
in vec3 v_normal;
in vec3 v_world;
in vec2 v_uv;
in vec4 v_color;
uniform sampler2D u_tex;
uniform vec4 u_base;
uniform float u_cutoff;
uniform vec3 u_light;
uniform vec3 u_eye;
out vec4 f_color;
void main() {
    vec4 base = texture(u_tex, v_uv) * u_base * v_color;
    if (base.a < u_cutoff) {
        discard;
    }
    vec3 n = normalize(v_normal);
    vec3 v = normalize(u_eye - v_world);
    // Seen from either side: a figure's skirts and sleeves are one surface thick.
    if (dot(n, v) < 0.0) {
        n = -n;
    }
    float diffuse = max(dot(n, normalize(u_light)), 0.0);
    float rim = pow(1.0 - max(dot(n, v), 0.0), 3.0);
    vec3 c = base.rgb * (0.66 + 0.34 * diffuse) + rim * 0.12 * vec3(0.55, 0.85, 1.0);
    f_color = vec4(c, 1.0);
}
"#;

/// The picture laid into the eye: premultiplied, as it was cleared to nothing and drawn opaque.
const LAY_VERT: &str = r#"
layout(location = 0) in vec2 a_pos;
uniform mat4 u_mvp;
out vec2 v_uv;
void main() {
    v_uv = a_pos + 0.5;
    gl_Position = u_mvp * vec4(a_pos, 0.0, 1.0);
}
"#;

const LAY_FRAG: &str = r#"
in vec2 v_uv;
uniform sampler2D u_tex;
uniform float u_alpha;
out vec4 f_color;
void main() {
    f_color = texture(u_tex, v_uv) * u_alpha;
}
"#;

/// What the figure is doing, as the assistant says.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mood {
    /// Not listening.
    #[default]
    Idle,
    /// Listening for its name.
    Listening,
    /// Out of a conversation, something said is being written to see if it begins with its name.
    Checking,
    /// In a conversation: what comes next needs no name.
    Attentive,
    /// The wearer is speaking to it.
    Hearing,
    /// What they said is being written down.
    Transcribing,
    /// The small model is answering.
    Thinking,
    /// The larger model is thinking.
    Pondering,
    /// Doing something in the room.
    Working,
    Talking,
    Happy,
}

impl Mood {
    fn of(name: &str) -> Mood {
        match name {
            "listening" => Mood::Listening,
            "checking" => Mood::Checking,
            "attentive" => Mood::Attentive,
            "hearing" => Mood::Hearing,
            "transcribing" => Mood::Transcribing,
            "thinking" => Mood::Thinking,
            "pondering" => Mood::Pondering,
            "working" => Mood::Working,
            "talking" => Mood::Talking,
            "happy" => Mood::Happy,
            _ => Mood::Idle,
        }
    }
}

/// How long a pose takes to become the next mood's, seconds (to about two thirds of the way).
const EASE_S: f32 = 0.16;

/// Where the robot holds itself in a mood, eased from one to the next; what moves on top of it --
/// breathing, typing, a gesture -- is added as it is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pose {
    /// The body tipped towards the wearer (+), radians.
    lean: f32,
    /// The head bowed (+) or raised, and tilted to its right (+), radians.
    nod: f32,
    tilt: f32,
    /// Where each hand is (its left, its right), from its shoulder, in the robot's metres: x
    /// towards the wearer, y to its left, z up. An arm reaches about 0.075.
    hands: [Vec3; 2],
    /// The eyes: how open (1 as usual), how far up and to its left they look.
    open: f32,
    look_up: f32,
    look_side: f32,
}

impl Pose {
    fn of(mood: Mood) -> Pose {
        let down = |out: f32| [Vec3::new(0.0, out, -0.074), Vec3::new(0.0, -out, -0.074)];
        let rest = Pose { lean: 0.0, nod: 0.03, tilt: 0.0, hands: down(0.016), open: 1.0, look_up: 0.0, look_side: 0.0 };
        match mood {
            Mood::Idle => Pose { nod: 0.07, hands: down(0.008), open: 0.42, look_up: -0.002, ..rest },
            Mood::Listening | Mood::Checking => rest,
            Mood::Attentive => Pose { lean: 0.06, nod: 0.10, tilt: 0.05, hands: [Vec3::new(0.03, 0.015, -0.068), Vec3::new(0.03, -0.015, -0.068)], open: 1.12, ..rest },
            // Leaning in, a hand cupped to the ear on its right, the head tipped to it.
            Mood::Hearing => Pose { lean: 0.10, nod: 0.12, tilt: 0.2, hands: [Vec3::new(0.03, 0.015, -0.068), Vec3::new(0.012, -0.016, 0.079)], open: 1.15, ..rest },
            // Writing it down: a hand at work over the other.
            Mood::Transcribing => Pose { lean: 0.04, nod: 0.10, hands: [Vec3::new(0.058, -0.012, -0.044), Vec3::new(0.06, 0.008, -0.04)], open: 0.36, ..rest },
            // A quick thought: a hand to its chin, looking up and aside.
            Mood::Thinking => Pose { lean: -0.02, nod: -0.13, tilt: -0.12, hands: [Vec3::new(0.052, -0.05, 0.036), Vec3::new(0.0, -0.016, -0.074)], open: 0.9, look_up: 0.006, look_side: 0.007, ..rest },
            // A long one: arms folded, head back.
            Mood::Pondering => Pose { lean: -0.04, nod: -0.19, tilt: 0.08, hands: [Vec3::new(0.05, -0.058, -0.012), Vec3::new(0.052, 0.058, -0.018)], open: 0.8, look_up: 0.008, ..rest },
            Mood::Working => Pose { lean: 0.05, nod: 0.11, hands: [Vec3::new(0.062, -0.02, -0.036), Vec3::new(0.062, 0.02, -0.036)], open: 0.75, ..rest },
            Mood::Talking => Pose { lean: 0.02, nod: 0.0, hands: [Vec3::new(0.0, 0.016, -0.074), Vec3::new(0.038, -0.02, -0.062)], ..rest },
            Mood::Happy => Pose { nod: -0.04, hands: [Vec3::new(0.01, 0.045, 0.058), Vec3::new(0.01, -0.045, 0.058)], ..rest },
        }
    }

    /// Part of the way to `to`.
    fn ease(&mut self, to: &Pose, dt: f32) {
        let k = 1.0 - (-dt / EASE_S).exp();
        let step = |a: f32, b: f32| a + (b - a) * k;
        self.lean = step(self.lean, to.lean);
        self.nod = step(self.nod, to.nod);
        self.tilt = step(self.tilt, to.tilt);
        for i in 0..2 {
            self.hands[i] += (to.hands[i] - self.hands[i]) * k;
        }
        self.open = step(self.open, to.open);
        self.look_up = step(self.look_up, to.look_up);
        self.look_side = step(self.look_side, to.look_side);
    }
}

struct Mesh {
    vao: u32,
    count: i32,
}

struct Live {
    /// Where it is, chasing where it should be.
    at: Option<DVec3>,
    last: Instant,
    /// Each eye's crop of the view, `(x0, x1, y0, y1)` in its normalised device coordinates.
    rects: [Option<[f32; 4]>; 2],
    /// How much of it there is, 0 to 1: it fades in and out rather than popping.
    shown: f32,
    mood: Mood,
    /// When the mood began, for the moods that play out (happy, a wave).
    mood_since: Instant,
    waves_seen: u64,
    wave_since: Option<Instant>,
    pose: Pose,
    /// How loud what it hears is, 0 to 1, quick to rise and slow to fall.
    level: f32,
}

pub struct Avatar {
    program: u32,
    loc: [i32; 8],
    model_program: u32,
    model_loc: [i32; 9],
    white: u32,
    /// The wearer's own figure, when one is chosen: read, being read, or not readable.
    chosen: RefCell<Chosen>,
    lay_program: u32,
    lay_loc: [i32; 3],
    lay_quad: u32,
    sphere: Mesh,
    cylinder: Mesh,
    fbo: u32,
    depth: u32,
    pictures: [u32; 2],
    started: Instant,
    live: RefCell<Live>,
}

impl Avatar {
    /// # Safety
    /// The GL context must be current.
    pub unsafe fn new(gl: &ffi::Gles2) -> Result<Avatar, String> {
        let program = crate::gl::link(gl, VERT, FRAG)?;
        let at = |p: u32, s: &str| gl.GetUniformLocation(p, std::ffi::CString::new(s).unwrap().as_ptr());
        let loc = ["u_mvp", "u_model", "u_normal", "u_color", "u_emissive", "u_light", "u_eye", "u_gloss"].map(|n| at(program, n));
        let model_program = crate::gl::link(gl, MODEL_VERT, MODEL_FRAG)?;
        let model_loc = ["u_mvp", "u_model", "u_normal", "u_tex", "u_base", "u_cutoff", "u_light", "u_eye", "u_unused"].map(|n| at(model_program, n));
        let white = crate::gl::white_texture(gl);
        let lay_program = crate::gl::link(gl, LAY_VERT, LAY_FRAG)?;
        let lay_loc = ["u_mvp", "u_tex", "u_alpha"].map(|n| at(lay_program, n));
        let lay_quad = quad(gl);
        let sphere = upload(gl, &sphere_mesh(14, 22));
        let cylinder = upload(gl, &cylinder_mesh(16));
        let mut pictures = [0u32; 2];
        gl.GenTextures(2, pictures.as_mut_ptr());
        for t in pictures {
            gl.BindTexture(ffi::TEXTURE_2D, t);
            gl.TexImage2D(ffi::TEXTURE_2D, 0, ffi::RGBA8 as i32, PICTURE, PICTURE, 0, ffi::RGBA, ffi::UNSIGNED_BYTE, std::ptr::null());
            // Drawn larger than it is shown and shrunk through its mipmaps: smooth edges without
            // a multisampled target.
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR_MIPMAP_LINEAR as i32);
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, ffi::LINEAR as i32);
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE as i32);
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE as i32);
        }
        let mut depth = 0;
        gl.GenRenderbuffers(1, &mut depth);
        gl.BindRenderbuffer(ffi::RENDERBUFFER, depth);
        gl.RenderbufferStorage(ffi::RENDERBUFFER, ffi::DEPTH_COMPONENT16, PICTURE, PICTURE);
        let mut fbo = 0;
        gl.GenFramebuffers(1, &mut fbo);
        Ok(Avatar {
            program,
            loc,
            model_program,
            model_loc,
            white,
            chosen: RefCell::new(Chosen::None),
            lay_program,
            lay_loc,
            lay_quad,
            sphere,
            cylinder,
            fbo,
            depth,
            pictures,
            started: Instant::now(),
            live: RefCell::new(Live {
                at: None,
                last: Instant::now(),
                rects: [None, None],
                shown: 0.0,
                mood: Mood::Idle,
                mood_since: Instant::now(),
                waves_seen: 0,
                wave_since: None,
                pose: Pose::of(Mood::Idle),
                level: 0.0,
            }),
        })
    }

    /// Move with the head, take the assistant's mood, and draw the figure for each eye into its
    /// picture. Before the eyes are drawn: the room's own target is bound again after.
    ///
    /// # Safety
    /// The GL context must be current.
    pub unsafe fn prepare(&self, gl: &ffi::Gles2, head: DQuat, eyes: &[Eye], fov_deg: (f64, f64), pinned_corner: spatiand_render::pip::Corner) {
        let mut live = self.live.borrow_mut();
        let now = Instant::now();
        let dt = now.duration_since(live.last).as_secs_f32().min(0.1);
        live.last = now;

        // What the assistant says of itself.
        let figure = crate::assist::FIGURE.lock().unwrap().clone().unwrap_or_default();
        let wanted = crate::assist::present() && figure.get("visible").and_then(|v| v.as_bool()) != Some(false);
        // What it is doing by name, from an assistant that names it; else one of the five moods.
        let named = figure.get("state").or_else(|| figure.get("mood")).and_then(|m| m.as_str()).unwrap_or("idle");
        let mood = Mood::of(named);
        if mood != live.mood {
            live.mood = mood;
            live.mood_since = now;
        }
        let heard = crate::assist::heard_level();
        let rate = if heard > live.level { 0.04 } else { 0.25 };
        live.level += (heard - live.level) * (1.0 - (-dt / rate).exp());
        // Coming into view, it is already as its mood holds it.
        let target = Pose::of(mood);
        if live.at.is_none() {
            live.pose = target;
            live.level = heard;
        }
        live.pose.ease(&target, dt);
        let waves = figure.get("waves").and_then(|w| w.as_u64()).unwrap_or(0);
        if waves != live.waves_seen {
            live.waves_seen = waves;
            live.wave_since = Some(now);
        }
        // Faded in and out as it is shown and hidden; there at once the first time.
        live.shown = if wanted && live.at.is_none() { 1.0 } else { (live.shown + if wanted { dt * 3.0 } else { -dt * 3.0 }).clamp(0.0, 1.0) };
        if live.shown <= 0.0 {
            live.rects = [None, None];
            live.at = None;
            return;
        }

        // Where it should be: low on the side away from the pinned windows.
        let (left, _) = pinned_corner.signs();
        let side = -left;
        let half = (TALL_M * 0.5 / DISTANCE_M).atan().to_degrees();
        let mut yaw = side * (fov_deg.0 * 0.5 - 2.5 - half).max(3.0);
        let mut pitch = -(fov_deg.1 * 0.5 - 1.5 - half).max(1.0);
        let place = |yaw: f64, pitch: f64| {
            let (y, p) = (yaw.to_radians(), pitch.to_radians());
            head * DVec3::new(p.cos() * y.cos(), p.cos() * y.sin(), p.sin()) * DISTANCE_M
        };
        // **All of it inside the view**, measured through the eye itself rather than trusted to
        // the field of view's numbers: what pokes out is brought in by as much.
        if let Some(eye) = eyes.first() {
            let vp = eye.view_projection();
            for _ in 0..3 {
                let middle = (place(yaw, pitch) + head * DVec3::new(0.0, 0.0, MIDDLE_M)).as_vec3();
                let Some([x0, x1, y0, y1]) = crop_of(&vp, middle, REACH_M * 0.85) else { break };
                // Which way is down in this eye's coordinates: the Deck draws its eyes flipped.
                let below = middle - (head * DVec3::Z).as_vec3() * 0.1;
                let ndc = |p: Vec3| {
                    let c = vp * p.extend(1.0);
                    c.y / c.w
                };
                let down_is_negative = ndc(below) < ndc(middle);
                let over_x = if side > 0.0 { -0.97 - x0 } else { x1 - 0.97 };
                let over_y = if down_is_negative { -0.95 - y0 } else { y1 - 0.95 };
                if over_x <= 0.0 && over_y <= 0.0 {
                    break;
                }
                yaw -= side * over_x.max(0.0) as f64 * fov_deg.0 * 0.5;
                pitch += over_y.max(0.0) as f64 * fov_deg.1 * 0.5;
            }
        }
        let target = place(yaw, pitch);
        let at = match live.at {
            Some(at) => at + (target - at) * (1.0 - (-dt / FOLLOW_S).exp()) as f64,
            None => target,
        };
        live.at = Some(at);

        let t = now.duration_since(self.started).as_secs_f32();
        let path = figure.get("path").and_then(|p| p.as_str()).map(str::to_string);
        self.choose(gl, path);
        // A figure of the wearer's, placed and posed as the robot would be, with only the robot's
        // signs of what it is doing over its head.
        let parts = if matches!(&*self.chosen.borrow(), Chosen::Ready { .. }) { self.signs_over(at, t, &live) } else { self.figure_parts(at, t, &live) };
        let mut chosen = self.chosen.borrow_mut();
        let model = match &mut *chosen {
            Chosen::Ready { gpu, .. } => {
                let to_eye = (-at).as_vec3();
                let face_yaw = to_eye.y.atan2(to_eye.x);
                let bob = 0.006 * SIZE * (t * 1.6).sin();
                let place = Mat4::from_translation(at.as_vec3() + Vec3::new(0.0, 0.0, bob)) * Mat4::from_rotation_z(face_yaw) * gpu.fit;
                gpu.pose(gl, &live, t);
                Some((place, &*gpu))
            }
            _ => None,
        };

        // The room's target, to put back.
        let mut previous_fbo = 0;
        gl.GetIntegerv(ffi::FRAMEBUFFER_BINDING, &mut previous_fbo);
        let mut viewport = [0i32; 4];
        gl.GetIntegerv(ffi::VIEWPORT, viewport.as_mut_ptr());

        gl.BindFramebuffer(ffi::FRAMEBUFFER, self.fbo);
        gl.FramebufferRenderbuffer(ffi::FRAMEBUFFER, ffi::DEPTH_ATTACHMENT, ffi::RENDERBUFFER, self.depth);
        gl.Viewport(0, 0, PICTURE, PICTURE);
        gl.Disable(ffi::BLEND);
        gl.Disable(ffi::SCISSOR_TEST);
        gl.Enable(ffi::DEPTH_TEST);
        gl.DepthFunc(ffi::LESS);
        gl.DepthMask(ffi::TRUE);
        gl.UseProgram(self.program);
        let light = Vec3::new(0.55, 0.6, 0.58).normalize();
        gl.Uniform3f(self.loc[5], light.x, light.y, light.z);
        let middle = (at + head * DVec3::new(0.0, 0.0, MIDDLE_M)).as_vec3();
        for (index, eye) in eyes.iter().enumerate().take(2) {
            let slot = match eye.side {
                EyeSide::Left => 0,
                EyeSide::Right => 1,
            };
            let vp = eye.view_projection();
            let Some(rect) = crop_of(&vp, middle, REACH_M) else {
                live.rects[slot] = None;
                continue;
            };
            live.rects[slot] = Some(rect);
            let crop = crop_matrix(rect);
            gl.FramebufferTexture2D(ffi::FRAMEBUFFER, ffi::COLOR_ATTACHMENT0, ffi::TEXTURE_2D, self.pictures[slot], 0);
            gl.ClearColor(0.0, 0.0, 0.0, 0.0);
            gl.ClearDepthf(1.0);
            gl.Clear(ffi::COLOR_BUFFER_BIT | ffi::DEPTH_BUFFER_BIT);
            let eye_at = eye.position.as_vec3();
            if let Some((place, gpu)) = &model {
                self.draw_model(gl, gpu, crop * vp, *place, light, eye_at);
                gl.UseProgram(self.program);
            }
            gl.Uniform3f(self.loc[6], eye_at.x, eye_at.y, eye_at.z);
            for part in &parts {
                let mesh = if part.cylinder { &self.cylinder } else { &self.sphere };
                let mvp = crop * vp * part.model;
                let normal = Mat3::from_mat4(part.model).inverse().transpose();
                gl.UniformMatrix4fv(self.loc[0], 1, ffi::FALSE, mvp.to_cols_array().as_ptr());
                gl.UniformMatrix4fv(self.loc[1], 1, ffi::FALSE, part.model.to_cols_array().as_ptr());
                gl.UniformMatrix3fv(self.loc[2], 1, ffi::FALSE, normal.to_cols_array().as_ptr());
                gl.Uniform3f(self.loc[3], part.color.x, part.color.y, part.color.z);
                gl.Uniform3f(self.loc[4], part.emissive.x, part.emissive.y, part.emissive.z);
                gl.Uniform1f(self.loc[7], part.gloss);
                gl.BindVertexArray(mesh.vao);
                gl.DrawElements(ffi::TRIANGLES, mesh.count, ffi::UNSIGNED_SHORT, std::ptr::null());
            }
            gl.BindTexture(ffi::TEXTURE_2D, self.pictures[slot]);
            gl.GenerateMipmap(ffi::TEXTURE_2D);
            let _ = index;
        }
        gl.BindVertexArray(0);
        gl.Disable(ffi::DEPTH_TEST);
        gl.BindFramebuffer(ffi::FRAMEBUFFER, previous_fbo as u32);
        gl.Viewport(viewport[0], viewport[1], viewport[2], viewport[3]);
        gl.Enable(ffi::BLEND);
    }

    /// Wait, up to `timeout`, for a figure being read: for a test's single picture, which would
    /// otherwise be taken before the file is read.
    pub fn settle_figure(&self, timeout: std::time::Duration) {
        let mut chosen = self.chosen.borrow_mut();
        if let Chosen::Reading { path, rx } = &*chosen {
            if let Ok(result) = rx.recv_timeout(timeout) {
                *chosen = Chosen::Read { path: path.clone(), result: Some(result) };
            }
        }
    }

    /// Where the figure is in the room, while it is shown: where its voice comes from.
    pub fn position(&self) -> Option<DVec3> {
        let live = self.live.borrow();
        live.at.filter(|_| live.shown > 0.0)
    }

    /// Lay the figure's picture into this eye, after the windows: it floats nearer than they do.
    ///
    /// # Safety
    /// The GL context must be current.
    pub unsafe fn draw(&self, gl: &ffi::Gles2, eye: &Eye) {
        let live = self.live.borrow();
        let slot = match eye.side {
            EyeSide::Left => 0,
            EyeSide::Right => 1,
        };
        let Some([x0, x1, y0, y1]) = live.rects[slot] else { return };
        let mvp = Mat4::from_translation(Vec3::new((x0 + x1) * 0.5, (y0 + y1) * 0.5, 0.0)) * Mat4::from_scale(Vec3::new(x1 - x0, y1 - y0, 1.0));
        gl.UseProgram(self.lay_program);
        gl.Enable(ffi::BLEND);
        gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
        gl.ActiveTexture(ffi::TEXTURE0);
        gl.BindTexture(ffi::TEXTURE_2D, self.pictures[slot]);
        gl.Uniform1i(self.lay_loc[1], 0);
        gl.Uniform1f(self.lay_loc[2], live.shown);
        gl.UniformMatrix4fv(self.lay_loc[0], 1, ffi::FALSE, mvp.to_cols_array().as_ptr());
        gl.BindVertexArray(self.lay_quad);
        gl.DrawArrays(ffi::TRIANGLES, 0, 6);
        gl.BindVertexArray(0);
        // What every other draw expects.
        gl.BlendFunc(ffi::SRC_ALPHA, ffi::ONE_MINUS_SRC_ALPHA);
    }

    /// Read the wearer's chosen figure when it changes, and once read, put it on the GPU. `None`
    /// is the robot.
    unsafe fn choose(&self, gl: &ffi::Gles2, path: Option<String>) {
        let mut chosen = self.chosen.borrow_mut();
        let current = match &*chosen {
            Chosen::None => None,
            Chosen::Reading { path, .. } | Chosen::Read { path, .. } | Chosen::Ready { path, .. } | Chosen::Unreadable { path } => Some(path.clone()),
        };
        if current != path {
            // The figure worn until now gives back what it held: a VRM's pictures are tens of
            // megabytes.
            if let Chosen::Ready { gpu, .. } = &*chosen {
                gpu.free(gl);
            }
            *chosen = match path {
                None => Chosen::None,
                Some(path) => {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let file = path.clone();
                    std::thread::Builder::new()
                        .name("figure".into())
                        .spawn(move || {
                            let _ = tx.send(crate::figure::read(std::path::Path::new(&file)));
                        })
                        .ok();
                    log::info!("avatar: reading the figure {path}");
                    Chosen::Reading { path, rx }
                }
            };
            return;
        }
        let read = match &mut *chosen {
            Chosen::Reading { rx, .. } => rx.try_recv().ok(),
            Chosen::Read { result, .. } => result.take(),
            _ => None,
        };
        if let (Some(read), Chosen::Reading { path, .. } | Chosen::Read { path, .. }) = (read, &*chosen) {
            let path = path.clone();
            *chosen = match read {
                Ok(figure) => {
                    log::info!("avatar: {path} read: {} parts, {} pictures, expressions {:?}", figure.parts.len(), figure.pictures.len(), figure.expressions.keys().collect::<Vec<_>>());
                    Chosen::Ready { path, gpu: ModelGpu::upload(gl, figure) }
                }
                Err(why) => {
                    log::warn!("avatar: the figure {path} could not be read: {why}; the robot stays");
                    Chosen::Unreadable { path }
                }
            };
        }
    }

    unsafe fn draw_model(&self, gl: &ffi::Gles2, gpu: &ModelGpu, vp: Mat4, place: Mat4, light: Vec3, eye: Vec3) {
        gl.UseProgram(self.model_program);
        // A part with no colours painted on it is white where they would be.
        gl.VertexAttrib4f(3, 1.0, 1.0, 1.0, 1.0);
        let normal = Mat3::from_mat4(place).inverse().transpose();
        let mvp = vp * place;
        gl.UniformMatrix4fv(self.model_loc[0], 1, ffi::FALSE, mvp.to_cols_array().as_ptr());
        gl.UniformMatrix4fv(self.model_loc[1], 1, ffi::FALSE, place.to_cols_array().as_ptr());
        gl.UniformMatrix3fv(self.model_loc[2], 1, ffi::FALSE, normal.to_cols_array().as_ptr());
        gl.Uniform1i(self.model_loc[3], 0);
        gl.Uniform3f(self.model_loc[6], light.x, light.y, light.z);
        gl.Uniform3f(self.model_loc[7], eye.x, eye.y, eye.z);
        gl.ActiveTexture(ffi::TEXTURE0);
        for part in &gpu.parts {
            let material = gpu.materials.get(part.material);
            let color = material.map_or([1.0; 4], |m| m.color);
            let texture = material.and_then(|m| m.texture).and_then(|t| gpu.textures.get(t)).copied().unwrap_or(self.white);
            gl.BindTexture(ffi::TEXTURE_2D, texture);
            gl.Uniform4f(self.model_loc[4], color[0], color[1], color[2], color[3]);
            gl.Uniform1f(self.model_loc[5], material.map_or(0.0, |m| m.cutoff).max(0.02));
            gl.BindVertexArray(part.vao);
            gl.DrawElements(ffi::TRIANGLES, part.count, ffi::UNSIGNED_INT, std::ptr::null());
        }
        gl.BindVertexArray(0);
    }

    /// The robot, part by part, at `at`, facing the wearer, at time `t`.
    fn figure_parts(&self, at: DVec3, t: f32, live: &Live) -> Vec<Part> {
        let since = live.mood_since.elapsed().as_secs_f32();
        let mood = live.mood;
        let pose = &live.pose;
        let (base, head) = frames(at, t, live);

        let white = Vec3::new(0.90, 0.93, 0.97);
        let teal = Vec3::new(0.09, 0.90, 0.75);
        let none = Vec3::ZERO;

        // Blinking: a tenth of a second, every few, not in step with anything -- and not while
        // its eyes are a line reading what was said.
        let cycle = (t * 0.27).fract();
        let blink = if cycle < 0.035 && mood != Mood::Transcribing { 0.12 } else { 1.0 };
        let (mut open, mut up, mut side) = (pose.open, pose.look_up, pose.look_side);
        match mood {
            Mood::Happy if since < 2.0 => (open, up) = (0.32, 0.004),
            Mood::Transcribing => side += 0.009 * (t * 7.0).sin(),
            Mood::Hearing => open += 0.15 * live.level,
            _ => {}
        }
        let talking = mood == Mood::Talking;
        let mouth_open = if talking { 0.35 + 0.65 * ((t * 11.0).sin() * 0.5 + 0.5) * ((t * 3.7).sin() * 0.3 + 0.7) } else { 0.0 };
        let glow = 1.0 + 0.25 * (t * 2.2).sin();

        let mut parts = vec![
            // The body, and a light on its front.
            Part::sphere(base * scaled(Vec3::ZERO, Vec3::new(0.060, 0.066, 0.075)), white, none, 0.6),
            Part::sphere(base * scaled(Vec3::new(0.057, 0.0, -0.002), Vec3::new(0.005, 0.011, 0.007)), none, teal * 0.9 * glow, 0.0),
            // The head, its visor, eyes and mouth.
            Part::sphere(head * scaled(Vec3::ZERO, Vec3::new(0.070, 0.086, 0.064)), white, none, 0.7),
            Part::sphere(head * scaled(Vec3::new(0.030, 0.0, 0.0), Vec3::new(0.046, 0.073, 0.046)), Vec3::new(0.04, 0.05, 0.08), none, 1.0),
            Part::sphere(head * scaled(Vec3::new(0.074, 0.024 + side, 0.007 + up), Vec3::new(0.005, 0.011, 0.016 * open * blink)), none, teal * 1.1, 0.0),
            Part::sphere(head * scaled(Vec3::new(0.074, -0.024 + side, 0.007 + up), Vec3::new(0.005, 0.011, 0.016 * open * blink)), none, teal * 1.1, 0.0),
        ];
        if talking || mood == Mood::Happy {
            let width = if talking { 0.012 } else { 0.016 };
            parts.push(Part::sphere(head * scaled(Vec3::new(0.074, 0.0, -0.017), Vec3::new(0.004, width, 0.002 + 0.006 * mouth_open)), none, teal * 0.8, 0.0));
        }
        // The antenna's stalk; its tip is among the signs.
        parts.push(Part::cylinder(head * scaled(Vec3::new(0.0, 0.0, 0.075), Vec3::new(0.003, 0.003, 0.03)), white, none, 0.4));

        // Arms, each from its shoulder towards where its hand should be, and what moves on top:
        // a swing at rest, typing, a hand writing, a gesture while it talks, a wave.
        let waving = live.wave_since.map(|w| w.elapsed().as_secs_f32()).filter(|w| *w < 1.8);
        for (i, side) in [1.0f32, -1.0].into_iter().enumerate() {
            let mut hand = pose.hands[i];
            match mood {
                Mood::Idle | Mood::Listening | Mood::Checking | Mood::Attentive => hand.x += 0.008 * (t * 1.3 + side).sin(),
                Mood::Working => hand.z += 0.007 * (t * 16.0 + side * 1.6).sin(),
                Mood::Transcribing if i == 1 => hand += Vec3::new(0.004 * (t * 9.0).cos(), 0.004 * (t * 9.0).sin(), 0.0),
                Mood::Talking if i == 1 => hand += Vec3::new(0.012 * (t * 2.6).sin(), 0.0, 0.014 * (t * 2.6 + 0.7).sin()),
                _ => {}
            }
            if let (1, Some(w)) = (i, waving) {
                hand = Vec3::new(0.02, -0.05 + 0.014 * (w * 12.0).sin(), 0.058);
            }
            let shoulder = Vec3::new(0.0, 0.074 * side, 0.025);
            let reach = glam::Quat::from_rotation_arc(Vec3::NEG_Z, hand.normalize_or(Vec3::NEG_Z));
            let arm = base * Mat4::from_translation(shoulder) * Mat4::from_quat(reach);
            parts.push(Part::sphere(arm * scaled(Vec3::new(0.0, 0.0, -0.035), Vec3::new(0.017, 0.017, 0.042)), white, none, 0.5));
        }
        parts.extend(signs(head, t, live, true));
        parts
    }

    /// Over a wearer's own figure, the robot's signs of what it is doing, as if its head were
    /// just under them: what the figure's face cannot say on its own.
    fn signs_over(&self, at: DVec3, t: f32, live: &Live) -> Vec<Part> {
        let (base, _) = frames(at, t, live);
        signs(base * Mat4::from_translation(Vec3::new(0.0, 0.0, 0.045)), t, live, false)
    }
}

/// Where the robot's body and head are: facing the wearer, bobbing, leaning and turning its head
/// as its pose has it.
fn frames(at: DVec3, t: f32, live: &Live) -> (Mat4, Mat4) {
    let since = live.mood_since.elapsed().as_secs_f32();
    let pose = &live.pose;
    let to_eye = (-at).as_vec3();
    let face_yaw = to_eye.y.atan2(to_eye.x);
    // A hop for joy, once.
    let hop = if live.mood == Mood::Happy && since < 0.5 { 0.012 * (since * std::f32::consts::TAU).sin().max(0.0) } else { 0.0 };
    let bob = 0.006 * SIZE * (t * 1.6).sin() + hop * SIZE;
    let base = Mat4::from_translation(at.as_vec3() + Vec3::new(0.0, 0.0, bob)) * Mat4::from_rotation_z(face_yaw) * Mat4::from_scale(Vec3::splat(SIZE)) * Mat4::from_rotation_y(pose.lean);
    // A little life in the head when the pose leaves it still; more for listening.
    let (nod, tilt) = match live.mood {
        Mood::Listening | Mood::Checking => (0.0, 0.10 * (t * 0.7).sin()),
        Mood::Hearing => (0.02 * live.level * (t * 5.0).sin(), 0.03 * (t * 1.1).sin()),
        Mood::Idle | Mood::Talking | Mood::Happy => (0.03 * (t * 0.9).sin(), 0.04 * (t * 0.6).sin()),
        _ => (0.0, 0.02 * (t * 0.8).sin()),
    };
    let head = base * Mat4::from_translation(Vec3::new(0.0, 0.0, 0.115)) * Mat4::from_rotation_y(pose.nod + nod) * Mat4::from_rotation_x(pose.tilt + tilt);
    (base, head)
}

/// What the figure is doing, as lights about its head (`head` is where the robot's head is):
/// the antenna's tip, the lights at its ears, and what each step of an answer shows. The ears
/// are the robot's only: on a wearer's figure they would float off its face.
fn signs(head: Mat4, t: f32, live: &Live, ears: bool) -> Vec<Part> {
    let teal = Vec3::new(0.09, 0.90, 0.75);
    let blue = Vec3::new(0.22, 0.65, 0.97);
    let violet = Vec3::new(0.56, 0.42, 0.95);
    let amber = Vec3::new(1.0, 0.66, 0.16);
    let green = Vec3::new(0.35, 0.95, 0.45);
    let none = Vec3::ZERO;
    let level = live.level;
    let on_off = |hz: f32| if (t * hz).fract() < 0.5 { 1.0 } else { 0.3 };
    let mut parts = Vec::new();

    // The antenna's tip says what it is doing; listening, it glows with the room's sound.
    let tip = match live.mood {
        Mood::Idle => blue * 0.25,
        Mood::Listening => teal * (0.45 + 0.9 * level),
        Mood::Checking => amber * (0.35 + 0.6 * on_off(3.0)),
        Mood::Attentive => teal * (1.0 + 0.15 * (t * 2.0).sin()),
        Mood::Hearing => teal * (0.9 + 1.2 * level),
        Mood::Transcribing => amber * 1.2,
        Mood::Thinking => blue * (0.7 + 0.7 * on_off(5.0)),
        Mood::Pondering => violet * 1.2,
        Mood::Working => green * (0.6 + 0.6 * on_off(4.0)),
        Mood::Talking => blue * 1.1,
        Mood::Happy => teal * 1.1,
    };
    let tip_size = if live.mood == Mood::Hearing { 0.009 * (1.0 + 0.5 * level) } else { 0.009 };
    parts.push(Part::sphere(head * scaled(Vec3::new(0.0, 0.0, 0.094), Vec3::splat(tip_size)), none, tip, 0.0));

    // In a conversation, the lights at its ears are lit: it is listening for more, no name needed.
    if ears && matches!(live.mood, Mood::Attentive | Mood::Hearing) {
        let lit = if live.mood == Mood::Hearing { 0.9 + 1.0 * level } else { 1.0 };
        for side in [1.0f32, -1.0] {
            parts.push(Part::sphere(head * scaled(Vec3::new(0.004, 0.087 * side, 0.004), Vec3::new(0.006, 0.004, 0.013)), none, teal * lit, 0.0));
        }
    }
    match live.mood {
        // The wearer's voice, drawn in as dots towards the cupped ear.
        Mood::Hearing => {
            for k in 0..3 {
                let phase = (t * 1.8 - k as f32 / 3.0).rem_euclid(1.0);
                let at = Vec3::new(0.012, -0.1 - 0.045 * (1.0 - phase), 0.012);
                let brightness = (0.35 + 1.3 * level) * (phase * std::f32::consts::PI).sin();
                parts.push(Part::sphere(head * scaled(at, Vec3::splat(0.0055)), none, teal * brightness, 0.0));
            }
        }
        // Written down: three dots over its head, lit in turn.
        Mood::Transcribing => {
            let lit = (t * 4.0) as usize % 3;
            for k in 0..3 {
                let at = Vec3::new(0.0, 0.018 * (k as f32 - 1.0), 0.124);
                parts.push(Part::sphere(head * scaled(at, Vec3::splat(0.0065)), none, amber * if k == lit { 1.4 } else { 0.35 }, 0.0));
            }
        }
        // The small model: one quick spark round its head, with a short tail.
        Mood::Thinking => {
            for k in 0..3 {
                let a = t * 6.5 - k as f32 * 0.32;
                let at = Vec3::new(0.05 * a.cos(), 0.062 * a.sin(), 0.098 + 0.008 * (a * 2.0).sin());
                parts.push(Part::sphere(head * scaled(at, Vec3::splat(0.008 * (1.0 - 0.28 * k as f32))), none, blue * (1.4 - 0.4 * k as f32), 0.0));
            }
        }
        // The larger model: three dots in the mark's colours, circling slowly over its head.
        Mood::Pondering => {
            for (k, color) in [teal, blue, violet].into_iter().enumerate() {
                let a = t * 2.4 + k as f32 * std::f32::consts::TAU / 3.0;
                let at = Vec3::new(0.045 * a.cos(), 0.055 * a.sin(), 0.115 + 0.008 * (a * 2.0).sin());
                parts.push(Part::sphere(head * scaled(at, Vec3::splat(0.008)), none, color * 1.2, 0.0));
            }
        }
        _ => {}
    }
    parts
}

struct Part {
    model: Mat4,
    color: Vec3,
    emissive: Vec3,
    gloss: f32,
    cylinder: bool,
}

impl Part {
    fn sphere(model: Mat4, color: Vec3, emissive: Vec3, gloss: f32) -> Part {
        Part { model, color, emissive, gloss, cylinder: false }
    }

    fn cylinder(model: Mat4, color: Vec3, emissive: Vec3, gloss: f32) -> Part {
        Part { model, color, emissive, gloss, cylinder: true }
    }
}

fn scaled(at: Vec3, size: Vec3) -> Mat4 {
    Mat4::from_translation(at) * Mat4::from_scale(size)
}

/// The part of an eye's view a sphere at `middle` of `reach` covers, in its normalised device
/// coordinates, a little more for safety; `None` when it is behind the eye.
fn crop_of(vp: &Mat4, middle: Vec3, reach: f32) -> Option<[f32; 4]> {
    let mut x0 = f32::MAX;
    let mut x1 = f32::MIN;
    let mut y0 = f32::MAX;
    let mut y1 = f32::MIN;
    for corner in 0..8 {
        let offset = Vec3::new(
            if corner & 1 == 0 { -reach } else { reach },
            if corner & 2 == 0 { -reach } else { reach },
            if corner & 4 == 0 { -reach } else { reach },
        );
        let clip = *vp * (middle + offset).extend(1.0);
        if clip.w <= 0.01 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        x0 = x0.min(ndc.x);
        x1 = x1.max(ndc.x);
        y0 = y0.min(ndc.y);
        y1 = y1.max(ndc.y);
    }
    // Square, so the picture's pixels are square on the glass.
    let (cx, cy) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
    let half = (x1 - x0).max(y1 - y0) * 0.5;
    let (hx, hy) = (half, half);
    if cx + hx < -1.2 || cx - hx > 1.2 || cy + hy < -1.2 || cy - hy > 1.2 {
        return None;
    }
    Some([cx - hx, cx + hx, cy - hy, cy + hy])
}

/// The clip-space transform that makes the crop `rect` the whole of what is drawn.
fn crop_matrix([x0, x1, y0, y1]: [f32; 4]) -> Mat4 {
    let sx = 2.0 / (x1 - x0);
    let sy = 2.0 / (y1 - y0);
    let (cx, cy) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
    Mat4::from_cols(Vec4::new(sx, 0.0, 0.0, 0.0), Vec4::new(0.0, sy, 0.0, 0.0), Vec4::new(0.0, 0.0, 1.0, 0.0), Vec4::new(-sx * cx, -sy * cy, 0.0, 1.0))
}

/// Positions and normals, and triangles over them.
struct Shape {
    vertices: Vec<f32>,
    indices: Vec<u16>,
}

fn sphere_mesh(stacks: u16, slices: u16) -> Shape {
    let mut vertices = Vec::new();
    for i in 0..=stacks {
        let phi = std::f32::consts::PI * i as f32 / stacks as f32;
        for j in 0..=slices {
            let theta = std::f32::consts::TAU * j as f32 / slices as f32;
            let n = Vec3::new(phi.sin() * theta.cos(), phi.sin() * theta.sin(), phi.cos());
            vertices.extend_from_slice(&[n.x, n.y, n.z, n.x, n.y, n.z]);
        }
    }
    let mut indices = Vec::new();
    let row = slices + 1;
    for i in 0..stacks {
        for j in 0..slices {
            let a = i * row + j;
            let b = a + row;
            indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    Shape { vertices, indices }
}

fn cylinder_mesh(slices: u16) -> Shape {
    let mut vertices = Vec::new();
    for j in 0..=slices {
        let theta = std::f32::consts::TAU * j as f32 / slices as f32;
        let (x, y) = (theta.cos(), theta.sin());
        vertices.extend_from_slice(&[x, y, -0.5, x, y, 0.0]);
        vertices.extend_from_slice(&[x, y, 0.5, x, y, 0.0]);
    }
    let mut indices = Vec::new();
    for j in 0..slices {
        let a = j * 2;
        indices.extend_from_slice(&[a, a + 2, a + 1, a + 1, a + 2, a + 3]);
    }
    Shape { vertices, indices }
}

unsafe fn upload(gl: &ffi::Gles2, shape: &Shape) -> Mesh {
    let mut vao = 0;
    gl.GenVertexArrays(1, &mut vao);
    gl.BindVertexArray(vao);
    let mut buffers = [0u32; 2];
    gl.GenBuffers(2, buffers.as_mut_ptr());
    gl.BindBuffer(ffi::ARRAY_BUFFER, buffers[0]);
    gl.BufferData(ffi::ARRAY_BUFFER, (shape.vertices.len() * 4) as isize, shape.vertices.as_ptr() as *const _, ffi::STATIC_DRAW);
    gl.BindBuffer(ffi::ELEMENT_ARRAY_BUFFER, buffers[1]);
    gl.BufferData(ffi::ELEMENT_ARRAY_BUFFER, (shape.indices.len() * 2) as isize, shape.indices.as_ptr() as *const _, ffi::STATIC_DRAW);
    gl.EnableVertexAttribArray(0);
    gl.VertexAttribPointer(0, 3, ffi::FLOAT, ffi::FALSE, 24, std::ptr::null());
    gl.EnableVertexAttribArray(1);
    gl.VertexAttribPointer(1, 3, ffi::FLOAT, ffi::FALSE, 24, 12 as *const _);
    gl.BindVertexArray(0);
    Mesh { vao, count: shape.indices.len() as i32 }
}

unsafe fn quad(gl: &ffi::Gles2) -> u32 {
    let verts: [f32; 12] = [-0.5, -0.5, 0.5, -0.5, 0.5, 0.5, -0.5, -0.5, 0.5, 0.5, -0.5, 0.5];
    let mut vbo = 0;
    gl.GenBuffers(1, &mut vbo);
    gl.BindBuffer(ffi::ARRAY_BUFFER, vbo);
    gl.BufferData(ffi::ARRAY_BUFFER, std::mem::size_of_val(&verts) as isize, verts.as_ptr() as *const _, ffi::STATIC_DRAW);
    let mut vao = 0;
    gl.GenVertexArrays(1, &mut vao);
    gl.BindVertexArray(vao);
    gl.BindBuffer(ffi::ARRAY_BUFFER, vbo);
    gl.EnableVertexAttribArray(0);
    gl.VertexAttribPointer(0, 2, ffi::FLOAT, ffi::FALSE, 8, std::ptr::null());
    gl.BindVertexArray(0);
    vao
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crop_fills_the_picture() {
        let m = crop_matrix([0.2, 0.6, -0.4, 0.0]);
        // A point at the crop's lower left corner lands on the picture's.
        let p = m * Vec4::new(0.2, -0.4, 0.5, 1.0);
        assert!((p.x + 1.0).abs() < 1e-5 && (p.y + 1.0).abs() < 1e-5);
        let q = m * Vec4::new(0.6 * 2.0, 0.0, 0.5, 2.0);
        assert!((q.x / q.w - 1.0).abs() < 1e-5 && (q.y / q.w - 1.0).abs() < 1e-5);
    }

    #[test]
    fn a_figure_in_front_is_cropped_square_and_one_behind_is_not() {
        let vp = Mat4::perspective_rh_gl(0.7, 16.0 / 9.0, 0.05, 100.0);
        let rect = crop_of(&vp, Vec3::new(0.0, 0.0, -1.3), 0.17).unwrap();
        assert!(rect[0] < 0.0 && rect[1] > 0.0);
        assert!(crop_of(&vp, Vec3::new(0.0, 0.0, 1.3), 0.17).is_none());
    }
}

/// The wearer's own figure, as far as it has got.
enum Chosen {
    None,
    Reading { path: String, rx: std::sync::mpsc::Receiver<Result<crate::figure::Figure, String>> },
    /// Read, waiting to be put on the GPU (see [`Avatar::settle_figure`]).
    Read { path: String, result: Option<Result<crate::figure::Figure, String>> },
    Ready { path: String, gpu: ModelGpu },
    Unreadable { path: String },
}

struct GpuPart {
    vao: u32,
    /// Positions, normals, uvs, indices, colours.
    buffers: [u32; 5],
    count: i32,
    material: usize,
    mesh: usize,
    /// For its face: where it stands, and how each target moves it.
    rest: Vec<[f32; 3]>,
    targets: Vec<Vec<[f32; 3]>>,
}

struct ModelGpu {
    parts: Vec<GpuPart>,
    materials: Vec<crate::figure::Material>,
    textures: Vec<u32>,
    expressions: std::collections::HashMap<String, Vec<(usize, usize, f32)>>,
    /// From the figure's own size and place to the robot's.
    fit: Mat4,
    /// The weights its targets were last moved to, by (part, target).
    weights: std::collections::HashMap<(usize, usize), f32>,
}

impl ModelGpu {
    unsafe fn upload(gl: &ffi::Gles2, figure: crate::figure::Figure) -> ModelGpu {
        let textures = figure
            .pictures
            .iter()
            .map(|p| {
                let t = crate::gl::upload_raw(gl, p.width, p.height, &p.rgba, false);
                gl.BindTexture(ffi::TEXTURE_2D, t);
                gl.GenerateMipmap(ffi::TEXTURE_2D);
                gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR_MIPMAP_LINEAR as i32);
                t
            })
            .collect();
        let mut parts = Vec::new();
        for part in figure.parts {
            let mut vao = 0;
            gl.GenVertexArrays(1, &mut vao);
            gl.BindVertexArray(vao);
            let mut buffers = [0u32; 5];
            gl.GenBuffers(5, buffers.as_mut_ptr());
            let floats = |gl: &ffi::Gles2, buffer: u32, data: &[f32], usage: u32| {
                gl.BindBuffer(ffi::ARRAY_BUFFER, buffer);
                gl.BufferData(ffi::ARRAY_BUFFER, (data.len() * 4) as isize, data.as_ptr() as *const _, usage);
            };
            let flat = |v: &[[f32; 3]]| v.iter().flatten().copied().collect::<Vec<f32>>();
            let usage = if part.targets.is_empty() { ffi::STATIC_DRAW } else { ffi::DYNAMIC_DRAW };
            floats(gl, buffers[0], &flat(&part.positions), usage);
            gl.EnableVertexAttribArray(0);
            gl.VertexAttribPointer(0, 3, ffi::FLOAT, ffi::FALSE, 12, std::ptr::null());
            floats(gl, buffers[1], &flat(&part.normals), ffi::STATIC_DRAW);
            gl.EnableVertexAttribArray(1);
            gl.VertexAttribPointer(1, 3, ffi::FLOAT, ffi::FALSE, 12, std::ptr::null());
            floats(gl, buffers[2], &part.uvs.iter().flatten().copied().collect::<Vec<f32>>(), ffi::STATIC_DRAW);
            gl.EnableVertexAttribArray(2);
            gl.VertexAttribPointer(2, 2, ffi::FLOAT, ffi::FALSE, 8, std::ptr::null());
            if !part.colors.is_empty() {
                floats(gl, buffers[4], &part.colors.iter().flatten().copied().collect::<Vec<f32>>(), ffi::STATIC_DRAW);
                gl.EnableVertexAttribArray(3);
                gl.VertexAttribPointer(3, 4, ffi::FLOAT, ffi::FALSE, 16, std::ptr::null());
            }
            gl.BindBuffer(ffi::ELEMENT_ARRAY_BUFFER, buffers[3]);
            gl.BufferData(ffi::ELEMENT_ARRAY_BUFFER, (part.indices.len() * 4) as isize, part.indices.as_ptr() as *const _, ffi::STATIC_DRAW);
            gl.BindVertexArray(0);
            parts.push(GpuPart {
                vao,
                buffers,
                count: part.indices.len() as i32,
                material: part.material,
                mesh: part.mesh,
                rest: if part.targets.is_empty() { Vec::new() } else { part.positions },
                targets: part.targets,
            });
        }
        // As tall as the robot, standing where it stands.
        let size = figure.high - figure.low;
        let scale = (0.155 / size.z.max(0.01)).min(0.155 / size.x.max(size.y).max(0.01) * 2.0);
        let middle = (figure.low + figure.high) * 0.5;
        let fit = Mat4::from_translation(Vec3::new(0.0, 0.0, -0.044)) * Mat4::from_scale(Vec3::splat(scale)) * Mat4::from_translation(Vec3::new(-middle.x, -middle.y, -figure.low.z));
        ModelGpu { parts, materials: figure.materials, textures, expressions: figure.expressions, fit, weights: Default::default() }
    }

    /// Give back its buffers and pictures.
    unsafe fn free(&self, gl: &ffi::Gles2) {
        for part in &self.parts {
            gl.DeleteBuffers(part.buffers.len() as i32, part.buffers.as_ptr());
            gl.DeleteVertexArrays(1, &part.vao);
        }
        gl.DeleteTextures(self.textures.len() as i32, self.textures.as_ptr());
    }

    /// Its face for the moment: blinking, smiling, the mouth moving while it talks. Only what
    /// changed is moved, and only the parts it moves are sent again.
    unsafe fn pose(&mut self, gl: &ffi::Gles2, live: &Live, t: f32) {
        let since = live.mood_since.elapsed().as_secs_f32();
        let cycle = (t * 0.27).fract();
        let mut want: Vec<(&str, f32)> = vec![("blink", if cycle < 0.035 { 1.0 } else { 0.0 })];
        // VRM 1.0 names its looks `lookUp`, 0.x `lookup`: both are asked for, and a figure has one.
        let look = |want: &mut Vec<(&str, f32)>, new: &'static str, old: &'static str, amount: f32| {
            want.push((new, amount));
            want.push((old, amount));
        };
        match live.mood {
            Mood::Happy if since < 2.0 => want.push(("happy", 1.0)),
            Mood::Talking => {
                let open = 0.35 + 0.55 * ((t * 11.0).sin() * 0.5 + 0.5);
                want.push(("aa", open * ((t * 3.7).sin() * 0.5 + 0.5)));
                want.push(("oh", open * ((t * 3.7).cos() * 0.5 + 0.5) * 0.7));
            }
            Mood::Idle => {
                want.push(("relaxed", 0.5));
                want[0].1 = want[0].1.max(0.35);
            }
            Mood::Attentive => want.push(("surprised", 0.12)),
            Mood::Hearing => want.push(("surprised", 0.2 + 0.3 * live.level)),
            Mood::Transcribing | Mood::Working => look(&mut want, "lookDown", "lookdown", 0.45),
            Mood::Thinking => {
                want.push(("relaxed", 0.3));
                look(&mut want, "lookUp", "lookup", 0.5);
            }
            Mood::Pondering => {
                want.push(("relaxed", 0.4));
                look(&mut want, "lookUp", "lookup", 0.35);
            }
            _ => {}
        }
        let mut weights: std::collections::HashMap<(usize, usize), f32> = std::collections::HashMap::new();
        for (name, amount) in want {
            for (mesh, target, weight) in self.expressions.get(name).into_iter().flatten() {
                for (p, part) in self.parts.iter().enumerate().filter(|(_, p)| p.mesh == *mesh && *target < p.targets.len()) {
                    *weights.entry((p, *target)).or_insert(0.0) += amount * weight;
                    let _ = part;
                }
            }
        }
        // Targets that were moved and are no longer: back to rest.
        let mut changed: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for (key, w) in &weights {
            if (self.weights.get(key).copied().unwrap_or(0.0) - w).abs() > 0.01 {
                changed.insert(key.0);
            }
        }
        for (key, w) in &self.weights {
            if *w > 0.0 && !weights.contains_key(key) {
                changed.insert(key.0);
            }
        }
        for p in changed {
            let part = &self.parts[p];
            let mut positions = part.rest.clone();
            for ((q, target), w) in &weights {
                if *q != p || *w <= 0.0 {
                    continue;
                }
                for (v, d) in positions.iter_mut().zip(&part.targets[*target]) {
                    v[0] += d[0] * w;
                    v[1] += d[1] * w;
                    v[2] += d[2] * w;
                }
            }
            let flat: Vec<f32> = positions.iter().flatten().copied().collect();
            gl.BindBuffer(ffi::ARRAY_BUFFER, part.buffers[0]);
            gl.BufferSubData(ffi::ARRAY_BUFFER, 0, (flat.len() * 4) as isize, flat.as_ptr() as *const _);
        }
        self.weights = weights;
    }
}
