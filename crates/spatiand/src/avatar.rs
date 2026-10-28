//! The assistant's figure in the room (`docs/ai-integrations.md`).
//!
//! **Where**: fixed to the view, low in the corner on the other side from the pinned windows, a
//! metre and a third away -- and following the head lazily rather than rigidly, so it drifts
//! after a turn of the head the way something that floats would, instead of being painted on
//! the glass.
//!
//! **What**: by default a small robot drawn from a few shapes -- a head with a visor and two
//! eyes, a body, two arms, an antenna -- made here rather than loaded, so the default needs no
//! file and no licence. It blinks, looks up while it thinks (three dots circling over it), moves
//! its mouth while it talks, smiles after an answer, waves when called. The wearer's own figures
//! (VRM, glTF) are to come through the same path.
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
/// The figure's bounding sphere, from its middle, metres: what is cropped to.
const REACH_M: f32 = 0.17 * SIZE;
/// Its height, metres, for keeping all of it in view.
const TALL_M: f64 = 0.24 * SIZE as f64;

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
    #[default]
    Idle,
    Listening,
    Thinking,
    Talking,
    Happy,
}

impl Mood {
    fn of(name: &str) -> Mood {
        match name {
            "listening" => Mood::Listening,
            "thinking" => Mood::Thinking,
            "talking" => Mood::Talking,
            "happy" => Mood::Happy,
            _ => Mood::Idle,
        }
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
}

pub struct Avatar {
    program: u32,
    loc: [i32; 8],
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
        let mood = Mood::of(figure.get("mood").and_then(|m| m.as_str()).unwrap_or("idle"));
        if mood != live.mood {
            live.mood = mood;
            live.mood_since = now;
        }
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
                let middle = (place(yaw, pitch) + head * DVec3::new(0.0, 0.0, 0.03 * SIZE as f64)).as_vec3();
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
        let parts = self.figure_parts(at, t, &live);

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
        let middle = (at + head * DVec3::new(0.0, 0.0, 0.03 * SIZE as f64)).as_vec3();
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

