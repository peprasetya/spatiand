//! The drawing thread: EGL on the glasses' display, one frame per refresh.
//!
//! What it draws is the Deck's room, minus what needs a compositor: the generated studio
//! environment the Deck starts in (`spatiand_render::Sky::studio`, sampled with the Deck's own
//! shader), every remote window on the sphere around the wearer, and -- when a remote
//! application takes the room, as SpatiWorld does -- that application's own two eye views in
//! place of the environment.
//!
//! Every frame it also tells the hosts where the head is, the same `Viewport` the Deck sends,
//! built from the same eyes the room is drawn with (`spatiand_render::openxr`).
//!
//! ## Pictures are GPU buffers
//!
//! A remote window's picture is an `AHardwareBuffer` the phone's decoder wrote (see
//! [`super::decode`]). It becomes an `EGLImage`, and the image an external texture, which the
//! GPU samples straight from the decoder's YUV. A picture replaced on screen is kept two more
//! frames before it goes back to the decoder, so the GPU is surely done reading it.

use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use glam::{DQuat, DVec3, Mat3, Mat4, Quat, Vec3, Vec4};
use glow::HasContext;
use khronos_egl as egl;
use spatiand_render::openxr::{openxr_eye, to_openxr};
use spatiand_render::{eye_for, sbs_viewport, Eye, EyeSide, Sky, StereoConfig};
use spatiand_stream::{Eyes, Layer};

use super::decode::{Output, Picture};
use super::remote::Handle;
use super::Shared;

