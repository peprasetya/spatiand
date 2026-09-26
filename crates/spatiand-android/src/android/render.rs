//! The drawing thread: EGL on the glasses' `Presentation`, one frame per refresh.
//!
//! This first cut draws what proves the pipeline end to end rather than the shell: a sky with
//! a grid every 15 degrees and a mark straight ahead, and one panel two metres in front, both
//! nailed to the room by the tracker and drawn per eye through `spatiand-render`'s cameras. A
//! world that stays put while the head turns, and a panel that sits at a depth, is the whole
//! of what has to work before the real scene is worth moving over.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use glam::{DVec3, Mat4, Vec3};
use glow::HasContext;
use khronos_egl as egl;
use spatiand_render::{eye_for, sbs_viewport, Eye, EyeSide, StereoConfig};

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

const SKY_VS: &str = r#"#version 300 es
out vec2 ndc;
void main() {
    // One triangle that covers the screen.
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2)) * 2.0 - 1.0;
    ndc = p;
    gl_Position = vec4(p, 1.0, 1.0);
}
"#;

const SKY_FS: &str = r#"#version 300 es
precision highp float;
in vec2 ndc;
uniform mat4 inv_vp;
uniform vec3 eye;
out vec4 colour;

// Spatiand's world frame: +X forward, +Y left, +Z up.
float line(float degrees, float every) {
    float d = abs(fract(degrees / every + 0.5) - 0.5) * every;
    float w = fwidth(degrees);
    return 1.0 - smoothstep(0.0, w * 1.5, d);
}

void main() {
    vec4 far = inv_vp * vec4(ndc, 1.0, 1.0);
    vec3 d = normalize(far.xyz / far.w - eye);
    float yaw = degrees(atan(d.y, d.x));
    float pitch = degrees(asin(clamp(d.z, -1.0, 1.0)));

    vec3 sky = mix(vec3(0.05, 0.08, 0.14), vec3(0.12, 0.22, 0.42), smoothstep(-5.0, 60.0, pitch));
    vec3 ground = mix(vec3(0.07, 0.06, 0.05), vec3(0.02, 0.02, 0.02), smoothstep(0.0, -60.0, pitch));
    vec3 c = pitch >= 0.0 ? sky : ground;

    float grid = max(line(yaw, 15.0), line(pitch, 15.0)) * 0.25;
    c += vec3(grid);
    c += vec3(0.5, 0.45, 0.3) * line(pitch, 1000.0);   // the horizon
    // Straight ahead, as recentred: a red ring.
    float ahead = length(vec2(yaw, pitch));
    c = mix(c, vec3(0.9, 0.2, 0.2), 1.0 - smoothstep(0.0, fwidth(ahead) * 1.5, abs(ahead - 1.5) - 0.25));
    colour = vec4(c, 1.0);
}
"#;

const PANEL_VS: &str = r#"#version 300 es
uniform mat4 mvp;
out vec2 uv;
void main() {
    vec2 corner = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
    uv = corner;
    gl_Position = mvp * vec4(corner - 0.5, 0.0, 1.0);
}
"#;

const PANEL_FS: &str = r#"#version 300 es
precision mediump float;
in vec2 uv;
out vec4 colour;
void main() {
    vec2 edge = min(uv, 1.0 - uv);
    float border = step(min(edge.x, edge.y * 16.0 / 9.0), 0.01);
    vec3 c = mix(vec3(0.16, 0.18, 0.22), vec3(0.22, 0.26, 0.34), uv.y);
    // Checks, to judge sharpness and depth by.
    vec2 cell = floor(uv * vec2(16.0, 9.0));
    c += 0.04 * mod(cell.x + cell.y, 2.0);
    c = mix(c, vec3(0.85, 0.9, 1.0), border);
    colour = vec4(c, 1.0);
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

/// A quad's model matrix in Spatiand's frame, facing back along −X towards the viewer, for
/// the panel shader's unit square (x right, y up).
fn panel_model(centre: Vec3, width: f32, height: f32) -> Mat4 {
    Mat4::from_cols(
        (-Vec3::Y * width).extend(0.0),
        (Vec3::Z * height).extend(0.0),
        Vec3::X.extend(0.0),
        centre.extend(1.0),
    )
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

    let gl = unsafe {
        glow::Context::from_loader_function(|name| {
            lib.get_proc_address(name)
                .map_or(std::ptr::null(), |f| f as *const std::ffi::c_void)
        })
    };
    let (sky, panel, vao) = unsafe {
        (
            program(&gl, SKY_VS, SKY_FS)?,
            program(&gl, PANEL_VS, PANEL_FS)?,
            gl.create_vertex_array()?,
        )
    };
    let sky_inv_vp = unsafe { gl.get_uniform_location(sky, "inv_vp") };
    let sky_eye = unsafe { gl.get_uniform_location(sky, "eye") };
    let panel_mvp = unsafe { gl.get_uniform_location(panel, "mvp") };

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
        let eyes: Vec<(Eye, (i32, i32, i32, i32))> = if stereo {
            EyeSide::both()
                .into_iter()
                .map(|side| (eye_for(side, orientation, DVec3::ZERO, &cfg), sbs_viewport(side, &cfg)))
                .collect()
        } else {
            vec![(eye_for(EyeSide::Left, orientation, DVec3::ZERO, &cfg), (0, 0, w, h))]
        };

        unsafe {
            gl.disable(glow::DEPTH_TEST);
            gl.bind_vertex_array(Some(vao));
            for (eye, (x, y, vw, vh)) in &eyes {
                gl.viewport(*x, *y, *vw, *vh);
                let vp = eye.view_projection();
                gl.use_program(Some(sky));
                gl.uniform_matrix_4_f32_slice(sky_inv_vp.as_ref(), false, &vp.inverse().to_cols_array());
                let p = eye.position.as_vec3();
                gl.uniform_3_f32(sky_eye.as_ref(), p.x, p.y, p.z);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);

                gl.use_program(Some(panel));
                let model = panel_model(Vec3::new(2.0, 0.0, 0.0), 1.6, 0.9);
                gl.uniform_matrix_4_f32_slice(panel_mvp.as_ref(), false, &(vp * model).to_cols_array());
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            }
        }
        lib.swap_buffers(display, surface)
            .map_err(|e| format!("swap: {e}"))?;

        frames += 1;
        if since.elapsed() >= Duration::from_secs(1) {
            shared.fps.store(frames, Ordering::Relaxed);
            frames = 0;
            since = Instant::now();
        }
    }

    let _ = lib.make_current(display, None, None, None);
    let _ = lib.destroy_surface(display, surface);
    let _ = lib.destroy_context(display, context);
    shared.fps.store(0, Ordering::Relaxed);
    Ok(())
}