    /// The robot, part by part, at `at`, facing the wearer, at time `t`.
    fn figure_parts(&self, at: DVec3, t: f32, live: &Live) -> Vec<Part> {
        let since = live.mood_since.elapsed().as_secs_f32();
        // Facing the eyes, upright.
        let to_eye = (-at).as_vec3();
        let face_yaw = to_eye.y.atan2(to_eye.x);
        let bob = 0.006 * SIZE * (t * 1.6).sin();
        let base = Mat4::from_translation(at.as_vec3() + Vec3::new(0.0, 0.0, bob)) * Mat4::from_rotation_z(face_yaw) * Mat4::from_scale(Vec3::splat(SIZE));

        let white = Vec3::new(0.90, 0.93, 0.97);
        let teal = Vec3::new(0.09, 0.90, 0.75);
        let blue = Vec3::new(0.22, 0.65, 0.97);
        let violet = Vec3::new(0.56, 0.42, 0.95);
        let none = Vec3::ZERO;

        // The head leans in to listen, and up to think.
        let (head_pitch, head_roll) = match live.mood {
            Mood::Listening => (0.10, 0.12 * (t * 0.7).sin()),
            Mood::Thinking => (-0.16, 0.08),
            _ => (0.03 * (t * 0.9).sin(), 0.04 * (t * 0.6).sin()),
        };
        let head_at = Vec3::new(0.0, 0.0, 0.115);
        let head = base * Mat4::from_translation(head_at) * Mat4::from_rotation_y(head_pitch) * Mat4::from_rotation_x(head_roll);

        // Blinking: a tenth of a second, every few, not in step with anything.
        let cycle = (t * 0.27).fract();
        let blink = if cycle < 0.035 { 0.12 } else { 1.0 };
        let (eye_h, eye_up, eye_side) = match live.mood {
            Mood::Happy if since < 2.0 => (0.32, 0.004, 0.0),
            Mood::Thinking => (0.85, 0.008, 0.006),
            Mood::Listening => (1.15, 0.0, 0.0),
            _ => (1.0, 0.0, 0.0),
        };
        let talking = live.mood == Mood::Talking;
        let mouth_open = if talking { 0.35 + 0.65 * ((t * 11.0).sin() * 0.5 + 0.5) * ((t * 3.7).sin() * 0.3 + 0.7) } else { 0.0 };
        let glow = 1.0 + 0.25 * (t * 2.2).sin();

        let mut parts = vec![
            // The body, and a light on its front.
            Part::sphere(base * scaled(Vec3::ZERO, Vec3::new(0.060, 0.066, 0.075)), white, none, 0.6),
            Part::sphere(base * scaled(Vec3::new(0.057, 0.0, -0.002), Vec3::new(0.005, 0.011, 0.007)), none, teal * 0.9 * glow, 0.0),
            // The head, its visor, eyes and mouth.
            Part::sphere(head * scaled(Vec3::ZERO, Vec3::new(0.070, 0.086, 0.064)), white, none, 0.7),
            Part::sphere(head * scaled(Vec3::new(0.030, 0.0, 0.0), Vec3::new(0.046, 0.073, 0.046)), Vec3::new(0.04, 0.05, 0.08), none, 1.0),
            Part::sphere(head * scaled(Vec3::new(0.074, 0.024 + eye_side, 0.007 + eye_up), Vec3::new(0.005, 0.011, 0.016 * eye_h * blink)), none, teal * 1.1, 0.0),
            Part::sphere(head * scaled(Vec3::new(0.074, -0.024 + eye_side, 0.007 + eye_up), Vec3::new(0.005, 0.011, 0.016 * eye_h * blink)), none, teal * 1.1, 0.0),
        ];
        if talking || live.mood == Mood::Happy {
            let width = if talking { 0.012 } else { 0.016 };
            parts.push(Part::sphere(head * scaled(Vec3::new(0.074, 0.0, -0.017), Vec3::new(0.004, width, 0.002 + 0.006 * mouth_open)), none, teal * 0.8, 0.0));
        }
        // The antenna: its tip says what it is doing.
        let tip = match live.mood {
            Mood::Listening => teal * (0.9 + 0.6 * (t * 4.0).sin().abs()),
            Mood::Thinking => violet * 1.2,
            Mood::Talking => blue * 1.1,
            _ => blue * 0.45,
        };
        parts.push(Part::cylinder(head * scaled(Vec3::new(0.0, 0.0, 0.075), Vec3::new(0.003, 0.003, 0.03)), white, none, 0.4));
        parts.push(Part::sphere(head * scaled(Vec3::new(0.0, 0.0, 0.094), Vec3::splat(0.009)), none, tip, 0.0));

        // Arms: hanging, swinging a little; the right one waves when it is called.
        let waving = live.wave_since.map(|w| w.elapsed().as_secs_f32()).filter(|w| *w < 1.8);
        for (side, sign) in [(1.0f32, 1.0f32), (-1.0, -1.0)] {
            let shoulder = Vec3::new(0.0, 0.074 * side, 0.025);
            let swing = 0.12 * (t * 1.3 + side).sin();
            let (raise, sway) = match (sign < 0.0, waving) {
                (true, Some(w)) => (-2.5, 0.35 * (w * 12.0).sin()),
                _ => (0.18 * side, swing),
            };
            let arm = base * Mat4::from_translation(shoulder) * Mat4::from_rotation_x(raise * sign.signum() * side.signum()) * Mat4::from_rotation_y(sway);
            parts.push(Part::sphere(arm * scaled(Vec3::new(0.0, 0.0, -0.035), Vec3::new(0.017, 0.017, 0.042)), white, none, 0.5));
        }

        // Thinking: three dots in the mark's colours, circling over its head.
        if live.mood == Mood::Thinking {
            for (k, color) in [teal, blue, violet].into_iter().enumerate() {
                let a = t * 2.4 + k as f32 * std::f32::consts::TAU / 3.0;
                let dot = head * scaled(Vec3::new(0.045 * a.cos(), 0.055 * a.sin(), 0.115 + 0.008 * (a * 2.0).sin()), Vec3::splat(0.008));
                parts.push(Part::sphere(dot, none, color * 1.2, 0.0));
            }
        }
        parts
    }
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