pub struct Renderer {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// An `ANativeWindow` handed to the thread that draws into it, which releases it.
struct Window(*mut ndk_sys::ANativeWindow);
unsafe impl Send for Window {}

impl Renderer {
    pub fn start(window: *mut ndk_sys::ANativeWindow, shared: Arc<Shared>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let quit = stop.clone();
        let window = Window(window);
        let thread = std::thread::Builder::new()
            .name("glasses-draw".into())
            .spawn(move || {
                let window = window;
                if let Err(e) = run(window.0, &shared, &quit) {
                    log::error!("drawing stopped: {e}");
                }
                unsafe { ndk_sys::ANativeWindow_release(window.0) };
            })
            .ok();
        Self { stop, thread }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// How far ahead to predict the head, from reading the pose to light leaving the panel.
///
/// Longer than the Deck's one refresh: there Spatiand scans out itself, and here a frame goes
/// through Android's compositor on the next 60 Hz tick before it is scanned out -- about two.
const PREDICTION_SECONDS: f64 = 2.0 / 60.0;

/// Resolution of the generated environment, as the Deck makes it.
const STUDIO_SIZE: (u32, u32) = (2048, 1024);

/// Where a new window goes, as on the Deck (`spatiand::window::Placement`'s default): 1.1 m
/// wide at 2.2 m, about 28 degrees of one eye's 45.
const WINDOW_RADIUS: f32 = 2.2;
const WINDOW_WIDTH: f32 = 1.1;
/// How far apart, in yaw, two windows opened in the same place are put.
const WINDOW_SPACING: f32 = 0.55;
/// How far above or below the horizon a window opens, at most: 30 degrees.
const MAX_OPEN_PITCH: f32 = 0.52;

const GL_TEXTURE_EXTERNAL_OES: u32 = 0x8D65;
const EGL_NATIVE_BUFFER_ANDROID: u32 = 0x3140;
const EGL_IMAGE_PRESERVED_KHR: i32 = 0x30D2;

/// A full-screen triangle, pushed to the far plane.
const FULLSCREEN_VS: &str = r#"#version 300 es
out vec2 v_ndc;
void main() {
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2)) * 2.0 - 1.0;
    v_ndc = p;
    gl_Position = vec4(p, 1.0, 1.0);
}
"#;

/// Equirectangular sampling: the Deck's `SKY_FRAG` (`crates/spatiand/src/gl.rs`), for a mono
/// 360 source. Its conventions are `spatiand_render::sky`'s, where the tests are.
const SKY_FS: &str = r#"#version 300 es
precision highp float;
in vec2 v_ndc;
uniform sampler2D u_sky;
uniform mat4 u_inv_vp;
uniform float u_yaw;
out vec4 f_color;
const float PI = 3.14159265358979;
void main() {
    vec4 far = u_inv_vp * vec4(v_ndc, 1.0, 1.0);
    vec3 dir = normalize(far.xyz / far.w);
    float azimuth = atan(-dir.y, dir.x) - u_yaw;
    azimuth = mod(azimuth + PI, 2.0 * PI) - PI;
    float v = 0.5 - asin(clamp(dir.z, -1.0, 1.0)) / PI;
    float u = 0.5 + azimuth / (2.0 * PI);
    f_color = vec4(texture(u_sky, vec2(u, v)).rgb, 1.0);
}
"#;

/// An application's own eye view as the room: the Deck's `PROJECTION_FRAG`, sampling the
/// decoder's buffer as an external texture. Each pixel's direction is turned into the camera
/// the picture was drawn with, so a picture drawn for where the head *was* is put back where it
/// was drawn -- what holds the world still over Wi-Fi that is always a little late.
const PROJECTION_FS: &str = r#"#version 300 es
#extension GL_OES_EGL_image_external_essl3 : require
precision highp float;
in vec2 v_ndc;
uniform samplerExternalOES u_tex;
uniform mat4 u_inv_vp;
uniform mat3 u_to_frame;
// tan(angleLeft), tan(angleRight), tan(angleDown), tan(angleUp).
uniform vec4 u_tan;
// (u0, v0, u1, v1): this eye's part of the buffer.
uniform vec4 u_rect;
out vec4 f_color;
void main() {
    vec4 far = u_inv_vp * vec4(v_ndc, 1.0, 1.0);
    vec3 dir = u_to_frame * normalize(far.xyz / far.w);
    if (dir.x <= 1e-4) { f_color = vec4(0.0, 0.0, 0.0, 1.0); return; }
    float tx = -dir.y / dir.x;
    float ty = dir.z / dir.x;
    float u = (tx - u_tan.x) / (u_tan.y - u_tan.x);
    float v = (u_tan.w - ty) / (u_tan.w - u_tan.z);
    if (u < 0.0 || u > 1.0 || v < 0.0 || v > 1.0) { f_color = vec4(0.0, 0.0, 0.0, 1.0); return; }
    vec2 uv = vec2(mix(u_rect.x, u_rect.z, u), mix(u_rect.y, u_rect.w, v));
    f_color = vec4(texture(u_tex, uv).rgb, 1.0);
}
"#;

const QUAD_VS: &str = r#"#version 300 es
uniform mat4 u_mvp;
out vec2 v_uv;
void main() {
    vec2 corner = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
    // v is 0 at the top of a picture.
    v_uv = vec2(corner.x, 1.0 - corner.y);
    gl_Position = u_mvp * vec4(corner - 0.5, 0.0, 1.0);
}
"#;

/// A remote window's picture, from the decoder's buffer.
const WINDOW_FS: &str = r#"#version 300 es
#extension GL_OES_EGL_image_external_essl3 : require
precision highp float;
in vec2 v_uv;
uniform samplerExternalOES u_tex;
// (u0, v0, u1, v1): the part of the buffer this eye sees.
uniform vec4 u_rect;
out vec4 f_color;
void main() {
    vec2 uv = vec2(mix(u_rect.x, u_rect.z, v_uv.x), mix(u_rect.y, u_rect.w, v_uv.y));
    f_color = vec4(texture(u_tex, uv).rgb, 1.0);
}
"#;

/// A window that has opened but has nothing to show yet.
const WAITING_FS: &str = r#"#version 300 es
precision mediump float;
in vec2 v_uv;
uniform float u_time;
out vec4 f_color;
void main() {
    vec2 edge = min(v_uv, 1.0 - v_uv);
    float border = step(min(edge.x, edge.y), 0.006);
    float pulse = 0.5 + 0.5 * sin(u_time * 3.0 - v_uv.x * 6.0);
    vec3 c = mix(vec3(0.10, 0.12, 0.16), vec3(0.16, 0.19, 0.26), pulse);
    f_color = vec4(mix(c, vec3(0.6, 0.7, 0.85), border), 1.0);
}
"#;

unsafe fn program(gl: &glow::Context, vs: &str, fs: &str) -> Result<glow::Program, String> {
    let program = gl.create_program()?;
    let mut shaders = Vec::new();
    for (kind, source) in [(glow::VERTEX_SHADER, vs), (glow::FRAGMENT_SHADER, fs)] {
        let shader = gl.create_shader(kind)?;
        gl.shader_source(shader, source);
        gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader) {
            return Err(gl.get_shader_info_log(shader));
        }
        gl.attach_shader(program, shader);
        shaders.push(shader);
    }
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        return Err(gl.get_program_info_log(program));
    }
    for shader in shaders {
        gl.detach_shader(program, shader);
        gl.delete_shader(shader);
    }
    Ok(program)
}

/// Where a window is: around the wearer, facing them.
#[derive(Debug, Clone, Copy)]
struct Placement {
    /// Radians, positive to the left.
    yaw: f32,
    /// Radians above the horizon.
    pitch: f32,
}

impl Placement {
    fn rotation(&self) -> Quat {
        Quat::from_rotation_z(self.yaw) * Quat::from_rotation_y(-self.pitch)
    }

    /// The model matrix of a `width` x `height` quad here, for [`QUAD_VS`]'s unit square.
    fn model(&self, width: f32, height: f32) -> Mat4 {
        let q = self.rotation();
        let centre = q * Vec3::new(WINDOW_RADIUS, 0.0, 0.0);
        Mat4::from_cols(
            (q * -Vec3::Y * width).extend(0.0),
            (q * Vec3::Z * height).extend(0.0),
            (q * Vec3::X).extend(0.0),
            centre.extend(1.0),
        )
    }
}

/// The EGL calls Android's buffers need, which are extensions and so found at run time.
struct Import {
    display: *mut c_void,
    client_buffer: unsafe extern "C" fn(*const ndk_sys::AHardwareBuffer) -> *mut c_void,
    create_image: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *mut c_void, *const i32) -> *mut c_void,
    destroy_image: unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32,
    target_texture: unsafe extern "C" fn(u32, *mut c_void),
}

impl Import {
    fn load(lib: &egl::DynamicInstance<egl::EGL1_4>, display: egl::Display) -> Result<Import, String> {
        let find = |name: &str| {
            lib.get_proc_address(name)
                .map(|f| f as *const c_void)
                .ok_or_else(|| format!("no {name}"))
        };
        unsafe {
            Ok(Import {
                display: display.as_ptr(),
                client_buffer: std::mem::transmute::<*const c_void, _>(find("eglGetNativeClientBufferANDROID")?),
                create_image: std::mem::transmute::<*const c_void, _>(find("eglCreateImageKHR")?),
                destroy_image: std::mem::transmute::<*const c_void, _>(find("eglDestroyImageKHR")?),
                target_texture: std::mem::transmute::<*const c_void, _>(find("glEGLImageTargetTexture2DOES")?),
            })
        }
    }

    /// The picture as an image the GL can sample, or null.
    unsafe fn image(&self, picture: &Picture) -> *mut c_void {
        let buffer = (self.client_buffer)(picture.buffer);
        if buffer.is_null() {
            return std::ptr::null_mut();
        }
        let attributes = [EGL_IMAGE_PRESERVED_KHR, 1, egl::NONE];
        (self.create_image)(self.display, std::ptr::null_mut(), EGL_NATIVE_BUFFER_ANDROID, buffer, attributes.as_ptr())
    }
}

/// A picture on screen, and the image made of it.
struct Showing {
    picture: Picture,
    image: *mut c_void,
}

/// What the drawing thread keeps for one remote window.
struct Shown {
    texture: glow::Texture,
    /// The decoder output this window's pictures come from, to notice when it is replaced.
    output: Option<Arc<Output>>,
    showing: Option<Showing>,
    placement: Placement,
}

/// Pictures taken off screen, kept until the GPU has surely finished with them.
struct Retired {
    showing: Showing,
    frame: u64,
}

/// The viewports sent lately, by number, so a picture drawn for one can be put back where the
/// head was then.
struct Sent {
    seq: u32,
    orientation: DQuat,
}

fn run(window: *mut ndk_sys::ANativeWindow, shared: &Shared, stop: &AtomicBool) -> Result<(), String> {
    let lib = unsafe { egl::DynamicInstance::<egl::EGL1_4>::load_required() }
        .map_err(|e| format!("no libEGL: {e}"))?;
    let display = unsafe { lib.get_display(egl::DEFAULT_DISPLAY) }.ok_or("no EGL display")?;
    lib.initialize(display).map_err(|e| format!("EGL initialise: {e}"))?;
    let attributes = [
        egl::RED_SIZE, 8,
        egl::GREEN_SIZE, 8,
        egl::BLUE_SIZE, 8,
        egl::RENDERABLE_TYPE, egl::OPENGL_ES3_BIT,
        egl::SURFACE_TYPE, egl::WINDOW_BIT,
        egl::NONE,
    ];
    let config = lib
        .choose_first_config(display, &attributes)
        .map_err(|e| format!("EGL config: {e}"))?
        .ok_or("no EGL config for GLES 3")?;
    let context = lib
        .create_context(display, config, None, &[egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE])
        .map_err(|e| format!("EGL context: {e}"))?;
    let surface = unsafe {
        lib.create_window_surface(display, config, window as egl::NativeWindowType, None)
    }
    .map_err(|e| format!("EGL surface: {e}"))?;
    lib.make_current(display, Some(surface), Some(surface), Some(context))
        .map_err(|e| format!("EGL make current: {e}"))?;
    // One frame per refresh: the swap waits for it.
    let _ = lib.swap_interval(display, 1);
    let import = Import::load(&lib, display)?;

    let gl = unsafe {
        glow::Context::from_loader_function(|name| {
            lib.get_proc_address(name)
                .map_or(std::ptr::null(), |f| f as *const std::ffi::c_void)
        })
    };
    let result = unsafe { draw_loop(&gl, &lib, display, surface, &import, shared, stop) };

    let _ = lib.make_current(display, None, None, None);
    let _ = lib.destroy_surface(display, surface);
    let _ = lib.destroy_context(display, context);
    shared.fps.store(0, Ordering::Relaxed);
    result
}

#[allow(clippy::too_many_arguments)]
unsafe fn draw_loop(
    gl: &glow::Context,
    lib: &egl::DynamicInstance<egl::EGL1_4>,
    display: egl::Display,
    surface: egl::Surface,
    import: &Import,
    shared: &Shared,
    stop: &AtomicBool,
) -> Result<(), String> {
    let sky_program = program(gl, FULLSCREEN_VS, SKY_FS)?;
    let projection_program = program(gl, FULLSCREEN_VS, PROJECTION_FS)?;
    let window_program = program(gl, QUAD_VS, WINDOW_FS)?;
    let waiting_program = program(gl, QUAD_VS, WAITING_FS)?;
    let vao = gl.create_vertex_array()?;
    let u = |p: glow::Program, name: &str| gl.get_uniform_location(p, name);
    let sky_inv_vp = u(sky_program, "u_inv_vp");
    let sky_yaw = u(sky_program, "u_yaw");
    let sky_tex = u(sky_program, "u_sky");
    let proj_inv_vp = u(projection_program, "u_inv_vp");
    let proj_to_frame = u(projection_program, "u_to_frame");
    let proj_tan = u(projection_program, "u_tan");
    let proj_rect = u(projection_program, "u_rect");
    let proj_tex = u(projection_program, "u_tex");
    let win_mvp = u(window_program, "u_mvp");
    let win_rect = u(window_program, "u_rect");
    let win_tex = u(window_program, "u_tex");
    let wait_mvp = u(waiting_program, "u_mvp");
    let wait_time = u(waiting_program, "u_time");

    // The Deck's generated environment: a studio with a key light, made once.
    let studio = Sky::studio(STUDIO_SIZE.0, STUDIO_SIZE.1);
    let sky_yaw_offset = studio.source.yaw_offset_radians();
    let sky_texture = gl.create_texture()?;
    gl.bind_texture(glow::TEXTURE_2D, Some(sky_texture));
    gl.tex_image_2d(
        glow::TEXTURE_2D,
        0,
        glow::RGBA8 as i32,
        studio.width as i32,
        studio.height as i32,
        0,
        glow::RGBA,
        glow::UNSIGNED_BYTE,
        glow::PixelUnpackData::Slice(Some(&studio.rgba)),
    );
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
    drop(studio);

    let started = Instant::now();
    let mut shown: HashMap<u32, Shown> = HashMap::new();
    let mut retired: VecDeque<Retired> = VecDeque::new();
    let mut sent: VecDeque<Sent> = VecDeque::new();
    let mut viewport_seq = 0u32;
    let mut frame_no = 0u64;
    let mut frames = 0u32;
    let mut since = Instant::now();
    let mut last_size = (0, 0);

    while !stop.load(Ordering::Relaxed) {
        let w = lib.query_surface(display, surface, egl::WIDTH).unwrap_or(0);
        let h = lib.query_surface(display, surface, egl::HEIGHT).unwrap_or(0);
        if w <= 0 || h <= 0 {
            std::thread::sleep(Duration::from_millis(16));
            continue;
        }
        if (w, h) != last_size {
            log::info!("drawing into {w}x{h}");
            last_size = (w, h);
        }
        frame_no += 1;
        // 3840x1080 is two eyes side by side; 1920x1080 is the glasses in 2D, drawn as one
        // eye from between the two.
        let stereo = w >= h * 3;
        let cfg = StereoConfig {
            h_fov_deg: *shared.h_fov_deg.lock().unwrap(),
            per_eye: (if stereo { w / 2 } else { w } as u32, h as u32),
            ipd_m: if stereo { 0.063 } else { 0.0 },
            ..StereoConfig::default()
        };
        let orientation = shared.tracker.lock().unwrap().predicted_orientation(
            PREDICTION_SECONDS,
            spatiand_track::DEFAULT_PREDICTION_MAX_DEGREES,
        );

        // --- the host: where the head is, and what it has to show ---
        let remote: Option<Handle> = shared.remote.lock().unwrap().clone();
        if let Some(handle) = &remote {
            viewport_seq = viewport_seq.wrapping_add(1);
            handle.viewport(viewport(orientation, &cfg, viewport_seq, started));
            sent.push_back(Sent { seq: viewport_seq, orientation });
            while sent.len() > 120 {
                sent.pop_front();
            }
        }
        let windows = remote
            .as_ref()
            .map(|h| h.view.lock().unwrap().windows.clone())
            .unwrap_or_default();

        // Windows gone since the last frame.
        let gone: Vec<u32> = shown.keys().filter(|id| !windows.contains_key(id)).copied().collect();
        for id in gone {
            if let Some(s) = shown.remove(&id) {
                if let Some(showing) = s.showing {
                    retired.push_back(Retired { showing, frame: frame_no });
                }
                gl.delete_texture(s.texture);
            }
        }
        // New ones, and new pictures.
        for (id, window) in &windows {
            if !shown.contains_key(id) {
                let placement = place_new(orientation, shown.values().map(|s| s.placement));
                log::info!(
                    "window {id} ({}) placed at {:.0} degrees",
                    window.title,
                    placement.yaw.to_degrees()
                );
                shown.insert(*id, Shown {
                    texture: gl.create_texture()?,
                    output: None,
                    showing: None,
                    placement,
                });
            }
            let s = shown.get_mut(id).unwrap();
            let same_output = match (&s.output, &window.output) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            };
            if !same_output {
                if let Some(showing) = s.showing.take() {
                    retired.push_back(Retired { showing, frame: frame_no });
                }
                s.output = window.output.clone();
            }
            let Some(output) = &s.output else { continue };
            if let Some(picture) = output.take_latest() {
                let image = import.image(&picture);
                if image.is_null() {
                    log::warn!("window {id}: a picture could not be imported");
                    continue;
                }
                gl.bind_texture(GL_TEXTURE_EXTERNAL_OES, Some(s.texture));
                (import.target_texture)(GL_TEXTURE_EXTERNAL_OES, image);
                gl.tex_parameter_i32(GL_TEXTURE_EXTERNAL_OES, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
                gl.tex_parameter_i32(GL_TEXTURE_EXTERNAL_OES, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                gl.tex_parameter_i32(GL_TEXTURE_EXTERNAL_OES, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                gl.tex_parameter_i32(GL_TEXTURE_EXTERNAL_OES, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
                if let Some(old) = s.showing.replace(Showing { picture, image }) {
                    retired.push_back(Retired { showing: old, frame: frame_no });
                }
            }
        }

        // The room: an application drawing its own eye views, if one is and has a picture.
        let room = windows.iter().find_map(|(id, window)| {
            let s = shown.get(id)?;
            (window.layer == Layer::Projection).then_some(())?;
            let showing = s.showing.as_ref()?;
            Some((s.texture, showing.picture.crop, showing.picture.viewport, window.eyes))
        });
        let room_frame = room.map(|(_, _, seq, _)| {
            seq.and_then(|seq| sent.iter().rev().find(|s| s.seq == seq))
                .map(|s| s.orientation)
                .unwrap_or(orientation)
        });

        let eyes: Vec<(Eye, (i32, i32, i32, i32))> = if stereo {
            EyeSide::both()
                .into_iter()
                .map(|side| (eye_for(side, orientation, DVec3::ZERO, &cfg), sbs_viewport(side, &cfg)))
                .collect()
        } else {
            vec![(eye_for(EyeSide::Left, orientation, DVec3::ZERO, &cfg), (0, 0, w, h))]
        };

        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::BLEND);
        gl.bind_vertex_array(Some(vao));
        for (eye, (x, y, vw, vh)) in &eyes {
            gl.viewport(*x, *y, *vw, *vh);
            let vp = eye.view_projection();
            // The sky is at infinity: no translation, or the neck model slides it.
            let mut view = eye.view;
            view.w_axis = Vec4::new(0.0, 0.0, 0.0, 1.0);
            let inv = (eye.projection * view).inverse();
            let right = eye.side == EyeSide::Right;

            if let (Some((texture, crop, _, packing)), Some(frame)) = (room, room_frame) {
                gl.use_program(Some(projection_program));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(GL_TEXTURE_EXTERNAL_OES, Some(texture));
                gl.uniform_1_i32(proj_tex.as_ref(), 0);
                gl.uniform_matrix_4_f32_slice(proj_inv_vp.as_ref(), false, &inv.to_cols_array());
                let to_frame = Mat3::from_quat(frame.inverse().as_quat());
                gl.uniform_matrix_3_f32_slice(proj_to_frame.as_ref(), false, &to_frame.to_cols_array());
                let fov = openxr_eye(eye.side, orientation, DVec3::ZERO, &cfg).fov;
                gl.uniform_4_f32(proj_tan.as_ref(), fov[0].tan(), fov[1].tan(), fov[3].tan(), fov[2].tan());
                let r = eye_rect(crop, packing, right);
                gl.uniform_4_f32(proj_rect.as_ref(), r[0], r[1], r[2], r[3]);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            } else {
                gl.use_program(Some(sky_program));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(sky_texture));
                gl.uniform_1_i32(sky_tex.as_ref(), 0);
                gl.uniform_matrix_4_f32_slice(sky_inv_vp.as_ref(), false, &inv.to_cols_array());
                gl.uniform_1_f32(sky_yaw.as_ref(), sky_yaw_offset);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }

            for (id, window) in &windows {
                if window.layer == Layer::Projection {
                    continue;
                }
                let Some(s) = shown.get(id) else { continue };
                match &s.showing {
                    Some(showing) => {
                        let picture = &showing.picture;
                        let (pw, ph) = match window.eyes {
                            Eyes::SideBySide => (picture.size.0 / 2, picture.size.1),
                            Eyes::TopBottom => (picture.size.0, picture.size.1 / 2),
                            Eyes::Mono => picture.size,
                        };
                        let height = WINDOW_WIDTH * ph.max(1) as f32 / pw.max(1) as f32;
                        gl.use_program(Some(window_program));
                        gl.active_texture(glow::TEXTURE0);
                        gl.bind_texture(GL_TEXTURE_EXTERNAL_OES, Some(s.texture));
                        gl.uniform_1_i32(win_tex.as_ref(), 0);
                        let mvp = vp * s.placement.model(WINDOW_WIDTH, height);
                        gl.uniform_matrix_4_f32_slice(win_mvp.as_ref(), false, &mvp.to_cols_array());
                        let r = eye_rect(picture.crop, window.eyes, right);
                        gl.uniform_4_f32(win_rect.as_ref(), r[0], r[1], r[2], r[3]);
                        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                    }
                    None => {
                        gl.use_program(Some(waiting_program));
                        let mvp = vp * s.placement.model(WINDOW_WIDTH, WINDOW_WIDTH * 9.0 / 16.0);
                        gl.uniform_matrix_4_f32_slice(wait_mvp.as_ref(), false, &mvp.to_cols_array());
                        gl.uniform_1_f32(wait_time.as_ref(), started.elapsed().as_secs_f32());
                        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                    }
                }
            }
        }
        lib.swap_buffers(display, surface).map_err(|e| format!("swap: {e}"))?;

        // Pictures off screen for two frames go back to their decoder.
        while retired.front().is_some_and(|r| frame_no >= r.frame + 2) {
            let r = retired.pop_front().unwrap();
            (import.destroy_image)(import.display, r.showing.image);
        }

        frames += 1;
        if since.elapsed() >= Duration::from_secs(1) {
            shared.fps.store(frames, Ordering::Relaxed);
            frames = 0;
            since = Instant::now();
        }
    }

    for r in retired.drain(..) {
        (import.destroy_image)(import.display, r.showing.image);
    }
    for (_, s) in shown.drain() {
        if let Some(showing) = s.showing {
            (import.destroy_image)(import.display, showing.image);
        }
        gl.delete_texture(s.texture);
    }
    gl.delete_texture(sky_texture);
    Ok(())
}

/// The part of a picture one eye sees, as (u0, v0, u1, v1), inside the decoder's crop.
fn eye_rect(crop: [f32; 4], eyes: Eyes, right: bool) -> [f32; 4] {
    let [u0, v0, u1, v1] = crop;
    let (um, vm) = ((u0 + u1) * 0.5, (v0 + v1) * 0.5);
    match (eyes, right) {
        (Eyes::Mono, _) => crop,
        (Eyes::SideBySide, false) => [u0, v0, um, v1],
        (Eyes::SideBySide, true) => [um, v0, u1, v1],
        (Eyes::TopBottom, false) => [u0, v0, u1, vm],
        (Eyes::TopBottom, true) => [u0, vm, u1, v1],
    }
}

/// Where a newly opened window goes: where the wearer is looking, beside anything already
/// there. Up or down too, within reason: the glasses are as often worn lying back, or looking
/// down at a desk, as looking at the horizon.
fn place_new(orientation: DQuat, taken: impl Iterator<Item = Placement>) -> Placement {
    let forward = (orientation * DVec3::X).as_vec3();
    let mut yaw = forward.y.atan2(forward.x);
    let pitch = forward.z.clamp(-1.0, 1.0).asin().clamp(-MAX_OPEN_PITCH, MAX_OPEN_PITCH);
    let taken: Vec<Placement> = taken.collect();
    let near = |yaw: f32| {
        taken.iter().any(|p| {
            let d = (p.yaw - yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
            d.abs() < WINDOW_SPACING * 0.9
        })
    };
    // Alternately left and right of where the wearer looks, nearest first.
    for step in 0..12 {
        let offset = WINDOW_SPACING * ((step + 1) / 2) as f32 * if step % 2 == 1 { 1.0 } else { -1.0 };
        let candidate = yaw + offset;
        if !near(candidate) {
            yaw = candidate;
            break;
        }
    }
    Placement { yaw, pitch }
}

/// The head as a host is told it: the Deck's `Viewport`, from the same eyes the room is drawn
/// with. `time_us` is this session's clock; a host only hands it back.
fn viewport(orientation: DQuat, cfg: &StereoConfig, seq: u32, started: Instant) -> spatiand_stream::Viewport {
    let (head_q, head_p) = to_openxr(orientation, DVec3::ZERO);
    let left = openxr_eye(EyeSide::Left, orientation, DVec3::ZERO, cfg);
    let right = openxr_eye(EyeSide::Right, orientation, DVec3::ZERO, cfg);
    spatiand_stream::Viewport {
        seq,
        time_us: started.elapsed().as_micros() as u64,
        orientation: head_q,
        position: head_p,
        eye_position: [left.position, right.position],
        fov: [left.fov, right.fov],
        render_size: (cfg.per_eye.0 * 2, cfg.per_eye.1),
    }
}
